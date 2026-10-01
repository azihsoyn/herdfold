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
    pub marks: Vec<Mark>,
    /// Longest row, in columns, as last set for this book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure: Option<usize>,
    /// Reading notes, in reading order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
}

/// A bookmark: where it is, and the colour of its ribbon.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Mark {
    pub at: Pos,
    #[serde(default)]
    pub color: Ribbon,
}

// Bookmarks were once kept as bare places; those read as red ribbons.
impl<'de> Deserialize<'de> for Mark {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Kept {
            Mark {
                at: Pos,
                #[serde(default)]
                color: Ribbon,
            },
            Bare(Pos),
        }
        Ok(match Kept::deserialize(d)? {
            Kept::Mark { at, color } => Mark { at, color },
            Kept::Bare(at) => Mark {
                at,
                color: Ribbon::default(),
            },
        })
    }
}

/// Colours a bookmark ribbon comes in, in the order `c` steps through them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Ribbon {
    #[default]
    Red,
    Yellow,
    Green,
    Cyan,
    Blue,
    Magenta,
}

impl Ribbon {
    pub fn next(self) -> Self {
        match self {
            Self::Red => Self::Yellow,
            Self::Yellow => Self::Green,
            Self::Green => Self::Cyan,
            Self::Cyan => Self::Blue,
            Self::Blue => Self::Magenta,
            Self::Magenta => Self::Red,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Yellow => "yellow",
            Self::Green => "green",
            Self::Cyan => "cyan",
            Self::Blue => "blue",
            Self::Magenta => "magenta",
        }
    }
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
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, clap::ValueEnum,
)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    /// A page, like a bookmark with something written on it.
    #[default]
    Page,
    /// One row, like a highlighter mark with a note beside it.
    Line,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, clap::ValueEnum,
)]
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
    /// Colour for new bookmarks: the one last chosen with `c`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ribbon: Option<Ribbon>,
    /// Whether a tip greets each book; off once "don't show again" is ticked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tips: Option<bool>,
    /// The tip to show next, so each opening shows a different one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_tip: Option<usize>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmarks_kept_as_bare_places_still_read() {
        let e: Entry = serde_json::from_str(
            r#"{"at":{"line":0,"offset":0},"marks":[{"line":3,"offset":1},{"at":{"line":5,"offset":0},"color":"cyan"}]}"#,
        )
        .unwrap();
        let marks: Vec<_> = e.marks.iter().map(|m| (m.at.line, m.color)).collect();
        assert_eq!(marks, [(3, Ribbon::Red), (5, Ribbon::Cyan)]);
    }
}
