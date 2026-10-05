//! One page as drawn in one pane: margins, running head, text, and a footer
//! with the page number and a progress bar. A `PageView` carries everything
//! needed to draw it, so the pane holding the right-hand page can draw
//! without the document.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::doc::{Style as TextStyle, Styled};
use crate::marks::Ribbon;

/// Rows above the text: margin, running head, gap.
pub const TOP: u16 = 3;
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
/// Cells a vertical column takes across: two for the characters, one gap.
pub const COLUMN_STRIDE: usize = 3;

/// Longest vertical column, in characters, as a printed page holds.
const COLUMN_MAX: usize = 42;

/// The size to set vertical text to in a pane of `width` x `height`: the
/// row length (columns' characters at two cells each) and the rows (columns)
/// a page holds, the page no wider than `measure`.
pub fn vertical_size(width: u16, height: u16, measure: usize) -> (usize, usize) {
    let across = (width.saturating_sub(2 * SIDE) as usize).clamp(1, measure.max(1));
    let columns = (across + 1) / COLUMN_STRIDE;
    let down = (height.saturating_sub(TOP + BOTTOM) as usize).clamp(1, COLUMN_MAX);
    (down * 2, columns.max(1))
}

/// The vertical form of `c`, for punctuation and brackets that turn.
pub fn upright(c: char) -> char {
    match c {
        '、' => '︑',
        '。' => '︒',
        '，' => '︐',
        '：' => '︓',
        '；' => '︔',
        '！' => '︕',
        '？' => '︖',
        '「' => '﹁',
        '」' => '﹂',
        '『' => '﹃',
        '』' => '﹄',
        '（' => '︵',
        '）' => '︶',
        '｛' => '︷',
        '｝' => '︸',
        '〔' => '︹',
        '〕' => '︺',
        '【' => '︻',
        '】' => '︼',
        '《' => '︽',
        '》' => '︾',
        '〈' => '︿',
        '〉' => '﹀',
        '［' => '﹇',
        '］' => '﹈',
        '…' => '︙',
        '‥' => '︰',
        'ー' => '丨',
        '—' | '―' => '︱',
        '－' => '︲',
        '〜' | '～' => '≀',
        c => c,
    }
}

/// The cells of a vertical column, top down: the character indexes each
/// holds. A wide character has a cell to itself; narrow ones (digits,
/// Latin) go two to a cell, side by side.
pub fn column_cells(text: &str) -> Vec<std::ops::Range<usize>> {
    let chars: Vec<char> = text.chars().collect();
    let mut cells = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let narrow = |c: char| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) < 2;
        let n = if narrow(chars[i]) && chars.get(i + 1).is_some_and(|&c| narrow(c)) {
            2
        } else {
            1
        };
        cells.push(i..i + n);
        i += n;
    }
    cells
}

/// The left edge of vertical column `i` (counted from the right) in a
/// column of text at `x`, `w` wide.
pub fn column_x(x: u16, w: u16, i: usize) -> Option<u16> {
    (x + w)
        .checked_sub(2 + (i * COLUMN_STRIDE) as u16)
        .filter(|&c| c >= x)
}

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
    /// The bookmark on this page, by the colour of its ribbon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ribbon: Option<Ribbon>,
    /// The page has a reading note on it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub noted: bool,
    /// Width the rows were set to; the column is centred on it.
    pub width: usize,
    /// A passing message, shown as a snackbar at the bottom right of the
    /// book: on the right page of a spread, else on the only one.
    pub status: Option<String>,
    /// Notes to set in the outer margin, beside their rows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub margin_notes: Vec<MarginNote>,
    /// Pictures to set over the rows left for them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pictures: Vec<crate::pictures::PagePicture>,
    /// How much is left, beside the page number: of the chapter, and
    /// (from the reading log's pace) of the book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<String>,
    /// Set in vertical columns, right to left, each row a column.
    #[serde(default, skip_serializing_if = "is_false")]
    pub vertical: bool,
}

/// A note shown in the margin, from row `row` (counted as in `rows`) down.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MarginNote {
    pub row: usize,
    pub text: String,
}

