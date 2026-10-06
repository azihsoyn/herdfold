//! `herdfold note export`: what was written in a book, its notes, markers,
//! answers and bookmarks, as Markdown to read or as JSON to take elsewhere.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Serialize;

use crate::cli::CliError;
use crate::doc::Document;
use crate::formats::{self, Format};
use crate::layout::Pos;
use crate::log::{self, Book};
use crate::marks::{self, Anchor, Author, Entry, Ribbon};

/// Everything written in one book.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Exported {
    pub book: Book,
    /// When this was exported, RFC 3339 in UTC.
    pub exported: String,
    /// In reading order.
    pub notes: Vec<ExportedNote>,
    /// In reading order.
    pub bookmarks: Vec<ExportedMark>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ExportedNote {
    pub at: Pos,
    /// For a range, where it ends (exclusive).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<Pos>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chapter: Option<String>,
    pub anchor: Anchor,
    pub by: Author,
    /// The note; empty for a marker with nothing written on it.
    pub text: String,
    /// For an agent's answer, the question it answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// The text the note is about: the chosen text, or the row's line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// For a marker, its colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Ribbon>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ExportedMark {
    pub at: Pos,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chapter: Option<String>,
    pub color: Ribbon,
    /// The line the bookmarked page starts in.
    pub text: String,
}

/// `herdfold note export --format FMT FILE [--json]`
pub fn run(format: Format, file: PathBuf, json: bool) -> Result<(), CliError> {
    let bytes = std::fs::read(&file).map_err(CliError::io)?;
    let key = std::fs::canonicalize(&file)
        .map_err(CliError::io)?
        .to_string_lossy()
        .into_owned();
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let doc = formats::load(format, bytes, &name, file.parent())
        .map_err(|e| CliError::new("unreadable_book", format!("{e:#}")))?;
    let entry = marks::load(&key).unwrap_or_default();
    let book = Book {
        key: Some(key),
        title: doc.title.clone(),
        format,
    };
    let exported = gather(&doc, &entry, book);
    if json {
        let mut result = serde_json::to_value(&exported)
            .map_err(|e| CliError::new("internal", e.to_string()))?;
        result["type"] = "note_export".into();
        let reply = serde_json::json!({ "id": "cli:note:export", "result": result });
        println!("{reply}");
    } else {
        print!("{}", markdown(&exported));
    }
    Ok(())
}

pub fn gather(doc: &Document, entry: &Entry, book: Book) -> Exported {
    let chapter = |line: usize| doc.chapter_of(line).map(|c| c.title.clone());
    let line_text = |line: usize| {
        doc.lines
            .get(line)
            .map(|l| l.text.trim().to_string())
            .unwrap_or_default()
    };
    let notes = entry
        .notes
        .iter()
        .map(|n| ExportedNote {
            at: n.at,
            end: n.end,
            chapter: chapter(n.at.line),
            anchor: n.anchor,
            by: n.by,
            text: n.text.clone(),
            question: n.question.clone(),
            quote: match (n.anchor, n.end) {
                (Anchor::Range, Some(end)) => Some(doc.text_between(n.at, end)),
                (Anchor::Line, _) => Some(line_text(n.at.line)),
                _ => None,
            },
            color: (n.anchor == Anchor::Range).then(|| n.color.unwrap_or(Ribbon::Yellow)),
        })
        .collect();
    let bookmarks = entry
        .marks
        .iter()
        .map(|m| ExportedMark {
            at: m.at,
            chapter: chapter(m.at.line),
            color: m.color,
            text: line_text(m.at.line),
        })
        .collect();
    Exported {
        book,
        exported: log::rfc3339(std::time::SystemTime::now()),
        notes,
        bookmarks,
    }
}

/// The notes as a Markdown document, under the chapters they are in.
pub fn markdown(e: &Exported) -> String {
    let mut out = format!("# {}\n\n", e.book.title);
    let date = e.exported.get(..10).unwrap_or(&e.exported);
    out += &format!(
        "{} notes · {} bookmarks · exported from herdfold on {date}\n",
        e.notes.len(),
        e.bookmarks.len()
    );
    let mut chapter: Option<&str> = None;
    for n in &e.notes {
        if n.chapter.as_deref() != chapter {
            chapter = n.chapter.as_deref();
            if let Some(c) = chapter {
                out += &format!("\n## {c}\n");
            }
        }
        out.push('\n');
        if let Some(q) = &n.quote {
            for l in q.lines() {
                out += &format!("> {l}\n");
            }
            out.push('\n');
        }
        match (&n.question, n.by) {
            (Some(q), _) => out += &format!("**Q.** {q}\n\n**A.** {} *(agent)*\n", n.text),
            (None, Author::Agent) => out += &format!("{} *(agent)*\n", n.text),
            (None, Author::Reader) if !n.text.is_empty() => out += &format!("{}\n", n.text),
            (None, Author::Reader) => {}
        }
    }
    if !e.bookmarks.is_empty() {
        out += "\n## Bookmarks\n\n";
        for m in &e.bookmarks {
            let words: String = m.text.chars().take(60).collect();
            let more = if m.text.chars().count() > 60 {
                "…"
            } else {
                ""
            };
            match &m.chapter {
                Some(c) => out += &format!("- {c}: “{words}{more}” ({})\n", m.color.name()),
                None => out += &format!("- “{words}{more}” ({})\n", m.color.name()),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marks::{Mark, Note};

    #[test]
    fn notes_read_as_markdown_under_their_chapters() {
        let doc = formats::load(
            Format::Md,
            b"# Book\n\nFirst words here.\n\n## Two\n\nSecond part of it.\n".to_vec(),
            "b.md",
            None,
        )
        .unwrap();
        let at = |line, offset| Pos { line, offset };
        let note = |at, anchor, text: &str| Note {
            at,
            anchor,
            text: text.into(),
            by: Author::Reader,
            question: None,
            end: None,
            color: None,
            quote: None,
            lost: false,
        };
        let entry = Entry {
            notes: vec![
                Note {
                    end: Some(at(3, 11)),
                    ..note(at(3, 6), Anchor::Range, "")
                },
                Note {
                    by: Author::Agent,
                    question: Some("Why?".into()),
                    ..note(at(8, 0), Anchor::Page, "Because.")
                },
            ],
            marks: vec![Mark {
                at: at(8, 0),
                color: Ribbon::Blue,
                quote: None,
                lost: false,
            }],
            ..Entry::default()
        };
        let book = Book {
            key: None,
            title: doc.title.clone(),
            format: Format::Md,
        };
        let mut e = gather(&doc, &entry, book);
        e.exported = "2026-10-05T12:00:00Z".into();
        assert_eq!(e.notes[0].quote.as_deref(), Some("words"));
        assert_eq!(
            markdown(&e),
            "# Book\n\n2 notes · 1 bookmarks · exported from herdfold on 2026-10-05\n\
             \n## Book\n\n> words\n\n\
             \n## Two\n\n**Q.** Why?\n\n**A.** Because. *(agent)*\n\
             \n## Bookmarks\n\n- Two: “Second part of it.” (blue)\n"
        );
    }
}
