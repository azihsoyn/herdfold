//! The reader: holds the place in the book, turns pages, keeps bookmarks and
//! notes, and answers the socket API for the pane showing the right-hand page.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, MouseButton, MouseEventKind};
use ratatui::layout::{Constraint, Flex, Layout as Split, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use unicode_width::UnicodeWidthStr;

use crate::api::{
    Call, EventData, MouseKind, PROTOCOL, ReaderMouseParams, Request, ResponseResult, SOCKET_ENV,
};
use crate::doc::Document;
use crate::doc::{Kind, Line as DocLine, Style as TextStyle};
use crate::herdr::{self, RightPane};
use crate::keys::{self, Keymap};
use crate::layout::{Layout, Pos};
use crate::log::Event as Logged;
use crate::marks::{self, Anchor, Author, Direction, Entry, Mark, Note, NoteDisplay, Ribbon};
use crate::pictures::Pictures;
use crate::server::{ConnId, Hub, Inbound, Server};
use crate::turn::{Turn, Turning};
use crate::view::{self, Cmd, MarginNote, PageRow, PageView, Side};

/// How long a passing message stays up.
const TOAST: Duration = Duration::from_millis(2000);

/// What keys are doing at the moment.
#[derive(Clone, Debug, PartialEq)]
enum Mode {
    Reading,
    /// The keys, listed.
    Help,
    /// A tip, with whether "don't show again" is ticked.
    Tip {
        index: usize,
        hide: bool,
    },
    /// The chapter list, with the selected row.
    Contents(usize),
    /// Bookmarks and notes, with the selected row.
    Shelf(usize),
    /// Choosing a row; the cursor is on the row that starts at this place.
    Select(Pos),
    /// Text chosen with the mouse, `from` up to `to` (exclusive), and what
    /// to do with it.
    Selected {
        from: Pos,
        to: Pos,
    },
    /// A highlighter marker (the note at this index), clicked on: what to
    /// do with it.
    Marker(usize),
    /// A bookmark (the one at this index), its ribbon clicked on.
    Bookmark(usize),
    /// Choosing among the links on the open pages; this one (in
    /// `doc.links`) is pointed at.
    Link(usize),
    /// What a link (in `doc.links`) leads to, shown where the reader is.
    Peek(usize),
    /// Searching: the words sought, where they were found, which find is
    /// shown, and whether the words are still being typed (and from where,
    /// to go back to if the search is dropped).
    Search {
        query: String,
        hits: Vec<(Pos, Pos)>,
        current: Option<usize>,
        typing: Option<Pos>,
        /// The list of finds, open at this row.
        list: Option<usize>,
    },
    /// Writing a note on a page or a row, or (`ask`) a question about it
    /// for the agent.
    Writing {
        anchor: Anchor,
        at: Pos,
        /// For a range, where it ends.
        end: Option<Pos>,
        text: String,
        ask: bool,
        /// Rewriting the text of the note at this index, rather than adding one.
        rewrite: Option<usize>,
    },
}

/// One entry in the list of bookmarks and notes.
struct Shelved {
    label: String,
    at: Pos,
    item: Item,
    /// A bookmark's ribbon, for colouring its mark in the list.
    color: Option<Ribbon>,
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
    /// A passing message, shown in place of the running head for a while.
    toast: Option<(String, Instant)>,
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
    /// How notes are shown.
    note_display: NoteDisplay,
    /// Bumped whenever notes change, so pages are set again around footnotes.
    notes_rev: u64,
    /// What the current layout was set for.
    laid_for: Option<LaidFor>,
    /// Width of this pane, for judging the margin.
    pane_width: u16,
    /// Whether settings are written to disk (not under test).
    keep_settings: bool,
    /// The agent named with `--agent`, if any.
    agent: Option<String>,
    /// Colour for the next bookmark.
    ribbon: Ribbon,
    /// Where a mouse drag began, while the button is held.
    dragging: Option<Pos>,
    /// Colour for the next highlighter marker.
    marker: Ribbon,
    /// Which keys do what.
    keymap: Keymap,
    /// The pages run right to left: the right page comes first.
    rtl: bool,
    /// A character cell's size in pixels, where herdr can set pictures.
    cell: Option<(u32, u32)>,
    /// Chapters folded shut in the contents, their sections hidden.
    folded: std::collections::BTreeSet<usize>,
    /// Places left by jumps (contents, list, search), the latest last.
    trail: Vec<Pos>,
    /// The reading log of this session; none under test.
    log: Option<crate::log::Log>,
    /// Seconds a page usually takes, from the reading log.
    pace: Option<f64>,
    /// Where this reader's socket is, for an agent to write notes back.
    socket: Option<PathBuf>,
    /// Outcomes of work done off the main loop (asking the agent).
    background: (Sender<String>, Receiver<String>),
}

/// Everything the left pane's frame is drawn from, so an unchanged frame
/// is not drawn again.
#[derive(Clone, PartialEq)]
struct Drawn {
    page: PageView,
    mode: Mode,
    dragging: Option<Pos>,
    folded: std::collections::BTreeSet<usize>,
    size: ratatui::layout::Size,
}

/// What a layout depends on besides the document.
#[derive(Clone, Copy, PartialEq)]
struct LaidFor {
    width: usize,
    height: usize,
    spread: bool,
    notes: NoteDisplay,
    notes_rev: u64,
    cell: Option<(u32, u32)>,
}

/// How a book is opened, as asked on the command line.
pub struct Opening {
    /// Inside herdr, the right-hand page goes to a pane split off for it.
    pub spread: bool,
    /// Longest row; defaults to the one last set for this book, then to
    /// the one last set for any book, then to 72.
    pub measure: Option<usize>,
    pub animate: Option<bool>,
    pub agent: Option<String>,
    pub format: crate::formats::Format,
    /// Where to open it, rather than where it was left.
    pub start: Option<Pos>,
}

/// Opens the book.
pub fn run(doc: Document, book: Option<String>, opening: Opening) -> Result<()> {
    let Opening {
        spread,
        measure,
        animate,
        agent,
        format,
        start,
    } = opening;
    let mut entry: Entry = book.as_deref().and_then(marks::load).unwrap_or_default();
    if let Some(at) = start {
        entry.at = at;
    }
    let log_book = crate::log::Book {
        key: book.clone(),
        title: doc.title.clone(),
        format,
    };
    let settings = marks::settings();
    let measure = starting_measure(measure, entry.measure, settings.measure);
    let animate = animate.or(settings.animate).unwrap_or(true);
    let note_display = settings.notes.unwrap_or_default();
    let rtl = match entry.direction {
        Some(d) => d == Direction::RightToLeft,
        None => doc.rtl,
    };
    let mut reader = Reader {
        layout: Layout::new(&doc, 1, 1),
        doc,
        book,
        entry,
        rtl,
        cell: std::env::var("HERDR_PANE_ID")
            .ok()
            .and_then(|pane| herdr::cell_size(&pane)),
        folded: Default::default(),
        trail: Vec::new(),
        pace: crate::log::pace(log_book.key.as_deref()),
        log: Some(crate::log::Log::new(log_book)),
        chapter_pages: Vec::new(),
        spread: false,
        mode: Mode::Reading,
        toast: None,
        attached: None,
        shown: None,
        animate,
        turning: None,
        unsent_turn: None,
        measure,
        note_display,
        notes_rev: 0,
        laid_for: None,
        pane_width: 0,
        keep_settings: true,
        agent,
        ribbon: settings.ribbon.unwrap_or_default(),
        dragging: None,
        marker: settings.marker.unwrap_or(Ribbon::Yellow),
        keymap: Keymap::default(),
        socket: None,
        background: mpsc::channel(),
    };
    // Always listening: the right-hand page attaches here, and an agent
    // writes its answers back here.
    let server = Server::listen().ok();
    reader.socket = server.as_ref().map(|s| s.path.clone());
    let (keymap, problems) = Keymap::load();
    reader.keymap = keymap;
    if !problems.is_empty() {
        reader.say(format!(
            "config.toml: {} problem(s), see `herdfold config check`",
            problems.len()
        ));
    }
    // A tip greets the book, a different one each time, until switched off.
    if settings.tips != Some(false) {
        let index = settings.next_tip.unwrap_or(0) % TIPS.len();
        reader.mode = Mode::Tip { index, hide: false };
        let _ = reader.update_settings(|s| s.next_tip = Some(index + 1));
    }
    let mut terminal = ratatui::init();
    // The book takes the mouse, so text is chosen within a page rather than
    // across both panes of a spread.
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
    let result = reader.run(&mut terminal, server.as_ref(), spread);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
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
    fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
        server: Option<&Server>,
        spread: bool,
    ) -> Result<()> {
        let width = terminal.size()?.width;
        let right = server
            .filter(|_| spread)
            .and_then(|s| RightPane::open(width, &s.path));
        // Keys, and whether they were pressed in the right-hand pane.
        let mut keys: Vec<(String, bool)> = Vec::new();
        let mut drawn: Option<Drawn> = None;
        let mut pictures = Pictures::new();
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
            self.pane_width = area.width;
            let mut size = (area.width, area.height);
            if let Some((_, w, h)) = self.attached {
                // Both pages are set to the smaller pane so they match.
                size = (size.0.min(w), size.1.min(h));
            }
            self.fit(
                view::text_size(size.0, size.1, self.measure),
                self.attached.is_some(),
            );
            // Pages passed through while typing a search are not read.
            if !matches!(
                self.mode,
                Mode::Search {
                    typing: Some(_),
                    ..
                }
            ) {
                let at = self.place(self.entry.at);
                if let Some(log) = &mut self.log {
                    log.shown(at);
                }
            }

            let (left, right) = self.views();
            let left_now = left.clone();
            // Draw only what changed. Writing to the terminal when nothing
            // has (even an empty frame) clears a selection made with the
            // mouse, so a quiet page is left alone.
            let frame = Drawn {
                page: left.clone(),
                mode: self.mode.clone(),
                dragging: self.dragging,
                folded: self.folded.clone(),
                size: area,
            };
            if self.turning.is_some() || drawn.as_ref() != Some(&frame) {
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
                drawn = Some(frame);
            }
            if self.turning.as_ref().is_some_and(Turning::done) {
                self.turning = None;
                // The last frame drawn was mid-turn; draw the page settled.
                drawn = None;
            }
            // Pictures only on a settled page with nothing open over it.
            let settled = self.turning.is_none()
                && matches!(
                    self.mode,
                    Mode::Reading | Mode::Select(_) | Mode::Selected { .. }
                );
            pictures.show(
                Rect::new(0, 0, area.width, area.height),
                settled.then_some(&left_now),
            );
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
            if event::poll(Duration::from_millis(wait))? {
                match event::read()? {
                    Event::Key(k) => keys.extend(view::key_name(k).map(|k| (k, false))),
                    Event::Mouse(m) => {
                        let kind = match m.kind {
                            MouseEventKind::Down(MouseButton::Left) => Some(MouseKind::Down),
                            MouseEventKind::Drag(MouseButton::Left) => Some(MouseKind::Drag),
                            MouseEventKind::Up(MouseButton::Left) => Some(MouseKind::Up),
                            _ => None,
                        };
                        if let Some(kind) = kind {
                            let side = if self.spread {
                                Side::Left
                            } else {
                                Side::Single
                            };
                            let size = (area.width, area.height);
                            self.mouse(side, size, kind, m.column, m.row);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(s) = server {
                while let Ok(msg) = s.inbound.try_recv() {
                    self.answer(&s.hub, msg, &mut keys);
                }
            }
            while let Ok(status) = self.background.1.try_recv() {
                self.say(status);
            }
            for (key, right) in std::mem::take(&mut keys) {
                if self.handle_key(&key, right) {
                    self.save();
                    let at = self.place(self.entry.at);
                    if let Some(log) = &mut self.log {
                        log.end(at);
                    }
                    if let Some(s) = server {
                        s.hub.emit(EventData::ReaderClosed);
                    }
                    return Ok(());
                }
            }
        }
    }

    /// Answers one request from the socket; keys it carries join `keys`.
    fn answer(&mut self, hub: &Hub, msg: Inbound, keys: &mut Vec<(String, bool)>) {
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
                // Handled as if typed here, text included while writing a note;
                // from the attached pane, as pressed over the right-hand page.
                let right = self.attached.is_some_and(|(c, ..)| c == conn);
                keys.extend(p.keys.into_iter().map(|k| (k, right)));
                ResponseResult::Ok
            }
            Call::ReaderSendMouse(ReaderMouseParams { kind, col, row }) => match self.attached {
                Some((c, cols, rows)) if c == conn => {
                    self.mouse(Side::Right, (cols, rows), kind, col, row);
                    ResponseResult::Ok
                }
                _ => {
                    return hub.fail(conn, id, "not_attached", "reader.attach first");
                }
            },
            Call::NoteAdd(p) => {
                if p.text.trim().is_empty() {
                    return hub.fail(conn, id, "invalid_params", "a note needs text");
                }
                let at = p.at.unwrap_or_else(|| self.layout.start_of(self.page()));
                let page = self.layout.page_of(at) + 1;
                self.add_note(Note {
                    at,
                    anchor: p.anchor,
                    text: p.text.trim().to_string(),
                    by: p.by,
                    question: p.question,
                    end: p.end,
                    color: None,
                });
                self.notes_rev += 1;
                self.say(match p.by {
                    Author::Agent => format!("A note from the agent on p.{page}"),
                    Author::Reader => format!("Note kept on p.{page}"),
                });
                self.save();
                ResponseResult::NoteAdded { page }
            }
        };
        hub.reply(conn, id, result);
    }

    fn fit(&mut self, (width, height): (usize, usize), spread: bool) {
        let want = LaidFor {
            width,
            height,
            spread,
            notes: self.note_display,
            notes_rev: self.notes_rev,
            cell: self.cell,
        };
        if self.laid_for == Some(want) {
            return;
        }
        self.laid_for = Some(want);
        self.layout = if self.note_display == NoteDisplay::Footnotes {
            let footnotes: Vec<(Pos, usize)> = self
                .entry
                .notes
                .iter()
                .filter(|n| !n.text.is_empty())
                .map(|n| (n.at, footnote_rows(n, width).len()))
                .collect();
            Layout::with_footnotes(&self.doc, width, height, &footnotes, self.cell)
        } else {
            Layout::with_footnotes(&self.doc, width, height, &[], self.cell)
        };
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

    /// The notes on page `n`, in reading order.
    fn notes_on(&self, n: usize) -> Vec<&Note> {
        self.entry
            .notes
            .iter()
            .filter(|note| match note.anchor {
                Anchor::Page => self.layout.page_of(note.at) == n,
                Anchor::Line => self.layout.row_of(n, note.at).is_some(),
                // A bare marker has no text to show beside the page.
                Anchor::Range => !note.text.is_empty() && self.layout.page_of(note.at) == n,
            })
            .collect()
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

    /// The open pages, what is left shown beside the last one's number.
    fn views(&self) -> (PageView, Option<PageView>) {
        let (mut left, mut right) = self.open_views();
        let remaining = self.remaining();
        match &mut right {
            Some(r) => r.remaining = remaining,
            None if left.side == Side::Single => left.remaining = remaining,
            None => {}
        }
        (left, right)
    }

    /// What is left: pages to the next chapter, and the time to the end of
    /// the book at the pace pages have been turned.
    fn remaining(&self) -> Option<String> {
        let count = self.layout.page_count();
        let last_open = (self.page() + self.step()).min(count).saturating_sub(1);
        let to_chapter = self
            .chapter_pages
            .iter()
            .find(|&&p| p > last_open)
            .map(|&p| p - last_open - 1)
            .filter(|&n| n > 0)
            .map(|n| match n {
                1 => "1 page left in chapter".to_string(),
                n => format!("{n} pages left in chapter"),
            });
        let rest = count.saturating_sub(last_open + 1);
        let to_end = self
            .pace
            .filter(|_| rest > 0)
            .map(|pace| crate::log::duration(rest as f64 * pace) + " to the end");
        let parts: Vec<String> = to_chapter.into_iter().chain(to_end).collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    fn open_views(&self) -> (PageView, Option<PageView>) {
        let p = self.page();
        if !self.spread {
            return (self.view(p, Side::Single), None);
        }
        let next = p + 1 < self.layout.page_count();
        if self.rtl {
            // Bound on the right: the first page of the spread is the right one.
            let left = if next {
                self.view(p + 1, Side::Left)
            } else {
                self.blank(Side::Left)
            };
            return (left, Some(self.view(p, Side::Right)));
        }
        let right = next.then(|| self.view(p + 1, Side::Right));
        (self.view(p, Side::Left), right)
    }

    /// The blank page facing a lone last page.
    fn blank(&self, side: Side) -> PageView {
        PageView {
            side,
            head: String::new(),
            rows: Vec::new(),
            number: 0,
            total: self.layout.page_count(),
            ribbon: None,
            noted: false,
            width: self.layout.width,
            status: None,
            margin_notes: Vec::new(),
            pictures: Vec::new(),
            remaining: None,
        }
    }

    /// The page shown in the right-hand pane (`right`) or the left (or only) one.
    fn pane_page(&self, right: bool) -> usize {
        let p = self.page();
        if self.spread && right != self.rtl && p + 1 < self.layout.page_count() {
            p + 1
        } else {
            p
        }
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
        // Highlighter markers, then the selection being made, over the text.
        let marker = TextStyle::default();
        let selected = TextStyle {
            selected: true,
            ..TextStyle::default()
        };
        let mut paint: Vec<(Pos, Pos, TextStyle)> = self
            .entry
            .notes
            .iter()
            .filter_map(|note| {
                let color = note.color.unwrap_or(Ribbon::Yellow);
                let style = TextStyle {
                    marker: Some(color),
                    ..marker
                };
                Some((note.at, note.end?, style))
            })
            .collect();
        if let Mode::Selected { from, to } = self.mode {
            paint.push((from, to, selected));
        }
        if let Mode::Link(i) = self.mode {
            let l = self.doc.links[i];
            let (from, to) = (
                Pos {
                    line: l.line,
                    offset: l.start,
                },
                Pos {
                    line: l.line,
                    offset: l.end,
                },
            );
            paint.push((from, to, selected));
            if let Some(row) = self.layout.row_of(n, from) {
                rows[pad + row].pointer = true;
            }
        }
        if let Mode::Search { hits, current, .. } = &self.mode {
            let found = TextStyle {
                found: true,
                ..TextStyle::default()
            };
            let shown = TextStyle {
                selected: true,
                ..found
            };
            let (first, last) = (self.layout.start_of(n), self.layout.start_of(n + 1));
            for (i, &(from, to)) in hits.iter().enumerate() {
                // Only the finds that can touch this page.
                if to < first || (n + 1 < self.layout.page_count() && from > last) {
                    continue;
                }
                let style = if Some(i) == *current { shown } else { found };
                paint.push((from, to, style));
                if Some(i) == *current
                    && let Some(row) = self.layout.row_of(n, from)
                {
                    rows[pad + row].pointer = true;
                }
            }
        }
        for (i, r) in self.layout.page(n).iter().enumerate() {
            for &(from, to, style) in &paint {
                paint_row(&mut rows[pad + i], r, from, to, style);
            }
        }
        let notes = self.notes_on(n);
        let mut margin_notes = Vec::new();
        match self.note_display {
            NoteDisplay::Footnotes if !notes.is_empty() => {
                // Footnotes sit at the foot of the page, under a short rule.
                let foot: Vec<PageRow> = notes
                    .iter()
                    .flat_map(|note| footnote_rows(note, self.layout.width))
                    .collect();
                let fill = self
                    .layout
                    .height
                    .saturating_sub(rows.len() + foot.len() + 1);
                rows.extend(std::iter::repeat_n(PageRow::default(), fill));
                rows.push(PageRow {
                    spans: vec![crate::doc::Styled {
                        text: "─".repeat(self.layout.width.min(16)),
                        style: TextStyle {
                            dim: true,
                            ..TextStyle::default()
                        },
                    }],
                    ..PageRow::default()
                });
                rows.extend(foot);
            }
            NoteDisplay::Margin => {
                for note in &notes {
                    let row = match note.anchor {
                        Anchor::Page => 0,
                        Anchor::Line | Anchor::Range => {
                            pad + self.layout.row_of(n, note.at).unwrap_or(0)
                        }
                    };
                    margin_notes.push(MarginNote {
                        row,
                        text: format!("{} {}", sign(note), note.text),
                    });
                }
            }
            NoteDisplay::Footnotes | NoteDisplay::Marks => {}
        }
        // Pictures, centred in the column over the rows left for them.
        let pictures = self
            .layout
            .page(n)
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                let (cols, rows) = r.picture?;
                let picture = self.doc.lines[r.pos.line].image.as_ref()?;
                Some(crate::pictures::PagePicture {
                    row: pad + i,
                    col: self.layout.width.saturating_sub(cols as usize) / 2,
                    cols,
                    rows,
                    path: picture.path.clone(),
                    width: picture.width,
                    height: picture.height,
                })
            })
            .collect();
        PageView {
            side,
            head,
            rows,
            number: n + 1,
            total: self.layout.page_count(),
            ribbon: self
                .entry
                .marks
                .iter()
                .find(|m| self.layout.page_of(m.at) == n)
                .map(|m| m.color),
            noted: self
                .entry
                .notes
                .iter()
                .any(|note| note.anchor == Anchor::Page && self.layout.page_of(note.at) == n),
            width: self.layout.width,
            // The snackbar sits at the bottom right of the book.
            status: (side != Side::Left).then(|| self.toast()).flatten(),
            remaining: None,
            margin_notes,
            pictures,
        }
    }

    /// Shows `text` in place of the running head for a few seconds.
    fn say(&mut self, text: String) {
        self.toast = Some((text, Instant::now()));
    }

    /// The message showing now, if one is still up.
    fn toast(&self) -> Option<String> {
        self.toast
            .as_ref()
            .filter(|(_, since)| since.elapsed() < TOAST)
            .map(|(text, _)| text.clone())
    }

    /// Handles one key pressed over the left (or only) page.
    #[cfg(test)]
    fn handle(&mut self, key: &str) -> bool {
        self.handle_key(key, false)
    }

    /// Handles one key, by herdr's key name, pressed in the right-hand pane
    /// when `right`. Returns true to quit.
    fn handle_key(&mut self, key: &str, right: bool) -> bool {
        self.toast = None;
        if let Mode::Writing { .. } = self.mode {
            self.write(key);
            return false;
        }
        if let Mode::Tip { index, hide } = self.mode {
            self.tip_key(key, index, hide);
            return false;
        }
        if let Mode::Selected { from, to } = self.mode {
            self.selected_key(key, from, to);
            return false;
        }
        if let Mode::Marker(i) = self.mode {
            self.marker_key(key, i);
            return false;
        }
        if let Mode::Bookmark(i) = self.mode {
            self.bookmark_key(key, i);
            return false;
        }
        if let Mode::Search { .. } = self.mode {
            if self.search_key(key) {
                // The search's own key, even one that closed it: done here.
                return false;
            }
            // A key that is not the search's own closes it and is read as usual.
            return self.handle_key(key, right);
        }
        // In the contents, the arrows only fold and unfold chapters that
        // have sections under them; in the bookmark list they do nothing.
        if matches!(key, "left" | "right") {
            match self.mode {
                Mode::Contents(sel) => {
                    if self.has_sections(sel) {
                        if key == "left" {
                            self.folded.insert(sel);
                        } else {
                            self.folded.remove(&sel);
                        }
                    }
                    return false;
                }
                Mode::Shelf(_) => return false,
                _ => {}
            }
        }
        // Bound on the right, the arrows point the way the pages run.
        let key = match (self.rtl, key) {
            (true, "left") => "right",
            (true, "right") => "left",
            (_, k) => k,
        };
        let Some(cmd) = self.keymap.cmd(key) else {
            return false;
        };
        match self.mode.clone() {
            Mode::Reading => self.read(cmd, right),
            Mode::Contents(sel) => self.in_contents(sel, cmd),
            Mode::Shelf(sel) => self.in_shelf(sel, cmd),
            Mode::Select(at) => self.in_select(at, cmd),
            Mode::Link(i) => self.in_links(i, cmd),
            Mode::Peek(i) => {
                match cmd {
                    Cmd::Enter | Cmd::Follow => {
                        self.mode = Mode::Reading;
                        let target = Pos {
                            line: self.doc.links[i].target,
                            offset: 0,
                        };
                        let page = self.layout.page_of(target);
                        self.jump(page);
                    }
                    Cmd::Quit => return true,
                    _ => self.mode = Mode::Reading,
                }
                false
            }
            // Any key puts the list of keys away; q too, rather than closing the book.
            Mode::Help => {
                self.mode = Mode::Reading;
                false
            }
            Mode::Writing { .. }
            | Mode::Tip { .. }
            | Mode::Selected { .. }
            | Mode::Marker(_)
            | Mode::Bookmark(_)
            | Mode::Search { .. } => false,
        }
    }

    /// A key on the tip: Enter or Esc to close, arrows for other tips,
    /// Space to tick "don't show again".
    fn tip_key(&mut self, key: &str, index: usize, hide: bool) {
        let n = TIPS.len();
        self.mode = match key {
            "right" | "l" | "tab" => Mode::Tip {
                index: (index + 1) % n,
                hide,
            },
            "left" | "h" => Mode::Tip {
                index: (index + n - 1) % n,
                hide,
            },
            "space" | "x" => Mode::Tip { index, hide: !hide },
            "enter" | "esc" | "q" | "ctrl+c" => {
                match self.update_settings(|s| s.tips = Some(!hide)) {
                    Err(e) => self.say(format!("Could not save the setting: {e}")),
                    Ok(()) if hide => self.say("Tips off (T shows one)".into()),
                    Ok(()) => {}
                }
                Mode::Reading
            }
            _ => Mode::Tip { index, hide },
        };
    }

    fn read(&mut self, cmd: Cmd, right: bool) -> bool {
        let page = self.page();
        // The page in the pane the key was pressed in.
        let here = self.pane_page(right);
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
            Cmd::Mark => self.toggle_mark(here),
            Cmd::Animate => self.toggle_animation(),
            Cmd::NoteDisplay => self.cycle_note_display(),
            Cmd::Help => self.mode = Mode::Help,
            Cmd::Direction => {
                self.rtl = !self.rtl;
                self.entry.direction = Some(if self.rtl {
                    Direction::RightToLeft
                } else {
                    Direction::LeftToRight
                });
                self.say(if self.rtl {
                    "Pages run right to left".into()
                } else {
                    "Pages run left to right".into()
                });
                self.save();
            }
            Cmd::Search => {
                self.mode = Mode::Search {
                    query: String::new(),
                    hits: Vec::new(),
                    current: None,
                    typing: Some(self.entry.at),
                    list: None,
                };
            }
            Cmd::Tip => {
                let settings = marks::settings();
                let index = settings.next_tip.unwrap_or(0) % TIPS.len();
                let hide = self.keep_settings && settings.tips == Some(false);
                self.mode = Mode::Tip { index, hide };
            }
            Cmd::Color => self.recolor_mark(here),
            // Step from the rows as set, which the pane may hold shorter than the measure.
            Cmd::Wider if self.layout.width < self.measure => {
                self.say("Rows are already as long as the pane allows".into());
            }
            Cmd::Wider => self.set_measure(self.layout.width + view::MEASURE_STEP),
            Cmd::Narrower => self.set_measure(self.layout.width.saturating_sub(view::MEASURE_STEP)),
            Cmd::Contents => {
                if self.doc.chapters.is_empty() {
                    self.say("No chapters in this input".into());
                } else {
                    let here = self.chapter_here().unwrap_or(0);
                    self.mode = Mode::Contents(self.shown_as(here));
                }
            }
            Cmd::Shelf => {
                let shelf = self.shelf();
                if shelf.is_empty() {
                    self.say("No bookmarks or notes yet".into());
                } else {
                    let here = self.layout.start_of(page);
                    let sel = shelf.iter().rposition(|s| s.at <= here).unwrap_or(0);
                    self.mode = Mode::Shelf(sel);
                }
            }
            Cmd::NotePage | Cmd::Ask => {
                self.mode = Mode::Writing {
                    anchor: Anchor::Page,
                    at: self
                        .layout
                        .start_of(if cmd == Cmd::Ask { page } else { here }),
                    end: None,
                    text: String::new(),
                    ask: cmd == Cmd::Ask,
                    rewrite: None,
                };
            }
            Cmd::Select => match self.open_rows().first() {
                Some(&at) => self.mode = Mode::Select(at),
                None => self.say("Nothing on this page to choose".into()),
            },
            // Esc steps back out of whatever is open; here, the book.
            Cmd::Quit | Cmd::Back => return true,
            Cmd::Follow => match self.open_links().first() {
                Some(&i) => self.mode = Mode::Link(i),
                None => self.say("No links on these pages".into()),
            },
            Cmd::Return => match self.trail.pop() {
                Some(at) => {
                    self.entry.at = at;
                    self.save();
                    let page = self.layout.page_of(at);
                    self.say(format!("Back to p.{}", page + 1));
                }
                None => self.say("Nowhere to go back to".into()),
            },
            Cmd::Up | Cmd::Down | Cmd::Enter | Cmd::Delete => {}
        }
        false
    }

    /// The chapter holding the open pages.
    fn chapter_here(&self) -> Option<usize> {
        let open = self.page() + self.step();
        self.chapter_pages.iter().rposition(|&p| p < open)
    }

    /// Whether chapter `i` has sections under it (the next chapter is deeper).
    fn has_sections(&self, i: usize) -> bool {
        let ch = &self.doc.chapters;
        ch.get(i + 1).is_some_and(|next| next.level > ch[i].level)
    }

    /// The chapters the contents shows: all but those inside a folded one.
    fn shown_chapters(&self) -> Vec<usize> {
        let mut shown = Vec::new();
        let mut hiding_below: Option<u8> = None;
        for (i, c) in self.doc.chapters.iter().enumerate() {
            if let Some(level) = hiding_below {
                if c.level > level {
                    continue;
                }
                hiding_below = None;
            }
            shown.push(i);
            if self.folded.contains(&i) {
                hiding_below = Some(c.level);
            }
        }
        shown
    }

    /// Chapter `i` if the contents shows it, else the folded chapter it is in.
    fn shown_as(&self, i: usize) -> usize {
        let shown = self.shown_chapters();
        shown.iter().rev().find(|&&s| s <= i).copied().unwrap_or(0)
    }

    fn in_contents(&mut self, sel: usize, cmd: Cmd) -> bool {
        let shown = self.shown_chapters();
        let at = shown.iter().position(|&i| i == sel).unwrap_or(0);
        match cmd {
            Cmd::Up => self.mode = Mode::Contents(shown[at.saturating_sub(1)]),
            Cmd::Down => self.mode = Mode::Contents(shown[(at + 1).min(shown.len() - 1)]),
            Cmd::Enter => {
                self.mode = Mode::Reading;
                if let Some(&p) = self.chapter_pages.get(sel) {
                    self.jump(p);
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
                    self.jump(p);
                }
            }
            Cmd::Delete => {
                match shelf.get(sel).map(|s| &s.item) {
                    Some(&Item::Mark(i)) => {
                        self.entry.marks.remove(i);
                        self.say("Bookmark removed".into());
                    }
                    Some(&Item::Note(i)) => {
                        self.entry.notes.remove(i);
                        self.notes_rev += 1;
                        self.say("Note removed".into());
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

    /// The links on the open pages, as indexes into `doc.links`.
    fn open_links(&self) -> Vec<usize> {
        let pages = self.open_pages();
        let (first, last) = (
            self.layout.start_of(pages.start),
            self.layout.start_of(pages.end),
        );
        let to_end = pages.end >= self.layout.page_count();
        self.doc
            .links
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                let at = Pos {
                    line: l.line,
                    offset: l.start,
                };
                at >= first && (to_end || at < last)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn in_links(&mut self, i: usize, cmd: Cmd) -> bool {
        let links = self.open_links();
        let at = links.iter().position(|&l| l == i).unwrap_or(0);
        match cmd {
            Cmd::Up | Cmd::Prev => self.mode = Mode::Link(links[at.saturating_sub(1)]),
            Cmd::Down | Cmd::Next | Cmd::Follow => {
                self.mode = Mode::Link(links[(at + 1) % links.len()]);
            }
            Cmd::Enter => self.mode = Mode::Peek(i),
            Cmd::Quit => return true,
            _ => self.mode = Mode::Reading,
        }
        false
    }

    /// The link whose text is at `at`, if any.
    fn link_at(&self, at: Pos) -> Option<usize> {
        self.doc
            .links
            .iter()
            .position(|l| l.line == at.line && l.start <= at.offset && at.offset < l.end)
    }

    fn in_select(&mut self, at: Pos, cmd: Cmd) -> bool {
        let rows = self.open_rows();
        let i = rows.iter().position(|&r| r == at).unwrap_or(0);
        match cmd {
            Cmd::Up => self.mode = Mode::Select(rows[i.saturating_sub(1)]),
            Cmd::Down => self.mode = Mode::Select(rows[(i + 1).min(rows.len() - 1)]),
            Cmd::Enter | Cmd::Ask => {
                self.mode = Mode::Writing {
                    anchor: Anchor::Line,
                    at,
                    end: None,
                    text: String::new(),
                    ask: cmd == Cmd::Ask,
                    rewrite: None,
                };
            }
            Cmd::Quit => return true,
            _ => self.mode = Mode::Reading,
        }
        false
    }

    /// A key while writing a note: text, or Enter to keep it, Esc to drop it.
    fn write(&mut self, key: &str) {
        let Mode::Writing {
            anchor,
            at,
            end,
            text,
            ask,
            rewrite,
        } = &mut self.mode
        else {
            return;
        };
        match key {
            "enter" if rewrite.is_some() => {
                let (i, text) = (rewrite.unwrap_or(0), text.trim().to_string());
                self.mode = Mode::Reading;
                if let Some(note) = self.entry.notes.get_mut(i) {
                    note.text = text;
                    self.notes_rev += 1;
                    self.say("Note kept".into());
                    self.save();
                }
            }
            "enter" if *ask => {
                let question = text.trim().to_string();
                let (anchor, at, end) = (*anchor, *at, *end);
                self.mode = Mode::Reading;
                if !question.is_empty() {
                    self.ask(&question, anchor, at, end);
                }
            }
            "enter" => {
                let text = text.trim().to_string();
                let (anchor, at, end) = (*anchor, *at, *end);
                self.mode = Mode::Reading;
                if !text.is_empty() {
                    let color = end.map(|_| self.marker);
                    self.add_note(Note {
                        at,
                        anchor,
                        text,
                        by: Author::Reader,
                        question: None,
                        end,
                        color,
                    });
                    self.notes_rev += 1;
                    self.say("Note kept".into());
                    self.save();
                }
            }
            "esc" | "ctrl+c" => {
                if !text.is_empty() {
                    self.say("Note dropped".into());
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

    /// A mouse button on a page: pressing starts choosing text, dragging
    /// extends the choice, releasing offers what to do with it. A click
    /// without a drag lets the choice go.
    fn mouse(&mut self, side: Side, size: (u16, u16), kind: MouseKind, col: u16, row: u16) {
        // Only while reading or choosing; panels in front take no mouse.
        if !matches!(
            self.mode,
            Mode::Reading | Mode::Selected { .. } | Mode::Marker(_) | Mode::Bookmark(_)
        ) {
            return;
        }
        let page = self.pane_page(side == Side::Right);
        // A press on the page's ribbon opens its bookmark.
        if kind == MouseKind::Down
            && let Some(i) = self.ribbon_at(page, side, size, col, row)
        {
            self.dragging = None;
            self.mode = Mode::Bookmark(i);
            return;
        }
        let Some(at) = self.point_at(page, size, col, row) else {
            return;
        };
        match kind {
            MouseKind::Down => {
                self.toast = None;
                self.dragging = Some(at);
                self.mode = Mode::Selected {
                    from: at,
                    to: self.next_char(at),
                };
            }
            MouseKind::Drag => {
                if let Some(start) = self.dragging {
                    let (from, last) = if at < start { (at, start) } else { (start, at) };
                    self.mode = Mode::Selected {
                        from,
                        to: self.next_char(last),
                    };
                }
            }
            MouseKind::Up => {
                let start = self.dragging.take();
                if start == Some(at) {
                    // A click: on a marker, open it; on a link, show where
                    // it leads; elsewhere, let go.
                    self.mode = match (self.marker_at(at), self.link_at(at)) {
                        (Some(i), _) => Mode::Marker(i),
                        (None, Some(i)) => Mode::Peek(i),
                        (None, None) => Mode::Reading,
                    };
                }
            }
        }
    }

    /// The place in the text under cell (`col`, `row`) of a pane of `size`
    /// showing page `n`; beside or past a row's text, its nearest end.
    fn point_at(&self, n: usize, size: (u16, u16), col: u16, row: u16) -> Option<Pos> {
        let rows = self.layout.page(n);
        if rows.is_empty() {
            return None;
        }
        let area = Rect::new(0, 0, size.0, size.1);
        let (x0, _) = view::column(area, self.layout.width);
        let i = (row as usize)
            .saturating_sub(view::TOP as usize + self.layout.pad(n))
            .min(rows.len() - 1);
        let r = &rows[i];
        let line: Vec<char> = self.doc.lines[r.pos.line].text.chars().collect();
        let mut x = (col as usize).saturating_sub(x0 as usize);
        if x < r.lead_width {
            return Some(r.pos);
        }
        x -= r.lead_width;
        let mut used = 0;
        for k in 0..r.len {
            let w = line
                .get(r.pos.offset + k)
                .map(|c| unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0))
                .unwrap_or(1);
            if x < used + w {
                return Some(Pos {
                    line: r.pos.line,
                    offset: r.pos.offset + k,
                });
            }
            used += w;
        }
        // Past the end of the row's text: its last character.
        Some(Pos {
            line: r.pos.line,
            offset: r.pos.offset + r.len.saturating_sub(1),
        })
    }

    /// The place just after the character at `at`.
    fn next_char(&self, at: Pos) -> Pos {
        Pos {
            line: at.line,
            offset: at.offset + 1,
        }
    }

    /// The text from `from` up to `to`, lines joined by newlines.
    fn text_between(&self, from: Pos, to: Pos) -> String {
        self.doc.text_between(from, to)
    }

    /// The bookmark whose ribbon is drawn at cell (`col`, `row`) of a pane
    /// of `size` showing page `n` on `side`.
    fn ribbon_at(
        &self,
        n: usize,
        side: Side,
        size: (u16, u16),
        col: u16,
        row: u16,
    ) -> Option<usize> {
        let i = self
            .entry
            .marks
            .iter()
            .position(|m| self.layout.page_of(m.at) == n)?;
        let area = Rect::new(0, 0, size.0, size.1);
        let (x, w) = view::column(area, self.layout.width);
        let ribbon = view::ribbon_area(area, x, w, side);
        // A cell's leeway around so small a target.
        let hit = Rect::new(
            ribbon.x.saturating_sub(1),
            ribbon.y,
            ribbon.width + 2,
            ribbon.height + 1,
        );
        hit.contains(ratatui::layout::Position::new(col, row))
            .then_some(i)
    }

    /// A key on a clicked ribbon: c recolours its bookmark, d takes it out.
    fn bookmark_key(&mut self, key: &str, i: usize) {
        let Some(mark) = self.entry.marks.get_mut(i) else {
            self.mode = Mode::Reading;
            return;
        };
        match key {
            "c" => {
                mark.color = mark.color.next();
                let color = mark.color;
                self.ribbon = color;
                if let Err(e) = self.update_settings(|s| s.ribbon = Some(color)) {
                    self.say(format!("Could not save the setting: {e}"));
                }
                self.save();
            }
            "d" | "m" | "delete" | "backspace" => {
                self.entry.marks.remove(i);
                self.mode = Mode::Reading;
                self.say("Bookmark removed".into());
                self.save();
            }
            "esc" | "q" | "enter" | "ctrl+c" => self.mode = Mode::Reading,
            _ => {}
        }
    }

    /// A key while searching. Returns false for a key that is not the
    /// search's own, having closed the search so the key can be read as usual.
    fn search_key(&mut self, key: &str) -> bool {
        let Mode::Search {
            query,
            hits,
            current,
            typing,
            list,
        } = &mut self.mode
        else {
            return false;
        };
        if let Some(sel) = *list {
            let last = hits.len().saturating_sub(1);
            match key {
                "up" | "k" => *list = Some(sel.saturating_sub(1)),
                "down" | "j" => *list = Some((sel + 1).min(last)),
                "pageup" => *list = Some(sel.saturating_sub(10)),
                "pagedown" => *list = Some((sel + 10).min(last)),
                "enter" => {
                    *list = None;
                    *current = Some(sel);
                    let page = self.layout.page_of(hits[sel].0);
                    self.go(page);
                }
                "esc" | "l" | "q" | "tab" => *list = None,
                _ => {}
            }
            return true;
        }
        if let Some(origin) = *typing {
            match key {
                "enter" => {
                    *typing = None;
                    let (query, finds) = (query.clone(), hits.len());
                    // The whole search counts as one jump, from where it began.
                    if self.layout.page_of(origin) != self.page() {
                        self.leave(origin);
                    }
                    if let Some(log) = &mut self.log
                        && !query.is_empty()
                    {
                        log.record(Logged::Searched { query, finds });
                    }
                }
                "esc" | "ctrl+c" => {
                    // Dropped while typing: back to where the search began.
                    self.entry.at = origin;
                    self.mode = Mode::Reading;
                    self.save();
                }
                "backspace" | "space" => {
                    if key == "space" {
                        query.push(' ');
                    } else {
                        query.pop();
                    }
                    self.find(origin);
                }
                k if k.chars().count() == 1 => {
                    query.push_str(k);
                    self.find(origin);
                }
                _ => {}
            }
            return true;
        }
        let count = hits.len();
        let step = |by: isize| match (*current, count) {
            (_, 0) => None,
            (None, _) => Some(0),
            (Some(i), n) => Some((i as isize + by).rem_euclid(n as isize) as usize),
        };
        let next = match key {
            "n" | "enter" | "down" | "ctrl+n" => step(1),
            "N" | "up" | "ctrl+p" => step(-1),
            "/" | "ctrl+f" => {
                *typing = Some(self.entry.at);
                return true;
            }
            "l" | "tab" => {
                if !hits.is_empty() {
                    *list = Some(current.unwrap_or(0));
                }
                return true;
            }
            "esc" | "q" => {
                self.mode = Mode::Reading;
                return true;
            }
            _ => {
                self.mode = Mode::Reading;
                return false;
            }
        };
        if let Some(i) = next {
            *current = Some(i);
            let page = self.layout.page_of(hits[i].0);
            self.go(page);
        }
        true
    }

    /// Finds the words being searched for, and shows the first find at or
    /// after `origin`, else the first in the book.
    fn find(&mut self, origin: Pos) {
        let Mode::Search {
            query,
            hits,
            current,
            ..
        } = &mut self.mode
        else {
            return;
        };
        *hits = search(&self.doc, query);
        *current = hits
            .iter()
            .position(|(from, _)| *from >= origin)
            .or((!hits.is_empty()).then_some(0));
        let to = match *current {
            Some(i) => hits[i].0,
            None => origin,
        };
        let page = self.layout.page_of(to);
        self.go(page);
    }

    /// The highlighter marker covering `at`, if any (the last laid wins).
    fn marker_at(&self, at: Pos) -> Option<usize> {
        self.entry
            .notes
            .iter()
            .rposition(|n| n.end.is_some_and(|end| n.at <= at && at < end))
    }

    /// A key on a clicked marker: c recolours it, n writes its note, d
    /// removes it.
    fn marker_key(&mut self, key: &str, i: usize) {
        let Some(note) = self.entry.notes.get_mut(i) else {
            self.mode = Mode::Reading;
            return;
        };
        match key {
            "c" => {
                let color = note.color.unwrap_or(Ribbon::Yellow).next();
                note.color = Some(color);
                self.marker = color;
                if let Err(e) = self.update_settings(|s| s.marker = Some(color)) {
                    self.say(format!("Could not save the setting: {e}"));
                }
                self.save();
            }
            "n" => {
                self.mode = Mode::Writing {
                    anchor: Anchor::Range,
                    at: note.at,
                    end: note.end,
                    text: note.text.clone(),
                    ask: false,
                    rewrite: Some(i),
                };
            }
            "d" | "delete" | "backspace" => {
                self.entry.notes.remove(i);
                self.notes_rev += 1;
                self.mode = Mode::Reading;
                self.say("Marker removed".into());
                self.save();
            }
            "esc" | "q" | "enter" | "ctrl+c" => self.mode = Mode::Reading,
            _ => {}
        }
    }

    /// A key while text is chosen: what to do with it.
    fn selected_key(&mut self, key: &str, from: Pos, to: Pos) {
        let write = |ask| Mode::Writing {
            anchor: Anchor::Range,
            at: from,
            end: Some(to),
            text: String::new(),
            ask,
            rewrite: None,
        };
        match key {
            "m" => {
                self.add_note(Note {
                    at: from,
                    anchor: Anchor::Range,
                    text: String::new(),
                    by: Author::Reader,
                    question: None,
                    end: Some(to),
                    color: Some(self.marker),
                });
                self.notes_rev += 1;
                self.mode = Mode::Reading;
                self.say(format!(
                    "Marked ({}); click it to change or remove",
                    self.marker.name()
                ));
                self.save();
            }
            "n" => self.mode = write(false),
            "?" => self.mode = write(true),
            "y" => {
                let text = self.text_between(from, to);
                self.mode = Mode::Reading;
                match copy(&text) {
                    Ok(()) => self.say("Copied".into()),
                    Err(e) => self.say(format!("Could not copy: {e}")),
                }
            }
            "esc" | "q" | "ctrl+c" => self.mode = Mode::Reading,
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
        // Bound on the right, a page turns the other way across the panes.
        let turn = match (self.rtl, turn) {
            (false, t) => t,
            (true, Turn::Forward) => Turn::Backward,
            (true, Turn::Backward) => Turn::Forward,
        };
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

    /// `at` as the reading log keeps it.
    fn place(&self, at: Pos) -> crate::log::Place {
        let chapter = self.doc.chapter_of(at.line).map(|c| c.title.clone());
        crate::log::Place {
            line: at.line,
            offset: at.offset,
            page: self.layout.page_of(at) + 1,
            pages: self.layout.page_count(),
            chapter,
        }
    }

    /// Writes what happened at `at` in the reading log.
    fn record(&mut self, event: impl FnOnce(crate::log::Place) -> Logged, at: Pos) {
        let place = self.place(at);
        if let Some(log) = &mut self.log {
            log.record(event(place));
        }
    }

    /// Keeps a note in the book, and in the reading log.
    fn add_note(&mut self, note: Note) {
        let quote = note.end.map(|end| self.text_between(note.at, end));
        let (anchor, text) = (note.anchor, note.text.clone());
        self.record(
            |at| Logged::NoteAdded {
                at,
                anchor,
                text,
                quote,
            },
            note.at,
        );
        self.entry.add_note(note);
    }

    /// Goes to `page` as a jump, which `Return` can come back from.
    fn jump(&mut self, page: usize) {
        if page != self.page() {
            self.leave(self.entry.at);
        }
        self.go(page);
    }

    /// Notes `at` as a place a jump left from.
    fn leave(&mut self, at: Pos) {
        const KEPT: usize = 100;
        if self.trail.last() != Some(&at) {
            self.trail.push(at);
        }
        if self.trail.len() > KEPT {
            self.trail.remove(0);
        }
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
        self.say(match self.update_settings(|s| s.measure = Some(measure)) {
            Ok(()) => format!("Rows up to {} columns", self.measure),
            Err(e) => format!("Could not save the setting: {e}"),
        });
        // Last, so a failure to keep the place shows over the status above.
        self.save();
    }

    /// Turns the drawing of page turns on or off, for every book from now on.
    fn toggle_animation(&mut self) {
        self.animate = !self.animate;
        self.turning = None;
        let animate = self.animate;
        self.say(match self.update_settings(|s| s.animate = Some(animate)) {
            Ok(()) if animate => "Page turns drawn".into(),
            Ok(()) => "Page turns instant".into(),
            Err(e) => format!("Could not save the setting: {e}"),
        });
    }

    /// Footnotes, margin, marks only, and round again; kept for every book.
    fn cycle_note_display(&mut self) {
        self.note_display = self.note_display.next();
        let display = self.note_display;
        let narrow = view::margin_room(self.pane_width, self.layout.width) < view::MARGIN_NOTE_MIN;
        self.say(match self.update_settings(|s| s.notes = Some(display)) {
            Ok(()) => match display {
                NoteDisplay::Footnotes => "Notes as footnotes".into(),
                NoteDisplay::Margin if narrow => {
                    "Notes in the margin: too narrow here, shorten rows with <".into()
                }
                NoteDisplay::Margin => "Notes in the margin".into(),
                NoteDisplay::Marks => "Notes as marks only (l to read them)".into(),
            },
            Err(e) => format!("Could not save the setting: {e}"),
        });
    }

    /// Sends the question, with the open pages, to an agent beside the
    /// book. The agent answers in its own pane, and is asked to keep a short
    /// answer in the book as a note where the question was asked.
    fn ask(&mut self, question: &str, anchor: Anchor, at: Pos, end: Option<Pos>) {
        let q = question.to_string();
        self.record(|at| Logged::Asked { at, question: q }, at);
        let agent = match herdr::find_agent(self.agent.as_deref()) {
            Ok(a) => a,
            Err(e) => {
                self.say(e);
                return;
            }
        };
        let prompt = self.prompt(question, anchor, at, end);
        self.say(format!("Asking {}…", agent.label));
        let done = self.background.0.clone();
        std::thread::spawn(move || {
            let status = match herdr::prompt(&agent.target, &prompt) {
                Ok(()) => format!("Asked {}: the answer comes back as a note", agent.label),
                Err(e) => format!("Could not ask {}: {e}", agent.label),
            };
            let _ = done.send(status);
        });
    }

    /// The question as the agent receives it: where the reader is, what is
    /// on the open pages, and how to put the answer back in the book.
    fn prompt(&self, question: &str, anchor: Anchor, at: Pos, end: Option<Pos>) -> String {
        let pages = self.open_pages();
        let chapter = self
            .chapter_pages
            .iter()
            .rposition(|&p| p <= pages.start)
            .map(|i| format!(", in \"{}\"", self.doc.chapters[i].title))
            .unwrap_or_default();
        let source = self.book.as_deref().unwrap_or("stdin");
        let mut out = format!(
            "Someone reading \"{}\" ({source}) in herdfold asks, at p.{} of {}{chapter}:\n\n{question}\n",
            self.doc.title,
            pages.start + 1,
            self.layout.page_count(),
        );
        if let Some(end) = end {
            out += &format!(
                "\nThey are asking about this passage:\n> {}\n",
                self.text_between(at, end).replace('\n', "\n> ")
            );
        } else if anchor == Anchor::Line {
            out += &format!(
                "\nThey are asking about this row:\n> {}\n",
                self.row_text(at)
            );
        }
        out += "\nThe pages open in front of them:\n";
        for p in pages {
            out += &format!("\n--- p.{} ---\n", p + 1);
            for row in self.layout.page(p) {
                out += &row.text;
                out += "\n";
            }
        }
        out += "\nAnswer them here. ";
        if let Some(socket) = &self.socket {
            let exe = std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "herdfold".into());
            // A note on a passage is set beside its first row.
            let anchor = match anchor {
                Anchor::Page => "page",
                Anchor::Line | Anchor::Range => "line",
            };
            out += &format!(
                "Then keep a short version of the answer in their book, as a note where they asked, by running:\n\n\
                 {SOCKET_ENV}={} {} note add --line {} --offset {} --anchor {anchor} --question {} {}\n",
                shell_quote(&socket.display().to_string()),
                shell_quote(&exe),
                at.line,
                at.offset,
                shell_quote(question),
                shell_quote("<your short answer>"),
            );
        }
        out
    }

    /// A bookmark marks one page: in a spread, the page of the pane `m` was
    /// pressed in. A new one takes the colour last chosen.
    fn toggle_mark(&mut self, page: usize) {
        let open = page..page + 1;
        let before = self.entry.marks.len();
        let layout = &self.layout;
        self.entry
            .marks
            .retain(|m| !open.contains(&layout.page_of(m.at)));
        if self.entry.marks.len() == before {
            let color = self.ribbon;
            let at = self.layout.start_of(page);
            let i = self.entry.marks.partition_point(|m| m.at <= at);
            self.entry.marks.insert(i, Mark { at, color });
            self.say(format!("Bookmarked p.{} ({})", page + 1, color.name()));
            self.record(|at| Logged::BookmarkAdded { at, color }, at);
        } else {
            self.say("Bookmark removed".into());
            let at = self.layout.start_of(page);
            self.record(|at| Logged::BookmarkRemoved { at }, at);
        }
        self.save();
    }

    /// Gives the bookmark on `page` its next colour, which new bookmarks
    /// then take too.
    /// Failing one there, the bookmark on the other open page.
    fn recolor_mark(&mut self, page: usize) {
        let open = self.open_pages();
        let page_of = |m: &Mark| self.layout.page_of(m.at);
        let marks = &self.entry.marks;
        let i = marks
            .iter()
            .position(|m| page_of(m) == page)
            .or_else(|| marks.iter().position(|m| open.contains(&page_of(m))));
        let Some(i) = i else {
            self.say("No bookmark here (m to place one)".into());
            return;
        };
        let mark = &mut self.entry.marks[i];
        mark.color = mark.color.next();
        let color = mark.color;
        self.ribbon = color;
        if let Err(e) = self.update_settings(|s| s.ribbon = Some(color)) {
            self.say(format!("Could not save the setting: {e}"));
        } else {
            self.say(format!("Ribbon: {}", color.name()));
        }
        self.save();
    }

    fn update_settings(&self, change: impl FnOnce(&mut marks::Settings)) -> Result<()> {
        if self.keep_settings {
            marks::update_settings(change)
        } else {
            Ok(())
        }
    }

    fn save(&mut self) {
        if let Some(book) = &self.book
            && let Err(e) = marks::save(book, &self.entry)
        {
            self.say(format!("Could not save the place: {e}"));
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
        let marks = self.entry.marks.iter().enumerate().map(|(i, m)| Shelved {
            label: format!("▍ {}", self.row_text(m.at)),
            at: m.at,
            item: Item::Mark(i),
            color: Some(m.color),
        });
        let notes = self.entry.notes.iter().enumerate().map(|(i, n)| {
            let sign = sign(n);
            let label = match (n.anchor, n.end) {
                (Anchor::Range, Some(end)) => {
                    let quote = self.text_between(n.at, end).replace('\n', " ");
                    if n.text.is_empty() {
                        format!("{sign} “{quote}”")
                    } else {
                        format!("{sign} {}  — “{quote}”", n.text)
                    }
                }
                (Anchor::Page, _) => format!("{sign} {}", n.text),
                _ => format!("{sign} {}  — {}", n.text, self.row_text(n.at)),
            };
            Shelved {
                label,
                at: n.at,
                item: Item::Note(i),
                color: None,
            }
        });
        let mut all: Vec<Shelved> = marks.chain(notes).collect();
        all.sort_by_key(|s| s.at);
        all
    }

    /// The passage starting at `line`, up to the next blank line.
    fn passage(&self, line: usize) -> Vec<String> {
        self.doc.lines[line.min(self.doc.lines.len())..]
            .iter()
            .skip_while(|l| l.text.trim().is_empty())
            .take_while(|l| !l.text.trim().is_empty())
            .take(12)
            .map(|l| l.text.clone())
            .collect()
    }

    fn draw_overlay(&self, f: &mut Frame) {
        if !matches!(
            self.mode,
            Mode::Reading
                | Mode::Select(_)
                | Mode::Selected { .. }
                | Mode::Marker(_)
                | Mode::Bookmark(_)
                | Mode::Link(_)
                | Mode::Search { list: None, .. }
        ) {
            let area = f.area();
            view::backdrop(f.buffer_mut(), area);
        }
        match &self.mode {
            Mode::Reading | Mode::Select(_) => {}
            Mode::Link(_) => draw_hint(f, "Links", "Enter see · f next · Esc back"),
            Mode::Peek(i) => {
                let link = self.doc.links[*i];
                let page = self.layout.page_of(Pos {
                    line: link.target,
                    offset: 0,
                }) + 1;
                draw_peek(f, &self.passage(link.target), page);
            }
            // The page stays bright: the chosen text is what is being acted on.
            Mode::Selected { from, to } if self.dragging.is_none() => {
                let quote = self.text_between(*from, *to).replace('\n', " ");
                draw_selection(f, &quote);
            }
            Mode::Selected { .. } => {}
            Mode::Search {
                query,
                hits,
                list: Some(sel),
                ..
            } => self.draw_finds(f, query, hits, *sel),
            Mode::Search {
                query,
                hits,
                current,
                typing,
                ..
            } => {
                let count = match (hits.len(), current) {
                    (0, _) if query.is_empty() => String::new(),
                    (0, _) => "no matches".to_string(),
                    (n, Some(i)) => format!("{} of {n}", i + 1),
                    (n, None) => format!("{n} found"),
                };
                draw_search(f, query, &count, typing.is_some());
            }
            Mode::Bookmark(i) => {
                if let Some(m) = self.entry.marks.get(*i) {
                    let page = self.layout.page_of(m.at) + 1;
                    draw_bookmark(f, page, &self.row_text(m.at), m.color);
                }
            }
            Mode::Marker(i) => {
                if let Some(note) = self.entry.notes.get(*i)
                    && let Some(end) = note.end
                {
                    let quote = self.text_between(note.at, end).replace('\n', " ");
                    draw_marker(f, &quote, &note.text, note.color.unwrap_or(Ribbon::Yellow));
                }
            }
            Mode::Help => draw_help(f, &self.keymap),
            Mode::Tip { index, hide } => draw_tip(f, &self.keymap, *index, *hide),
            Mode::Contents(sel) => self.draw_drawer(f, *sel),
            Mode::Shelf(sel) => {
                let entries: Vec<(String, Pos, Option<Ribbon>)> = self
                    .shelf()
                    .into_iter()
                    .map(|s| (s.label, s.at, s.color))
                    .collect();
                self.draw_list(
                    f,
                    "Bookmarks and notes",
                    "Enter go · d remove · Esc close",
                    &entries,
                    *sel,
                );
            }
            Mode::Writing {
                anchor,
                at,
                text,
                ask,
                ..
            } => {
                let page = self.layout.page_of(*at) + 1;
                let title = match (anchor, ask) {
                    (Anchor::Page, false) => format!("Note on p.{page}"),
                    (Anchor::Line, false) => "Note on this row".to_string(),
                    (Anchor::Range, false) => "Note on the chosen text".to_string(),
                    (Anchor::Page, true) => format!("Ask the agent about p.{page}"),
                    (Anchor::Line, true) => "Ask the agent about this row".to_string(),
                    (Anchor::Range, true) => "Ask the agent about the chosen text".to_string(),
                };
                draw_input(f, &title, text);
            }
        }
    }

    /// The finds, one a row with the words around them, the find lit.
    fn draw_finds(&self, f: &mut Frame, query: &str, hits: &[(Pos, Pos)], sel: usize) {
        let area = f.area();
        let width = area.width.saturating_sub(4).min(80);
        let height = view::panel_height(hits.len() as u16, 0).min(area.height.saturating_sub(2));
        let [row] = Split::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(area);
        let [popup] = Split::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(row);
        let title = format!("“{query}” — {} found", hits.len());
        let room = view::draw_panel(f.buffer_mut(), popup, &title, "Enter go · Esc back", 0);
        // Two columns go to the pointer on the chosen row.
        let inner = (room.width as usize).saturating_sub(2);
        let dim = Style::new().fg(ratatui::style::Color::Indexed(245));
        let lit = Style::new()
            .fg(ratatui::style::Color::Cyan)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        let items: Vec<ListItem> = hits
            .iter()
            .map(|&(from, to)| {
                let num = format!(" {}", self.layout.page_of(from) + 1);
                let room = inner.saturating_sub(num.width());
                let line: Vec<char> = self.doc.lines[from.line].text.chars().collect();
                // Some words before the find, the find, then what fits after.
                let lead = 24.min(room / 3);
                let mut start = from.offset;
                let mut used = 0;
                while start > 0 {
                    let w = unicode_width::UnicodeWidthChar::width(line[start - 1]).unwrap_or(0);
                    if used + w > lead {
                        break;
                    }
                    used += w;
                    start -= 1;
                }
                let cut = if start > 0 { "…" } else { "" };
                let before: String = cut.to_string()
                    + line[start..from.offset]
                        .iter()
                        .collect::<String>()
                        .trim_start();
                let found: String = line[from.offset..to.offset.min(line.len())]
                    .iter()
                    .collect();
                let after: String = line[to.offset.min(line.len())..].iter().collect();
                let after = view::fit(&after, room.saturating_sub(before.width() + found.width()));
                let gap = room.saturating_sub(before.width() + found.width() + after.width());
                ListItem::new(Line::from(vec![
                    Span::raw(before),
                    Span::styled(found, lit),
                    Span::raw(after),
                    Span::raw(" ".repeat(gap)),
                    Span::styled(num, dim),
                ]))
            })
            .collect();
        let list = List::new(items)
            .style(view::panel_style())
            .highlight_symbol(Span::styled("▶ ", view::current_find()))
            .highlight_spacing(ratatui::widgets::HighlightSpacing::Always)
            .highlight_style(Style::new().bg(ratatui::style::Color::Indexed(239)));
        let mut state = ListState::default().with_selected(Some(sel));
        f.render_stateful_widget(list, room, &mut state);
    }

    /// The contents as a drawer down the left side of the pane, the chapter
    /// open now pointed at.
    fn draw_drawer(&self, f: &mut Frame, sel: usize) {
        use ratatui::widgets::{Block, Borders, Clear};
        let area = f.area();
        let width = (area.width / 2).clamp(24, 46).min(area.width);
        let drawer = Rect::new(area.x, area.y, width, area.height);
        f.render_widget(Clear, drawer);
        let edge = Style::new().fg(ratatui::style::Color::Cyan);
        f.render_widget(
            Block::new()
                .borders(Borders::RIGHT)
                .border_style(edge)
                .style(view::panel_style()),
            drawer,
        );
        let inner = Rect::new(
            drawer.x + 2,
            drawer.y + 1,
            drawer.width.saturating_sub(5),
            drawer.height.saturating_sub(2),
        );
        if inner.height < 4 {
            return;
        }
        let buf = f.buffer_mut();
        buf.set_stringn(
            inner.x,
            inner.y,
            "Contents",
            inner.width as usize,
            view::key_style(),
        );
        let grey = Style::new().fg(ratatui::style::Color::Indexed(245));
        let hint = "Enter go · ← → fold · Esc close";
        buf.set_stringn(
            inner.x,
            inner.bottom() - 1,
            hint,
            inner.width as usize,
            grey,
        );
        let room = Rect::new(
            inner.x,
            inner.y + 2,
            inner.width,
            inner.height.saturating_sub(4),
        );

        // The chapter holding the open pages, or the folded one it is in.
        let here = self.chapter_here().map(|i| self.shown_as(i));
        let shown = self.shown_chapters();
        let top = self.doc.chapters.iter().map(|c| c.level).min().unwrap_or(1);
        let w = room.width as usize;
        let items: Vec<ListItem> = shown
            .iter()
            .map(|&i| {
                let c = &self.doc.chapters[i];
                let pointer = if Some(i) == here { "▶ " } else { "  " };
                let indent = "  ".repeat((c.level - top) as usize);
                let fold = match (self.has_sections(i), self.folded.contains(&i)) {
                    (true, true) => "▸ ",
                    (true, false) => "▾ ",
                    (false, _) => "  ",
                };
                let num = format!(" {}", self.chapter_pages[i] + 1);
                let label = view::fit(
                    &format!("{indent}{fold}{}", c.title),
                    w.saturating_sub(num.width() + pointer.width()),
                );
                let gap = w.saturating_sub(pointer.width() + label.width() + num.width());
                ListItem::new(Line::from(vec![
                    Span::styled(pointer, view::current_find()),
                    Span::raw(label),
                    Span::raw(" ".repeat(gap)),
                    Span::styled(num, grey),
                ]))
            })
            .collect();
        let list = List::new(items).style(view::panel_style()).highlight_style(
            Style::new()
                .bg(ratatui::style::Color::Cyan)
                .fg(ratatui::style::Color::Black),
        );
        let at = shown.iter().position(|&i| i == sel);
        let mut state = ListState::default().with_selected(at);
        f.render_stateful_widget(list, room, &mut state);
    }

    fn draw_list(
        &self,
        f: &mut Frame,
        title: &str,
        hint: &str,
        entries: &[(String, Pos, Option<Ribbon>)],
        sel: usize,
    ) {
        let area = f.area();
        let width = area.width.saturating_sub(4).min(72);
        let height = view::panel_height(entries.len() as u16, 0).min(area.height.saturating_sub(2));
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
            .map(|(label, at, color)| {
                let num = format!(" {}", self.layout.page_of(*at) + 1);
                let label = view::fit(label, inner.saturating_sub(num.width()));
                let gap = " ".repeat(inner.saturating_sub(label.width() + num.width()));
                // A bookmark's mark (its first character) in its ribbon's colour.
                let mut chars = label.chars();
                let lead = match color {
                    Some(c) => Span::styled(
                        chars.next().map(String::from).unwrap_or_default(),
                        Style::new().fg(view::ribbon_color(*c)),
                    ),
                    None => Span::raw(""),
                };
                ListItem::new(Line::from(vec![
                    lead,
                    Span::raw(chars.as_str().to_string()),
                    Span::raw(gap),
                    Span::styled(num, dim),
                ]))
            })
            .collect();
        let room = view::draw_panel(f.buffer_mut(), popup, title, hint, 0);
        let list = List::new(items).style(view::panel_style()).highlight_style(
            Style::new()
                .bg(ratatui::style::Color::Cyan)
                .fg(ratatui::style::Color::Black),
        );
        let mut state = ListState::default().with_selected(Some(sel));
        f.render_stateful_widget(list, room, &mut state);
    }
}

/// Adds `style` to the characters of page row `row` (set from layout row
/// `r`) that lie from `from` up to `to`.
fn paint_row(row: &mut PageRow, r: &crate::layout::Row, from: Pos, to: Pos, style: TextStyle) {
    let starts = r.pos;
    let ends = Pos {
        line: r.pos.line,
        offset: r.pos.offset + r.len,
    };
    if r.len == 0 || to <= starts || from >= ends {
        return;
    }
    let mut cells: Vec<(char, TextStyle)> = row
        .spans
        .iter()
        .flat_map(|s| s.text.chars().map(move |c| (c, s.style)))
        .collect();
    for (k, cell) in cells.iter_mut().enumerate().skip(r.lead).take(r.len) {
        let at = Pos {
            line: r.pos.line,
            offset: r.pos.offset + k - r.lead,
        };
        if from <= at && at < to {
            cell.1 = cell.1.with(style);
        }
    }
    let mut spans: Vec<crate::doc::Styled> = Vec::new();
    for (c, st) in cells {
        match spans.last_mut() {
            Some(last) if last.style == st => last.text.push(c),
            _ => spans.push(crate::doc::Styled {
                text: c.to_string(),
                style: st,
            }),
        }
    }
    row.spans = spans;
}

/// Puts `text` on the clipboard: through `pbcopy` where there is one, else
/// by asking the terminal (OSC 52), which herdr may or may not pass on.
fn copy(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Ok(mut child) = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
        return Ok(());
    }
    let mut out = std::io::stdout();
    write!(
        out,
        "\x1b]52;c;{}\x07",
        crate::pictures::base64(text.as_bytes())
    )?;
    out.flush()
}

/// Every place `query` occurs in the book, as (start, end). Letter case is
/// ignored unless the query has a capital (as less and vim do).
fn search(doc: &Document, query: &str) -> Vec<(Pos, Pos)> {
    const MOST: usize = 10_000;
    let fold = !query.chars().any(char::is_uppercase);
    let norm = |c: char| {
        if fold {
            c.to_lowercase().next().unwrap_or(c)
        } else {
            c
        }
    };
    let q: Vec<char> = query.chars().map(norm).collect();
    let mut hits = Vec::new();
    if q.is_empty() {
        return hits;
    }
    for (line, l) in doc.lines.iter().enumerate() {
        if l.kind == Kind::Rule {
            continue;
        }
        let text: Vec<char> = l.text.chars().map(norm).collect();
        let mut i = 0;
        while i + q.len() <= text.len() {
            if text[i..i + q.len()] == q[..] {
                hits.push((
                    Pos { line, offset: i },
                    Pos {
                        line,
                        offset: i + q.len(),
                    },
                ));
                if hits.len() == MOST {
                    return hits;
                }
                i += q.len();
            } else {
                i += 1;
            }
        }
    }
    hits
}

/// The search box, at the foot of the pane.
fn draw_search(f: &mut Frame, query: &str, count: &str, typing: bool) {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(80);
    let height = view::panel_height(1, 0);
    let x = area.x + (area.width - width) / 2;
    let y = area.bottom().saturating_sub(height + 3);
    let panel = Rect::new(x, y, width, height).intersection(area);
    let hint = if typing {
        "Enter keep · Esc go back"
    } else {
        "n next · N previous · l list · / change · Esc close"
    };
    let room = view::draw_panel(f.buffer_mut(), panel, "Search", hint, 0);
    let w = room.width as usize;
    let mut shown = format!("/{query}");
    while shown.width() + count.width() + 3 > w && shown.chars().count() > 1 {
        shown.remove(1);
    }
    let cursor = if typing { "▏" } else { "" };
    let gap = w.saturating_sub(shown.width() + cursor.width() + count.width());
    let line = Line::from(vec![
        Span::raw(shown),
        Span::raw(cursor),
        Span::raw(" ".repeat(gap)),
        Span::styled(
            count.to_string(),
            Style::new().fg(ratatui::style::Color::Indexed(245)),
        ),
    ]);
    f.render_widget(Paragraph::new(line).style(view::panel_style()), room);
}

/// What can be done with a clicked ribbon, in a panel at the foot.
fn draw_bookmark(f: &mut Frame, page: usize, first: &str, color: Ribbon) {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(80);
    let height = view::panel_height(1, 0);
    let x = area.x + (area.width - width) / 2;
    let y = area.bottom().saturating_sub(height + 3);
    let panel = Rect::new(x, y, width, height).intersection(area);
    let room = view::draw_panel(
        f.buffer_mut(),
        panel,
        "Bookmark",
        "c colour · d remove · Esc close",
        0,
    );
    let ribbon = Style::new().fg(view::ribbon_color(color));
    let line = Line::from(vec![
        Span::styled("██ ", ribbon),
        Span::raw(format!("p.{page}  {}  ", color.name())),
        Span::styled(
            view::fit(first, (room.width as usize).saturating_sub(20)),
            Style::new().add_modifier(Modifier::DIM),
        ),
    ]);
    f.render_widget(Paragraph::new(line).style(view::panel_style()), room);
}

/// What can be done with a clicked marker, in a panel at the foot.
fn draw_marker(f: &mut Frame, quote: &str, note: &str, color: Ribbon) {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(80);
    let rows = if note.is_empty() { 1 } else { 2 };
    let height = view::panel_height(rows, 0);
    let x = area.x + (area.width - width) / 2;
    let y = area.bottom().saturating_sub(height + 3);
    let panel = Rect::new(x, y, width, height).intersection(area);
    let hint = "c colour · n note · d remove · Esc close";
    let room = view::draw_panel(f.buffer_mut(), panel, "Marker", hint, 0);
    let swatch = Style::new()
        .bg(view::ribbon_color(color))
        .fg(ratatui::style::Color::Black);
    let w = room.width as usize;
    let mut lines = vec![Line::from(vec![
        Span::styled(
            view::fit(&format!("“{quote}”"), w.saturating_sub(12)),
            swatch,
        ),
        Span::raw(format!("  {}", color.name())),
    ])];
    if !note.is_empty() {
        lines.push(Line::raw(view::fit(&format!("✎ {note}"), w)));
    }
    f.render_widget(Paragraph::new(lines).style(view::panel_style()), room);
}

/// A one-line panel at the foot naming what the keys do now.
fn draw_hint(f: &mut Frame, title: &str, hint: &str) {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(60);
    let height = view::panel_height(0, 0);
    let x = area.x + (area.width - width) / 2;
    let y = area.bottom().saturating_sub(height + 3);
    let panel = Rect::new(x, y, width, height).intersection(area);
    view::draw_panel(f.buffer_mut(), panel, title, hint, 0);
}

/// Where a link leads: the passage there, wrapped, and its page.
fn draw_peek(f: &mut Frame, passage: &[String], page: usize) {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(80);
    let inner = width.saturating_sub(4) as usize;
    let mut lines: Vec<Line> = passage
        .iter()
        .flat_map(|p| crate::layout::set(0, &DocLine::new(p.as_str(), Kind::Body), inner.max(10)))
        .map(|r| Line::raw(r.text))
        .collect();
    let most = area.height.saturating_sub(10).max(3) as usize;
    if lines.len() > most {
        lines.truncate(most);
        lines.push(Line::raw("…"));
    }
    let height = view::panel_height(lines.len() as u16, 0);
    let x = area.x + (area.width - width) / 2;
    let y = area.bottom().saturating_sub(height + 3);
    let panel = Rect::new(x, y, width, height).intersection(area);
    let title = format!("p.{page}");
    let room = view::draw_panel(
        f.buffer_mut(),
        panel,
        &title,
        "Enter go there · Esc close",
        0,
    );
    f.render_widget(Paragraph::new(lines).style(view::panel_style()), room);
}

/// What can be done with text chosen with the mouse, in a panel at the foot.
fn draw_selection(f: &mut Frame, quote: &str) {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(80);
    let height = view::panel_height(1, 0);
    let x = area.x + (area.width - width) / 2;
    let y = area.bottom().saturating_sub(height + 3);
    let panel = Rect::new(x, y, width, height).intersection(area);
    let hint = "m marker · n note · ? ask · y copy · Esc let go";
    let room = view::draw_panel(f.buffer_mut(), panel, "Chosen", hint, 0);
    let quote = view::fit(&format!("“{quote}”"), room.width as usize);
    f.render_widget(
        Paragraph::new(Line::raw(quote)).style(view::panel_style()),
        room,
    );
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The mark a note is listed and shown with.
fn sign(note: &Note) -> &'static str {
    match (note.by, note.anchor) {
        (Author::Agent, _) => "✦",
        (Author::Reader, Anchor::Page) => "✎",
        (Author::Reader, Anchor::Line) => "▎",
        (Author::Reader, Anchor::Range) => "▌",
    }
}

/// A note set as footnote rows `width` wide: its mark, then its text in
/// italics, wrapped rows hung after the mark.
fn footnote_rows(note: &Note, width: usize) -> Vec<PageRow> {
    let mut line = DocLine::new(format!("{} {}", sign(note), note.text), Kind::Body);
    line.hang = Some(2);
    line.style = TextStyle {
        italic: true,
        ..TextStyle::default()
    };
    crate::layout::set(0, &line, width)
        .into_iter()
        .map(|r| PageRow {
            spans: r.spans,
            ..PageRow::default()
        })
        .collect()
}

/// Actions shown one a row in the key list; the list keys share a row.
const LISTED: &[Cmd] = &[
    Cmd::Next,
    Cmd::Prev,
    Cmd::Contents,
    Cmd::Search,
    Cmd::Mark,
    Cmd::Color,
    Cmd::NotePage,
    Cmd::Select,
    Cmd::Shelf,
    Cmd::NoteDisplay,
    Cmd::Ask,
    Cmd::Narrower,
    Cmd::Wider,
    Cmd::Animate,
    Cmd::Direction,
    Cmd::Help,
    Cmd::Tip,
    Cmd::Quit,
];

/// The key list, as bound now (`h`).
fn draw_help(f: &mut Frame, keymap: &Keymap) {
    let mut rows: Vec<(String, String)> = LISTED
        .iter()
        .map(|&cmd| {
            let what = keys::ACTIONS
                .iter()
                .find(|a| a.0 == cmd)
                .map(|a| a.3)
                .unwrap_or("");
            (keymap.label(cmd), what.to_string())
        })
        .collect();
    let list = [Cmd::Up, Cmd::Down, Cmd::Enter, Cmd::Delete, Cmd::Back]
        .iter()
        .map(|&c| keymap.first(c))
        .collect::<Vec<_>>()
        .join(" ");
    rows.push((list, "in lists: up, down, go, remove, back".into()));
    rows.push(("drag".into(), "choose text: marker, note, ask, copy".into()));
    rows.push((
        "click".into(),
        "a ribbon or marker: colour, remove, ...".into(),
    ));
    let area = f.area();
    let width = area.width.saturating_sub(4).min(72);
    let height = view::panel_height(rows.len() as u16, 1).min(area.height);
    let [row] = Split::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Split::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(row);
    let keys_w = rows
        .iter()
        .map(|(k, _)| k.width())
        .max()
        .unwrap_or(0)
        .min(16)
        + 2;
    let lines: Vec<Line> = rows
        .into_iter()
        .map(|(k, what)| {
            let pad = " ".repeat(keys_w.saturating_sub(k.width()));
            Line::from(vec![
                Span::styled(format!("{k}{pad}"), view::key_style()),
                Span::raw(what),
            ])
        })
        .collect();
    let hint = "any key to close · rebind in config.toml (see README)";
    let room = view::draw_panel(f.buffer_mut(), popup, "Keys", hint, 1);
    f.render_widget(Paragraph::new(lines).style(view::panel_style()), room);
}

/// Tips, one shown each time a book opens.
/// Keys are set off in backticks.
const TIPS: &[&str] = &[
    "`{help}` lists every key, any time.",
    "`{bookmark}` hangs a ribbon on the page as a bookmark; `{bookmark_color}` changes its colour.",
    "`{note}` writes a note on the page. `{choose_row}` picks a row, and `Enter` writes a note on that row.",
    "`{ask}` asks the agent beside the book about the pages in front of you. Its answer comes back as a note.",
    "`{note_display}` shows notes as footnotes, in the margin, or as marks only.",
    "`{shorter_rows}` and `{longer_rows}` shorten and lengthen the rows. Each book remembers its own.",
    "`{list}` lists the bookmarks and notes: `{enter}` goes there, `{remove}` removes one.",
    "Drag over text to choose it, then `m` lays a highlighter marker over it.",
    "Click a marker to change its colour (`c`), write a note on it (`n`), or remove it (`d`).",
    "Click a bookmark's ribbon to change its colour (`c`) or take it out (`d`).",
    "In a spread, `{bookmark}` bookmarks the page of the pane you press it in.",
    "`{search}` searches the book; `n` and `N` go to the next and previous find.",
    "After a search, `l` lists every find with the words around it.",
    "`{contents}` opens the contents, when the book has chapters.",
    "`{animation}` turns the page-turn animation off, or on again.",
    "`{direction}` turns the book round for pages that run right to left, as Japanese books and manga do.",
    "Every key can be changed under [keys] in ~/.config/herdfold/config.toml.",
];

/// A tip with `{action}` replaced by the first key bound to that action.
fn tip_text(keymap: &Keymap, index: usize) -> String {
    let mut out = TIPS[index].to_string();
    for &(cmd, name, ..) in keys::ACTIONS {
        out = out.replace(&format!("{{{name}}}"), &keymap.first(cmd));
    }
    out
}

fn draw_tip(f: &mut Frame, keymap: &Keymap, index: usize, hide: bool) {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(58);
    let inner = width.saturating_sub(4) as usize;
    // Wrap the tip as text, its keys carried along as styled runs.
    let mut plain = String::new();
    let mut runs = Vec::new();
    for (i, part) in tip_text(keymap, index).split('`').enumerate() {
        let start = plain.chars().count();
        plain.push_str(part);
        if i % 2 == 1 {
            runs.push(crate::doc::Run {
                start,
                end: plain.chars().count(),
                style: TextStyle {
                    bold: true,
                    accent: true,
                    ..TextStyle::default()
                },
            });
        }
    }
    let mut tip = DocLine::new(plain, Kind::Body);
    tip.hang = Some(0);
    tip.runs = runs;
    let rows: Vec<Line> = crate::layout::set(0, &tip, inner)
        .into_iter()
        .map(|r| {
            Line::from(
                r.spans
                    .into_iter()
                    .map(|s| Span::styled(s.text, view::style_of(s.style)))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    // The tip, a blank row and the tick box, inside the panel.
    let height = view::panel_height(rows.len() as u16 + 2, 1).min(area.height);
    let [row] = Split::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Split::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(row);
    let dim = Style::new().fg(ratatui::style::Color::Indexed(245));
    let mut lines: Vec<Line> = rows;
    lines.push(Line::raw(""));
    let tick = if hide { "[x]" } else { "[ ]" };
    let count = format!("{}/{}", index + 1, TIPS.len());
    let label = format!("{tick} Don't show tips again");
    let gap = inner.saturating_sub(label.width() + count.width());
    lines.push(Line::from(vec![
        Span::styled(tick, view::key_style()),
        Span::raw(" Don't show tips again"),
        Span::raw(" ".repeat(gap)),
        Span::styled(count, dim),
    ]));
    let room = view::draw_panel(
        f.buffer_mut(),
        popup,
        "Tip",
        "Enter close · ← → more · Space don't show again",
        1,
    );
    f.render_widget(Paragraph::new(lines).style(view::panel_style()), room);
}

/// A one-line box near the foot of the pane for writing a note.
fn draw_input(f: &mut Frame, title: &str, text: &str) {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(80);
    let height = view::panel_height(1, 0);
    let x = area.x + (area.width - width) / 2;
    let y = area.bottom().saturating_sub(height + 3);
    let box_area = Rect::new(x, y, width, height).intersection(area);
    let room = view::draw_panel(f.buffer_mut(), box_area, title, "Enter keep · Esc drop", 0);
    // Keep the end of the text, where the writing is, in view.
    let mut shown = text.to_string();
    while shown.width() + 1 > room.width as usize && !shown.is_empty() {
        shown.remove(0);
    }
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::raw(shown), Span::raw("▏")]))
            .style(view::panel_style()),
        room,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
            toast: None,
            attached: None,
            shown: None,
            animate: false,
            turning: None,
            unsent_turn: None,
            measure: view::MEASURE,
            note_display: NoteDisplay::Marks,
            notes_rev: 0,
            laid_for: None,
            pane_width: 80,
            keep_settings: false,
            agent: None,
            ribbon: Ribbon::default(),
            dragging: None,
            marker: Ribbon::Yellow,
            keymap: Keymap::default(),
            rtl: false,
            cell: None,
            folded: Default::default(),
            trail: Vec::new(),
            log: None,
            pace: None,
            socket: None,
            background: mpsc::channel(),
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

    fn texts(v: &PageView) -> Vec<String> {
        v.rows
            .iter()
            .map(|r| r.spans.iter().map(|s| s.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn footnotes_sit_at_the_foot_and_push_text_on() {
        let mut r = reader(&["one", "two", "three", "four", "five"], 5);
        keys(&mut r, &["N"]);
        assert_eq!(r.note_display, NoteDisplay::Footnotes);
        keys(&mut r, &["v", "j", "enter", "x", "enter"]);
        r.fit((20, 5), false);
        // Five rows: three of text, the rule and the note.
        assert_eq!(
            texts(&r.views().0),
            ["one", "two", "three", "────────────────", "▎ x"]
        );
        keys(&mut r, &["space"]);
        assert_eq!(texts(&r.views().0)[0], "four");
    }

    #[test]
    fn margin_notes_name_their_row() {
        let mut r = reader(&["one", "two", "three"], 3);
        keys(
            &mut r,
            &["N", "N", "v", "j", "enter", "x", "enter", "n", "p", "enter"],
        );
        assert_eq!(r.note_display, NoteDisplay::Margin);
        let notes: Vec<_> = r
            .views()
            .0
            .margin_notes
            .into_iter()
            .map(|m| (m.row, m.text))
            .collect();
        assert_eq!(notes, [(0, "✎ p".to_string()), (1, "▎ x".to_string())]);
    }

    #[test]
    fn the_question_carries_the_pages_and_the_way_back() {
        let mut r = reader(&["one", "two's", "three"], 3);
        r.socket = Some(PathBuf::from("/tmp/hb.sock"));
        let at = r.open_rows()[1];
        let p = r.prompt("why?", Anchor::Line, at, None);
        assert!(p.contains("at p.1 of 1:\n\nwhy?\n"));
        assert!(p.contains("this row:\n> two's\n"));
        assert!(p.contains("--- p.1 ---\none\ntwo's\nthree\n"));
        assert!(p.contains("HERDFOLD_SOCKET_PATH='/tmp/hb.sock' "));
        assert!(p.contains(
            "note add --line 1 --offset 0 --anchor line --question 'why?' '<your short answer>'"
        ));
    }

    #[test]
    fn messages_pass() {
        let mut r = reader(&["one"], 3);
        keys(&mut r, &["m"]);
        assert_eq!(r.views().0.status.as_deref(), Some("Bookmarked p.1 (red)"));
        r.toast.as_mut().unwrap().1 -= TOAST;
        assert_eq!(r.views().0.status, None);
    }

    #[test]
    fn a_bookmark_changes_colour_and_new_ones_follow() {
        let mut r = reader(&["one", "two"], 1);
        keys(&mut r, &["c"]);
        assert!(r.entry.marks.is_empty());
        keys(&mut r, &["m", "c", "c"]);
        assert_eq!(r.views().0.ribbon, Some(Ribbon::Green));
        keys(&mut r, &["space", "m"]);
        assert_eq!(r.entry.marks[1].color, Ribbon::Green);
    }

    #[test]
    fn q_puts_the_keys_away_rather_than_closing_the_book() {
        let mut r = reader(&["one"], 3);
        keys(&mut r, &["h"]);
        assert_eq!(r.mode, Mode::Help);
        assert!(!r.handle("q"));
        assert_eq!(r.mode, Mode::Reading);
    }

    #[test]
    fn a_tip_can_be_turned_away_for_good() {
        let mut r = reader(&["one", "two"], 1);
        r.mode = Mode::Tip {
            index: 0,
            hide: false,
        };
        // Space ticks the box; the arrows go round the tips.
        keys(&mut r, &["space", "right", "right", "left"]);
        assert_eq!(
            r.mode,
            Mode::Tip {
                index: 1,
                hide: true
            }
        );
        assert!(!r.handle("enter"));
        assert_eq!(r.mode, Mode::Reading);
        assert_eq!(
            r.views().0.status.as_deref(),
            Some("Tips off (T shows one)")
        );
        // Keys on the tip did not reach the book: still on the first page.
        assert_eq!(r.page(), 0);
    }

    fn drag(r: &mut Reader, from: (u16, u16), to: (u16, u16)) {
        // The text column fills a 20-column pane; rows start below the head.
        let size = (20, 40);
        r.mouse(
            Side::Single,
            size,
            MouseKind::Down,
            from.0,
            from.1 + view::TOP,
        );
        r.mouse(Side::Single, size, MouseKind::Drag, to.0, to.1 + view::TOP);
        r.mouse(Side::Single, size, MouseKind::Up, to.0, to.1 + view::TOP);
    }

    #[test]
    fn a_drag_chooses_text_by_character() {
        let mut r = reader(&["hello world", "second line"], 5);
        drag(&mut r, (6, 0), (2, 1));
        let Mode::Selected { from, to } = r.mode else {
            panic!("not chosen: {:?}", r.mode);
        };
        assert_eq!(r.text_between(from, to), "world\nsec");
        // Backwards drags choose the same text.
        drag(&mut r, (2, 1), (6, 0));
        assert_eq!(r.mode, Mode::Selected { from, to });
    }

    #[test]
    fn a_click_lets_a_choice_go() {
        let mut r = reader(&["hello"], 3);
        drag(&mut r, (1, 0), (3, 0));
        drag(&mut r, (1, 0), (1, 0));
        assert_eq!(r.mode, Mode::Reading);
    }

    #[test]
    fn chosen_text_is_marked_and_painted() {
        let mut r = reader(&["hello world"], 3);
        drag(&mut r, (0, 0), (4, 0));
        keys(&mut r, &["m"]);
        let note = &r.entry.notes[0];
        assert_eq!((note.anchor, note.end.unwrap().offset), (Anchor::Range, 5));
        let spans = &r.views().0.rows[0].spans;
        let marked: Vec<_> = spans
            .iter()
            .map(|s| (s.text.as_str(), s.style.marker))
            .collect();
        assert_eq!(marked, [("hello", Some(Ribbon::Yellow)), (" world", None)]);
        let labels: Vec<_> = r.shelf().into_iter().map(|s| s.label).collect();
        assert_eq!(labels, ["▌ “hello”"]);
    }

    #[test]
    fn chosen_text_takes_a_note_or_a_question() {
        let mut r = reader(&["hello world"], 3);
        drag(&mut r, (6, 0), (10, 0));
        keys(&mut r, &["n", "x", "enter"]);
        let note = &r.entry.notes[0];
        assert_eq!(
            (note.text.as_str(), note.at.offset, note.end.unwrap().offset),
            ("x", 6, 11)
        );
        drag(&mut r, (6, 0), (10, 0));
        keys(&mut r, &["?"]);
        assert!(matches!(
            r.mode,
            Mode::Writing {
                ask: true,
                end: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn each_pane_bookmarks_its_own_page() {
        let mut r = reader(&["one", "two", "three"], 1);
        r.spread = true;
        r.handle_key("m", true);
        r.handle_key("m", false);
        let pages: Vec<_> = r
            .entry
            .marks
            .iter()
            .map(|m| r.layout.page_of(m.at))
            .collect();
        assert_eq!(pages, [0, 1]);
        r.handle_key("c", true);
        assert_eq!(r.entry.marks[1].color, Ribbon::Yellow);
        assert_eq!(r.entry.marks[0].color, Ribbon::Red);
    }

    #[test]
    fn a_clicked_marker_is_recoloured_noted_and_removed() {
        let mut r = reader(&["hello world"], 3);
        drag(&mut r, (0, 0), (4, 0));
        keys(&mut r, &["m"]);
        // A click on the marked text opens it.
        drag(&mut r, (2, 0), (2, 0));
        assert_eq!(r.mode, Mode::Marker(0));
        keys(&mut r, &["c"]);
        assert_eq!(r.entry.notes[0].color, Some(Ribbon::Green));
        assert_eq!(r.marker, Ribbon::Green);
        drag(&mut r, (2, 0), (2, 0));
        keys(&mut r, &["n", "w", "h", "y", "enter"]);
        assert_eq!(r.entry.notes[0].text, "why");
        drag(&mut r, (2, 0), (2, 0));
        keys(&mut r, &["d"]);
        assert!(r.entry.notes.is_empty());
        // A click off any marker opens nothing.
        drag(&mut r, (8, 0), (8, 0));
        assert_eq!(r.mode, Mode::Reading);
    }

    #[test]
    fn colour_reaches_the_bookmark_on_the_other_page() {
        let mut r = reader(&["one", "two", "three"], 1);
        r.spread = true;
        r.handle_key("m", true);
        // Pressed over the left page, which has none: the right page's.
        r.handle_key("c", false);
        assert_eq!(r.entry.marks[0].color, Ribbon::Yellow);
    }

    #[test]
    fn a_clicked_ribbon_opens_its_bookmark() {
        let mut r = reader(&["one", "two"], 3);
        keys(&mut r, &["m"]);
        // A 40-column pane centres the 20-column text at 10; the ribbon
        // hangs two columns past its right edge, from the top.
        let size = (40, 40);
        r.mouse(Side::Single, size, MouseKind::Down, 33, 2);
        assert_eq!(r.mode, Mode::Bookmark(0));
        keys(&mut r, &["c"]);
        assert_eq!(r.entry.marks[0].color, Ribbon::Yellow);
        keys(&mut r, &["d"]);
        assert!(r.entry.marks.is_empty());
        assert_eq!(r.mode, Mode::Reading);
        // Off the ribbon, a press is text.
        r.mouse(Side::Single, size, MouseKind::Down, 33, 2);
        assert!(matches!(r.mode, Mode::Selected { .. }));
    }

    #[test]
    fn search_ignores_case_unless_asked_not_to() {
        let mut r = reader(&["Apple apple", "APPLE"], 3);
        r.doc.lines.push(DocLine::new("━", Kind::Rule));
        assert_eq!(search(&r.doc, "apple").len(), 3);
        assert_eq!(search(&r.doc, "Apple").len(), 1);
        assert!(search(&r.doc, "").is_empty());
    }

    #[test]
    fn search_finds_as_you_type_and_steps_through() {
        let mut r = reader(&["one", "two cat", "three", "cat four", "five"], 1);
        keys(&mut r, &["/", "c", "a", "t"]);
        assert_eq!(r.page(), 1);
        let Mode::Search { hits, current, .. } = &r.mode else {
            panic!("not searching");
        };
        assert_eq!((hits.len(), *current), (2, Some(0)));
        // The find on the open page is painted as the one shown, its row
        // pointed at.
        let row = &r.views().0.rows[0];
        assert!(row.pointer);
        assert!(row.spans.iter().any(|s| s.style.found && s.style.selected));
        keys(&mut r, &["enter", "n"]);
        assert_eq!(r.page(), 3);
        keys(&mut r, &["n"]);
        assert_eq!(r.page(), 1);
        keys(&mut r, &["N"]);
        assert_eq!(r.page(), 3);
    }

    #[test]
    fn a_jump_can_be_gone_back_from() {
        let mut r = reader(&["one", "two cat", "three", "cat four", "five"], 1);
        keys(&mut r, &["space"]);
        assert_eq!(r.page(), 1);
        keys(&mut r, &["backspace"]);
        assert_eq!(r.page(), 1, "a turn is not a jump");
        // A search is one jump, however many finds it steps through.
        keys(
            &mut r,
            &["space", "/", "c", "a", "t", "enter", "n", "n", "esc"],
        );
        assert_eq!(r.page(), 3);
        keys(&mut r, &["backspace"]);
        assert_eq!(r.page(), 2);
        keys(&mut r, &["backspace"]);
        assert_eq!(r.page(), 2, "nothing left to go back to");
    }

    #[test]
    fn a_link_shows_where_it_leads_and_goes_there() {
        let mut r = reader(&["see note 1", "two", "three", "the note"], 1);
        r.doc.links.push(crate::doc::Link {
            line: 0,
            start: 9,
            end: 10,
            target: 3,
        });
        keys(&mut r, &["space", "f"]);
        assert_eq!(r.mode, Mode::Reading, "no link on the second page");
        keys(&mut r, &["b", "f"]);
        assert_eq!(r.mode, Mode::Link(0));
        assert!(r.views().0.rows[0].spans.iter().any(|s| s.style.selected));
        keys(&mut r, &["enter"]);
        assert_eq!(r.mode, Mode::Peek(0));
        assert_eq!(r.passage(3), ["the note"]);
        assert_eq!(r.page(), 0, "seeing is not going");
        keys(&mut r, &["enter"]);
        assert_eq!((r.page(), &r.mode), (3, &Mode::Reading));
        keys(&mut r, &["backspace"]);
        assert_eq!(r.page(), 0);
    }

    #[test]
    fn what_is_left_is_shown_beside_the_number() {
        let mut r = reader(&["a", "b", "c", "d", "e"], 1);
        r.chapter_pages = vec![0, 3];
        r.doc.chapters = [("One", 0), ("Two", 3)]
            .map(|(title, line)| crate::doc::Chapter {
                title: title.into(),
                level: 1,
                line,
            })
            .to_vec();
        assert_eq!(
            r.views().0.remaining.as_deref(),
            Some("2 pages left in chapter")
        );
        r.pace = Some(90.0);
        keys(&mut r, &["space", "space"]);
        assert_eq!(
            r.views().0.remaining.as_deref(),
            Some("about 3 min to the end")
        );
        r.spread = true;
        let (left, right) = r.views();
        assert_eq!(left.remaining, None, "only beside the last open page");
        assert!(right.unwrap().remaining.is_some());
    }

    #[test]
    fn the_finds_are_listed_and_one_is_gone_to() {
        let mut r = reader(&["cat", "two", "a cat", "four", "cat"], 1);
        keys(&mut r, &["/", "c", "a", "t", "enter", "l"]);
        assert!(matches!(r.mode, Mode::Search { list: Some(0), .. }));
        keys(&mut r, &["j", "j", "j", "k", "enter"]);
        let Mode::Search { list, current, .. } = &r.mode else {
            panic!("search closed");
        };
        assert_eq!((*list, *current), (None, Some(1)));
        assert_eq!(r.page(), 2);
        // Esc in the list goes back to the search, not out of it.
        keys(&mut r, &["l", "esc"]);
        assert!(matches!(r.mode, Mode::Search { list: None, .. }));
    }

    #[test]
    fn dropping_a_search_while_typing_goes_back() {
        let mut r = reader(&["one", "two", "three cat"], 1);
        keys(&mut r, &["/", "c", "a", "t"]);
        assert_eq!(r.page(), 2);
        // Esc drops the search, and only the search.
        assert!(!r.handle("esc"));
        assert_eq!((r.page(), r.mode.clone()), (0, Mode::Reading));
    }

    #[test]
    fn closing_a_search_leaves_the_book_open() {
        let mut r = reader(&["cat", "two"], 1);
        keys(&mut r, &["/", "c", "enter", "l", "enter"]);
        assert!(!r.handle("q"), "q closed the book, not the search");
        keys(&mut r, &["/", "c", "enter"]);
        assert!(!r.handle("esc"), "Esc closed the book, not the search");
        assert_eq!(r.mode, Mode::Reading);
    }

    #[test]
    fn another_key_ends_a_search_and_does_its_own_work() {
        let mut r = reader(&["cat", "two", "three"], 1);
        keys(&mut r, &["/", "c", "enter", "space"]);
        assert_eq!((r.page(), r.mode.clone()), (1, Mode::Reading));
    }

    #[test]
    fn rebound_keys_take_over() {
        let mut r = reader(&["one", "two", "three"], 1);
        r.keymap = Keymap::parse("[keys]\nnext_page = \"l\"\nlist = \"L\"\n").0;
        keys(&mut r, &["space"]);
        assert_eq!(r.page(), 0);
        keys(&mut r, &["l"]);
        assert_eq!(r.page(), 1);
    }

    #[test]
    fn tips_name_the_keys_as_bound() {
        let k = Keymap::parse("[keys]\nhelp = \"?\"\nask = \"A\"\n").0;
        assert_eq!(tip_text(&k, 0), "`?` lists every key, any time.");
        assert!(tip_text(&Keymap::default(), 0).starts_with("`h`"));
        assert!(
            TIPS.iter()
                .enumerate()
                .all(|(i, _)| !tip_text(&k, i).contains('{'))
        );
    }

    fn numbers(r: &Reader) -> (usize, Option<usize>) {
        let (l, rt) = r.views();
        (l.number, rt.map(|v| v.number))
    }

    #[test]
    fn a_book_bound_on_the_right_reads_from_the_right_page() {
        let mut r = reader(&["one", "two", "three"], 1);
        r.spread = true;
        keys(&mut r, &["D"]);
        assert!(r.rtl);
        assert_eq!(r.entry.direction, Some(Direction::RightToLeft));
        // Page 1 on the right, page 2 on the left.
        assert_eq!(numbers(&r), (2, Some(1)));
        // The left arrow goes on; the right arrow comes back.
        keys(&mut r, &["left"]);
        assert_eq!(numbers(&r), (0, Some(3)), "a lone last page faces a blank");
        keys(&mut r, &["right"]);
        assert_eq!(numbers(&r), (2, Some(1)));
        // m in the right-hand pane marks the right page, here the first.
        r.handle_key("m", true);
        assert_eq!(r.layout.page_of(r.entry.marks[0].at), 0);
        keys(&mut r, &["D"]);
        assert_eq!(numbers(&r), (1, Some(2)));
    }

    #[test]
    fn arrows_only_fold_and_unfold_the_contents() {
        let mut r = reader(&["x"], 1);
        r.doc = crate::formats::load(
            crate::formats::Format::Md,
            b"# One\n\na\n\n## One.1\n\nb\n\n## One.2\n\nc\n\n# Two\n\nd\n".to_vec(),
            "x",
            None,
        )
        .unwrap();
        r.laid_for = None;
        r.fit((20, 4), false);
        let shown = |r: &Reader| r.shown_chapters();
        keys(&mut r, &["g"]);
        assert_eq!(r.mode, Mode::Contents(0));
        assert_eq!(shown(&r), [0, 1, 2, 3]);
        // Left folds the chapter with sections; the page does not move.
        keys(&mut r, &["left"]);
        assert_eq!((shown(&r), r.page()), (vec![0, 3], 0));
        assert_eq!(r.mode, Mode::Contents(0));
        // Down skips what is folded away.
        keys(&mut r, &["j"]);
        assert_eq!(r.mode, Mode::Contents(3));
        // A chapter without sections ignores the arrows.
        keys(&mut r, &["left", "right"]);
        assert_eq!((r.mode.clone(), shown(&r)), (Mode::Contents(3), vec![0, 3]));
        keys(&mut r, &["k", "right"]);
        assert_eq!(shown(&r), [0, 1, 2, 3]);
        // The bookmark list ignores them too.
        keys(&mut r, &["esc", "m", "l", "right", "left"]);
        assert_eq!(r.mode, Mode::Shelf(0));
    }

    #[test]
    fn base64_pads() {
        use crate::pictures::base64;
        assert_eq!(base64(b"hi"), "aGk=");
        assert_eq!(base64(b"hello"), "aGVsbG8=");
        assert_eq!(base64("あ".as_bytes()), "44GC");
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
