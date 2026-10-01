//! A client of a reader's socket, found through `HERDBOOK_SOCKET_PATH`:
//! requests in herdr's shape, answers matched to them by id.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use crate::api::{Call, EmptyParams, Incoming, PROTOCOL, Request, ResponseResult, SOCKET_ENV};
use crate::cli::CliError;

const TIMEOUT: Duration = Duration::from_secs(2);

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
        let path = std::env::var_os(SOCKET_ENV)
            .ok_or_else(|| CliError::new("reader_not_found", format!("{SOCKET_ENV} is not set")))?;
        let stream = UnixStream::connect(&path).map_err(|e| {
            CliError::new(
                "reader_not_found",
                format!("no reader at {}: {e}", path.to_string_lossy()),
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
