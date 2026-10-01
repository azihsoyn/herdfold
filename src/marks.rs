//! Where each book was left, and the bookmarks in it, kept in
//! `$XDG_DATA_HOME/<name>/marks.json` (default `~/.local/share/<name>/`),
//! and the reader's own settings beside them in `settings.json`.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::NAME;
use crate::layout::Pos;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Where reading stopped.
    pub at: Pos,
    /// Bookmarks, in reading order.
    #[serde(default)]
    pub marks: Vec<Pos>,
    /// Longest row, in columns, as last set for this book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure: Option<usize>,
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    books: BTreeMap<String, Entry>,
}

/// Settings that carry over from one book to the next.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Longest row, in columns, as last set with `<` / `>` in any book: the
    /// starting length for a book that has none of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure: Option<usize>,
}

fn data_file(name: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join(NAME).join(name))
}

fn path() -> Option<PathBuf> {
    data_file("marks.json")
}

fn write_json(path: &std::path::Path, value: &impl Serialize) -> Result<()> {
    fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension(format!("json.{}", std::process::id()));
    fs::write(&tmp, serde_json::to_string_pretty(value)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn settings() -> Settings {
    data_file("settings.json")
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_settings(settings: &Settings) -> Result<()> {
    let path = data_file("settings.json").context("no HOME or XDG_DATA_HOME to keep settings in")?;
    write_json(&path, settings)
}

fn read_store() -> Store {
    path()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn load(book: &str) -> Option<Entry> {
    read_store().books.remove(book)
}

/// Writes one book's entry, re-reading the file first so that other books
/// saved meanwhile (by another reader) are kept.
pub fn save(book: &str, entry: &Entry) -> Result<()> {
    let path = path().context("no HOME or XDG_DATA_HOME to keep bookmarks in")?;
    let mut store = read_store();
    store.books.insert(book.to_string(), entry.clone());
    write_json(&path, &store)
}
