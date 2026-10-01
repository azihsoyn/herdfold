//! The reader: holds the place in the book, turns pages, keeps bookmarks,
//! and answers the socket API for the pane showing the right-hand page.

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event};
use ratatui::layout::{Constraint, Flex, Layout as Split};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState};
use ratatui::{DefaultTerminal, Frame};
use unicode_width::UnicodeWidthStr;

use crate::api::{Call, EventData, PROTOCOL, Request, ResponseResult};
use crate::doc::Document;
use crate::herdr::RightPane;
use crate::layout::{Layout, Pos};
use crate::marks::{self, Entry};
use crate::server::{ConnId, Hub, Inbound, Server};
use crate::turn::{Turn, Turning};
use crate::view::{self, Cmd, PageRow, PageView, Side};

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
    /// The connection drawing the right-hand page, and its size.
    attached: Option<(ConnId, u16, u16)>,
    /// The pages last sent in `page_shown`, so unchanged pages are not resent.
    shown: Option<(PageView, Option<PageView>)>,
    /// Whether turns are drawn.
    animate: bool,
    /// The turn being drawn in this pane.
    turning: Option<Turning>,
    /// A turn not yet announced to other panes.
    unsent_turn: Option<Turn>,
    /// Longest row, in columns.
    measure: usize,
}

/// Opens the book. With `spread`, and inside herdr, the right-hand page goes
/// to a pane split off for it.
/// `measure` (longest row) defaults to the one last set for this book, then
/// to the one last set for any book, then to 72.
pub fn run(
    doc: Document,
    book: Option<String>,
    spread: bool,
    measure: Option<usize>,
    animate: Option<bool>,
) -> Result<()> {
    let entry = book.as_deref().and_then(marks::load).unwrap_or_default();
    let settings = marks::settings();
    let measure = starting_measure(measure, entry.measure, settings.measure);
    let animate = animate.or(settings.animate).unwrap_or(true);
    let mut reader = Reader {
        layout: Layout::new(&doc, 1, 1),
        doc,
        book,
        entry,
        chapter_pages: Vec::new(),
        spread: false,
        toc: None,
        note: None,
        attached: None,
        shown: None,
        animate,
        turning: None,
        unsent_turn: None,
        measure,
    };
    let server = if spread { Server::listen().ok() } else { None };
    let mut terminal = ratatui::init();
    let result = reader.run(&mut terminal, server.as_ref());
    ratatui::restore();
    result
}

/// The first of: asked for on the command line, last set for this book,
/// last set for any book; else the default.
fn starting_measure(asked: Option<usize>, book: Option<usize>, last: Option<usize>) -> usize {
    asked
        .or(book)
        .or(last)
        .unwrap_or(view::MEASURE)
        .clamp(view::MEASURE_MIN, view::MEASURE_MAX)
}