/// Narrowest margin a note is set in; below this only the marks show.
pub const MARGIN_NOTE_MIN: usize = 12;

/// Columns of outer margin a pane of `width` leaves for notes beside a
/// column of `column` (two kept clear for the stroke and a gap).
pub fn margin_room(width: u16, column: usize) -> usize {
    ((width as usize).saturating_sub(column) / 2).saturating_sub(3)
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PageRow {
    pub spans: Vec<Styled>,
    /// The find being shown is on this row; a pointer is drawn beside it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pointer: bool,
    /// A note is attached to this row; a mark is drawn beside it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub marker: bool,
    /// The row under the cursor while choosing a row.
    #[serde(default, skip_serializing_if = "is_false")]
    pub selected: bool,
}

/// What a key asks for, in either pane. Which keys ask for what is in
/// `keys`, and can be changed in the config.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// Draw page turns, or stop drawing them.
    Animate,
    /// Write a note on the page.
    NotePage,
    /// Choose a row, to write a note on it.
    Select,
    /// The list of bookmarks and notes.
    Shelf,
    /// Change how notes are shown.
    NoteDisplay,
    /// Ask the agent about the page, or the chosen row.
    Ask,
    /// Change the colour of the bookmark here.
    Color,
    /// The keys, listed.
    Help,
    /// Which way the pages run: left to right, or right to left.
    Direction,
    /// A tip on using the reader.
    Tip,
    /// Search the book.
    Search,
    /// Set the book in vertical columns, or across again.
    Vertical,
    /// Remove the bookmark or note chosen in the list.
    Delete,
    /// Go back to where the last jump left from.
    Return,
    /// Choose a link on the open pages, to see where it leads.
    Follow,
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
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Tab => "tab".into(),
        _ => return None,
    };
    Some(if key.modifiers.contains(KeyModifiers::CONTROL) {
        format!("ctrl+{base}")
    } else {
        base
    })
}

/// Draws one frame inside a synchronized update, so the terminal (and herdr)
/// shows it whole rather than row by row; a turn is many frames in a row.
pub fn draw_whole(
    terminal: &mut ratatui::DefaultTerminal,
    render: impl FnOnce(&mut ratatui::Frame),
) -> std::io::Result<()> {
    use crossterm::execute;
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
    execute!(std::io::stdout(), BeginSynchronizedUpdate)?;
    let drawn = terminal.draw(render).map(|_| ());
    execute!(std::io::stdout(), EndSynchronizedUpdate)?;
    drawn
}

/// The left edge and width of the text column in `area`.
pub fn column(area: Rect, width: usize) -> (u16, u16) {
    let w = (width as u16).min(area.width);
    (area.x + (area.width - w) / 2, w)
}

pub fn render(buf: &mut Buffer, area: Rect, view: Option<&PageView>) {
    let Some(v) = view else { return };
    // Page 0 is the blank facing the last page of a book bound on the right.
    if v.number == 0 {
        return;
    }
    if area.height < TOP + BOTTOM || area.width < 4 {
        return;
    }
    let (x, w) = column(area, v.width);
    let dim = Style::new().add_modifier(Modifier::DIM);

    // Running head on the outer edge; the pencil (a note on the page) by
    // the gutter.
    let ribbon = if v.noted { "✎" } else { "" };
    let (head, style) = (v.head.as_str(), dim.add_modifier(Modifier::ITALIC));
    let room = (w as usize).saturating_sub(2);
    let head = fit(head, room);
    let gap = " ".repeat((w as usize).saturating_sub(head.width() + ribbon.width()));
    let line = match v.side {
        Side::Right => Line::from(vec![
            Span::raw(ribbon),
            Span::raw(gap),
            Span::styled(head, style),
        ]),
        Side::Left | Side::Single => Line::from(vec![
            Span::styled(head, style),
            Span::raw(gap),
            Span::raw(ribbon),
        ]),
    };
    Paragraph::new(line).render(Rect::new(x, area.y + 1, w, 1), buf);

    let text_h = area.height - TOP - BOTTOM;
    if v.vertical {
        render_columns(buf, area, x, w, text_h, v);
    } else {
        render_rows(buf, area, x, w, text_h, v);
    }
    if let Some(text) = &v.status {
        draw_snackbar(buf, area, text);
    }
    if let Some(color) = v.ribbon {
        draw_ribbon(buf, area, x, w, v.side, color);
    }
    let footer = footer(v, w as usize);
    Paragraph::new(footer).render(Rect::new(x, area.y + area.height - 2, w, 1), buf);
}

