//! Turning an input into a `Document`. The format is always named by the
//! caller; nothing here guesses it from the bytes or the file name.

mod aozora;
mod diff;
mod epub;
pub(crate) mod md;
mod pdf;
mod text;
mod xml;

use anyhow::Result;
use clap::ValueEnum;

use crate::doc::Document;

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// Plain text. No chapters.
    Text,
    /// Markdown, rendered. Headings are chapters.
    Md,
    /// Unified diff (e.g. `git diff`, `gh pr diff`). Each file is a chapter.
    Diff,
    /// EPUB. The book's own table of contents gives the chapters.
    Epub,
    /// Aozora Bunko text (青空文庫), Shift_JIS or UTF-8: ruby set after its
    /// text, notes dropped. Headings its notes name are chapters.
    Aozora,
    /// PDF, read for its text: set again as the book's own pages, the
    /// layout (columns, figures) not kept. The outline gives the chapters.
    PdfText,
}

/// Reads `bytes` as `format`. Pictures a Markdown file points at are looked
/// for relative to `dir`, the file's directory (none for stdin).
pub fn load(
    format: Format,
    bytes: Vec<u8>,
    name: &str,
    dir: Option<&std::path::Path>,
) -> Result<Document> {
    let mut doc = match format {
        Format::Text => text::load(&String::from_utf8_lossy(&bytes)),
        Format::Md => md::load_in(&String::from_utf8_lossy(&bytes), dir),
        Format::Diff => diff::load(&String::from_utf8_lossy(&bytes)),
        Format::Epub => epub::load(bytes)?,
        Format::Aozora => aozora::load(&bytes),
        Format::PdfText => pdf::load(&bytes)?,
    };
    if doc.title.is_empty() {
        doc.title = name.to_string();
    }
    Ok(doc)
}
