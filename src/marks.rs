//! Where each book was left, its bookmarks and reading notes, kept in
//! `$XDG_DATA_HOME/<name>/marks.json` (default `~/.local/share/<name>/`),
//! and the reader's own settings beside them in `settings.json`.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use schemars::JsonSchema;
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
    /// Reading notes, in reading order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
}

/// A note written in the book.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Note {
    /// The start of the page, or of the row, the note is about.
    pub at: Pos,
    #[serde(default)]
    pub anchor: Anchor,
    pub text: String,
    #[serde(default)]
    pub by: Author,
    /// For an agent's answer, the question it answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
}

/// What a note is attached to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    /// A page, like a bookmark with something written on it.
    #[default]
    Page,
    /// One row, like a highlighter mark with a note beside it.
    Line,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    #[default]
    Reader,
    Agent,
}

impl Entry {
    /// Adds a note, keeping notes in reading order.
    pub fn add_note(&mut self, note: Note) {
        let i = self.notes.partition_point(|n| n.at <= note.at);
        self.notes.insert(i, note);
    }
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
    /// Whether page turns are drawn, as last toggled with `a`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animate: Option<bool>,
    /// How notes are shown, as last chosen with `N`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<NoteDisplay>,
}

/// Where a note's text is shown on its page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoteDisplay {
    /// At the foot of the page, under a short rule.
    #[default]
    Footnotes,
    /// In the outer margin, beside the row it is on.
    Margin,
    /// Only the pencil and the stroke; the text is read in the list.
    Marks,
}

impl NoteDisplay {
    pub fn next(self) -> Self {
        match self {
            Self::Footnotes => Self::Margin,
            Self::Margin => Self::Marks,
            Self::Marks => Self::Footnotes,
        }
    }
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

/// Changes some settings, keeping the rest as they are on disk.
pub fn update_settings(change: impl FnOnce(&mut Settings)) -> Result<()> {
    let path =
        data_file("settings.json").context("no HOME or XDG_DATA_HOME to keep settings in")?;
    let mut settings = settings();
    change(&mut settings);
    write_json(&path, &settings)
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
