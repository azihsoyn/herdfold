//! The shelf: `herdfold` with no book shows the books read before, the one
//! last read first, with how far each has got. Choosing one opens it where
//! it was left; closing it comes back here.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event};
use ratatui::layout::{Constraint, Flex, Layout as Split};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::formats::Format;
use crate::keys::Keymap;
use crate::log::{self, Place};
use crate::marks;
use crate::view::{self, Cmd};

/// A book on the shelf.
#[derive(Clone, Debug, PartialEq)]
pub struct Shelved {
    pub key: String,
    pub title: String,
    /// How to read it; unknown for books last read before it was kept.
    pub format: Option<Format>,
    /// When it was last read, RFC 3339.
    pub last_read: Option<String>,
    /// Where the last session left it.
    pub at: Option<Place>,
    /// Time spent reading it, over every session.
    pub seconds: u64,
    pub notes: usize,
    pub bookmarks: usize,
    /// When the last page was first reached.
    pub finished: Option<String>,
    /// The folder it is in, shown when another book has the same title.
    pub folder: Option<String>,
}

/// Every book read, from the reading log and the bookmarks file, the one
/// last read first.
pub fn books() -> Vec<Shelved> {
    let mut by_key: BTreeMap<String, Shelved> = BTreeMap::new();
    for (key, e) in marks::all() {
        let title = e.title.clone().unwrap_or_else(|| file_name(&key));
        by_key.insert(
            key.clone(),
            Shelved {
                key,
                title,
                format: e.format,
                last_read: None,
                at: None,
                seconds: 0,
                notes: e.notes.len(),
                bookmarks: e.marks.len(),
                finished: None,
                folder: None,
            },
        );
    }
    for s in log::summaries() {
        let Some(key) = s.book.key.clone() else {
            continue;
        };
        let b = by_key.entry(key.clone()).or_insert_with(|| Shelved {
            key,
            title: s.book.title.clone(),
            format: None,
            last_read: None,
            at: None,
            seconds: 0,
            notes: 0,
            bookmarks: 0,
            finished: None,
            folder: None,
        });
        if s.finished && b.finished.as_ref().is_none_or(|t| *t > s.ended) {
            b.finished = Some(s.ended.clone());
        }
        b.format = b.format.or(Some(s.book.format));
        b.seconds += s.seconds.unwrap_or(0);
        if b.last_read.as_ref().is_none_or(|t| *t <= s.ended) {
            b.last_read = Some(s.ended.clone());
            b.at = Some(s.to.clone());
        }
    }
    let mut all: Vec<Shelved> = by_key.into_values().collect();
    // Books of the same title are told apart by their folders.
    let mut titles: BTreeMap<String, usize> = BTreeMap::new();
    for b in &all {
        *titles.entry(b.title.clone()).or_default() += 1;
    }
    for b in &mut all {
        if titles[&b.title] > 1 {
            b.folder = Some(folder_of(&b.key));
        }
    }
    all.sort_by(|a, b| b.last_read.cmp(&a.last_read).then(a.title.cmp(&b.title)));
    all
}

/// The folder `key` is in, home written as `~`.
fn folder_of(key: &str) -> String {
    let dir = std::path::Path::new(key)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && dir.starts_with(&home) => {
            format!("~{}", &dir[home.len()..])
        }
        _ => dir,
    }
}

fn file_name(key: &str) -> String {
    std::path::Path::new(key)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| key.to_string())
}

/// Shows the shelf until a book is chosen (its format and file) or the
/// shelf is closed (`None`). `say` is shown first, as a passing message.
pub fn pick(say: Option<String>) -> Result<Option<(Format, PathBuf)>> {
    let books = books();
    if books.is_empty() {
        println!("No books on the shelf yet. Open one with: herdfold --format FORMAT FILE");
        return Ok(None);
    }
    let (keymap, _) = Keymap::load();
    let mut sel = 0;
    let mut message = say;
    let mut terminal = ratatui::init();
    let chosen = loop {
        terminal.draw(|f| draw(f, &books, sel, message.as_deref()))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(k) = event::read()? else {
            continue;
        };
        let Some(key) = view::key_name(k) else {
            continue;
        };
        message = None;
        match keymap.cmd(&key) {
            Some(Cmd::Up | Cmd::Prev) => sel = sel.saturating_sub(1),
            Some(Cmd::Down | Cmd::Next) => sel = (sel + 1).min(books.len() - 1),
            Some(Cmd::Enter) => {
                let b = &books[sel];
                match b.format {
                    _ if !std::path::Path::new(&b.key).is_file() => {
                        message = Some(format!("{} is no longer there", b.key));
                    }
                    Some(format) => break Some((format, PathBuf::from(&b.key))),
                    None => {
                        message = Some(
                            "Read before its format was kept: open it once with --format".into(),
                        );
                    }
                }
            }
            Some(Cmd::Quit | Cmd::Back) => break None,
            _ => {}
        }
    };
    ratatui::restore();
    Ok(chosen)
}

