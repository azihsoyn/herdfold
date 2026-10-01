//! `herdbook reader attach`: a client of a reader's socket that draws the
//! right-hand page. It checks the protocol with `ping`, attaches with its
//! size, subscribes to `page.shown` and `reader.closed`, and passes every
//! key back through `reader.send_keys`, so the book turns from either pane.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event};

use crate::api::{
    Call, EmptyParams, EventData, EventsSubscribeParams, Incoming, PROTOCOL, ReaderSendKeysParams,
    ReaderSizeParams, Request, ResponseResult, SOCKET_ENV, Subscription,
};
use crate::cli::CliError;
use crate::view::{self, PageView};

const TIMEOUT: Duration = Duration::from_secs(2);

struct Client {
    writer: UnixStream,
    incoming: Receiver<Incoming>,
    next: u64,
}

impl Client {
    fn connect() -> Result<Self, CliError> {
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
        Ok(Self {
            writer,
            incoming,
            next: 1,
        })
    }

    fn send(&mut self, call: Call) -> Result<String, CliError> {
        let id = format!("attach:{}", self.next);
        self.next += 1;
        let req = Request {
            id: id.clone(),
            call,
        };
        let line = serde_json::to_string(&req).map_err(|e| CliError::new("internal", e.to_string()))?;
        writeln!(self.writer, "{line}").map_err(CliError::io)?;
        Ok(id)
    }

    /// Sends a request and waits for its answer.
    fn call(&mut self, call: Call) -> Result<ResponseResult, CliError> {
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

pub fn run() -> Result<(), CliError> {
    let mut client = Client::connect()?;
    match client.call(Call::Ping(EmptyParams {}))? {
        ResponseResult::Pong { protocol, .. } if protocol == PROTOCOL => {}
        ResponseResult::Pong { protocol, .. } => {
            return Err(CliError::new(
                "protocol_mismatch",
                format!("reader speaks protocol {protocol}, this is {PROTOCOL}"),
            ));
        }
        other => return Err(CliError::new("invalid_response", format!("{other:?}"))),
    }
    let (cols, rows) = crossterm::terminal::size().map_err(CliError::io)?;
    client.call(Call::ReaderAttach(ReaderSizeParams { cols, rows }))?;
    client.call(Call::EventsSubscribe(EventsSubscribeParams {
        subscriptions: vec![Subscription::PageShown, Subscription::ReaderClosed],
    }))?;

    let mut terminal = ratatui::init();
    let result = draw(&mut terminal, &mut client);
    ratatui::restore();
    result
}

fn draw(terminal: &mut ratatui::DefaultTerminal, client: &mut Client) -> Result<(), CliError> {
    let mut page: Option<PageView> = None;
    loop {
        terminal
            .draw(|f| view::render(f, f.area(), page.as_ref()))
            .map_err(CliError::io)?;
        if event::poll(Duration::from_millis(30)).map_err(CliError::io)? {
            match event::read().map_err(CliError::io)? {
                Event::Key(k) => {
                    if let Some(key) = view::key_name(k) {
                        client.send(Call::ReaderSendKeys(ReaderSendKeysParams { keys: vec![key] }))?;
                    }
                }
                Event::Resize(cols, rows) => {
                    client.send(Call::ReaderResize(ReaderSizeParams { cols, rows }))?;
                }
                _ => {}
            }
        }
        loop {
            match client.incoming.try_recv() {
                Ok(Incoming::Event(e)) => match e.data {
                    EventData::PageShown { right, .. } => page = right,
                    EventData::ReaderClosed => return Ok(()),
                },
                Ok(_) => {}
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }
}
