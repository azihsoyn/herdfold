//! Control commands answer the way herdr's do: JSON on stdout,
//! `{"id":"cli:<group>:<command>","result":{..}}`; on failure the same
//! envelope with `error` goes to stderr and the exit code is 1.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::api::{self, Call, ErrorResponse, PROTOCOL, SCHEMA_VERSION, Subscription};
use crate::client::Client;

#[derive(Debug)]
pub struct CliError {
    pub code: String,
    pub message: String,
}

impl CliError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn io(e: std::io::Error) -> Self {
        Self::new("io_error", e.to_string())
    }
}

/// Reports the outcome of `cli:<id>` and gives the exit code.
pub fn finish(id: &str, result: Result<(), CliError>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let body = ErrorResponse::new(format!("cli:{id}"), &e.code, e.message);
            eprintln!("{}", serde_json::to_string(&body).unwrap_or_default());
            ExitCode::FAILURE
        }
    }
}

/// Sends `call` to the reader and prints its answer, as herdr's commands
/// print theirs: `{"id":"cli:<id>","result":{..}}`.
pub fn call(id: &'static str, call: Call) -> Result<(), CliError> {
    let mut client = Client::connect(id)?;
    let result = client.call(call)?;
    println!(
        "{}",
        serde_json::json!({ "id": format!("cli:{id}"), "result": result })
    );
    Ok(())
}

/// `herdfold reader list`: the readers open, each with what it has open.
pub fn readers() -> Result<(), CliError> {
    let readers: Vec<serde_json::Value> = crate::client::readers()
        .into_iter()
        .filter_map(|path| {
            let mut client = Client::connect_to(&path, "reader:list").ok()?;
            let state = client.call(Call::ReaderState(api::EmptyParams {})).ok()?;
            let api::ResponseResult::ReaderState { state } = state else {
                return None;
            };
            Some(serde_json::json!({ "socket": path, "state": state }))
        })
        .collect();
    println!(
        "{}",
        serde_json::json!({
            "id": "cli:reader:list",
            "result": { "type": "readers", "readers": readers },
        })
    );
    Ok(())
}

/// `herdfold events [TYPE...]`: the reader's events, one JSON line each, as
/// they happen, until it closes. Every kind when none is named.
pub fn events(types: Vec<String>) -> Result<(), CliError> {
    let all = [
        "page.shown",
        "reader.closed",
        "session.started",
        "session.ended",
        "reader.moved",
        "settings.changed",
        "bookmark.added",
        "bookmark.changed",
        "bookmark.removed",
        "note.added",
        "note.changed",
        "note.removed",
        "search.done",
        "question.asked",
        "book.finished",
    ];
    let types: Vec<String> = if types.is_empty() {
        all.iter()
            .filter(|t| **t != "page.shown")
            .map(|t| t.to_string())
            .collect()
    } else {
        types
    };
    let subscriptions = types
        .iter()
        .map(|t| {
            serde_json::from_value::<Subscription>(serde_json::json!({ "type": t })).map_err(|_| {
                CliError::new(
                    "invalid_params",
                    format!("no event {t:?}; there are: {}", all.join(", ")),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut client = Client::connect("events")?;
    client.call(Call::EventsSubscribe(api::EventsSubscribeParams {
        subscriptions,
    }))?;
    for msg in client.incoming.iter() {
        if let api::Incoming::Event(e) = msg {
            let line =
                serde_json::to_string(&e).map_err(|e| CliError::new("internal", e.to_string()))?;
            println!("{line}");
            if e.data == api::EventData::ReaderClosed {
                break;
            }
        }
    }
    Ok(())
}

/// `herdfold api schema [--json | --output PATH]`
pub fn api_schema(json: bool, output: Option<PathBuf>) -> Result<(), CliError> {
    let schema = api::schema();
    if let Some(path) = output {
        let text = serde_json::to_string_pretty(&schema)
            .map_err(|e| CliError::new("internal", e.to_string()))?;
        std::fs::write(&path, text + "\n").map_err(CliError::io)?;
        println!("wrote API schema to {}", path.display());
    } else if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&schema).unwrap_or_default()
        );
    } else {
        let names: Vec<&String> = schema["schemas"]
            .as_object()
            .map(|m| m.keys().collect())
            .unwrap_or_default();
        let names: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        println!("Herdfold API schema");
        println!("protocol: {PROTOCOL}");
        println!("schema_version: {SCHEMA_VERSION}");
        println!("schemas: {}", names.join(", "));
        println!();
        println!("Use `herdfold api schema --json` to print the full schema.");
        println!("Use `herdfold api schema --output PATH` to write it to a file.");
    }
    Ok(())
}
