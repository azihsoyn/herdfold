//! The reader: holds the place in the book, turns pages, keeps bookmarks.

use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event};
use ratatui::layout::{Constraint, Flex, Layout as Split};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState};
use ratatui::{DefaultTerminal, Frame};
use unicode_width::UnicodeWidthStr;

use crate::doc::Document;
use crate::layout::{Layout, Pos};
use crate::marks::{self, Entry};
use crate::spread::{FromPartner, Partner};
use crate::view::{self, Cmd, PageView, Side};

struct Reader {
    doc: Document,
    /// Key under which the place is saved; `None` when reading stdin.
    book: Option<String>,
    entry: Entry,
    layout: Layout,
    /// Page of each chapter in `doc.chapters`, for the current layout.
    chapter_pages: Vec<usize>,
    spread: bool,
    /// Selected row while the contents list is open.
    toc: Option<usize>,
    note: Option<String>,
}

pub fn run(doc: Document, book: Option<String>, single: bool) -> Result<()> {
    let entry = book.as_deref().and_then(marks::load).unwrap_or_default();
    let mut reader = Reader {
        layout: Layout::new(&doc, 1, 1),
        doc,
        book,
        entry,
        chapter_pages: Vec::new(),
        spread: false,
        toc: None,
        note: None,
    };
    let mut terminal = ratatui::init();
    let result = reader.run(&mut terminal, single);
    ratatui::restore();
    result
}

