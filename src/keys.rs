//! Key bindings, read as herdr reads its own: a `[keys]` table in
//! `$XDG_CONFIG_HOME/herdbook/config.toml` (default `~/.config/herdbook/`),
//! each action bound to a key or a list of keys by herdr's key names. An
//! action named there loses its default keys; `""` leaves it unbound.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::NAME;
use crate::view::Cmd;

/// Every action that can be bound: its name in the config, its default
/// keys, and what it does, as the key list shows it.
pub const ACTIONS: &[(Cmd, &str, &[&str], &str)] = &[
    (
        Cmd::Next,
        "next_page",
        &["space", "right", "pagedown"],
        "turn the page",
    ),
    (
        Cmd::Prev,
        "previous_page",
        &["b", "left", "pageup"],
        "turn back",
    ),
    (Cmd::Contents, "contents", &["g"], "contents"),
    (
        Cmd::Search,
        "search",
        &["/", "ctrl+f"],
        "search (n / N next, previous; l list)",
    ),
    (
        Cmd::Mark,
        "bookmark",
        &["m"],
        "bookmark this page (again to remove)",
    ),
    (
        Cmd::Color,
        "bookmark_color",
        &["c"],
        "colour of the bookmark here",
    ),
    (Cmd::NotePage, "note", &["n"], "write a note on this page"),
    (
        Cmd::Select,
        "choose_row",
        &["v"],
        "choose a row: Enter to note it, ? to ask",
    ),
    (
        Cmd::Shelf,
        "list",
        &["l"],
        "bookmarks and notes (Enter go, d remove)",
    ),
    (
        Cmd::NoteDisplay,
        "note_display",
        &["N"],
        "notes as footnotes / in margin / marks",
    ),
    (Cmd::Ask, "ask", &["?"], "ask the agent about these pages"),
    (Cmd::Narrower, "shorter_rows", &["<"], "shorter rows"),
    (Cmd::Wider, "longer_rows", &[">"], "longer rows"),
    (
        Cmd::Animate,
        "animation",
        &["a"],
        "page-turn animation on / off",
    ),
    (Cmd::Help, "help", &["h", "H"], "these keys"),
    (Cmd::Tip, "tip", &["T"], "a tip"),
    (Cmd::Up, "up", &["k", "up"], "up, in a list"),
    (Cmd::Down, "down", &["j", "down"], "down, in a list"),
    (Cmd::Enter, "enter", &["enter"], "go, in a list"),
    (Cmd::Delete, "remove", &["d"], "remove, in a list"),
    (
        Cmd::Back,
        "back",
        &["esc"],
        "step back; on the page, close the book",
    ),
    (Cmd::Quit, "quit", &["q", "ctrl+c"], "close the book"),
];

/// Named keys, besides single characters, with or without `ctrl+`.
const NAMED: &[&str] = &[
    "space",
    "enter",
    "esc",
    "tab",
    "backspace",
    "delete",
    "up",
    "down",
    "left",
    "right",
    "pageup",
    "pagedown",
];

pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join(NAME).join("config.toml"))
}

/// Whether `key` is a key name herdbook can receive.
pub fn valid(key: &str) -> bool {
    let base = key.strip_prefix("ctrl+").unwrap_or(key);
    base.chars().count() == 1 || NAMED.contains(&base)
}

#[derive(Clone, Debug)]
pub struct Keymap {
    by_key: HashMap<String, Cmd>,
    by_cmd: HashMap<Cmd, Vec<String>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::with(&HashMap::new())
    }
}

impl Keymap {
    /// The defaults, with the actions named in `bound` bound as given.
    fn with(bound: &HashMap<Cmd, Vec<String>>) -> Self {
        let mut by_cmd = HashMap::new();
        let mut by_key = HashMap::new();
        for &(cmd, _, defaults, _) in ACTIONS {
            let keys: Vec<String> = match bound.get(&cmd) {
                Some(keys) => keys.clone(),
                None => defaults.iter().map(|k| k.to_string()).collect(),
            };
            for k in &keys {
                // A key claimed twice goes to the first action that claims it.
                by_key.entry(k.clone()).or_insert(cmd);
            }
            by_cmd.insert(cmd, keys);
        }
        Self { by_key, by_cmd }
    }

