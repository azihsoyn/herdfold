//! Breaking a document into rows of a fixed width, and rows into pages of a
//! fixed height. Mechanical: where a page ends depends on the size of the
//! page and on the chapters the input declared, never on what the text says.

use std::collections::{BTreeMap, HashSet};

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
        let width = width.max(1);
        let height = height.max(1);
        let mut rows: Vec<Row> = doc
            .lines
            .iter()
            .enumerate()
            .flat_map(|(i, line)| set(i, line, width))
            .collect();
        if rows.is_empty() {
            rows.push(Row {
                pos: Pos::default(),
                text: String::new(),
                spans: Vec::new(),
                kind: Kind::Body,
            });
        }
        let pages = paginate(&rows, height, &chapter_breaks(doc));
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
fn paginate(rows: &[Row], height: usize, breaks: &HashSet<usize>) -> Vec<Span> {
    let opens = |r: &Row| r.pos.offset == 0 && breaks.contains(&r.pos.line);
    let drop = if height >= 12 { height / 4 } else { 0 };
    let mut pages = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        while i < rows.len() && rows[i].text.trim().is_empty() {
            i += 1;
        }
        if i == rows.len() {
            break;
        }
        let pad = if opens(&rows[i]) { drop } else { 0 };
        let room = height - pad;
        let mut end = i + 1;
        while end < rows.len() && end - i < room && !opens(&rows[end]) {
            end += 1;
        }
        // Pull a heading (and the rule under it) over to the next page.
        if end - i == room && end < rows.len() {
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
const NO_START: &str =
    "、。，．・：；？！ー）」』】〕〉》’”…ぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮ";
/// Characters that may not close a row.
const NO_END: &str = "（「『【〔〈《‘“";

fn can_break(before: char, after: char) -> bool {
    if before == ' ' {
        return true;
    }
    after != ' '
        && (width_of(before) == 2 || width_of(after) == 2)
        && !NO_START.contains(after)
        && !NO_END.contains(before)
}

/// Sets line `i` as rows of `width` columns, its gutter and styles applied.
fn set(i: usize, line: &Line, width: usize) -> Vec<Row> {
    let w = width.saturating_sub(line.gutter.width()).max(1);
    let gutter = (!line.gutter.is_empty()).then(|| Styled {
        text: line.gutter.clone(),
        style: Style {
            dim: true,
            ..Style::default()
        },
    });
    let row = |offset: usize, spans: Vec<Styled>| Row {
        pos: Pos { line: i, offset },
        text: spans.iter().map(|s| s.text.as_str()).collect(),
        spans,
        kind: line.kind,
    };
    if line.kind == Kind::Rule {
        let c = line.text.chars().next().unwrap_or('─');
        let rule = Styled {
            text: std::iter::repeat_n(c, w / width_of(c).max(1)).collect(),
            style: line.style,
        };
        return vec![row(0, gutter.into_iter().chain([rule]).collect())];
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
            row(p.start, spans)
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
    fn empty_document_has_one_page() {
        let l = Layout::new(&Document::default(), 10, 3);
        assert_eq!(l.page_count(), 1);
    }
}
