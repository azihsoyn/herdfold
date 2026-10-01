//! One page as drawn in one pane: margins, running head, text, and a footer
//! with the page number and a progress bar. A `PageView` carries everything
//! needed to draw it, so the pane holding the right-hand page can draw
//! without the document.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde::{Deserialize, Serialize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::doc::Kind;

/// Rows above the text: margin, running head, gap.
const TOP: u16 = 3;
/// Rows below the text: gap, footer, margin.
const BOTTOM: u16 = 3;
/// Minimum blank columns on each side of the text.
const SIDE: u16 = 4;
/// Longest row we set. Typesetting convention is 60-80 characters; past that
/// the eye loses its way back to the next row.
pub const MEASURE: usize = 72;

/// The text area a pane of `width` x `height` leaves after margins.
pub fn text_size(width: u16, height: u16) -> (usize, usize) {
    let w = (width.saturating_sub(2 * SIDE) as usize).clamp(1, MEASURE);
    let h = height.saturating_sub(TOP + BOTTOM).max(1) as usize;
    (w, h)
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Side {
    Single,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageView {
    pub side: Side,
    pub head: String,
    pub rows: Vec<(String, Kind)>,
    /// 1-based.
    pub number: usize,
    pub total: usize,
    pub marked: bool,
    /// Width the rows were set to; the column is centred on it.
    pub width: usize,
    /// A one-off message shown in place of the running head.
    pub note: Option<String>,
}

/// What a key asks for, in either pane.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Cmd {
    Next,
    Prev,
    Mark,
    Contents,
    Quit,
    Up,
    Down,
    Enter,
    Back,
}

pub fn cmd_of(key: KeyEvent) -> Option<Cmd> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return (key.code == KeyCode::Char('c')).then_some(Cmd::Quit);
    }
    Some(match key.code {
        KeyCode::Char(' ') | KeyCode::Right | KeyCode::PageDown => Cmd::Next,
        KeyCode::Char('b') | KeyCode::Left | KeyCode::PageUp => Cmd::Prev,
        KeyCode::Char('m') => Cmd::Mark,
        KeyCode::Char('g') => Cmd::Contents,
        KeyCode::Char('q') => Cmd::Quit,
        KeyCode::Char('k') | KeyCode::Up => Cmd::Up,
        KeyCode::Char('j') | KeyCode::Down => Cmd::Down,
        KeyCode::Enter => Cmd::Enter,
        KeyCode::Esc => Cmd::Back,
        _ => return None,
    })
}

/// The left edge and width of the text column in `area`.
pub fn column(area: Rect, width: usize) -> (u16, u16) {
    let w = (width as u16).min(area.width);
    (area.x + (area.width - w) / 2, w)
}

pub fn render(f: &mut Frame, area: Rect, view: Option<&PageView>) {
    let Some(v) = view else { return };
    if area.height < TOP + BOTTOM || area.width < 4 {
        return;
    }
    let (x, w) = column(area, v.width);
    let dim = Style::new().add_modifier(Modifier::DIM);

    // Running head on the outer edge; the ribbon, when bookmarked, by the gutter.
    let ribbon = if v.marked { "▍" } else { "" };
    let (head, style) = match &v.note {
        Some(n) => (n.as_str(), Style::new()),
        None => (v.head.as_str(), dim.add_modifier(Modifier::ITALIC)),
    };
    let room = (w as usize).saturating_sub(2);
    let head = fit(head, room);
    let gap = " ".repeat((w as usize).saturating_sub(head.width() + ribbon.width()));
    let line = match v.side {
        Side::Right => Line::from(vec![Span::raw(ribbon), Span::raw(gap), Span::styled(head, style)]),
        Side::Left | Side::Single => {
            Line::from(vec![Span::styled(head, style), Span::raw(gap), Span::raw(ribbon)])
        }
    };
    f.render_widget(Paragraph::new(line), Rect::new(x, area.y + 1, w, 1));

    let text_h = area.height - TOP - BOTTOM;
    let lines: Vec<Line> = v
        .rows
        .iter()
        .take(text_h as usize)
        .map(|(s, kind)| match kind {
            Kind::Heading => Line::styled(s.as_str(), Style::new().add_modifier(Modifier::BOLD)),
            Kind::Body | Kind::Pre => Line::raw(s.as_str()),
        })
        .collect();
    f.render_widget(Paragraph::new(lines), Rect::new(x, area.y + TOP, w, text_h));

    let footer = footer(v, w as usize);
    f.render_widget(Paragraph::new(footer), Rect::new(x, area.y + area.height - 2, w, 1));
}

/// `12  ━━━━━━──────` on a left page, `━━━━━━──────  13 / 240` otherwise,
/// so page numbers sit on the outer edges of the spread.
fn footer(v: &PageView, width: usize) -> Line<'static> {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let label = match v.side {
        Side::Left => format!("{}  ", v.number),
        Side::Right | Side::Single => format!("  {} / {}", v.number, v.total),
    };
    let bar = width.saturating_sub(label.width());
    let filled = (bar * v.number).div_ceil(v.total.max(1)).min(bar);
    let full = Span::raw("━".repeat(filled));
    let rest = Span::styled("─".repeat(bar - filled), dim);
    match v.side {
        Side::Left => Line::from(vec![Span::raw(label), full, rest]),
        Side::Right | Side::Single => Line::from(vec![full, rest, Span::raw(label)]),
    }
}

/// Truncates `s` to `width` columns, marking the cut with an ellipsis.
pub fn fit(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_capped_at_the_measure() {
        assert_eq!(text_size(200, 40), (MEASURE, 34));
        assert_eq!(text_size(50, 40), (42, 34));
    }

    #[test]
    fn fit_truncates_by_columns() {
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("あいう", 5), "あい…");
        assert_eq!(fit("ab", 4), "ab");
    }
}
