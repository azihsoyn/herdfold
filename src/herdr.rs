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
    /// Splits the calling pane and runs `herdfold reader attach` in the new
    /// half, pointed at `socket`.
    pub fn open(width: u16, socket: &Path) -> Option<Self> {
        if std::env::var_os("HERDR_PANE_ID").is_none() || width < MIN_WIDTH {
            return None;
        }
        let herdr = herdr_bin();
        let env = format!("{SOCKET_ENV}={}", socket.display());
        let out = Command::new(&herdr)
            .args([
                "pane",
                "split",
                "--current",
                "--direction",
                "right",
                "--no-focus",
            ])
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

fn herdr_bin() -> PathBuf {
    std::env::var_os("HERDR_BIN_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| "herdr".into())
}

/// An agent to ask: its pane, and what to call it in a status line.
pub struct Agent {
    pub target: String,
    pub label: String,
}

/// The agent to ask: the one named, else one in this tab, else one in this
/// workspace, preferring one that is not busy.
pub fn find_agent(named: Option<&str>) -> Result<Agent, String> {
    if let Some(name) = named {
        return Ok(Agent {
            target: name.to_string(),
            label: name.to_string(),
        });
    }
    if std::env::var_os("HERDR_PANE_ID").is_none() {
        return Err("Not inside herdr: no agent to ask".into());
    }
    let out = Command::new(herdr_bin())
        .args(["agent", "list"])
        .output()
        .map_err(|e| format!("Could not ask herdr for agents: {e}"))?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let agents = v["result"]["agents"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let tab = std::env::var("HERDR_TAB_ID").unwrap_or_default();
    let workspace = std::env::var("HERDR_WORKSPACE_ID").unwrap_or_default();
    let free = |a: &serde_json::Value| matches!(a["agent_status"].as_str(), Some("idle" | "done"));
    let pick = |scope: &str, id: &str| {
        let near: Vec<&serde_json::Value> = agents.iter().filter(|a| a[scope] == id).collect();
        near.iter().find(|a| free(a)).or(near.first()).copied()
    };
    let a = pick("tab_id", &tab)
        .or_else(|| pick("workspace_id", &workspace))
        .ok_or("No agent in this tab or workspace (or pass --agent)")?;
    let target = a["pane_id"].as_str().unwrap_or_default().to_string();
    let label = format!("{} in {}", a["agent"].as_str().unwrap_or("agent"), target);
    Ok(Agent { target, label })
}

/// Submits `text` to the agent at `target`. herdr reports failures (a busy
/// or blocked agent, an unknown name) as JSON on stderr; their message is
/// returned.
pub fn prompt(target: &str, text: &str) -> Result<(), String> {
    let out = Command::new(herdr_bin())
        .args(["agent", "prompt", target, text])
        .output()
        .map_err(|e| format!("Could not run herdr: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap_or_default();
    Err(err["error"]["message"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| String::from_utf8_lossy(&out.stderr).trim().to_string()))
}

/// One request to the herdr server this pane belongs to, in herdr's own
/// protocol, through $HERDR_SOCKET_PATH.
fn call(method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    use std::io::{BufRead, BufReader, Write};
    let path = std::env::var_os("HERDR_SOCKET_PATH").ok_or("not inside herdr")?;
    let mut stream = std::os::unix::net::UnixStream::connect(path).map_err(|e| e.to_string())?;
    let request = serde_json::json!({ "id": "herdfold", "method": method, "params": params });
    writeln!(stream, "{request}").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let reply: serde_json::Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    match reply.get("error") {
        Some(e) => Err(e["message"].as_str().unwrap_or("herdr refused").to_string()),
        None => Ok(reply["result"].clone()),
    }
}

/// The size of a character cell in `pane`, in pixels, if herdr can set
/// pictures there.
pub fn cell_size(pane: &str) -> Option<(u32, u32)> {
    let info = call("pane.graphics.info", serde_json::json!({ "pane_id": pane })).ok()?;
    let w = info["cell_width_px"].as_u64()? as u32;
    let h = info["cell_height_px"].as_u64()? as u32;
    (w > 0 && h > 0).then_some((w, h))
}

/// Sets a picture of `width` x `height` pixels in `pane` as layer `layer`,
/// over `cols` x `rows` cells from (`col`, `row`). `format` is herdr's: "png"
/// for a PNG file's bytes, "rgba" for raw pixels.
#[allow(clippy::too_many_arguments)]
pub fn set_picture(
    pane: &str,
    layer: &str,
    format: &str,
    data_base64: &str,
    (width, height): (u32, u32),
    (col, row): (u16, u16),
    (cols, rows): (u16, u16),
) -> Result<(), String> {
    call(
        "pane.graphics.set",
        serde_json::json!({
            "pane_id": pane,
            "layer_id": layer,
            "format": format,
            "image_width": width,
            "image_height": height,
            "data_base64": data_base64,
            "placement": {
                "grid_cols": cols, "grid_rows": rows,
                "viewport_col": col, "viewport_row": row,
            },
        }),
    )
    .map(|_| ())
}

/// Takes the picture on layer `layer` out of `pane`.
pub fn clear_picture(pane: &str, layer: &str) {
    let _ = call(
        "pane.graphics.clear",
        serde_json::json!({ "pane_id": pane, "layer_id": layer }),
    );
}
