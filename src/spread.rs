//! Facing pages across two herdr panes. The reader in the left pane splits
//! off a right pane and runs a second copy of itself there (`--follow`). The
//! follower draws whatever right-hand page it is sent and passes its keys
//! back, so the book turns from either pane. Outside herdr, or when the pane
//! is too narrow to halve, nothing is split and the reader shows one page.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event};
use serde::{Deserialize, Serialize};

use crate::NAME;
use crate::view::{self, Cmd, PageView};

/// Narrowest pane worth halving: each half must still hold a readable page.
const MIN_WIDTH: u16 = 100;

#[derive(Serialize, Deserialize)]
enum ToFollower {
    Show(Option<PageView>),
    Bye,
}

#[derive(Serialize, Deserialize)]
enum ToLeader {
    Size(u16, u16),
    Cmd(Cmd),
}

/// What the leader hears from the follower.
pub enum FromPartner {
    Size(u16, u16),
    Cmd(Cmd),
    Gone,
}

/// The right-hand pane, seen from the left.
pub struct Partner {
    herdr: PathBuf,
    pane: String,
    sock: PathBuf,
    writer: Arc<Mutex<Option<UnixStream>>>,
    /// The follower's terminal size, once it has connected.
    pub size: Option<(u16, u16)>,
    last: Option<String>,
}

impl Partner {
    pub fn open(width: u16, tx: Sender<FromPartner>) -> Option<Self> {
        if std::env::var_os("HERDR_PANE_ID").is_none() || width < MIN_WIDTH {
            return None;
        }
        let herdr = std::env::var_os("HERDR_BIN_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| "herdr".into());
        let sock = socket_path();
        let _ = fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).ok()?;
        let writer = Arc::new(Mutex::new(None));
        accept(listener, writer.clone(), tx);

        let out = Command::new(&herdr)
            .args(["pane", "split", "--current", "--direction", "right", "--no-focus"])
            .args(["--ratio", "0.5"])
            .output()
            .ok()
            .filter(|o| o.status.success());
        let pane = out
            .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
            .and_then(|v| v["result"]["pane"]["pane_id"].as_str().map(str::to_string));
        let Some(pane) = pane else {
            let _ = fs::remove_file(&sock);
            return None;
        };
        let partner = Self {
            herdr,
            pane,
            sock,
            writer,
            size: None,
            last: None,
        };
        let exe = std::env::current_exe().ok()?;
        let cmd = format!("exec {} --follow {}", quote(&exe), quote(&partner.sock));
        let ran = Command::new(&partner.herdr)
            .args(["pane", "run", &partner.pane, &cmd])
            .output()
            .is_ok_and(|o| o.status.success());
        // Dropping `partner` closes the pane it split.
        ran.then_some(partner)
    }

    pub fn hear(&mut self, msg: &FromPartner) {
        match *msg {
            FromPartner::Size(w, h) => {
                self.size = Some((w, h));
                self.last = None;
            }
            FromPartner::Gone => self.size = None,
            FromPartner::Cmd(_) => {}
        }
    }

    /// Sends the right-hand page, if it changed since the last send.
    pub fn show(&mut self, view: Option<&PageView>) {
        let msg = ToFollower::Show(view.cloned());
        let line = serde_json::to_string(&msg).unwrap_or_default();
        if self.last.as_ref() == Some(&line) {
            return;
        }
        if let Some(w) = self.writer.lock().unwrap().as_mut()
            && writeln!(w, "{line}").is_ok()
        {
            self.last = Some(line);
        }
    }
}

impl Drop for Partner {
    fn drop(&mut self) {
        if let Some(w) = self.writer.lock().unwrap().as_mut() {
            let line = serde_json::to_string(&ToFollower::Bye).unwrap_or_default();
            let _ = writeln!(w, "{line}");
        }
        // Give the follower a moment to restore its terminal before the pane goes.
        thread::sleep(Duration::from_millis(50));
        let _ = Command::new(&self.herdr)
            .args(["pane", "close", &self.pane])
            .output();
        let _ = fs::remove_file(&self.sock);
    }
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

fn quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', r"'\''"))
}

/// Accepts follower connections and forwards what they say. A later
/// connection replaces an earlier one.
fn accept(listener: UnixListener, writer: Arc<Mutex<Option<UnixStream>>>, tx: Sender<FromPartner>) {
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let Ok(w) = stream.try_clone() else { continue };
            *writer.lock().unwrap() = Some(w);
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                let msg = match serde_json::from_str(&line) {
                    Ok(ToLeader::Size(w, h)) => FromPartner::Size(w, h),
                    Ok(ToLeader::Cmd(c)) => FromPartner::Cmd(c),
                    Err(_) => continue,
                };
                if tx.send(msg).is_err() {
                    return;
                }
            }
            *writer.lock().unwrap() = None;
            if tx.send(FromPartner::Gone).is_err() {
                return;
            }
        }
    });
}

/// Runs in the right-hand pane: draws the pages it is sent.
pub fn follow(sock: &Path) -> Result<()> {
    let stream = UnixStream::connect(sock)?;
    let mut out = stream.try_clone()?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if let Ok(msg) = serde_json::from_str(&line)
                && tx.send(msg).is_err()
            {
                return;
            }
        }
        let _ = tx.send(ToFollower::Bye);
    });

    let mut send = move |msg: ToLeader| -> Result<()> {
        writeln!(out, "{}", serde_json::to_string(&msg)?)?;
        Ok(())
    };
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        let size = terminal.size()?;
        send(ToLeader::Size(size.width, size.height))?;
        let mut page: Option<PageView> = None;
        loop {
            terminal.draw(|f| view::render(f, f.area(), page.as_ref()))?;
            if event::poll(Duration::from_millis(30))? {
                match event::read()? {
                    Event::Key(k) => {
                        if let Some(c) = view::cmd_of(k) {
                            send(ToLeader::Cmd(c))?;
                        }
                    }
                    Event::Resize(w, h) => send(ToLeader::Size(w, h))?,
                    _ => {}
                }
            }
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    ToFollower::Show(v) => page = v,
                    ToFollower::Bye => return Ok(()),
                }
            }
        }
    })();
    ratatui::restore();
    result
}