fn draw(f: &mut ratatui::Frame, books: &[Shelved], sel: usize, message: Option<&str>) {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(80);
    let height = view::panel_height(books.len() as u16 * 2, 1).min(area.height.saturating_sub(2));
    let [row] = Split::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [panel] = Split::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(row);
    let hint = message.unwrap_or("Enter open · q close");
    let room = view::draw_panel(f.buffer_mut(), panel, "Shelf", hint, 1);
    let w = room.width.saturating_sub(2) as usize;
    let dim = Style::new().add_modifier(Modifier::DIM);
    let items: Vec<ListItem> = books
        .iter()
        .map(|b| {
            let date = b
                .last_read
                .as_deref()
                .and_then(|t| t.get(..10))
                .unwrap_or("");
            let room = w.saturating_sub(date.width() + 2);
            let title = view::fit(&b.title, room);
            let folder = b
                .folder
                .as_deref()
                .map(|f| view::fit(&format!("  {f}"), room.saturating_sub(title.width())))
                .unwrap_or_default();
            let gap = " ".repeat(w.saturating_sub(title.width() + folder.width() + date.width()));
            let first = Line::from(vec![
                Span::styled(title, Style::new().add_modifier(Modifier::BOLD)),
                Span::styled(folder, dim),
                Span::raw(gap),
                Span::styled(date.to_string(), dim),
            ]);
            ListItem::new(vec![first, Line::from(detail(b, w))])
        })
        .collect();
    let list = List::new(items)
        .style(view::panel_style())
        .highlight_symbol("▶ ")
        .highlight_spacing(ratatui::widgets::HighlightSpacing::Always)
        .highlight_style(Style::new().fg(Color::Cyan));
    let mut state = ListState::default().with_selected(Some(sel));
    f.render_stateful_widget(list, room, &mut state);
    if room.height == 0 {
        f.render_widget(Paragraph::new("Too small for the shelf"), area);
    }
}

/// `━━━━──────  38%  p.120 / 314 · in “Chapter” · 2 h 10 min · 4 notes`
fn detail(b: &Shelved, width: usize) -> Vec<Span<'static>> {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let mut parts = Vec::new();
    if let Some(at) = &b.at {
        parts.push(format!("p.{} / {}", at.page, at.pages));
        if let Some(c) = &at.chapter {
            parts.push(format!("in “{c}”"));
        }
    }
    if b.seconds >= 60 {
        parts.push(log::duration(b.seconds as f64).replace("about ", ""));
    }
    match b.notes {
        0 => {}
        1 => parts.push("1 note".into()),
        n => parts.push(format!("{n} notes")),
    }
    if let Some(t) = &b.finished {
        parts.insert(0, format!("✓ finished {}", t.get(..10).unwrap_or(t)));
    }
    if b.format.is_none() {
        parts.push("format not kept".into());
    }
    let (bar, percent) = match &b.at {
        Some(at) => {
            // A book read to the end is full, wherever it was left.
            let p = if b.finished.is_some() {
                100
            } else {
                (at.page * 100 / at.pages.max(1)).min(100)
            };
            let filled = p / 10;
            (
                vec![
                    Span::raw("━".repeat(filled)),
                    Span::styled("─".repeat(10 - filled), dim),
                ],
                format!("{p:>4}%  "),
            )
        }
        None => (vec![Span::styled("─".repeat(10), dim)], "   –   ".into()),
    };
    let text = view::fit(
        &parts.join(" · "),
        width.saturating_sub(10 + percent.width()),
    );
    let mut spans = bar;
    spans.push(Span::styled(percent, dim));
    spans.push(Span::styled(text, dim));
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_reads_as_a_bar_and_a_percentage() {
        let b = Shelved {
            key: "/b.md".into(),
            title: "B".into(),
            format: Some(Format::Md),
            last_read: Some("2026-10-05T12:00:00Z".into()),
            at: Some(Place {
                line: 0,
                offset: 0,
                page: 30,
                pages: 100,
                chapter: Some("Two".into()),
            }),
            seconds: 3600,
            notes: 2,
            bookmarks: 0,
            finished: None,
            folder: None,
        };
        let text: String = detail(&b, 80).iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            text,
            "━━━───────  30%  p.30 / 100 · in “Two” · 1 h · 2 notes"
        );
    }
}
