//! The reader's side of the socket API. Connections are read on their own
//! threads; every request is handed to the reader's loop as an `Inbound`,
//! answered there, and written back through the `Hub`. Events go to the
//! connections subscribed to them.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::Result;
use serde::Serialize;

use crate::NAME;
use crate::api::{
    ErrorResponse, EventData, EventEnvelope, Request, ResponseResult, SuccessResponse,
    Subscription,
};

pub type ConnId = u64;

pub enum Inbound {
    Request(ConnId, Request),
    Closed(ConnId),
}

struct Conn {
    writer: UnixStream,
    subscriptions: HashSet<Subscription>,
}

#[derive(Clone, Default)]
pub struct Hub {
    conns: Arc<Mutex<HashMap<ConnId, Conn>>>,
}

impl Hub {
    pub fn reply(&self, conn: ConnId, id: String, result: ResponseResult) {
        self.send(conn, &SuccessResponse { id, result });
    }

    pub fn fail(&self, conn: ConnId, id: String, code: &str, message: impl Into<String>) {
        self.send(conn, &ErrorResponse::new(id, code, message));
    }

    pub fn subscribe(&self, conn: ConnId, subscriptions: &[Subscription]) {
        if let Some(c) = self.conns.lock().unwrap().get_mut(&conn) {
            c.subscriptions.extend(subscriptions);
        }
    }

    /// Drops a closed connection. Called after the requests it sent before
    /// closing have been answered: a client may shut its side down as soon
    /// as it has written, and still expect its replies.
    pub fn forget(&self, conn: ConnId) {
        self.conns.lock().unwrap().remove(&conn);
    }

    pub fn emit(&self, data: EventData) {
        let event = EventEnvelope::new(data);
        let kind = event.subscription();
        let Ok(line) = serde_json::to_string(&event) else { return };
        for c in self.conns.lock().unwrap().values_mut() {
            if c.subscriptions.contains(&kind) {
                let _ = writeln!(c.writer, "{line}");
            }
        }
    }

    fn send(&self, conn: ConnId, msg: &impl Serialize) {
        let Ok(line) = serde_json::to_string(msg) else { return };
        if let Some(c) = self.conns.lock().unwrap().get_mut(&conn) {
            let _ = writeln!(c.writer, "{line}");
        }
    }
}

/// A listening socket. Dropping it removes the socket file.
pub struct Server {
    pub path: PathBuf,
    pub hub: Hub,
    pub inbound: Receiver<Inbound>,
}

impl Server {
    pub fn listen() -> Result<Self> {
        let path = socket_path();
        let _ = fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        let hub = Hub::default();
        let (tx, inbound) = mpsc::channel();
        let accept_hub = hub.clone();
        thread::spawn(move || {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            for stream in listener.incoming().flatten() {
                let id = NEXT.fetch_add(1, Ordering::Relaxed);
                serve(id, stream, accept_hub.clone(), tx.clone());
            }
        });
        Ok(Self { path, hub, inbound })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn serve(id: ConnId, stream: UnixStream, hub: Hub, tx: Sender<Inbound>) {
    let Ok(writer) = stream.try_clone() else { return };
    hub.conns.lock().unwrap().insert(
        id,
        Conn {
            writer,
            subscriptions: HashSet::new(),
        },
    );
    thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Request>(&line) {
                Ok(req) => {
                    if tx.send(Inbound::Request(id, req)).is_err() {
                        break;
                    }
                }
                // Like herdr: a request that cannot be read is answered with an empty id.
                Err(e) => hub.fail(id, String::new(), "invalid_request", format!("invalid request: {e}")),
            }
        }
        // The connection is forgotten only once the reader has answered
        // everything sent before it closed (see `Hub::forget`).
        let _ = tx.send(Inbound::Closed(id));
    });
}

fn socket_path() -> PathBuf {
    let name = format!("{NAME}-{}.sock", std::process::id());
    let tmp = std::env::temp_dir();
    // Unix socket paths are limited to ~104 bytes on macOS.
    if tmp.as_os_str().len() + name.len() < 100 {
        tmp.join(name)
    } else {
        PathBuf::from("/tmp").join(name)
    }
}
