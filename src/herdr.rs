//! The right-hand pane, opened through herdr's CLI. Outside herdr, or when
//! the pane is too narrow to halve, no pane is opened and the reader shows
//! one page.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::api::SOCKET_ENV;

/// Narrowest pane worth halving: each half must still hold a readable page.
const MIN_WIDTH: u16 = 100;

/// A pane split off for the right-hand page. Dropping it closes the pane.
pub struct RightPane {
    herdr: PathBuf,
    pub pane_id: String,
}

impl RightPane {
    /// Splits the calling pane and runs `herdbook reader attach` in the new
    /// half, pointed at `socket`.
    pub fn open(width: u16, socket: &Path) -> Option<Self> {
        if std::env::var_os("HERDR_PANE_ID").is_none() || width < MIN_WIDTH {
            return None;
        }
        let herdr: PathBuf = std::env::var_os("HERDR_BIN_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| "herdr".into());
        let env = format!("{SOCKET_ENV}={}", socket.display());
        let out = Command::new(&herdr)
            .args(["pane", "split", "--current", "--direction", "right", "--no-focus"])
            .args(["--ratio", "0.5", "--env", &env])
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
        let pane = Self {
            pane_id: v["result"]["pane"]["pane_id"].as_str()?.to_string(),
            herdr,
        };
        let exe = std::env::current_exe().ok()?;
        let cmd = format!("exec {} reader attach", quote(&exe));
        let ran = Command::new(&pane.herdr)
            .args(["pane", "run", &pane.pane_id, &cmd])
            .output()
            .is_ok_and(|o| o.status.success());
        // On failure, dropping `pane` closes what was split.
        ran.then_some(pane)
    }
}

impl Drop for RightPane {
    fn drop(&mut self) {
        // Give the attached reader a moment to restore its terminal first.
        thread::sleep(Duration::from_millis(50));
        let _ = Command::new(&self.herdr)
            .args(["pane", "close", &self.pane_id])
            .output();
    }
}

fn quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', r"'\''"))
}