impl Reader {
    fn run(&mut self, terminal: &mut DefaultTerminal, server: Option<&Server>) -> Result<()> {
        let width = terminal.size()?.width;
        let right = server.and_then(|s| RightPane::open(width, &s.path));
        let mut cmds = Vec::new();
        // Hold the first page until the right pane attaches, so the book
        // opens as a spread rather than flashing a single page first.
        if let (Some(s), Some(_)) = (server, &right) {
            let deadline = Instant::now() + Duration::from_millis(1500);
            while self.attached.is_none() {
                let left = deadline.saturating_duration_since(Instant::now());
                let Ok(msg) = s.inbound.recv_timeout(left) else {
                    break;
                };
                self.answer(&s.hub, msg, &mut cmds);
            }
        }
        loop {
            let area = terminal.size()?;
            let mut size = (area.width, area.height);
            if let Some((_, w, h)) = self.attached {
                // Both pages are set to the smaller pane so they match.
                size = (size.0.min(w), size.1.min(h));
            }
            self.fit(
                view::text_size(size.0, size.1, self.measure),
                self.attached.is_some(),
            );

            let (left, right) = self.views();
            view::draw_whole(terminal, |f| {
                let area = f.area();
                let turned = self
                    .turning
                    .as_ref()
                    .is_some_and(|t| t.render(f.buffer_mut(), area, Some(&left)));
                if !turned {
                    view::render(f.buffer_mut(), area, Some(&left));
                }
                if let Some(sel) = self.toc {
                    self.draw_contents(f, sel);
                }
            })?;
            if self.turning.as_ref().is_some_and(Turning::done) {
                self.turning = None;
            }
            let pages = (left, right);
            if let Some(s) = server
                && (self.unsent_turn.is_some() || self.shown.as_ref() != Some(&pages))
            {
                s.hub.emit(EventData::PageShown {
                    left: pages.0.clone(),
                    right: pages.1.clone(),
                    turn: self.unsent_turn.take(),
                });
                self.shown = Some(pages);
            }

            // Draw a turn at about 60 frames a second; otherwise wait on keys.
            let wait = if self.turning.is_some() { 16 } else { 30 };
            if event::poll(Duration::from_millis(wait))?
                && let Event::Key(k) = event::read()?
            {
                cmds.extend(view::key_name(k).as_deref().and_then(view::cmd_of));
            }
            if let Some(s) = server {
                while let Ok(msg) = s.inbound.try_recv() {
                    self.answer(&s.hub, msg, &mut cmds);
                }
            }
            for c in std::mem::take(&mut cmds) {
                if self.handle(c) {
                    self.save();
                    if let Some(s) = server {
                        s.hub.emit(EventData::ReaderClosed);
                    }
                    return Ok(());
                }
            }
        }
    }

