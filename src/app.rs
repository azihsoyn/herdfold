//! The reader: holds the place in the book, turns pages, keeps bookmarks and
//! notes, and answers the socket API for the pane showing the right-hand page.

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event};
use ratatui::layout::{Constraint, Flex, Layout as Split, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use unicode_width::UnicodeWidthStr;

use crate::api::{Call, EventData, PROTOCOL, Request, ResponseResult};
use crate::doc::Document;
use crate::herdr::RightPane;
use crate::layout::{Layout, Pos};
use crate::marks::{self, Anchor, Author, Entry, Note};
use crate::server::{ConnId, Hub, Inbound, Server};
use crate::turn::{Turn, Turning};
use crate::view::{self, Cmd, PageRow, PageView, Side};

/// What keys are doing at the moment.
#[derive(Clone, Debug, PartialEq)]
enum Mode {
    Reading,
    /// The chapter list, with the selected row.
    Contents(usize),
    /// Bookmarks and notes, with the selected row.
    Shelf(usize),
    /// Choosing a row; the cursor is on the row that starts at this place.
    Select(Pos),
    /// Writing a note on a page or a row.
    Writing {
        anchor: Anchor,
        at: Pos,
        text: String,
    },
}

/// One entry in the list of bookmarks and notes.
struct Shelved {
    label: String,
    at: Pos,
    item: Item,
}

enum Item {
    Mark(usize),
    Note(usize),
}

struct Reader {
    doc: Document,
    /// Key under which the place is saved; `None` when reading stdin.
    book: Option<String>,
    entry: Entry,
    layout: Layout,
    /// Page of each chapter in `doc.chapters`, for the current layout.
    chapter_pages: Vec<usize>,
    spread: bool,
    mode: Mode,
    /// A one-off message, shown in place of the running head.
    status: Option<String>,
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
        mode: Mode::Reading,
        status: None,
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
        let mut keys = Vec::new();
        // Hold the first page until the right pane attaches, so the book
        // opens as a spread rather than flashing a single page first.
        if let (Some(s), Some(_)) = (server, &right) {
            let deadline = Instant::now() + Duration::from_millis(1500);
            while self.attached.is_none() {
                let left = deadline.saturating_duration_since(Instant::now());
                let Ok(msg) = s.inbound.recv_timeout(left) else {
                    break;
                };
                self.answer(&s.hub, msg, &mut keys);
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
                self.draw_overlay(f);
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
                keys.extend(view::key_name(k));
            }
            if let Some(s) = server {
                while let Ok(msg) = s.inbound.try_recv() {
                    self.answer(&s.hub, msg, &mut keys);
                }
            }
            for key in std::mem::take(&mut keys) {
                if self.handle(&key) {
                    self.save();
                    if let Some(s) = server {
                        s.hub.emit(EventData::ReaderClosed);
                    }
                    return Ok(());
                }
            }
        }
    }

    /// Answers one request from the socket; keys it carries join `keys`.
    fn answer(&mut self, hub: &Hub, msg: Inbound, keys: &mut Vec<String>) {
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
                // Handled as if typed here, text included while writing a note.
                keys.extend(p.keys);
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

    /// The pages open now.
    fn open_pages(&self) -> std::ops::Range<usize> {
        let p = self.page();
        p..(p + self.step()).min(self.layout.page_count())
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
        let pad = self.layout.pad(n);
        let mut rows: Vec<PageRow> = std::iter::repeat_n(PageRow::default(), pad)
            .chain(self.layout.page(n).iter().map(|r| PageRow {
                spans: r.spans.clone(),
                ..PageRow::default()
            }))
            .collect();
        let line_notes = self.entry.notes.iter().filter(|n| n.anchor == Anchor::Line);
        for note in line_notes {
            if let Some(i) = self.layout.row_of(n, note.at) {
                rows[pad + i].marker = true;
            }
        }
        if let Mode::Select(at) = self.mode
            && let Some(i) = self.layout.row_of(n, at)
        {
            rows[pad + i].selected = true;
        }
        PageView {
            side,
            head,
            rows,
            number: n + 1,
            total: self.layout.page_count(),
            marked: self
                .entry
                .marks
                .iter()
                .any(|&m| self.layout.page_of(m) == n),
            noted: self
                .entry
                .notes
                .iter()
                .any(|note| note.anchor == Anchor::Page && self.layout.page_of(note.at) == n),
            width: self.layout.width,
            status: (side != Side::Right).then(|| self.status.clone()).flatten(),
        }
    }

    /// Handles one key, by herdr's key name. Returns true to quit.
    fn handle(&mut self, key: &str) -> bool {
        self.status = None;
        if let Mode::Writing { .. } = self.mode {
            self.write(key);
            return false;
        }
        let Some(cmd) = view::cmd_of(key) else {
            return false;
        };
        match self.mode.clone() {
            Mode::Reading => self.read(cmd),
            Mode::Contents(sel) => self.in_contents(sel, cmd),
            Mode::Shelf(sel) => self.in_shelf(sel, cmd),
            Mode::Select(at) => self.in_select(at, cmd),
            Mode::Writing { .. } => false,
        }
    }

    fn read(&mut self, cmd: Cmd) -> bool {
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
                self.status = Some("Rows are already as long as the pane allows".into());
            }
            Cmd::Wider => self.set_measure(self.layout.width + view::MEASURE_STEP),
            Cmd::Narrower => self.set_measure(self.layout.width.saturating_sub(view::MEASURE_STEP)),
            Cmd::Contents => {
                if self.doc.chapters.is_empty() {
                    self.status = Some("No chapters in this input".into());
                } else {
                    let open = page + self.step();
                    let here = self.chapter_pages.iter().rposition(|&p| p < open);
                    self.mode = Mode::Contents(here.unwrap_or(0));
                }
            }
            Cmd::Shelf => {
                let shelf = self.shelf();
                if shelf.is_empty() {
                    self.status = Some("No bookmarks or notes yet".into());
                } else {
                    let here = self.layout.start_of(page);
                    let sel = shelf.iter().rposition(|s| s.at <= here).unwrap_or(0);
                    self.mode = Mode::Shelf(sel);
                }
            }
            Cmd::NotePage => {
                self.mode = Mode::Writing {
                    anchor: Anchor::Page,
                    at: self.layout.start_of(page),
                    text: String::new(),
                };
            }
            Cmd::Select => match self.open_rows().first() {
                Some(&at) => self.mode = Mode::Select(at),
                None => self.status = Some("Nothing on this page to choose".into()),
            },
            // Esc steps back out of whatever is open; here, the book.
            Cmd::Quit | Cmd::Back => return true,
            Cmd::Up | Cmd::Down | Cmd::Enter | Cmd::Delete => {}
        }
        false
    }

    fn in_contents(&mut self, sel: usize, cmd: Cmd) -> bool {
        let count = self.doc.chapters.len();
        match cmd {
            Cmd::Up => self.mode = Mode::Contents(sel.saturating_sub(1)),
            Cmd::Down => self.mode = Mode::Contents((sel + 1).min(count.saturating_sub(1))),
            Cmd::Enter => {
                self.mode = Mode::Reading;
                if let Some(&p) = self.chapter_pages.get(sel) {
                    self.go(p);
                }
            }
            Cmd::Quit => return true,
            _ => self.mode = Mode::Reading,
        }
        false
    }

    fn in_shelf(&mut self, sel: usize, cmd: Cmd) -> bool {
        let shelf = self.shelf();
        match cmd {
            Cmd::Up => self.mode = Mode::Shelf(sel.saturating_sub(1)),
            Cmd::Down => self.mode = Mode::Shelf((sel + 1).min(shelf.len().saturating_sub(1))),
            Cmd::Enter => {
                self.mode = Mode::Reading;
                if let Some(s) = shelf.get(sel) {
                    let p = self.layout.page_of(s.at);
                    self.go(p);
                }
            }
            Cmd::Delete => {
                match shelf.get(sel).map(|s| &s.item) {
                    Some(&Item::Mark(i)) => {
                        self.entry.marks.remove(i);
                        self.status = Some("Bookmark removed".into());
                    }
                    Some(&Item::Note(i)) => {
                        self.entry.notes.remove(i);
                        self.status = Some("Note removed".into());
                    }
                    None => {}
                }
                self.save();
                let left = shelf.len().saturating_sub(1);
                self.mode = if left == 0 {
                    Mode::Reading
                } else {
                    Mode::Shelf(sel.min(left - 1))
                };
            }
            Cmd::Quit => return true,
            _ => self.mode = Mode::Reading,
        }
        false
    }

    fn in_select(&mut self, at: Pos, cmd: Cmd) -> bool {
        let rows = self.open_rows();
        let i = rows.iter().position(|&r| r == at).unwrap_or(0);
        match cmd {
            Cmd::Up => self.mode = Mode::Select(rows[i.saturating_sub(1)]),
            Cmd::Down => self.mode = Mode::Select(rows[(i + 1).min(rows.len() - 1)]),
            Cmd::Enter => {
                self.mode = Mode::Writing {
                    anchor: Anchor::Line,
                    at,
                    text: String::new(),
                };
            }
            Cmd::Quit => return true,
            _ => self.mode = Mode::Reading,
        }
        false
    }

    /// A key while writing a note: text, or Enter to keep it, Esc to drop it.
    fn write(&mut self, key: &str) {
        let Mode::Writing { anchor, at, text } = &mut self.mode else {
            return;
        };
        match key {
            "enter" => {
                let text = text.trim().to_string();
                let (anchor, at) = (*anchor, *at);
                self.mode = Mode::Reading;
                if !text.is_empty() {
                    self.entry.add_note(Note {
                        at,
                        anchor,
                        text,
                        by: Author::Reader,
                        question: None,
                    });
                    self.status = Some("Note kept".into());
                    self.save();
                }
            }
            "esc" | "ctrl+c" => {
                if !text.is_empty() {
                    self.status = Some("Note dropped".into());
                }
                self.mode = Mode::Reading;
            }
            "backspace" => {
                text.pop();
            }
            "space" => text.push(' '),
            // Other named keys (arrows, tab, ...) are not text.
            k if k.chars().count() == 1 => text.push_str(k),
            _ => {}
        }
    }

    /// The rows on the open pages that hold text, in reading order.
    fn open_rows(&self) -> Vec<Pos> {
        self.open_pages()
            .flat_map(|p| self.layout.page(p))
            .filter(|r| !r.text.trim().is_empty())
            .map(|r| r.pos)
            .collect()
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
        self.status = Some(
            match marks::update_settings(|s| s.measure = Some(measure)) {
                Ok(()) => format!("Rows up to {} columns", self.measure),
                Err(e) => format!("Could not save the setting: {e}"),
            },
        );
        // Last, so a failure to keep the place shows over the status above.
        self.save();
    }

    /// Turns the drawing of page turns on or off, for every book from now on.
    fn toggle_animation(&mut self) {
        self.animate = !self.animate;
        self.turning = None;
        let animate = self.animate;
        self.status = Some(
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
            self.status = Some(format!("Bookmarked p.{}", page + 1));
        } else {
            self.status = Some("Bookmark removed".into());
        }
        self.save();
    }

    fn save(&mut self) {
        if let Some(book) = &self.book
            && let Err(e) = marks::save(book, &self.entry)
        {
            self.status = Some(format!("Could not save the place: {e}"));
        }
    }

    /// The text of the row holding `at`, trimmed, for quoting in a list.
    fn row_text(&self, at: Pos) -> String {
        let p = self.layout.page_of(at);
        self.layout
            .row_of(p, at)
            .and_then(|i| self.layout.page(p).get(i))
            .map(|r| r.text.trim().to_string())
            .unwrap_or_default()
    }

    /// Bookmarks and notes, in reading order.
    fn shelf(&self) -> Vec<Shelved> {
        let marks = self.entry.marks.iter().enumerate().map(|(i, &at)| Shelved {
            label: format!("▍ {}", self.row_text(at)),
            at,
            item: Item::Mark(i),
        });
        let notes = self.entry.notes.iter().enumerate().map(|(i, n)| {
            let sign = match (n.by, n.anchor) {
                (Author::Agent, _) => "✦",
                (Author::Reader, Anchor::Page) => "✎",
                (Author::Reader, Anchor::Line) => "▎",
            };
            let label = match n.anchor {
                Anchor::Page => format!("{sign} {}", n.text),
                Anchor::Line => format!("{sign} {}  — {}", n.text, self.row_text(n.at)),
            };
            Shelved {
                label,
                at: n.at,
                item: Item::Note(i),
            }
        });
        let mut all: Vec<Shelved> = marks.chain(notes).collect();
        all.sort_by_key(|s| s.at);
        all
    }

    fn draw_overlay(&self, f: &mut Frame) {
        match &self.mode {
            Mode::Reading | Mode::Select(_) => {}
            Mode::Contents(sel) => {
                let top = self.doc.chapters.iter().map(|c| c.level).min().unwrap_or(1);
                let entries: Vec<(String, Pos)> = self
                    .doc
                    .chapters
                    .iter()
                    .map(|c| {
                        let indent = "  ".repeat((c.level - top) as usize);
                        let at = Pos {
                            line: c.line,
                            offset: 0,
                        };
                        (format!("{indent}{}", c.title), at)
                    })
                    .collect();
                self.draw_list(f, " Contents ", &entries, *sel);
            }
            Mode::Shelf(sel) => {
                let entries: Vec<(String, Pos)> =
                    self.shelf().into_iter().map(|s| (s.label, s.at)).collect();
                self.draw_list(
                    f,
                    " Bookmarks and notes — Enter to go, d to remove ",
                    &entries,
                    *sel,
                );
            }
            Mode::Writing { anchor, at, text } => {
                let title = match anchor {
                    Anchor::Page => format!(" Note on p.{} ", self.layout.page_of(*at) + 1),
                    Anchor::Line => " Note on this row ".to_string(),
                };
                draw_input(f, &title, text);
            }
        }
    }

    fn draw_list(&self, f: &mut Frame, title: &str, entries: &[(String, Pos)], sel: usize) {
        let area = f.area();
        let width = area.width.saturating_sub(4).min(72);
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
            .map(|(label, at)| {
                let num = format!(" {}", self.layout.page_of(*at) + 1);
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
            .block(Block::bordered().title(title.to_string()))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        let mut state = ListState::default().with_selected(Some(sel));
        f.render_widget(Clear, popup);
        f.render_stateful_widget(list, popup, &mut state);
    }
}

/// A one-line box near the foot of the pane for writing a note.
fn draw_input(f: &mut Frame, title: &str, text: &str) {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(80);
    let x = area.x + (area.width - width) / 2;
    let y = area.bottom().saturating_sub(6);
    let box_area = Rect::new(x, y, width, 3);
    let room = width.saturating_sub(3) as usize;
    // Keep the end of the text, where the writing is, in view.
    let mut shown = text.to_string();
    while shown.width() > room.saturating_sub(1) {
        shown.remove(0);
    }
    let block = Block::bordered()
        .title(title.to_string())
        .title_bottom(" Enter to keep, Esc to drop ");
    f.render_widget(Clear, box_area);
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::raw(shown), Span::raw("▏")])).block(block),
        box_area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Kind, Line as DocLine};

    /// A reader over `lines`, one row per line, `height` rows a page.
    fn reader(lines: &[&str], height: usize) -> Reader {
        let doc = Document {
            lines: lines.iter().map(|l| DocLine::new(*l, Kind::Body)).collect(),
            ..Default::default()
        };
        let mut r = Reader {
            layout: Layout::new(&doc, 1, 1),
            doc,
            book: None,
            entry: Entry::default(),
            chapter_pages: Vec::new(),
            spread: false,
            mode: Mode::Reading,
            status: None,
            attached: None,
            shown: None,
            animate: false,
            turning: None,
            unsent_turn: None,
            measure: view::MEASURE,
        };
        r.fit((20, height), false);
        r
    }

    fn keys(r: &mut Reader, keys: &[&str]) {
        for k in keys {
            r.handle(k);
        }
    }

    #[test]
    fn a_note_is_written_on_the_page() {
        let mut r = reader(&["one", "two", "three", "four"], 2);
        keys(
            &mut r,
            &[
                "space",
                "n",
                "h",
                "i",
                "space",
                "あ",
                "backspace",
                "!",
                "enter",
            ],
        );
        assert_eq!(r.mode, Mode::Reading);
        let n = &r.entry.notes[0];
        assert_eq!(
            (n.text.as_str(), n.anchor, n.at.line),
            ("hi !", Anchor::Page, 2)
        );
        assert!(r.views().0.noted);
    }

    #[test]
    fn a_note_is_written_on_a_chosen_row() {
        let mut r = reader(&["one", "two", "three"], 3);
        keys(&mut r, &["v", "j", "enter", "x", "enter"]);
        let n = &r.entry.notes[0];
        assert_eq!((n.anchor, n.at.line), (Anchor::Line, 1));
        let rows = r.views().0.rows;
        assert_eq!(
            rows.iter().map(|r| r.marker).collect::<Vec<_>>(),
            [false, true, false]
        );
    }

    #[test]
    fn esc_drops_a_note_being_written() {
        let mut r = reader(&["one"], 3);
        keys(&mut r, &["n", "x", "esc"]);
        assert!(r.entry.notes.is_empty());
        assert_eq!(r.mode, Mode::Reading);
    }

    #[test]
    fn the_shelf_lists_marks_and_notes_in_order_and_removes_them() {
        let mut r = reader(&["one", "two", "three", "four"], 1);
        keys(&mut r, &["space", "space", "m", "b", "n", "x", "enter"]);
        let labels: Vec<_> = r.shelf().into_iter().map(|s| s.label).collect();
        assert_eq!(labels, ["✎ x", "▍ three"]);
        keys(&mut r, &["l", "j", "d"]);
        assert!(r.entry.marks.is_empty());
        assert_eq!(r.entry.notes.len(), 1);
        keys(&mut r, &["d"]);
        assert!(r.entry.notes.is_empty());
        assert_eq!(r.mode, Mode::Reading);
    }

    #[test]
    fn a_book_keeps_its_own_measure() {
        assert_eq!(starting_measure(None, Some(84), Some(100)), 84);
        assert_eq!(starting_measure(None, None, Some(100)), 100);
        assert_eq!(starting_measure(Some(60), Some(84), Some(100)), 60);
        assert_eq!(starting_measure(None, None, None), view::MEASURE);
        assert_eq!(starting_measure(None, Some(9999), None), view::MEASURE_MAX);
    }
}