/// The rows set as vertical columns, the first on the right.
fn render_columns(buf: &mut Buffer, area: Rect, x: u16, w: u16, text_h: u16, v: &PageView) {
    for (i, r) in v.rows.iter().enumerate() {
        let Some(cx) = column_x(x, w, i) else {
            break;
        };
        // Each character with its style, in reading order.
        let chars: Vec<(char, Style)> = r
            .spans
            .iter()
            .flat_map(|s| {
                let style = style_of(s.style);
                s.text.chars().map(move |c| (c, style))
            })
            .collect();
        let text: String = chars.iter().map(|(c, _)| *c).collect();
        for (k, cell) in column_cells(&text)
            .into_iter()
            .enumerate()
            .take(text_h as usize)
        {
            let y = area.y + TOP + k as u16;
            for (n, j) in cell.enumerate() {
                let (c, mut style) = chars[j];
                if r.selected {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                buf.set_string(cx + n as u16, y, upright(c).to_string(), style);
            }
        }
        if r.selected {
            for k in 0..text_h {
                let cell = &mut buf[(cx, area.y + TOP + k)];
                cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
            }
        }
        // Above its column: the find being shown, or a noted row.
        let above = area.y + TOP - 1;
        if r.pointer {
            buf.set_string(cx, above, "▼", Style::new().fg(CURRENT_FIND));
        } else if r.marker {
            buf.set_string(cx, above, "▁", Style::new().fg(Color::Yellow));
        }
    }
}

/// The rows set across, as most text is.
fn render_rows(buf: &mut Buffer, area: Rect, x: u16, w: u16, text_h: u16, v: &PageView) {
    let lines: Vec<Line> = v
        .rows
        .iter()
        .take(text_h as usize)
        .map(|r| {
            let mut spans: Vec<Span> = r
                .spans
                .iter()
                .map(|s| Span::styled(s.text.as_str(), style_of(s.style)))
                .collect();
            if r.selected {
                // Fill the row so the cursor reads as a bar, even on a blank row.
                let used: usize = r.spans.iter().map(|s| s.text.width()).sum();
                spans.push(Span::raw(" ".repeat((w as usize).saturating_sub(used))));
                return Line::from(spans).patch_style(Modifier::REVERSED);
            }
            Line::from(spans)
        })
        .collect();
    Paragraph::new(lines).render(Rect::new(x, area.y + TOP, w, text_h), buf);

    draw_margin_notes(buf, area, x, w, text_h, v);

    // A row with a note gets a mark in the margin, like a highlighter's
    // stroke; the row of the find being shown, a pointer.
    if x >= area.x + 2 {
        for (i, r) in v.rows.iter().take(text_h as usize).enumerate() {
            let y = area.y + TOP + i as u16;
            if r.pointer {
                buf[(x - 2, y)].set_symbol("▶").set_fg(CURRENT_FIND);
            } else if r.marker {
                buf[(x - 2, y)].set_symbol("▎").set_fg(Color::Yellow);
            }
        }
    }
}

/// Sets margin notes in the outer margin, each from its row down, below the
/// one before when they would meet. Too narrow a margin shows none.
fn draw_margin_notes(buf: &mut Buffer, area: Rect, x: u16, w: u16, text_h: u16, v: &PageView) {
    let room = margin_room(area.width, w as usize);
    if v.margin_notes.is_empty() || room < MARGIN_NOTE_MIN {
        return;
    }
    let left = match v.side {
        Side::Left => area.x + 1,
        Side::Right | Side::Single => x + w + 3,
    };
    let style = Style::new().add_modifier(Modifier::ITALIC);
    let mut next_free = 0;
    for note in &v.margin_notes {
        let mut line = crate::doc::Line::new(note.text.as_str(), crate::doc::Kind::Body);
        // Wrapped rows hang after the note's mark.
        line.hang = Some(2);
        let mut row = note.row.max(next_free);
        for r in crate::layout::set(0, &line, room) {
            if row >= text_h as usize {
                return;
            }
            let y = area.y + TOP + row as u16;
            buf.set_stringn(left, y, &r.text, room, style);
            row += 1;
        }
        next_free = row + 1;
    }
}

/// The find being shown, set apart from the other finds.
const CURRENT_FIND: Color = Color::Indexed(208);

/// The colour that marks the find being shown, as a style.
pub fn current_find() -> Style {
    Style::new().fg(CURRENT_FIND)
}

/// The reader's own panels (tips, keys, lists, messages) sit on a ground
/// of their own, so they are never taken for the book's text.
const PANEL_BG: Color = Color::Indexed(236);
const PANEL_FG: Color = Color::Indexed(253);
const PANEL_EDGE: Color = Color::Cyan;

pub fn panel_style() -> Style {
    Style::new().bg(PANEL_BG).fg(PANEL_FG)
}

/// How a key's name is set within a panel.
pub fn key_style() -> Style {
    Style::new().fg(PANEL_EDGE).add_modifier(Modifier::BOLD)
}

/// A panel's frame: rounded and edged in the accent colour. Nothing is
/// written on the frame itself, so its edge stays clean.
pub fn panel_block() -> ratatui::widgets::Block<'static> {
    ratatui::widgets::Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::new().fg(PANEL_EDGE))
        .style(panel_style())
}

