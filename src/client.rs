//! A client of a reader's socket: the one `HERDFOLD_SOCKET_PATH` names, else
//! the one reader open on this machine. Requests in herdr's shape, answers
//! matched to them by id.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use crate::api::{Call, EmptyParams, Incoming, PROTOCOL, Request, ResponseResult, SOCKET_ENV};
use crate::cli::CliError;

const TIMEOUT: Duration = Duration::from_secs(2);

/// The sockets of the readers open on this machine (those that answer).
pub fn readers() -> Vec<std::path::PathBuf> {
    let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(crate::server::socket_dir())
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    found.retain(|p| {
        p.extension().is_some_and(|e| e == "sock") && UnixStream::connect(p).is_ok()
    });
    found.sort();
    found
}

/// The reader to talk to: the one `HERDFOLD_SOCKET_PATH` names, else the
/// only one open.
fn socket() -> Result<std::path::PathBuf, CliError> {
    if let Some(path) = std::env::var_os(SOCKET_ENV) {
        return Ok(path.into());
    }
    let mut open = readers();
    match open.len() {
        1 => Ok(open.remove(0)),
        0 => Err(CliError::new("reader_not_found", "no reader is open")),
        n => Err(CliError::new(
            "ambiguous_reader",
            format!(
                "{n} readers are open; set {SOCKET_ENV} to one (`herdfold reader list` shows them)"
            ),
        )),
    }
}

pub struct Client {
    writer: UnixStream,
    pub incoming: Receiver<Incoming>,
    next: u64,
    /// Prefix of the ids this client gives its requests.
    name: &'static str,
}

impl Client {
    /// Connects, and checks with `ping` that the reader speaks this protocol.
    pub fn connect(name: &'static str) -> Result<Self, CliError> {
        Self::connect_to(&socket()?, name)
    }

    /// Connects to the reader at `path`.
    pub fn connect_to(path: &std::path::Path, name: &'static str) -> Result<Self, CliError> {
        let stream = UnixStream::connect(path).map_err(|e| {
            CliError::new(
                "reader_not_found",
                format!("no reader at {}: {e}", path.display()),
            )
        })?;
        let writer = stream.try_clone().map_err(CliError::io)?;
        let (tx, incoming) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                if let Ok(msg) = serde_json::from_str(&line)
                    && tx.send(msg).is_err()
                {
                    break;
                }
            }
        });
        let mut client = Self {
            writer,
            incoming,
            next: 1,
            name,
        };
        match client.call(Call::Ping(EmptyParams {}))? {
            ResponseResult::Pong { protocol, .. } if protocol == PROTOCOL => Ok(client),
            ResponseResult::Pong { protocol, .. } => Err(CliError::new(
                "protocol_mismatch",
                format!("reader speaks protocol {protocol}, this is {PROTOCOL}"),
            )),
            other => Err(CliError::new("invalid_response", format!("{other:?}"))),
        }
    }

    /// Sends a request without waiting for its answer.
    pub fn send(&mut self, call: Call) -> Result<String, CliError> {
        let id = format!("{}:{}", self.name, self.next);
        self.next += 1;
        let req = Request {
            id: id.clone(),
            call,
        };
        let line =
            serde_json::to_string(&req).map_err(|e| CliError::new("internal", e.to_string()))?;
        writeln!(self.writer, "{line}").map_err(CliError::io)?;
        Ok(id)
    }

    /// Sends a request and waits for its answer.
    pub fn call(&mut self, call: Call) -> Result<ResponseResult, CliError> {
        let id = self.send(call)?;
        loop {
            match self.incoming.recv_timeout(TIMEOUT) {
                Ok(Incoming::Success(r)) if r.id == id => return Ok(r.result),
                Ok(Incoming::Error(e)) if e.id == id || e.id.is_empty() => {
                    return Err(CliError::new(&e.error.code, e.error.message));
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    return Err(CliError::new("timeout", "the reader did not answer"));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(CliError::new("reader_closed", "the reader went away"));
                }
            }
        }
    }
}
