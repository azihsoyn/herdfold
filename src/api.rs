//! The socket API, in herdr's shape: newline-delimited JSON over a Unix
//! socket. A request is `{"id","method","params"}`; the reply is
//! `{"id","result":{"type",..}}` or `{"id","error":{"code","message"}}`.
//! After `events.subscribe` the same connection also carries
//! `{"event","data":{"type",..}}`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::formats::Format;
use crate::layout::Pos;
use crate::marks::{Anchor, Author, Direction, NoteDisplay, Ribbon, Writing};
use crate::turn::Turn;
use crate::view::PageView;

pub const PROTOCOL: u32 = 1;
pub const SCHEMA_VERSION: u32 = 1;
/// Where a reader's socket is, for the processes it starts.
pub const SOCKET_ENV: &str = "HERDFOLD_SOCKET_PATH";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "Request")]
pub struct Request {
    pub id: String,
    #[serde(flatten)]
    pub call: Call,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", content = "params")]
pub enum Call {
    #[serde(rename = "ping")]
    Ping(EmptyParams),
    #[serde(rename = "events.subscribe")]
    EventsSubscribe(EventsSubscribeParams),
    /// Become the pane that shows the right-hand page.
    #[serde(rename = "reader.attach")]
    ReaderAttach(ReaderSizeParams),
    /// The attached pane changed size.
    #[serde(rename = "reader.resize")]
    ReaderResize(ReaderSizeParams),
    /// Keys pressed elsewhere, by herdr's key names (`space`, `esc`, `ctrl+c`, `b`, ...).
    #[serde(rename = "reader.send_keys")]
    ReaderSendKeys(ReaderSendKeysParams),
    /// A mouse button pressed, dragged or released in the attached pane,
    /// at a cell of that pane.
    #[serde(rename = "reader.send_mouse")]
    ReaderSendMouse(ReaderMouseParams),
    /// What is open, where, and how it is set.
    #[serde(rename = "reader.state")]
    ReaderState(EmptyParams),
    /// Turn the page forward or back, as Space and b do.
    #[serde(rename = "reader.turn")]
    ReaderTurn(ReaderTurnParams),
    /// Go to a page, a chapter or a place: a jump, which `reader.back`
    /// comes back from.
    #[serde(rename = "reader.go_to")]
    ReaderGoTo(ReaderGoToParams),
    /// Go back to where the last jump left from.
    #[serde(rename = "reader.back")]
    ReaderBack(EmptyParams),
    /// Change how the book is set; what is left out stays as it is.
    #[serde(rename = "reader.set")]
    ReaderSet(ReaderSetParams),
    /// Close the book.
    #[serde(rename = "reader.close")]
    ReaderClose(EmptyParams),
    /// The book's chapters, with the pages they open on.
    #[serde(rename = "contents.list")]
    ContentsList(EmptyParams),
    /// Find words in the book. Nothing moves.
    #[serde(rename = "search.run")]
    SearchRun(SearchRunParams),
    /// Links in the book: notes and cross-references, with where they lead.
    #[serde(rename = "link.list")]
    LinkList(LinkListParams),
    /// Go where a link leads, as a jump.
    #[serde(rename = "link.follow")]
    LinkFollow(IndexParams),
    #[serde(rename = "bookmark.list")]
    BookmarkList(EmptyParams),
    /// Bookmark a page: the first open page when none is named.
    #[serde(rename = "bookmark.add")]
    BookmarkAdd(BookmarkAddParams),
    /// Recolour a bookmark.
    #[serde(rename = "bookmark.update")]
    BookmarkUpdate(BookmarkUpdateParams),
    #[serde(rename = "bookmark.remove")]
    BookmarkRemove(IndexParams),
    #[serde(rename = "note.list")]
    NoteList(EmptyParams),
    /// Write a note in the book, as an agent answering a question does.
    #[serde(rename = "note.add")]
    NoteAdd(NoteAddParams),
    /// Rewrite a note, or recolour a marker.
    #[serde(rename = "note.update")]
    NoteUpdate(NoteUpdateParams),
    #[serde(rename = "note.remove")]
    NoteRemove(IndexParams),
    /// Ask an agent beside the book about the open pages (or a place in
    /// them); it answers in its own pane, and back in the book as a note.
    #[serde(rename = "question.ask")]
    QuestionAsk(QuestionAskParams),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnDirection {
    Forward,
    Backward,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReaderTurnParams {
    pub direction: TurnDirection,
}

/// Where to go: one of a page (from 1), a chapter (its index in
/// `contents.list`), or a place in the text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReaderGoToParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Pos>,
}

/// How the book is set. Each is optional in `reader.set`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReaderSetParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writing: Option<Writing>,
    /// Longest row, in columns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure: Option<usize>,
    /// Whether page turns are drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_display: Option<NoteDisplay>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SearchRunParams {
    /// Case is ignored unless the words have a capital letter.
    pub query: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LinkListParams {
    /// Only the links on the open pages.
    #[serde(default)]
    pub open: bool,
}

/// One of a list, by its `index` there.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IndexParams {
    pub index: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BookmarkAddParams {
    /// The page (from 1); the first open page when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    /// The ribbon's colour; the one last chosen when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Ribbon>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BookmarkUpdateParams {
    pub index: usize,
    pub color: Ribbon,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoteUpdateParams {
    pub index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// For a marker, its colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Ribbon>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionAskParams {
    pub question: String,
    /// What the question is about; the open pages when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Pos>,
    /// For a passage, where it ends (exclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Pos>,
    #[serde(default)]
    pub anchor: Anchor,
}

/// The book a reader has open, or a session read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Book {
    /// The book's file, as its place and marks are kept under; none for
    /// stdin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub title: String,
    pub format: Format,
}

/// A place in the book: in the text, and on the pages as they are set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Place {
    pub line: usize,
    pub offset: usize,
    /// Page number, from 1, as the pages are set.
    pub page: usize,
    /// Pages in the book, as set.
    pub pages: usize,
    /// The chapter the place is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter: Option<String>,
}

impl Place {
    pub fn pos(&self) -> Pos {
        Pos {
            line: self.line,
            offset: self.offset,
        }
    }
}

/// How the book is set, as `reader.set` changes it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReaderSettings {
    pub direction: Direction,
    pub writing: Writing,
    pub measure: usize,
    pub animation: bool,
    pub note_display: NoteDisplay,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReaderStateInfo {
    pub book: Book,
    /// Where the open pages begin.
    pub at: Place,
    /// The pages open, from 1, in reading order.
    pub open: Vec<usize>,
    /// Whether the book is open as a spread across two panes.
    pub spread: bool,
    pub settings: ReaderSettings,
    pub bookmarks: usize,
    pub notes: usize,
    /// How many jumps `reader.back` can go back along.
    pub trail: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChapterInfo {
    pub index: usize,
    pub title: String,
    pub level: u8,
    pub page: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FindInfo {
    pub at: Pos,
    pub end: Pos,
    pub page: usize,
    /// The row the find is in.
    pub context: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LinkInfo {
    pub index: usize,
    /// The link's own text.
    pub text: String,
    pub at: Pos,
    pub page: usize,
    pub target: Place,
    /// The passage it leads to.
    pub passage: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BookmarkInfo {
    pub index: usize,
    pub at: Place,
    pub color: Ribbon,
    /// The row the bookmarked page starts with.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoteInfo {
    pub index: usize,
    pub at: Place,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Pos>,
    pub anchor: Anchor,
    pub by: Author,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// The text under a marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Ribbon>,
}

/// How the reader came to a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Move {
    /// Turned to, page by page.
    Turn,
    /// Jumped to: a chapter, a bookmark, a find, a link, a page.
    Jump,
    /// Back along the trail of jumps.
    Back,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EmptyParams {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EventsSubscribeParams {
    pub subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum Subscription {
    #[serde(rename = "page.shown")]
    PageShown,
    #[serde(rename = "reader.closed")]
    ReaderClosed,
    #[serde(rename = "session.started")]
    SessionStarted,
    #[serde(rename = "session.ended")]
    SessionEnded,
    #[serde(rename = "reader.moved")]
    ReaderMoved,
    #[serde(rename = "settings.changed")]
    SettingsChanged,
    #[serde(rename = "bookmark.added")]
    BookmarkAdded,
    #[serde(rename = "bookmark.changed")]
    BookmarkChanged,
    #[serde(rename = "bookmark.removed")]
    BookmarkRemoved,
    #[serde(rename = "note.added")]
    NoteAdded,
    #[serde(rename = "note.changed")]
    NoteChanged,
    #[serde(rename = "note.removed")]
    NoteRemoved,
    #[serde(rename = "search.done")]
    SearchDone,
    #[serde(rename = "question.asked")]
    QuestionAsked,
    #[serde(rename = "book.finished")]
    BookFinished,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReaderSizeParams {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReaderMouseParams {
    pub kind: MouseKind,
    pub col: u16,
    pub row: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MouseKind {
    Down,
    Drag,
    Up,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoteAddParams {
    pub text: String,
    /// Where the note goes; the page open now when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Pos>,
    /// For a range, where it ends (exclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Pos>,
    #[serde(default)]
    pub anchor: Anchor,
    /// Who wrote it; an agent when absent.
    #[serde(default = "agent")]
    pub by: Author,
    /// The question the note answers, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// For a range, the colour of its highlighter; the one last chosen
    /// when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Ribbon>,
}

fn agent() -> Author {
    Author::Agent
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReaderSendKeysParams {
    pub keys: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "SuccessResponse")]
pub struct SuccessResponse {
    pub id: String,
    pub result: ResponseResult,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseResult {
    Pong {
        version: String,
        protocol: u32,
    },
    SubscriptionStarted,
    ReaderAttached,
    /// The note is in the book, on page `page` (1-based) as now set.
    NoteAdded {
        page: usize,
    },
    ReaderState {
        state: ReaderStateInfo,
    },
    /// Where the reader is now.
    Moved {
        at: Place,
    },
    Settings {
        settings: ReaderSettings,
    },
    Contents {
        chapters: Vec<ChapterInfo>,
    },
    SearchResults {
        query: String,
        finds: Vec<FindInfo>,
    },
    Links {
        links: Vec<LinkInfo>,
    },
    Bookmarks {
        bookmarks: Vec<BookmarkInfo>,
    },
    BookmarkAdded {
        bookmark: BookmarkInfo,
    },
    Notes {
        notes: Vec<NoteInfo>,
    },
    /// The question is on its way to the agent named.
    QuestionSent {
        agent: String,
    },
    Ok,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "ErrorResponse")]
pub struct ErrorResponse {
    pub id: String,
    pub error: ErrorBody,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

impl ErrorResponse {
    pub fn new(id: impl Into<String>, code: &str, message: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            error: ErrorBody {
                code: code.to_string(),
                message: message.into(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "EventEnvelope")]
pub struct EventEnvelope {
    pub event: EventKind,
    pub data: EventData,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    PageShown,
    ReaderClosed,
    SessionStarted,
    SessionEnded,
    ReaderMoved,
    SettingsChanged,
    BookmarkAdded,
    BookmarkChanged,
    BookmarkRemoved,
    NoteAdded,
    NoteChanged,
    NoteRemoved,
    SearchDone,
    QuestionAsked,
    BookFinished,
}

/// What happened. The reading log keeps these too, but for the pages shown
/// and the reader closing: a log record is one of these with a session and
/// a time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
// Built once per page turn; boxing the pages would only add indirection.
#[allow(clippy::large_enum_variant)]
pub enum EventData {
    /// What is open now: one page, or the two pages of a spread.
    PageShown {
        left: PageView,
        right: Option<PageView>,
        /// Set when the pages were turned to (rather than jumped to), so
        /// panes can draw the turn.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn: Option<Turn>,
    },
    ReaderClosed,
    /// The book was opened; the first of a session.
    SessionStarted {
        book: Book,
        at: Place,
        /// The herdfold reading it.
        version: String,
    },
    /// The book was closed; the last of a session that ended well.
    SessionEnded {
        at: Place,
        /// Different pages shown during the session.
        pages_read: usize,
        /// How long the book was open.
        seconds: u64,
    },
    /// The open pages begin somewhere else now.
    ReaderMoved {
        at: Place,
        #[serde(default = "turned")]
        how: Move,
    },
    SettingsChanged {
        settings: ReaderSettings,
    },
    BookmarkAdded {
        at: Place,
        color: Ribbon,
    },
    BookmarkChanged {
        at: Place,
        color: Ribbon,
    },
    BookmarkRemoved {
        at: Place,
    },
    /// A note, or a highlighter marker (`anchor` is `range`, `quote` the
    /// text under it), was written.
    NoteAdded {
        at: Place,
        anchor: Anchor,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quote: Option<String>,
        #[serde(default)]
        by: Author,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        question: Option<String>,
    },
    NoteChanged {
        at: Place,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<Ribbon>,
    },
    NoteRemoved {
        at: Place,
    },
    /// Words were searched for.
    #[serde(alias = "searched")]
    SearchDone {
        query: String,
        finds: usize,
    },
    /// An agent was asked a question.
    #[serde(alias = "asked")]
    QuestionAsked {
        at: Place,
        question: String,
    },
    /// The last page was reached.
    #[serde(alias = "finished")]
    BookFinished {
        at: Place,
    },
}

fn turned() -> Move {
    Move::Turn
}

impl EventData {
    pub fn kind(&self) -> EventKind {
        match self {
            Self::PageShown { .. } => EventKind::PageShown,
            Self::ReaderClosed => EventKind::ReaderClosed,
            Self::SessionStarted { .. } => EventKind::SessionStarted,
            Self::SessionEnded { .. } => EventKind::SessionEnded,
            Self::ReaderMoved { .. } => EventKind::ReaderMoved,
            Self::SettingsChanged { .. } => EventKind::SettingsChanged,
            Self::BookmarkAdded { .. } => EventKind::BookmarkAdded,
            Self::BookmarkChanged { .. } => EventKind::BookmarkChanged,
            Self::BookmarkRemoved { .. } => EventKind::BookmarkRemoved,
            Self::NoteAdded { .. } => EventKind::NoteAdded,
            Self::NoteChanged { .. } => EventKind::NoteChanged,
            Self::NoteRemoved { .. } => EventKind::NoteRemoved,
            Self::SearchDone { .. } => EventKind::SearchDone,
            Self::QuestionAsked { .. } => EventKind::QuestionAsked,
            Self::BookFinished { .. } => EventKind::BookFinished,
        }
    }

    /// Whether the reading log keeps it: all but what panes draw from.
    pub fn logged(&self) -> bool {
        !matches!(
            self,
            Self::PageShown { .. } | Self::ReaderClosed | Self::SettingsChanged { .. }
        )
    }
}

impl EventEnvelope {
    pub fn new(data: EventData) -> Self {
        Self {
            event: data.kind(),
            data,
        }
    }

    pub fn subscription(&self) -> Subscription {
        match self.event {
            EventKind::PageShown => Subscription::PageShown,
            EventKind::ReaderClosed => Subscription::ReaderClosed,
            EventKind::SessionStarted => Subscription::SessionStarted,
            EventKind::SessionEnded => Subscription::SessionEnded,
            EventKind::ReaderMoved => Subscription::ReaderMoved,
            EventKind::SettingsChanged => Subscription::SettingsChanged,
            EventKind::BookmarkAdded => Subscription::BookmarkAdded,
            EventKind::BookmarkChanged => Subscription::BookmarkChanged,
            EventKind::BookmarkRemoved => Subscription::BookmarkRemoved,
            EventKind::NoteAdded => Subscription::NoteAdded,
            EventKind::NoteChanged => Subscription::NoteChanged,
            EventKind::NoteRemoved => Subscription::NoteRemoved,
            EventKind::SearchDone => Subscription::SearchDone,
            EventKind::QuestionAsked => Subscription::QuestionAsked,
            EventKind::BookFinished => Subscription::BookFinished,
        }
    }
}

/// Any line a client can receive.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
// One is read per line and handled at once; boxing the pages buys nothing.
#[allow(clippy::large_enum_variant)]
pub enum Incoming {
    Event(EventEnvelope),
    Error(ErrorResponse),
    Success(SuccessResponse),
}

/// The bundled schema, laid out as herdr lays out its own.
pub fn schema() -> Value {
    fn one<T: JsonSchema>(name: &str) -> Value {
        let text = serde_json::to_string(&schemars::schema_for!(T)).unwrap_or_default();
        let text = text.replace("\"#/$defs/", &format!("\"#/schemas/{name}/$defs/"));
        serde_json::from_str(&text).unwrap_or_default()
    }
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "protocol": PROTOCOL,
        "schema_version": SCHEMA_VERSION,
        "schemas": {
            "error_response": one::<ErrorResponse>("error_response"),
            "event": one::<EventEnvelope>("event"),
            "log_record": one::<crate::log::Record>("log_record"),
            "note_export": one::<crate::export::Exported>("note_export"),
            "request": one::<Request>("request"),
            "success_response": one::<SuccessResponse>("success_response"),
        },
        "title": "Herdfold API",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_have_herdrs_shape() {
        let r = Request {
            id: "1".into(),
            call: Call::ReaderSendKeys(ReaderSendKeysParams {
                keys: vec!["space".into()],
            }),
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"id":"1","method":"reader.send_keys","params":{"keys":["space"]}})
        );
        let ping: Request =
            serde_json::from_str(r#"{"id":"p","method":"ping","params":{}}"#).unwrap();
        assert_eq!(ping.call, Call::Ping(EmptyParams {}));
    }

    #[test]
    fn params_are_required_even_when_empty() {
        assert!(serde_json::from_str::<Request>(r#"{"id":"p","method":"ping"}"#).is_err());
    }

    #[test]
    fn results_are_tagged_by_type() {
        let r = SuccessResponse {
            id: "1".into(),
            result: ResponseResult::SubscriptionStarted,
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"id":"1","result":{"type":"subscription_started"}})
        );
    }

    #[test]
    fn events_name_their_kind_twice() {
        let e = EventEnvelope::new(EventData::ReaderClosed);
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"event":"reader_closed","data":{"type":"reader_closed"}})
        );
    }

    #[test]
    fn events_read_under_their_old_log_names() {
        let e: EventData =
            serde_json::from_str(r#"{"type":"searched","query":"x","finds":2}"#).unwrap();
        assert_eq!(
            e,
            EventData::SearchDone {
                query: "x".into(),
                finds: 2
            }
        );
        assert_eq!(
            serde_json::to_value(&e).unwrap()["type"],
            json!("search_done")
        );
    }

    #[test]
    fn incoming_lines_are_told_apart() {
        let lines = [
            r#"{"event":"reader_closed","data":{"type":"reader_closed"}}"#,
            r#"{"id":"","error":{"code":"invalid_request","message":"x"}}"#,
            r#"{"id":"1","result":{"type":"ok"}}"#,
        ];
        let got: Vec<_> = lines
            .iter()
            .map(|l| match serde_json::from_str::<Incoming>(l).unwrap() {
                Incoming::Event(_) => "event",
                Incoming::Error(_) => "error",
                Incoming::Success(_) => "success",
            })
            .collect();
        assert_eq!(got, ["event", "error", "success"]);
    }

    #[test]
    fn schema_refs_point_inside_their_family() {
        let s = schema().to_string();
        assert!(!s.contains("\"#/$defs/"));
        assert!(s.contains("#/schemas/event/$defs/PageView"));
    }
}