/// Draws a panel over `area`: the frame, `title` on the first row inside
/// and `hint` (what the keys do here) on the last, each `gap` blank rows
/// from what lies between. Returns the room left between them.
pub fn draw_panel(buf: &mut Buffer, area: Rect, title: &str, hint: &str, gap: u16) -> Rect {
    ratatui::widgets::Clear.render(area, buf);
    let block = panel_block();
    let inner = block.inner(area);
    block.render(area, buf);
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let mut top = inner.y;
    let mut bottom = inner.bottom();
    if !title.is_empty() && top < bottom {
        buf.set_stringn(x, top, title, w as usize, key_style());
        top = (top + 1 + gap).min(bottom);
    }
    if !hint.is_empty() && bottom > top {
        bottom -= 1;
        let grey = Style::new().fg(Color::Indexed(245));
        buf.set_stringn(x, bottom, hint, w as usize, grey);
        bottom = bottom.saturating_sub(gap).max(top);
    }
    Rect::new(x, top, w, bottom - top)
}

/// Rows a panel needs around `content` rows: frame, title, hint and gaps.
pub fn panel_height(content: u16, gap: u16) -> u16 {
    content + 4 + 2 * gap
}

/// Dims the page behind a panel, so the panel reads as in front of it.
pub fn backdrop(buf: &mut Buffer, area: Rect) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].modifier.insert(Modifier::DIM);
        }
    }
}

/// A message in a small panel at the bottom right of the pane, above the
/// footer, as herdr shows its notifications.
fn draw_snackbar(buf: &mut Buffer, area: Rect, text: &str) {
    let room = (area.width as usize).saturating_sub(6);
    if room < 8 || area.height < 8 {
        return;
    }
    let text = fit(text, room.min(60));
    let w = text.width() as u16 + 4;
    let x = area.right() - w - 1;
    let y = area.bottom() - 6;
    let frame = Rect::new(x, y, w, 3);
    ratatui::widgets::Clear.render(frame, buf);
    Paragraph::new(Line::raw(format!(" {text} ")))
        .block(panel_block())
        .render(frame, buf);
}

