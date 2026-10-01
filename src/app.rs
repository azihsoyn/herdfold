//! The reader: holds the place in the book, turns pages, keeps bookmarks and
//! notes, and answers the socket API for the pane showing the right-hand page.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event};
use ratatui::layout::{Constraint, Flex, Layout as Split, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use unicode_width::UnicodeWidthStr;

use crate::api::{Call, EventData, PROTOCOL, Request, ResponseResult, SOCKET_ENV};
use crate::doc::Document;
use crate::doc::{Kind, Line as DocLine, Style as TextStyle};
use crate::herdr::{self, RightPane};
use crate::layout::{Layout, Pos};
use crate::marks::{self, Anchor, Author, Entry, Mark, Note, NoteDisplay, Ribbon};
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
    /// Writing a note on a page or a row, or (`ask`) a question about it
    /// for the agent.
    Writing {
        anchor: Anchor,
        at: Pos,
        text: String,
        ask: bool,
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
    /// Where this reader's socket is, for an agent to write notes back.
    socket: Option<PathBuf>,
    /// Outcomes of work done off the main loop (asking the agent).
    background: (Sender<String>, Receiver<String>),
}

/// What a layout depends on besides the document.
#[derive(Clone, Copy, PartialEq)]
struct LaidFor {
    width: usize,
    height: usize,
    spread: bool,
    notes: NoteDisplay,
    notes_rev: u64,
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
    agent: Option<String>,
) -> Result<()> {
    let entry = book.as_deref().and_then(marks::load).unwrap_or_default();
    let settings = marks::settings();
    let measure = starting_measure(measure, entry.measure, settings.measure);
    let animate = animate.or(settings.animate).unwrap_or(true);
    let note_display = settings.notes.unwrap_or_default();
    let mut reader = Reader {
        layout: Layout::new(&doc, 1, 1),
        doc,
        book,
        entry,
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
        socket: None,
        background: mpsc::channel(),
    };
    // Always listening: the right-hand page attaches here, and an agent
    // writes its answers back here.
    let server = Server::listen().ok();
    reader.socket = server.as_ref().map(|s| s.path.clone());
    // A tip greets the book, a different one each time, until switched off.
    if settings.tips != Some(false) {
        let index = settings.next_tip.unwrap_or(0) % TIPS.len();
        reader.mode = Mode::Tip { index, hide: false };
        let _ = reader.update_settings(|s| s.next_tip = Some(index + 1));
    }
    let mut terminal = ratatui::init();
    let result = reader.run(&mut terminal, server.as_ref(), spread);
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
            while let Ok(status) = self.background.1.try_recv() {
                self.say(status);
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
            Call::NoteAdd(p) => {
                if p.text.trim().is_empty() {
                    return hub.fail(conn, id, "invalid_params", "a note needs text");
                }
                let at = p.at.unwrap_or_else(|| self.layout.start_of(self.page()));
                let page = self.layout.page_of(at) + 1;
                self.entry.add_note(Note {
                    at,
                    anchor: p.anchor,
                    text: p.text.trim().to_string(),
                    by: p.by,
                    question: p.question,
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
                .map(|n| (n.at, footnote_rows(n, width).len()))
                .collect();
            Layout::with_footnotes(&self.doc, width, height, &footnotes)
        } else {
            Layout::new(&self.doc, width, height)
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
                        Anchor::Line => pad + self.layout.row_of(n, note.at).unwrap_or(0),
                    };
                    margin_notes.push(MarginNote {
                        row,
                        text: format!("{} {}", sign(note), note.text),
                    });
                }
            }
            NoteDisplay::Footnotes | NoteDisplay::Marks => {}
        }
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
            margin_notes,
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

    /// Handles one key, by herdr's key name. Returns true to quit.
    fn handle(&mut self, key: &str) -> bool {
        self.toast = None;
        if let Mode::Writing { .. } = self.mode {
            self.write(key);
            return false;
        }
        if let Mode::Tip { index, hide } = self.mode {
            self.tip_key(key, index, hide);
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
            // Any key puts the list of keys away; q too, rather than closing the book.
            Mode::Help => {
                self.mode = Mode::Reading;
                false
            }
            Mode::Writing { .. } | Mode::Tip { .. } => false,
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
            Cmd::NoteDisplay => self.cycle_note_display(),
            Cmd::Help => self.mode = Mode::Help,
            Cmd::Tip => {
                let settings = marks::settings();
                let index = settings.next_tip.unwrap_or(0) % TIPS.len();
                let hide = self.keep_settings && settings.tips == Some(false);
                self.mode = Mode::Tip { index, hide };
            }
            Cmd::Color => self.recolor_mark(page),
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
                    let open = page + self.step();
                    let here = self.chapter_pages.iter().rposition(|&p| p < open);
                    self.mode = Mode::Contents(here.unwrap_or(0));
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
                    at: self.layout.start_of(page),
                    text: String::new(),
                    ask: cmd == Cmd::Ask,
                };
            }
            Cmd::Select => match self.open_rows().first() {
                Some(&at) => self.mode = Mode::Select(at),
                None => self.say("Nothing on this page to choose".into()),
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
                    text: String::new(),
                    ask: cmd == Cmd::Ask,
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
            text,
            ask,
        } = &mut self.mode
        else {
            return;
        };
        match key {
            "enter" if *ask => {
                let question = text.trim().to_string();
                let (anchor, at) = (*anchor, *at);
                self.mode = Mode::Reading;
                if !question.is_empty() {
                    self.ask(&question, anchor, at);
                }
            }
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
    fn ask(&mut self, question: &str, anchor: Anchor, at: Pos) {
        let agent = match herdr::find_agent(self.agent.as_deref()) {
            Ok(a) => a,
            Err(e) => {
                self.say(e);
                return;
            }
        };
        let prompt = self.prompt(question, anchor, at);
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
    fn prompt(&self, question: &str, anchor: Anchor, at: Pos) -> String {
        let pages = self.open_pages();
        let chapter = self
            .chapter_pages
            .iter()
            .rposition(|&p| p <= pages.start)
            .map(|i| format!(", in \"{}\"", self.doc.chapters[i].title))
            .unwrap_or_default();
        let source = self.book.as_deref().unwrap_or("stdin");
        let mut out = format!(
            "Someone reading \"{}\" ({source}) in herdbook asks, at p.{} of {}{chapter}:\n\n{question}\n",
            self.doc.title,
            pages.start + 1,
            self.layout.page_count(),
        );
        if anchor == Anchor::Line {
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
                .unwrap_or_else(|_| "herdbook".into());
            let anchor = match anchor {
                Anchor::Page => "page",
                Anchor::Line => "line",
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

    /// A bookmark covers what is open: one page, or both pages of a spread.
    /// A new one takes the colour last chosen.
    fn toggle_mark(&mut self, page: usize) {
        let open = page..page + self.step();
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
        } else {
            self.say("Bookmark removed".into());
        }
        self.save();
    }

    /// Gives the bookmark on the open pages its next colour, which new
    /// bookmarks then take too.
    fn recolor_mark(&mut self, page: usize) {
        let open = page..page + self.step();
        let layout = &self.layout;
        let Some(mark) = self
            .entry
            .marks
            .iter_mut()
            .find(|m| open.contains(&layout.page_of(m.at)))
        else {
            self.say("No bookmark here (m to place one)".into());
            return;
        };
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
            let label = match n.anchor {
                Anchor::Page => format!("{sign} {}", n.text),
                Anchor::Line => format!("{sign} {}  — {}", n.text, self.row_text(n.at)),
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

    fn draw_overlay(&self, f: &mut Frame) {
        match &self.mode {
            Mode::Reading | Mode::Select(_) => {}
            Mode::Help => draw_help(f),
            Mode::Tip { index, hide } => draw_tip(f, *index, *hide),
            Mode::Contents(sel) => {
                let top = self.doc.chapters.iter().map(|c| c.level).min().unwrap_or(1);
                let entries: Vec<(String, Pos, Option<Ribbon>)> = self
                    .doc
                    .chapters
                    .iter()
                    .map(|c| {
                        let indent = "  ".repeat((c.level - top) as usize);
                        let at = Pos {
                            line: c.line,
                            offset: 0,
                        };
                        (format!("{indent}{}", c.title), at, None)
                    })
                    .collect();
                self.draw_list(f, " Contents ", &entries, *sel);
            }
            Mode::Shelf(sel) => {
                let entries: Vec<(String, Pos, Option<Ribbon>)> = self
                    .shelf()
                    .into_iter()
                    .map(|s| (s.label, s.at, s.color))
                    .collect();
                self.draw_list(
                    f,
                    " Bookmarks and notes — Enter to go, d to remove ",
                    &entries,
                    *sel,
                );
            }
            Mode::Writing {
                anchor,
                at,
                text,
                ask,
            } => {
                let page = self.layout.page_of(*at) + 1;
                let title = match (anchor, ask) {
                    (Anchor::Page, false) => format!(" Note on p.{page} "),
                    (Anchor::Line, false) => " Note on this row ".to_string(),
                    (Anchor::Page, true) => format!(" Ask the agent about p.{page} "),
                    (Anchor::Line, true) => " Ask the agent about this row ".to_string(),
                };
                draw_input(f, &title, text);
            }
        }
    }

    fn draw_list(
        &self,
        f: &mut Frame,
        title: &str,
        entries: &[(String, Pos, Option<Ribbon>)],
        sel: usize,
    ) {
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
        let list = List::new(items)
            .block(Block::bordered().title(title.to_string()))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        let mut state = ListState::default().with_selected(Some(sel));
        f.render_widget(Clear, popup);
        f.render_stateful_widget(list, popup, &mut state);
    }
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

/// The keys, as `h` lists them.
const KEYS: &[(&str, &str)] = &[
    ("Space  →", "turn the page"),
    ("b  ←", "turn back"),
    ("g", "contents"),
    ("", ""),
    ("m", "bookmark this page (again to remove)"),
    ("c", "colour of the bookmark here"),
    ("n", "write a note on this page"),
    ("v", "choose a row: Enter to note it, ? to ask"),
    ("l", "bookmarks and notes (Enter go, d remove)"),
    ("N", "notes as footnotes / in margin / marks"),
    ("?", "ask the agent about these pages"),
    ("", ""),
    ("<  >", "shorter / longer rows"),
    ("a", "page-turn animation on / off"),
    ("h", "these keys"),
    ("T", "a tip"),
    ("q  Esc", "close the book"),
];

fn draw_help(f: &mut Frame) {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(56);
    let height = area.height.saturating_sub(2).min(KEYS.len() as u16 + 2);
    let [row] = Split::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Split::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(row);
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let lines: Vec<Line> = KEYS
        .iter()
        .map(|(k, what)| {
            Line::from(vec![
                Span::styled(format!(" {k:<9}"), bold),
                Span::raw(*what),
            ])
        })
        .collect();
    let block = Block::bordered()
        .title(" Keys ")
        .title_bottom(" any key to close ");
    f.render_widget(Clear, popup);
    f.render_widget(Paragraph::new(lines).block(block), popup);
}

/// Tips, one shown each time a book opens.
const TIPS: &[&str] = &[
    "h lists every key, any time.",
    "m hangs a ribbon on the page as a bookmark; c changes its colour.",
    "n writes a note on the page. v picks a row, and Enter writes a note on that row.",
    "? asks the agent beside the book about the pages in front of you. Its answer comes back as a note.",
    "N shows notes as footnotes, in the margin, or as marks only.",
    "< and > shorten and lengthen the rows. Each book remembers its own.",
    "l lists the bookmarks and notes: Enter goes there, d removes one.",
    "g opens the contents, when the book has chapters.",
    "a turns the page-turn animation off, or on again.",
];

fn draw_tip(f: &mut Frame, index: usize, hide: bool) {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(56);
    let inner = width.saturating_sub(4) as usize;
    let mut tip = DocLine::new(TIPS[index], Kind::Body);
    tip.hang = Some(0);
    let rows: Vec<String> = crate::layout::set(0, &tip, inner)
        .into_iter()
        .map(|r| r.text)
        .collect();
    // A blank row, the tip, a blank row, the tick box, and the frame.
    let height = (rows.len() as u16 + 5).min(area.height);
    let [row] = Split::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Split::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(row);
    let dim = Style::new().add_modifier(Modifier::DIM);
    let mut lines: Vec<Line> = vec![Line::raw("")];
    lines.extend(rows.into_iter().map(|r| Line::raw(format!(" {r}"))));
    lines.push(Line::raw(""));
    let tick = if hide { "[x]" } else { "[ ]" };
    let count = format!("{}/{} ", index + 1, TIPS.len());
    let label = format!(" {tick} Don't show tips again");
    let gap = (width as usize).saturating_sub(2 + label.width() + count.width());
    lines.push(Line::from(vec![
        Span::raw(label),
        Span::raw(" ".repeat(gap)),
        Span::styled(count, dim),
    ]));
    let block = Block::bordered()
        .title(" Tip ")
        .title_bottom(" Enter close · ← → more · Space don't show again ");
    f.render_widget(Clear, popup);
    f.render_widget(Paragraph::new(lines).block(block), popup);
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
        let p = r.prompt("why?", Anchor::Line, at);
        assert!(p.contains("at p.1 of 1:\n\nwhy?\n"));
        assert!(p.contains("this row:\n> two's\n"));
        assert!(p.contains("--- p.1 ---\none\ntwo's\nthree\n"));
        assert!(p.contains("HERDBOOK_SOCKET_PATH='/tmp/hb.sock' "));
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