impl Reader {
    fn run(&mut self, terminal: &mut DefaultTerminal, single: bool) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        let mut partner = if single {
            None
        } else {
            Partner::open(terminal.size()?.width, tx)
        };
        // Hold the first page until the right pane reports its size, so the
        // book opens as a spread rather than flashing a single page first.
        if let Some(p) = partner.as_mut()
            && let Ok(msg) = rx.recv_timeout(Duration::from_millis(1500))
        {
            p.hear(&msg);
        }
        loop {
            let area = terminal.size()?;
            let mut size = (area.width, area.height);
            let follower = partner.as_ref().and_then(|p| p.size);
            if let Some((w, h)) = follower {
                // Both pages are set to the smaller pane so they match.
                size = (size.0.min(w), size.1.min(h));
            }
            self.fit(view::text_size(size.0, size.1), follower.is_some());

            let (left, right) = self.views();
            terminal.draw(|f| {
                view::render(f, f.area(), Some(&left));
                if let Some(sel) = self.toc {
                    self.draw_contents(f, sel);
                }
            })?;
            if let Some(p) = partner.as_mut()
                && follower.is_some()
            {
                p.show(right.as_ref());
            }

            let mut cmds = Vec::new();
            if event::poll(Duration::from_millis(30))?
                && let Event::Key(k) = event::read()?
            {
                cmds.extend(view::cmd_of(k));
            }
            while let Ok(msg) = rx.try_recv() {
                if let FromPartner::Cmd(c) = msg {
                    cmds.push(c);
                }
                if let Some(p) = partner.as_mut() {
                    p.hear(&msg);
                }
            }
            for c in cmds {
                if self.handle(c) {
                    self.save();
                    return Ok(());
                }
            }
        }
    }

    fn fit(&mut self, (width, height): (usize, usize), spread: bool) {
        if self.layout.width == width && self.layout.height == height && self.spread == spread {
            return;
        }
        self.layout = Layout::new(&self.doc, width, height);
        self.spread = spread;
        self.chapter_pages = self
            .doc
            .chapters
            .iter()
            .map(|c| self.layout.page_of(Pos { line: c.line, offset: 0 }))
            .collect();
    }

    fn step(&self) -> usize {
        if self.spread { 2 } else { 1 }
    }

    /// The page shown first (the left page of a spread).
    fn page(&self) -> usize {
        let p = self.layout.page_of(self.entry.at);
        p - p % self.step()
    }

    fn views(&self) -> (PageView, Option<PageView>) {
        let p = self.page();
        if !self.spread {
            return (self.view(p, Side::Single), None);
        }
        let right = (p + 1 < self.layout.page_count()).then(|| self.view(p + 1, Side::Right));
        (self.view(p, Side::Left), right)
    }

    fn view(&self, n: usize, side: Side) -> PageView {
        // Verso carries the book's title, recto the chapter, as in print.
        let head = match side {
            Side::Left => self.doc.title.clone(),
            Side::Right | Side::Single => self
                .chapter_pages
                .iter()
                .rposition(|&p| p <= n)
                .map(|i| self.doc.chapters[i].title.clone())
                .unwrap_or_else(|| self.doc.title.clone()),
        };
        PageView {
            side,
            head,
            rows: self.layout.page(n).iter().map(|r| (r.text.clone(), r.kind)).collect(),
            number: n + 1,
            total: self.layout.page_count(),
            marked: self.entry.marks.iter().any(|&m| self.layout.page_of(m) == n),
            width: self.layout.width,
            note: (side != Side::Right).then(|| self.note.clone()).flatten(),
        }
    }

    /// Returns true to quit.
    fn handle(&mut self, cmd: Cmd) -> bool {
        self.note = None;
        if let Some(sel) = self.toc {
            let entries = self.contents();
            match cmd {
                Cmd::Up => self.toc = Some(sel.saturating_sub(1)),
                Cmd::Down => self.toc = Some((sel + 1).min(entries.len().saturating_sub(1))),
                Cmd::Enter => {
                    self.toc = None;
                    if let Some((_, pos)) = entries.get(sel) {
                        let p = self.layout.page_of(*pos);
                        self.go(p);
                    }
                }
                Cmd::Quit => return true,
                _ => self.toc = None,
            }
            return false;
        }

        let page = self.page();
        match cmd {
            Cmd::Next => {
                if page + self.step() < self.layout.page_count() {
                    self.go(page + self.step());
                }
            }
            Cmd::Prev => self.go(page.saturating_sub(self.step())),
            Cmd::Mark => self.toggle_mark(page),
            Cmd::Contents => {
                let entries = self.contents();
                if entries.is_empty() {
                    self.note = Some("No chapters in this input, and no bookmarks".into());
                } else {
                    // Chapters come first, so this indexes `entries` too.
                    let open = page + self.step();
                    let here = self.chapter_pages.iter().rposition(|&p| p < open);
                    self.toc = Some(here.unwrap_or(0));
                }
            }
            Cmd::Quit => return true,
            Cmd::Up | Cmd::Down | Cmd::Enter | Cmd::Back => {}
        }
        false
    }

    fn go(&mut self, page: usize) {
        self.entry.at = self.layout.start_of(page);
        self.save();
    }

    /// A bookmark covers what is open: one page, or both pages of a spread.
    fn toggle_mark(&mut self, page: usize) {
        let open = page..page + self.step();
        let before = self.entry.marks.len();
        let layout = &self.layout;
        self.entry.marks.retain(|&m| !open.contains(&layout.page_of(m)));
        if self.entry.marks.len() == before {
            self.entry.marks.push(self.layout.start_of(page));
            self.entry.marks.sort();
            self.note = Some(format!("Bookmarked p.{}", page + 1));
        } else {
            self.note = Some("Bookmark removed".into());
        }
        self.save();
    }

    fn save(&mut self) {
        if let Some(book) = &self.book
            && let Err(e) = marks::save(book, &self.entry)
        {
            self.note = Some(format!("Could not save the place: {e}"));
        }
    }

    /// Chapters, then bookmarks.
    fn contents(&self) -> Vec<(String, Pos)> {
        let top = self.doc.chapters.iter().map(|c| c.level).min().unwrap_or(1);
        let chapters = self.doc.chapters.iter().map(|c| {
            let indent = "  ".repeat((c.level - top) as usize);
            (format!("{indent}{}", c.title), Pos { line: c.line, offset: 0 })
        });
        let marks = self.entry.marks.iter().map(|&m| {
            let p = self.layout.page_of(m);
            let first = self.layout.page(p).first().map(|r| r.text.trim()).unwrap_or("");
            (format!("▍ {first}"), m)
        });
        chapters.chain(marks).collect()
    }

    fn draw_contents(&self, f: &mut Frame, sel: usize) {
        let entries = self.contents();
        let area = f.area();
        let width = area.width.saturating_sub(4).min(64);
        let height = area.height.saturating_sub(4).min(entries.len() as u16 + 2);
        let [row] = Split::vertical([Constraint::Length(height)]).flex(Flex::Center).areas(area);
        let [popup] = Split::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(row);
        let inner = width.saturating_sub(4) as usize;
        let dim = Style::new().add_modifier(Modifier::DIM);
        let items: Vec<ListItem> = entries
            .iter()
            .map(|(label, pos)| {
                let num = format!(" {}", self.layout.page_of(*pos) + 1);
                let label = view::fit(label, inner.saturating_sub(num.width()));
                let gap = " ".repeat(inner.saturating_sub(label.width() + num.width()));
                ListItem::new(Line::from(vec![
                    Span::raw(label),
                    Span::raw(gap),
                    Span::styled(num, dim),
                ]))
            })
            .collect();
        let list = List::new(items)
            .block(Block::bordered().title(" Contents "))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        let mut state = ListState::default().with_selected(Some(sel));
        f.render_widget(Clear, popup);
        f.render_stateful_widget(list, popup, &mut state);
    }
}