pub fn ribbon_color(r: Ribbon) -> Color {
    match r {
        Ribbon::Red => Color::Red,
        Ribbon::Yellow => Color::Yellow,
        Ribbon::Green => Color::Green,
        Ribbon::Cyan => Color::Cyan,
        Ribbon::Blue => Color::Blue,
        Ribbon::Magenta => Color::Magenta,
    }
}

/// A bookmark ribbon hanging from the top edge into the margin by the
/// gutter, swallow-tailed at its end; in a pane with no margin to hang
/// it in, a mark at the corner of the running head instead.
fn draw_ribbon(buf: &mut Buffer, area: Rect, x: u16, w: u16, side: Side, color: Ribbon) {
    let fg = Style::new().fg(ribbon_color(color));
    let at = ribbon_area(area, x, w, side);
    if at.height == 1 {
        buf.set_string(at.x, at.y, "▍", fg);
        return;
    }
    for dy in 0..3 {
        buf.set_string(at.x, at.y + dy, "██", fg);
    }
    buf.set_string(at.x, at.y + 3, "▛▜", fg);
}

/// Where a page's ribbon hangs, in a pane `area` whose text column starts
/// at `x` and is `w` wide: two columns by four rows in the margin by the
/// gutter, or one cell at the head's corner when there is no margin.
pub fn ribbon_area(area: Rect, x: u16, w: u16, side: Side) -> Rect {
    let rx = match side {
        Side::Right => x.checked_sub(4).filter(|&rx| rx >= area.x),
        Side::Left | Side::Single => Some(x + w + 2).filter(|&rx| rx + 2 <= area.right()),
    };
    match rx {
        Some(rx) => Rect::new(rx, area.y, 2, 4),
        None => {
            let corner = match side {
                Side::Right => x,
                Side::Left | Side::Single => x + w.saturating_sub(1),
            };
            Rect::new(corner, area.y + 1, 1, 1)
        }
    }
}

pub fn style_of(t: TextStyle) -> Style {
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
    if let Some(c) = t.marker {
        s = s.bg(ribbon_color(c)).fg(Color::Black);
    }
    match (t.found, t.selected) {
        // The find being shown: unmistakable.
        (true, true) => {
            s = s
                .bg(CURRENT_FIND)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD);
        }
        // Other finds: marked, but quietly.
        (true, false) => {
            s = s.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED);
        }
        (false, true) => s = s.add_modifier(Modifier::REVERSED),
        (false, false) => {}
    }
    s
}

/// Shortest progress bar worth drawing beside what is left.
const MIN_BAR: usize = 8;

/// `12  ━━━━━━──────` on a left page, `━━━━━━──────  13 / 240` otherwise,
/// so page numbers sit on the outer edges of the spread.
fn footer(v: &PageView, width: usize) -> Line<'static> {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let label = match v.side {
        Side::Left => format!("{}  ", v.number),
        Side::Right | Side::Single => format!("  {} / {}", v.number, v.total),
    };
    // What is left goes before the number, while the bar keeps some length.
    let left = v
        .remaining
        .as_deref()
        .map(|r| format!("  {r}"))
        .filter(|r| width >= label.width() + r.width() + MIN_BAR)
        .unwrap_or_default();
    let bar = width.saturating_sub(label.width() + left.width());
    let filled = (bar * v.number).div_ceil(v.total.max(1)).min(bar);
    let full = Span::raw("━".repeat(filled));
    let rest = Span::styled("─".repeat(bar - filled), dim);
    match v.side {
        Side::Left => Line::from(vec![Span::raw(label), full, rest]),
        Side::Right | Side::Single => {
            Line::from(vec![full, rest, Span::styled(left, dim), Span::raw(label)])
        }
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
        assert_eq!(
            press(KeyCode::Char(' '), KeyModifiers::NONE).as_deref(),
            Some("space")
        );
        assert_eq!(
            press(KeyCode::Char('c'), KeyModifiers::CONTROL).as_deref(),
            Some("ctrl+c")
        );
    }

    #[test]
    fn fit_truncates_by_columns() {
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("あいう", 5), "あい…");
        assert_eq!(fit("ab", 4), "ab");
    }
}
