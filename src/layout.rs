//! Breaking a document into rows of a fixed width, and rows into pages of a
//! fixed height. Mechanical: where a page ends depends on the size of the
//! page and on the chapters the input declared, never on what the text says.

use std::collections::{BTreeMap, HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::doc::{Document, Kind, Line, Style, Styled};

/// A place in the source text: a line and a character offset into it.
/// Bookmarks are kept as these rather than page numbers, because the page a
/// sentence lands on changes whenever the page size does.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
pub struct Pos {
    pub line: usize,
    pub offset: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub pos: Pos,
    /// The row as plain text.
    pub text: String,
    /// The row as drawn.
    pub spans: Vec<Styled>,
    pub kind: Kind,
    /// What precedes the line's own text on this row (gutter, continuation
    /// mark, indent): in characters, and in columns.
    pub lead: usize,
    pub lead_width: usize,
    /// How many of the line's characters, from `pos.offset`, this row holds.
    pub len: usize,
    /// On the first row of a picture: the columns and rows it is set in.
    pub picture: Option<(u16, u16)>,
    /// Where each character after the lead comes from in the line, for a
    /// row that is not one stretch of it (a table's, its cells side by
    /// side); `None` for padding and rules.
    pub map: Option<Vec<Option<usize>>>,
}

impl Row {
    /// Where in the line the row's `k`-th character after its lead comes
    /// from, if anywhere.
    pub fn source(&self, k: usize) -> Option<usize> {
        match &self.map {
            Some(map) => map.get(k).copied().flatten(),
            None => (k < self.len).then_some(self.pos.offset + k),
        }
    }
}

pub struct Layout {
    pub width: usize,
    pub height: usize,
    rows: Vec<Row>,
    pages: Vec<Span>,
}

/// One page: `rows[start..end]`, set `pad` blank rows down from the top.
struct Span {
    start: usize,
    end: usize,
    pad: usize,
}

impl Layout {
    pub fn new(doc: &Document, width: usize, height: usize) -> Self {
        Self::with_footnotes(doc, width, height, &[], None)
    }

    /// Sets the document leaving room at the foot of each page for the
    /// footnotes on it: `(where, rows)` for each note, in any order. The
    /// first footnote on a page also takes a row for the rule above it.
    /// Pictures are given rows of their own when `cell` (a character cell's
    /// size in pixels) is known, that is where they can be shown; otherwise
    /// their description is set as text.
    pub fn with_footnotes(
        doc: &Document,
        width: usize,
        height: usize,
        footnotes: &[(Pos, usize)],
        cell: Option<(u32, u32)>,
    ) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let tables = table_columns(doc, width);
        let mut rows: Vec<Row> = doc
            .lines
            .iter()
            .enumerate()
            .flat_map(|(i, line)| match (&line.image, cell) {
                (Some(picture), Some(cell)) => picture_rows(i, picture, width, height, cell),
                _ if line.kind == Kind::Table => {
                    table_rows(i, line, tables.get(&i).map_or(&[][..], Vec::as_slice))
                }
                _ => set(i, line, width),
            })
            .collect();
        if rows.is_empty() {
            rows.push(Row {
                pos: Pos::default(),
                text: String::new(),
                spans: Vec::new(),
                kind: Kind::Body,
                lead: 0,
                lead_width: 0,
                len: 0,
                picture: None,
                map: None,
            });
        }
        // Rows of footnote each row carries: the notes on it.
        let mut extra = vec![0; rows.len()];
        for &(at, n) in footnotes {
            let i = rows.partition_point(|r| r.pos <= at).saturating_sub(1);
            extra[i] += n;
        }
        let pages = paginate(&rows, height, &chapter_breaks(doc), &extra);
        Self {
            width,
            height,
            rows,
            pages,
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn page(&self, n: usize) -> &[Row] {
        self.pages
            .get(n)
            .map_or(&[], |p| &self.rows[p.start..p.end])
    }

    /// Blank rows above the text of page `n`: the drop that opens a chapter.
    pub fn pad(&self, n: usize) -> usize {
        self.pages.get(n).map_or(0, |p| p.pad)
    }

    /// The page that shows `pos`.
    pub fn page_of(&self, pos: Pos) -> usize {
        let row = self
            .rows
            .partition_point(|r| r.pos <= pos)
            .saturating_sub(1);
        self.pages
            .partition_point(|p| p.start <= row)
            .saturating_sub(1)
    }

    /// Index, within page `n`, of the row that holds `pos`, if it is on that page.
    pub fn row_of(&self, n: usize, pos: Pos) -> Option<usize> {
        if self.page_of(pos) != n {
            return None;
        }
        let rows = self.page(n);
        let i = rows.partition_point(|r| r.pos <= pos);
        (i > 0).then(|| i - 1)
    }

    /// Where page `n` begins in the source.
    pub fn start_of(&self, n: usize) -> Pos {
        let n = n.min(self.pages.len() - 1);
        self.rows[self.pages[n].start].pos
    }
}

/// Lines that open a chapter and so begin a new page. Only the top tier
/// counts: the shallowest level the input uses more than once (a lone `#`
/// title over many `##` sections makes the sections the chapters).
fn chapter_breaks(doc: &Document) -> HashSet<usize> {
    let mut counts = BTreeMap::new();
    for c in &doc.chapters {
        *counts.entry(c.level).or_insert(0) += 1;
    }
    let tier = counts
        .iter()
        .find(|&(_, &n)| n > 1)
        .or(counts.iter().next())
        .map_or(0, |(&level, _)| level);
    doc.chapters
        .iter()
        .filter(|c| c.level <= tier)
        .map(|c| c.line)
        .collect()
}

/// Rows are cut into pages of `height`. A page never opens on blank rows,
/// a chapter always opens a page (set a quarter of the way down, as a book
/// sets its chapter openings), and a heading is never left as a page's last
/// row, cut off from what it heads.
fn paginate(rows: &[Row], height: usize, breaks: &HashSet<usize>, extra: &[usize]) -> Vec<Span> {
    let opens = |r: &Row| r.pos.offset == 0 && breaks.contains(&r.pos.line);
    let drop = if height >= 12 { height / 4 } else { 0 };
    // Footnote rows row `k` brings onto a page already holding `held`.
    let cost = |k: usize, held: usize| match (extra[k], held) {
        (0, _) => 0,
        (n, 0) => n + 1,
        (n, _) => n,
    };
    let mut pages = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        while i < rows.len() && rows[i].text.trim().is_empty() && rows[i].kind != Kind::Image {
            i += 1;
        }
        if i == rows.len() {
            break;
        }
        let pad = if opens(&rows[i]) { drop } else { 0 };
        let room = height - pad;
        let mut held = cost(i, 0);
        let mut end = i + 1;
        let mut full = false;
        while end < rows.len() && !opens(&rows[end]) {
            // A picture goes whole onto one page.
            if let Some((_, n)) = rows[end].picture
                && end - i + held + n as usize > room
            {
                full = true;
                break;
            }
            let more = cost(end, held);
            if end + 1 - i + held + more > room {
                full = true;
                break;
            }
            held += more;
            end += 1;
        }
        // Pull a heading (and the rule under it) over to the next page.
        if full {
            let mut k = end;
            while k > i + 1 && rows[k - 1].kind == Kind::Rule {
                k -= 1;
            }
            if k > i + 1 && rows[k - 1].kind == Kind::Heading && rows[k - 1].pos.offset == 0 {
                end = k - 1;
            }
        }
        pages.push(Span { start: i, end, pad });
        i = end;
    }
    if pages.is_empty() {
        pages.push(Span {
            start: 0,
            end: rows.len(),
            pad: 0,
        });
    }
    pages
}

fn width_of(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// Characters that may not open a row (kinsoku).
const NO_START: &str = concat!(
    "、。，．・：；？！‼⁇⁈⁉ー〜～）」』】〕〉》〙〗〟’”｝］",
    "ゝゞヽヾ々〻",
    "ぁぃぅぇぉっゃゅょゎゕゖァィゥェォッャュョヮヵヶㇰㇱㇲㇳㇴㇵㇶㇷㇸㇹㇺㇻㇼㇽㇾㇿ",
    ")]},.!?:;",
);
/// Characters that may not close a row.
const NO_END: &str = "（「『【〔〈《〘〖〝‘“｛［([{";
/// Characters kept together when doubled, as ……, ‥‥ and ——.
const NO_SPLIT: &str = "…‥—―";

fn can_break(before: char, after: char) -> bool {
    if before == ' ' {
        return true;
    }
    after != ' '
        && (width_of(before) == 2 || width_of(after) == 2)
        && !NO_START.contains(after)
        && !NO_END.contains(before)
        && !(before == after && NO_SPLIT.contains(before))
}

/// A picture's rows: as wide as it is in cells, at most the column, and
/// never so tall that it cannot share a page with a chapter's opening drop.
fn picture_rows(
    i: usize,
    picture: &crate::doc::Picture,
    width: usize,
    height: usize,
    (cw, ch): (u32, u32),
) -> Vec<Row> {
    let (pw, ph) = (picture.width as f64, picture.height as f64);
    let (cw, ch) = (cw.max(1) as f64, ch.max(1) as f64);
    let most = (height - height / 4).saturating_sub(1).max(1);
    let mut cols = ((pw / cw).ceil() as usize).clamp(1, width);
    let mut rows = ((cols as f64 * cw * ph / pw) / ch).ceil().max(1.0) as usize;
    if rows > most {
        rows = most;
        cols = ((rows as f64 * ch * pw / ph) / cw)
            .floor()
            .clamp(1.0, width as f64) as usize;
    }
    (0..rows)
        .map(|k| Row {
            pos: Pos { line: i, offset: 0 },
            text: String::new(),
            spans: Vec::new(),
            kind: Kind::Image,
            lead: 0,
            lead_width: 0,
            len: 0,
            picture: (k == 0).then_some((cols as u16, rows as u16)),
            map: None,
        })
        .collect()
}

/// Sets line `i` as rows of `width` columns, its gutter and styles applied.
pub fn set(i: usize, line: &Line, width: usize) -> Vec<Row> {
    let w = width.saturating_sub(line.gutter.width()).max(1);
    let gutter = (!line.gutter.is_empty()).then(|| Styled {
        text: line.gutter.clone(),
        style: Style {
            dim: true,
            ..Style::default()
        },
    });
    let row = |offset: usize, spans: Vec<Styled>, lead: &str, len: usize| Row {
        pos: Pos { line: i, offset },
        text: spans.iter().map(|s| s.text.as_str()).collect(),
        spans,
        kind: line.kind,
        lead: lead.chars().count(),
        lead_width: lead.width(),
        len,
        picture: None,
        map: None,
    };
    if line.kind == Kind::Rule {
        let c = line.text.chars().next().unwrap_or('─');
        let rule = Styled {
            text: std::iter::repeat_n(c, w / width_of(c).max(1)).collect(),
            style: line.style,
        };
        return vec![row(
            0,
            gutter.into_iter().chain([rule]).collect(),
            &line.gutter,
            0,
        )];
    }
    let chars: Vec<char> = line.text.chars().collect();
    pieces(&chars, w, line.kind, line.hang)
        .into_iter()
        .map(|p| {
            let mut spans: Vec<Styled> = gutter.iter().cloned().collect();
            if p.carried {
                spans.push(Styled {
                    text: CARRY.to_string(),
                    style: Style {
                        dim: true,
                        ..Style::default()
                    },
                });
            }
            if p.prefix > 0 {
                spans.push(Styled {
                    text: " ".repeat(p.prefix),
                    style: Style::default(),
                });
            }
            for (k, &c) in chars.iter().enumerate().take(p.end).skip(p.start) {
                let style = line.style_at(k);
                match spans.last_mut() {
                    Some(last) if last.style == style && k > p.start => last.text.push(c),
                    _ => spans.push(Styled {
                        text: c.to_string(),
                        style,
                    }),
                }
            }
            let lead = format!(
                "{}{}{}",
                line.gutter,
                if p.carried { CARRY } else { "" },
                " ".repeat(p.prefix)
            );
            row(p.start, spans, &lead, p.end - p.start)
        })
        .collect()
}

/// Columns between table cells.
const CELL_GAP: &str = " │ ";
/// Narrowest a table column is squeezed to.
const MIN_COLUMN: usize = 4;

/// The column widths of each table, by the lines of its rows: each column
/// as wide as its widest cell, unless the table is wider than `width`;
/// then the narrow columns keep their width and the wide ones share what
/// is left.
fn table_columns(doc: &Document, width: usize) -> HashMap<usize, Vec<usize>> {
    let mut out = HashMap::new();
    let mut i = 0;
    while i < doc.lines.len() {
        if doc.lines[i].kind != Kind::Table {
            i += 1;
            continue;
        }
        let start = i;
        while i < doc.lines.len() && doc.lines[i].kind == Kind::Table {
            i += 1;
        }
        let lines = &doc.lines[start..i];
        let cols = lines.iter().map(|l| l.cells.len()).max().unwrap_or(0);
        let natural: Vec<usize> = (0..cols)
            .map(|c| {
                lines
                    .iter()
                    .filter_map(|l| l.cells.get(c).map(|r| cell_text(l, r).width()))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let gutter = lines[0].gutter.width();
        let room = width.saturating_sub(gutter + CELL_GAP.width() * cols.saturating_sub(1));
        let widths = fit_columns(&natural, room);
        for k in start..i {
            out.insert(k, widths.clone());
        }
    }
    out
}

fn cell_text(line: &Line, r: &std::ops::Range<usize>) -> String {
    line.text
        .chars()
        .skip(r.start)
        .take(r.end - r.start)
        .collect()
}

/// `natural` column widths fitted into `room`: those under an equal share
/// keep their width; the rest split what is left.
fn fit_columns(natural: &[usize], room: usize) -> Vec<usize> {
    if natural.iter().sum::<usize>() <= room {
        return natural.to_vec();
    }
    let mut widths = natural.to_vec();
    let mut kept = vec![false; natural.len()];
    loop {
        let used: usize = (0..natural.len())
            .filter(|&c| kept[c])
            .map(|c| widths[c])
            .sum();
        let free: Vec<usize> = (0..natural.len()).filter(|&c| !kept[c]).collect();
        if free.is_empty() {
            return widths;
        }
        let left = room.saturating_sub(used);
        let share = left / free.len();
        let fits: Vec<usize> = free
            .iter()
            .copied()
            .filter(|&c| natural[c] <= share)
            .collect();
        if fits.is_empty() {
            let spare = left - share * free.len();
            for (n, &c) in free.iter().enumerate() {
                widths[c] = (share + usize::from(n < spare)).max(MIN_COLUMN);
            }
            return widths;
        }
        for c in fits {
            kept[c] = true;
        }
    }
}

/// A table row's rows on the page: its cells side by side in their
/// columns, each wrapped within its own; or, for the rule under the head,
/// a rule across the columns.
fn table_rows(i: usize, line: &Line, widths: &[usize]) -> Vec<Row> {
    let dim = Style {
        dim: true,
        ..Style::default()
    };
    let gutter = (!line.gutter.is_empty()).then(|| Styled {
        text: line.gutter.clone(),
        style: dim,
    });
    let row = |offset: usize, spans: Vec<Styled>, len: usize, map: Vec<Option<usize>>| Row {
        pos: Pos { line: i, offset },
        text: spans.iter().map(|s| s.text.as_str()).collect(),
        spans,
        kind: Kind::Table,
        lead: line.gutter.chars().count(),
        lead_width: line.gutter.width(),
        len,
        picture: None,
        map: Some(map),
    };
    if line.cells.is_empty() {
        let rule: Vec<String> = widths.iter().map(|&w| "─".repeat(w)).collect();
        let rule = Styled {
            text: rule.join("─┼─"),
            style: dim,
        };
        let n = rule.text.chars().count();
        return vec![row(
            0,
            gutter.into_iter().chain([rule]).collect(),
            0,
            vec![None; n],
        )];
    }
    let chars: Vec<char> = line.text.chars().collect();
    // Each cell broken into rows within its column, as character ranges.
    let pieces: Vec<Vec<std::ops::Range<usize>>> = widths
        .iter()
        .enumerate()
        .map(|(c, &w)| {
            let Some(cell) = line.cells.get(c) else {
                return Vec::new();
            };
            let text = &chars[cell.start..cell.end.min(chars.len())];
            if text.is_empty() {
                return Vec::new();
            }
            pieces(text, w, Kind::Body, Some(0))
                .into_iter()
                .map(|p| cell.start + p.start..cell.start + p.end)
                .collect()
        })
        .collect();
    let height = pieces.iter().map(Vec::len).max().unwrap_or(0).max(1);
    (0..height)
        .map(|k| {
            let mut spans: Vec<Styled> = gutter.iter().cloned().collect();
            let mut map = Vec::new();
            let push = |spans: &mut Vec<Styled>, c: char, style: Style| match spans.last_mut() {
                Some(last) if last.style == style => last.text.push(c),
                _ => spans.push(Styled {
                    text: c.to_string(),
                    style,
                }),
            };
            for (c, &w) in widths.iter().enumerate() {
                let mut used = 0;
                if let Some(piece) = pieces[c].get(k) {
                    for j in piece.clone() {
                        push(&mut spans, chars[j], line.style_at(j));
                        map.push(Some(j));
                        used += width_of(chars[j]);
                    }
                }
                let last = c + 1 == widths.len();
                if !last {
                    for _ in used..w {
                        push(&mut spans, ' ', line.style);
                        map.push(None);
                    }
                    for g in CELL_GAP.chars() {
                        push(&mut spans, g, dim);
                        map.push(None);
                    }
                }
            }
            // The row starts where its first piece does; the first row, at
            // the line's start, so the line is found there.
            let starts = pieces.iter().filter_map(|p| p.get(k).map(|r| r.start));
            let ends = pieces.iter().filter_map(|p| p.get(k).map(|r| r.end));
            let offset = if k == 0 { 0 } else { starts.min().unwrap_or(0) };
            let len = ends.max().unwrap_or(offset).saturating_sub(offset);
            row(offset, spans, len, map)
        })
        .collect()
}

/// One row of a line: characters `start..end` (trailing spaces dropped),
/// after `prefix` columns of indent, or after the continuation mark when
/// `carried` (a preformatted line broken at the edge).
#[derive(Debug, PartialEq)]
struct Piece {
    start: usize,
    end: usize,
    prefix: usize,
    carried: bool,
}

/// Opens a preformatted row carried over from the row above.
const CARRY: &str = "↪ ";

/// Breaks one line into rows no wider than `width`. Wrapped rows are
/// indented by `hang`, or by the line's own leading spaces when `None`.
fn pieces(chars: &[char], width: usize, kind: Kind, hang: Option<usize>) -> Vec<Piece> {
    if chars.is_empty() {
        return vec![Piece {
            start: 0,
            end: 0,
            prefix: 0,
            carried: false,
        }];
    }
    if kind == Kind::Pre {
        return pieces_hard(chars, width);
    }

    let indent = hang.unwrap_or_else(|| chars.iter().take_while(|&&c| c == ' ').count());
    let hang = if indent * 2 <= width { indent } else { 0 };
    let mut out: Vec<Piece> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let prefix = if out.is_empty() {
            0
        } else {
            while i < chars.len() && chars[i] == ' ' {
                i += 1;
            }
            if i == chars.len() {
                break;
            }
            hang
        };
        let avail = width.saturating_sub(prefix).max(1);
        let mut j = i;
        let mut used = 0;
        while j < chars.len() && used + width_of(chars[j]) <= avail {
            used += width_of(chars[j]);
            j += 1;
        }
        let end = if j == chars.len() {
            j
        } else if j == i {
            i + 1
        } else if chars[j] == ' ' {
            j
        } else {
            (i + 1..=j)
                .rev()
                .find(|&k| can_break(chars[k - 1], chars[k]))
                .filter(|&k| chars[i..k].iter().any(|&c| c != ' '))
                .unwrap_or(j)
        };
        let mut trimmed = end;
        while trimmed > i && chars[trimmed - 1] == ' ' {
            trimmed -= 1;
        }
        out.push(Piece {
            start: i,
            end: trimmed,
            prefix,
            carried: false,
        });
        i = end;
    }
    out
}

/// Breaks a preformatted line hard at the edge. Rows after the first open
/// with the continuation mark, so they are not read as lines of their own.
fn pieces_hard(chars: &[char], width: usize) -> Vec<Piece> {
    // Too narrow to spare room for the mark: break without it.
    let mark = if width > 2 * CARRY.width() {
        CARRY.width()
    } else {
        0
    };
    let mut out = Vec::new();
    let mut start = 0;
    let mut used = 0;
    for (i, &c) in chars.iter().enumerate() {
        let room = if out.is_empty() { width } else { width - mark };
        let w = width_of(c);
        if used + w > room && i > start {
            out.push(Piece {
                start,
                end: i,
                prefix: 0,
                carried: !out.is_empty() && mark > 0,
            });
            start = i;
            used = 0;
        }
        used += w;
    }
    out.push(Piece {
        start,
        end: chars.len(),
        prefix: 0,
        carried: !out.is_empty() && mark > 0,
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Run;

    /// Rows of one line as (offset, text).
    fn wrap(text: &str, width: usize, kind: Kind) -> Vec<(usize, String)> {
        set(0, &Line::new(text, kind), width)
            .into_iter()
            .map(|r| (r.pos.offset, r.text))
            .collect()
    }

    fn rows(text: &str, width: usize) -> Vec<String> {
        wrap(text, width, Kind::Body)
            .into_iter()
            .map(|(_, s)| s)
            .collect()
    }

    fn table(src: &str, width: usize) -> Vec<String> {
        let d = crate::formats::md::load(src);
        Layout::new(&d, width, 100)
            .rows
            .iter()
            .filter(|r| r.kind == Kind::Table)
            .map(|r| r.text.trim_end().to_string())
            .collect()
    }

    #[test]
    fn a_table_that_fits_keeps_its_columns() {
        assert_eq!(
            table("| a | bb |\n|---|---|\n| ccc | d |\n", 40),
            ["a   │ bb", "────┼───", "ccc │ d"]
        );
    }

    #[test]
    fn a_wide_table_wraps_within_its_columns() {
        let src = "| id | text |\n|---|---|\n| 1 | a long cell that will not fit |\n";
        assert_eq!(
            table(src, 20),
            [
                "id │ text",
                "───┼────────────────",
                "1  │ a long cell",
                "   │ that will not",
                "   │ fit",
            ]
        );
    }

    #[test]
    fn table_rows_map_back_to_their_cells() {
        let d = crate::formats::md::load("| ab | cd ef |\n|---|---|\n");
        let l = Layout::new(&d, 9, 100);
        let first = &l.rows[0];
        // "ab │ cd" / "   │ ef": the second row's text is the second cell's.
        assert_eq!(first.source(0), Some(0));
        assert_eq!(first.source(2), None, "the gap between cells");
        assert_eq!(first.source(5), Some(5));
        let second = &l.rows[1];
        assert_eq!(second.text, "   │ ef");
        assert_eq!(second.pos.offset, 8);
        assert_eq!(second.source(5), Some(8));
        assert!(l.rows[0].pos < l.rows[1].pos);
    }

    #[test]
    fn columns_share_what_room_there_is() {
        assert_eq!(fit_columns(&[3, 30, 40], 50), [3, 24, 23]);
        assert_eq!(fit_columns(&[3, 4], 50), [3, 4]);
        assert_eq!(
            fit_columns(&[30, 30, 30], 6),
            [4, 4, 4],
            "never below the least"
        );
    }

    #[test]
    fn styles_follow_the_characters_across_rows() {
        let mut line = Line::new("aa bb cc", Kind::Body);
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        line.runs.push(Run {
            start: 3,
            end: 8,
            style: bold,
        });
        let rows = set(0, &line, 5);
        let spans: Vec<Vec<(&str, bool)>> = rows
            .iter()
            .map(|r| {
                r.spans
                    .iter()
                    .map(|s| (s.text.as_str(), s.style.bold))
                    .collect()
            })
            .collect();
        assert_eq!(
            spans,
            [vec![("aa ", false), ("bb", true)], vec![("cc", true)]]
        );
    }

    #[test]
    fn gutters_repeat_on_every_row_and_hang_aligns_items() {
        let mut line = Line::new("• one two three", Kind::Body);
        line.gutter = "┃ ".into();
        line.hang = Some(2);
        assert_eq!(
            set(0, &line, 11)
                .into_iter()
                .map(|r| r.text)
                .collect::<Vec<_>>(),
            ["┃ • one two", "┃   three"]
        );
    }

    #[test]
    fn rules_span_the_column() {
        assert_eq!(set(0, &Line::new("━", Kind::Rule), 4)[0].text, "━━━━");
    }

    #[test]
    fn breaks_at_spaces() {
        assert_eq!(
            rows("the quick brown fox jumps", 10),
            ["the quick", "brown fox", "jumps"]
        );
    }

    #[test]
    fn breaks_long_words_hard() {
        assert_eq!(rows("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn keeps_hanging_indent() {
        assert_eq!(
            rows("  - one two three", 9),
            ["  - one", "  two", "  three"]
        );
    }

    #[test]
    fn breaks_between_wide_characters() {
        // Each kana is two columns wide.
        assert_eq!(rows("あいうえお", 4), ["あい", "うえ", "お"]);
    }

    #[test]
    fn kinsoku_keeps_punctuation_off_the_row_start() {
        assert_eq!(rows("あい。うえ", 4), ["あ", "い。", "うえ"]);
        assert_eq!(rows("あい)うえ", 4), ["あ", "い)", "うえ"]);
        assert_eq!(rows("あ人々うえ", 4), ["あ", "人々", "うえ"]);
        assert_eq!(rows("あい「うえ", 6), ["あい", "「うえ"]);
        // "…" is one column wide.
        assert_eq!(rows("あ……う", 3), ["あ", "……", "う"]);
    }

    #[test]
    fn offsets_point_into_the_source() {
        let w = wrap("aa bb cc", 5, Kind::Body);
        assert_eq!(w, [(0, "aa bb".into()), (6, "cc".into())]);
    }

    #[test]
    fn preformatted_keeps_spaces_and_marks_carried_rows() {
        assert_eq!(
            wrap("    x  yz", 5, Kind::Pre),
            [(0, "    x".into()), (5, "↪   y".into()), (8, "↪ z".into())]
        );
    }

    fn doc(lines: &[&str]) -> Document {
        Document {
            lines: lines.iter().map(|s| Line::new(*s, Kind::Body)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn pages_hold_height_rows() {
        let d = doc(&["1", "2", "3", "4", "5"]);
        let l = Layout::new(&d, 10, 2);
        assert_eq!(l.page_count(), 3);
        assert_eq!(l.page(2)[0].text, "5");
    }

    #[test]
    fn pages_do_not_open_on_blank_rows() {
        let d = doc(&["1", "2", "", "3"]);
        let l = Layout::new(&d, 10, 2);
        assert_eq!(l.page_count(), 2);
        assert_eq!(l.page(1)[0].text, "3");
    }

    #[test]
    fn a_position_survives_a_relayout() {
        let d = doc(&["aa bb cc dd", "ee ff", "gg"]);
        let wide = Layout::new(&d, 20, 1);
        let at = wide.start_of(1);
        assert_eq!(at, Pos { line: 1, offset: 0 });
        let narrow = Layout::new(&d, 5, 1);
        assert_eq!(narrow.page(narrow.page_of(at))[0].text, "ee ff");
    }

    fn chaptered(lines: &[(&str, Option<u8>)]) -> Document {
        let mut d = Document::default();
        for (i, (text, level)) in lines.iter().enumerate() {
            let kind = if level.is_some() {
                Kind::Heading
            } else {
                Kind::Body
            };
            d.lines.push(Line::new(*text, kind));
            if let Some(level) = level {
                d.chapters.push(crate::doc::Chapter {
                    title: text.to_string(),
                    level: *level,
                    line: i,
                });
            }
        }
        d
    }

    #[test]
    fn a_chapter_opens_a_new_page_set_down() {
        let d = chaptered(&[("A", Some(1)), ("a", None), ("B", Some(1)), ("b", None)]);
        let l = Layout::new(&d, 10, 12);
        assert_eq!(l.page_count(), 2);
        assert_eq!(l.page(1)[0].text, "B");
        assert_eq!((l.pad(0), l.pad(1)), (3, 3));
    }

    #[test]
    fn only_the_top_repeated_tier_breaks_pages() {
        // One title over two sections: the sections are the chapters, and
        // the sub-section stays on its section's page.
        let d = chaptered(&[
            ("T", Some(1)),
            ("S1", Some(2)),
            ("s", Some(3)),
            ("S2", Some(2)),
        ]);
        let l = Layout::new(&d, 10, 20);
        let firsts: Vec<_> = (0..l.page_count())
            .map(|n| l.page(n)[0].text.as_str())
            .collect();
        assert_eq!(firsts, ["T", "S1", "S2"]);
    }

    #[test]
    fn a_heading_is_not_left_at_the_foot_of_a_page() {
        let mut d = doc(&["1", "2", "H", "3"]);
        d.lines[2].kind = Kind::Heading;
        let l = Layout::new(&d, 10, 3);
        assert_eq!(l.page(0).len(), 2);
        assert_eq!(l.page(1)[0].text, "H");
    }

    #[test]
    fn footnotes_take_room_from_their_page() {
        let d = doc(&["1", "2", "3", "4", "5"]);
        // A two-row note on row "2": it and its rule take three of five rows.
        let l = Layout::with_footnotes(&d, 10, 5, &[(Pos { line: 1, offset: 0 }, 2)], None);
        let firsts: Vec<_> = (0..l.page_count())
            .map(|n| l.page(n)[0].text.as_str())
            .collect();
        assert_eq!(firsts, ["1", "3"]);
        assert_eq!(l.page(0).len(), 2);
    }

    fn pictured(lines: &[&str], at: usize, size: (u32, u32)) -> Document {
        let mut d = doc(lines);
        d.lines[at].kind = Kind::Image;
        d.lines[at].image = Some(crate::doc::Picture {
            path: "p.png".into(),
            width: size.0,
            height: size.1,
        });
        d
    }

    #[test]
    fn a_picture_takes_rows_by_its_shape() {
        // 10x20-pixel cells; a 100x100 picture is 10 cells wide, 5 rows tall.
        let d = pictured(&["a", "pic", "b"], 1, (100, 100));
        let l = Layout::with_footnotes(&d, 40, 20, &[], Some((10, 20)));
        let rows = l.page(0);
        assert_eq!(rows[1].picture, Some((10, 5)));
        assert_eq!(rows.len(), 1 + 5 + 1);
        // Without a cell size it is its description, as text.
        let l = Layout::new(&d, 40, 20);
        assert_eq!(l.page(0)[1].text, "pic");
    }

    #[test]
    fn a_picture_is_never_split_across_pages() {
        let d = pictured(&["a", "b", "c", "pic", "d"], 3, (100, 100));
        let l = Layout::with_footnotes(&d, 40, 6, &[], Some((10, 20)));
        // On a six-row page a picture is held to four rows (room for a
        // chapter's drop), 8 x 4; after a, b, c it does not fit, and moves
        // on whole rather than split.
        let firsts: Vec<_> = (0..l.page_count())
            .map(|n| (l.page(n)[0].picture, l.page(n).len()))
            .collect();
        assert_eq!(firsts, [(None, 3), (Some((8, 4)), 5)]);
    }

    #[test]
    fn a_wide_picture_is_held_to_the_column_and_a_tall_one_to_the_page() {
        let d = pictured(&["wide"], 0, (2000, 100));
        let l = Layout::with_footnotes(&d, 40, 20, &[], Some((10, 20)));
        assert_eq!(l.page(0)[0].picture, Some((40, 1)));
        let d = pictured(&["tall"], 0, (100, 5000));
        let l = Layout::with_footnotes(&d, 40, 20, &[], Some((10, 20)));
        let (cols, rows) = l.page(0)[0].picture.unwrap();
        assert_eq!(rows, 14);
        assert!(cols < 10);
    }

    #[test]
    fn empty_document_has_one_page() {
        let l = Layout::new(&Document::default(), 10, 3);
        assert_eq!(l.page_count(), 1);
    }
}