    /// Answers one request from the socket; keys it carries join `cmds`.
    fn answer(&mut self, hub: &Hub, msg: Inbound, cmds: &mut Vec<Cmd>) {
        let (conn, Request { id, call }) = match msg {
            Inbound::Request(conn, req) => (conn, req),
            Inbound::Closed(conn) => {
                hub.forget(conn);
                if self.attached.is_some_and(|(c, ..)| c == conn) {
                    self.attached = None;
                }
                return;
            }
        };
        let result = match call {
            Call::Ping(_) => ResponseResult::Pong {
                version: env!("CARGO_PKG_VERSION").to_string(),
                protocol: PROTOCOL,
            },
            Call::EventsSubscribe(p) => {
                hub.subscribe(conn, &p.subscriptions);
                // Resend what is open, so the new subscriber has it.
                self.shown = None;
                ResponseResult::SubscriptionStarted
            }
            Call::ReaderAttach(p) => {
                self.attached = Some((conn, p.cols, p.rows));
                ResponseResult::ReaderAttached
            }
            Call::ReaderResize(p) => match self.attached {
                Some((c, ..)) if c == conn => {
                    self.attached = Some((conn, p.cols, p.rows));
                    ResponseResult::Ok
                }
                _ => {
                    return hub.fail(conn, id, "not_attached", "reader.attach first");
                }
            },
            Call::ReaderSendKeys(p) => {
                // Keys with no binding do nothing, as they would if typed here.
                cmds.extend(p.keys.iter().filter_map(|k| view::cmd_of(k)));
                ResponseResult::Ok
            }
        };
        hub.reply(conn, id, result);
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
            .map(|c| {
                self.layout.page_of(Pos {
                    line: c.line,
                    offset: 0,
                })
            })
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
            rows: std::iter::repeat_n(PageRow::default(), self.layout.pad(n))
                .chain(self.layout.page(n).iter().map(|r| PageRow {
                    spans: r.spans.clone(),
                }))
                .collect(),
            number: n + 1,
            total: self.layout.page_count(),
            marked: self
                .entry
                .marks
                .iter()
                .any(|&m| self.layout.page_of(m) == n),
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
                    self.turn_to(page + self.step(), Turn::Forward);
                }
            }
            Cmd::Prev => {
                if page > 0 {
                    self.turn_to(page.saturating_sub(self.step()), Turn::Backward);
                }
            }
            Cmd::Mark => self.toggle_mark(page),
            Cmd::Animate => self.toggle_animation(),
            // Step from the rows as set, which the pane may hold shorter than the measure.
            Cmd::Wider if self.layout.width < self.measure => {
                self.note = Some("Rows are already as long as the pane allows".into());
            }
            Cmd::Wider => self.set_measure(self.layout.width + view::MEASURE_STEP),
            Cmd::Narrower => self.set_measure(self.layout.width.saturating_sub(view::MEASURE_STEP)),
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
            // Esc steps back out of whatever is open: the contents (above), else the book.
            Cmd::Quit | Cmd::Back => return true,
            Cmd::Up | Cmd::Down | Cmd::Enter => {}
        }
        false
    }

    /// Goes to `page` by turning to it, which panes draw when animating.
    fn turn_to(&mut self, page: usize, turn: Turn) {
        if self.animate {
            let side = if self.spread {
                Side::Left
            } else {
                Side::Single
            };
            self.turning = Some(Turning::new(turn, Some(self.views().0), side));
            self.unsent_turn = Some(turn);
        }
        self.go(page);
    }

    fn go(&mut self, page: usize) {
        self.entry.at = self.layout.start_of(page);
        self.save();
    }

    /// Changes the longest row and remembers it for this book, and as the
    /// start for books not yet opened. The page is set again around the
    /// place being read.
    fn set_measure(&mut self, measure: usize) {
        self.measure = measure.clamp(view::MEASURE_MIN, view::MEASURE_MAX);
        self.entry.measure = Some(self.measure);
        let measure = self.measure;
        self.note = Some(
            match marks::update_settings(|s| s.measure = Some(measure)) {
                Ok(()) => format!("Rows up to {} columns", self.measure),
                Err(e) => format!("Could not save the setting: {e}"),
            },
        );
        // Last, so a failure to keep the place shows over the note above.
        self.save();
    }

    /// Turns the drawing of page turns on or off, for every book from now on.
    fn toggle_animation(&mut self) {
        self.animate = !self.animate;
        self.turning = None;
        let animate = self.animate;
        self.note = Some(
            match marks::update_settings(|s| s.animate = Some(animate)) {
                Ok(()) if animate => "Page turns drawn".into(),
                Ok(()) => "Page turns instant".into(),
                Err(e) => format!("Could not save the setting: {e}"),
            },
        );
    }

    /// A bookmark covers what is open: one page, or both pages of a spread.
    fn toggle_mark(&mut self, page: usize) {
        let open = page..page + self.step();
        let before = self.entry.marks.len();
        let layout = &self.layout;
        self.entry
            .marks
            .retain(|&m| !open.contains(&layout.page_of(m)));
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
            (
                format!("{indent}{}", c.title),
                Pos {
                    line: c.line,
                    offset: 0,
                },
            )
        });
        let marks = self.entry.marks.iter().map(|&m| {
            let p = self.layout.page_of(m);
            let first = self
                .layout
                .page(p)
                .first()
                .map(|r| r.text.trim())
                .unwrap_or("");
            (format!("▍ {first}"), m)
        });
        chapters.chain(marks).collect()
    }

    fn draw_contents(&self, f: &mut Frame, sel: usize) {
        let entries = self.contents();
        let area = f.area();
        let width = area.width.saturating_sub(4).min(64);
        let height = area.height.saturating_sub(4).min(entries.len() as u16 + 2);
        let [row] = Split::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(area);
        let [popup] = Split::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(row);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_book_keeps_its_own_measure() {
        assert_eq!(starting_measure(None, Some(84), Some(100)), 84);
        assert_eq!(starting_measure(None, None, Some(100)), 100);
        assert_eq!(starting_measure(Some(60), Some(84), Some(100)), 60);
        assert_eq!(starting_measure(None, None, None), view::MEASURE);
        assert_eq!(starting_measure(None, Some(9999), None), view::MEASURE_MAX);
    }
}
