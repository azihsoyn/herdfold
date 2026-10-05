//! The socket API, in herdr's shape: newline-delimited JSON over a Unix
//! socket. A request is `{"id","method","params"}`; the reply is
//! `{"id","result":{"type",..}}` or `{"id","error":{"code","message"}}`.
//! After `events.subscribe` the same connection also carries
//! `{"event","data":{"type",..}}`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::layout::Pos;
use crate::marks::{Anchor, Author};
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
    /// Write a note in the book, as an agent answering a question does.
    #[serde(rename = "note.add")]
    NoteAdd(NoteAddParams),
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
}

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
}

impl EventEnvelope {
    pub fn new(data: EventData) -> Self {
        let event = match data {
            EventData::PageShown { .. } => EventKind::PageShown,
            EventData::ReaderClosed => EventKind::ReaderClosed,
        };
        Self { event, data }
    }

    pub fn subscription(&self) -> Subscription {
        match self.event {
            EventKind::PageShown => Subscription::PageShown,
            EventKind::ReaderClosed => Subscription::ReaderClosed,
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
