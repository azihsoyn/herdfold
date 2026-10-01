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
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use ratatui::style::Color;

use crate::doc::{Style as TextStyle, Styled};

/// Rows above the text: margin, running head, gap.
const TOP: u16 = 3;
/// Rows below the text: gap, footer, margin.
const BOTTOM: u16 = 3;
/// Minimum blank columns on each side of the text.
const SIDE: u16 = 4;
/// Longest row set by default. Typesetting convention is 60-80 characters;
/// past that the eye loses its way back to the next row.
pub const MEASURE: usize = 72;
/// Bounds and step for changing the measure with `<` / `>`.
pub const MEASURE_MIN: usize = 24;
pub const MEASURE_MAX: usize = 240;
pub const MEASURE_STEP: usize = 4;

/// The text area a pane of `width` x `height` leaves after margins, with
/// rows no longer than `measure`.
pub fn text_size(width: u16, height: u16, measure: usize) -> (usize, usize) {
    let w = (width.saturating_sub(2 * SIDE) as usize).clamp(1, measure.max(1));
    let h = height.saturating_sub(TOP + BOTTOM).max(1) as usize;
    (w, h)
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Single,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PageView {
    pub side: Side,
    pub head: String,
    pub rows: Vec<PageRow>,
    /// 1-based.
    pub number: usize,
    pub total: usize,
    pub marked: bool,
    /// Width the rows were set to; the column is centred on it.
    pub width: usize,
    /// A one-off message shown in place of the running head.
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PageRow {
    pub spans: Vec<Styled>,
}

/// What a key asks for, in either pane.
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// Longer rows (narrower margins).
    Wider,
    /// Shorter rows (wider margins).
    Narrower,
}

/// A key press under herdr's key names (`space`, `esc`, `ctrl+c`, `b`, ...),
/// which is how keys travel between panes.
pub fn key_name(key: KeyEvent) -> Option<String> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    let base = match key.code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::PageUp => "pageup".into(),
        KeyCode::PageDown => "pagedown".into(),
        _ => return None,
    };
    Some(if key.modifiers.contains(KeyModifiers::CONTROL) {
        format!("ctrl+{base}")
    } else {
        base
    })
}

pub fn cmd_of(key: &str) -> Option<Cmd> {
    Some(match key {
        "space" | "right" | "pagedown" => Cmd::Next,
        "b" | "left" | "pageup" => Cmd::Prev,
        "m" => Cmd::Mark,
        "g" => Cmd::Contents,
        "q" | "ctrl+c" => Cmd::Quit,
        "k" | "up" => Cmd::Up,
        "j" | "down" => Cmd::Down,
        "enter" => Cmd::Enter,
        "esc" => Cmd::Back,
        ">" => Cmd::Wider,
        "<" => Cmd::Narrower,
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
        .map(|r| {
            Line::from(
                r.spans
                    .iter()
                    .map(|s| Span::styled(s.text.as_str(), style_of(s.style)))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    f.render_widget(Paragraph::new(lines), Rect::new(x, area.y + TOP, w, text_h));

    let footer = footer(v, w as usize);
    f.render_widget(Paragraph::new(footer), Rect::new(x, area.y + area.height - 2, w, 1));
}

fn style_of(t: TextStyle) -> Style {
    let mut s = Style::new();
    for (on, m) in [
        (t.bold, Modifier::BOLD),
        (t.italic, Modifier::ITALIC),
        (t.underline, Modifier::UNDERLINED),
        (t.strike, Modifier::CROSSED_OUT),
        (t.dim, Modifier::DIM),
    ] {
        if on {
            s = s.add_modifier(m);
        }
    }
    if t.accent {
        s = s.fg(Color::Cyan);
    }
    if t.code {
        s = s.fg(Color::Yellow);
    }
    s
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
        assert_eq!(text_size(200, 40, MEASURE), (MEASURE, 34));
        assert_eq!(text_size(50, 40, MEASURE), (42, 34));
        assert_eq!(text_size(200, 40, 100), (100, 34));
    }

    #[test]
    fn keys_travel_by_herdr_names() {
        let press = |code, modifiers| key_name(KeyEvent::new(code, modifiers));
        assert_eq!(press(KeyCode::Char(' '), KeyModifiers::NONE).as_deref(), Some("space"));
        assert_eq!(press(KeyCode::Char('c'), KeyModifiers::CONTROL).as_deref(), Some("ctrl+c"));
        assert_eq!(cmd_of("space"), Some(Cmd::Next));
        assert_eq!(cmd_of("ctrl+c"), Some(Cmd::Quit));
        assert_eq!(cmd_of("x"), None);
    }

    #[test]
    fn fit_truncates_by_columns() {
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("あいう", 5), "あい…");
        assert_eq!(fit("ab", 4), "ab");
    }
}