    /// Reads the config. Problems are returned with the keymap, which keeps
    /// whatever could be read and the defaults for the rest.
    pub fn load() -> (Self, Vec<String>) {
        let Some(path) = config_path() else {
            return (Self::default(), Vec::new());
        };
        match std::fs::read_to_string(&path) {
            Ok(src) => Self::parse(&src),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), Vec::new()),
            Err(e) => (Self::default(), vec![format!("cannot read: {e}")]),
        }
    }

    pub fn parse(src: &str) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        let table: toml::Table = match toml::from_str(src) {
            Ok(t) => t,
            Err(e) => {
                problems.push(e.message().to_string());
                return (Self::default(), problems);
            }
        };
        let mut bound: HashMap<Cmd, Vec<String>> = HashMap::new();
        if let Some(keys) = table.get("keys") {
            let Some(keys) = keys.as_table() else {
                problems.push("[keys] must be a table".into());
                return (Self::default(), problems);
            };
            for (name, value) in keys {
                let Some(&(cmd, ..)) = ACTIONS.iter().find(|a| a.1 == name) else {
                    problems.push(format!("keys.{name}: no such action"));
                    continue;
                };
                let list: Vec<&str> = match value {
                    toml::Value::String(k) if k.is_empty() => Vec::new(),
                    toml::Value::String(k) => vec![k.as_str()],
                    toml::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect(),
                    _ => {
                        problems.push(format!("keys.{name}: give a key name or a list of them"));
                        continue;
                    }
                };
                let mut keys = Vec::new();
                for k in list {
                    if valid(k) {
                        keys.push(k.to_string());
                    } else {
                        problems.push(format!("keys.{name}: \"{k}\" is not a key herdbook knows"));
                    }
                }
                bound.insert(cmd, keys);
            }
        }
        let map = Self::with(&bound);
        // Report keys that two actions both claim.
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for &(cmd, name, ..) in ACTIONS {
            for k in &map.by_cmd[&cmd] {
                if let Some(first) = seen.insert(k, name) {
                    problems.push(format!(
                        "\"{k}\" is bound to both {first} and {name}; {first} keeps it"
                    ));
                    seen.insert(k, first);
                }
            }
        }
        (map, problems)
    }

    pub fn cmd(&self, key: &str) -> Option<Cmd> {
        self.by_key.get(key).copied()
    }

    pub fn keys(&self, cmd: Cmd) -> &[String] {
        self.by_cmd.get(&cmd).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The first key bound to `cmd`, as shown in tips; `—` if none.
    pub fn first(&self, cmd: Cmd) -> String {
        self.keys(cmd)
            .first()
            .map(|k| shown(k))
            .unwrap_or_else(|| "—".into())
    }

    /// The keys bound to `cmd`, as the key list shows them.
    pub fn label(&self, cmd: Cmd) -> String {
        self.keys(cmd)
            .iter()
            .map(|k| shown(k))
            .collect::<Vec<_>>()
            .join("  ")
    }
}

/// A key name as shown to the reader: arrows as arrows, `ctrl+f` as `^F`.
fn shown(key: &str) -> String {
    if let Some(c) = key.strip_prefix("ctrl+") {
        return format!("^{}", c.to_uppercase());
    }
    match key {
        "space" => "Space".into(),
        "enter" => "Enter".into(),
        "esc" => "Esc".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "pageup" => "PgUp".into(),
        "pagedown" => "PgDn".into(),
        k => k.into(),
    }
}

/// `herdbook config check`: says whether the config reads cleanly.
pub fn check() -> Result<(), crate::cli::CliError> {
    let (_, problems) = Keymap::load();
    let path = config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    if problems.is_empty() {
        println!("config: ok");
        return Ok(());
    }
    for p in &problems {
        eprintln!("config: {path}: {p}");
    }
    Err(crate::cli::CliError::new(
        "invalid_config",
        format!("{} problem(s) in {path}", problems.len()),
    ))
}

/// `herdbook config reset-keys`: backs the config up, then removes `[keys]`.
pub fn reset() -> Result<(), crate::cli::CliError> {
    use crate::cli::CliError;
    let path = config_path().ok_or_else(|| CliError::new("no_config", "no HOME to find it in"))?;
    let src = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("config: no custom keybindings");
            return Ok(());
        }
        Err(e) => return Err(CliError::io(e)),
    };
    let mut doc: toml_edit::DocumentMut = src
        .parse()
        .map_err(|e: toml_edit::TomlError| CliError::new("invalid_config", e.to_string()))?;
    if doc.remove("keys").is_none() {
        println!("config: no custom keybindings");
        return Ok(());
    }
    let backup = path.with_extension("toml.bak");
    std::fs::copy(&path, &backup).map_err(CliError::io)?;
    std::fs::write(&path, doc.to_string()).map_err(CliError::io)?;
    println!("config: keybindings reset (backup at {})", backup.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_bind_every_action() {
        let k = Keymap::default();
        assert_eq!(k.cmd("space"), Some(Cmd::Next));
        assert_eq!(k.cmd("ctrl+f"), Some(Cmd::Search));
        assert_eq!(k.label(Cmd::Next), "Space  →  PgDn");
    }

    #[test]
    fn a_named_action_replaces_its_defaults() {
        let (k, problems) =
            Keymap::parse("[keys]\nnext_page = [\"l\", \"space\"]\nlist = \"L\"\nhelp = \"\"\n");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(k.cmd("l"), Some(Cmd::Next));
        assert_eq!(k.cmd("right"), None);
        assert_eq!(k.cmd("L"), Some(Cmd::Shelf));
        assert_eq!(k.cmd("h"), None);
        assert_eq!(k.first(Cmd::Help), "—");
    }

    #[test]
    fn problems_are_named() {
        let (k, problems) =
            Keymap::parse("[keys]\nturn = \"x\"\nbookmark = \"shift+m\"\nnote = \"g\"\nhelp = 3\n");
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("turn: no such action")));
        assert!(
            problems
                .iter()
                .any(|p| p.contains("\"shift+m\" is not a key"))
        );
        assert!(problems.iter().any(|p| p.contains("help: give a key")));
        assert!(
            problems
                .iter()
                .any(|p| p.contains("\"g\" is bound to both contents and note"))
        );
        // What could be read still applies; the first claim on a key wins.
        assert_eq!(k.cmd("g"), Some(Cmd::Contents));
        assert_eq!(k.cmd("m"), None);
    }

    #[test]
    fn a_broken_file_falls_back_to_the_defaults() {
        let (k, problems) = Keymap::parse("[keys\n");
        assert_eq!(problems.len(), 1);
        assert_eq!(k.cmd("space"), Some(Cmd::Next));
    }
}
