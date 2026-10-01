//! Breaking a document into rows of a fixed width, and rows into pages of a
//! fixed height. Purely mechanical: where a page ends depends only on the
//! size of the page, never on what the text says.

use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthChar;

use crate::doc::{Document, Kind};

/// A place in the source text: a line and a character offset into it.
/// Bookmarks are kept as these rather than page numbers, because the page a
/// sentence lands on changes whenever the page size does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Pos {
    pub line: usize,
    pub offset: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub pos: Pos,
    pub text: String,
    pub kind: Kind,
}

pub struct Layout {
    pub width: usize,
    pub height: usize,
    rows: Vec<Row>,
    /// Index into `rows` of the first row of each page.
    starts: Vec<usize>,
}

impl Layout {
    pub fn new(doc: &Document, width: usize, height: usize) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let mut rows = Vec::new();
        for (i, line) in doc.lines.iter().enumerate() {
            for (offset, text) in wrap(&line.text, width, line.kind) {
                rows.push(Row {
                    pos: Pos { line: i, offset },
                    text,
                    kind: line.kind,
                });
            }
        }
        if rows.is_empty() {
            rows.push(Row {
                pos: Pos::default(),
                text: String::new(),
                kind: Kind::Body,
            });
        }
        let starts = paginate(&rows, height);
        Self {
            width,
            height,
            rows,
            starts,
        }
    }

    pub fn page_count(&self) -> usize {
        self.starts.len()
    }

    pub fn page(&self, n: usize) -> &[Row] {
        let Some(&start) = self.starts.get(n) else {
            return &[];
        };
        let end = self
            .starts
            .get(n + 1)
            .copied()
            .unwrap_or(self.rows.len())
            .min(start + self.height);
        &self.rows[start..end]
    }

    /// The page that shows `pos`.
    pub fn page_of(&self, pos: Pos) -> usize {
        let row = self.rows.partition_point(|r| r.pos <= pos).saturating_sub(1);
        self.starts.partition_point(|&s| s <= row).saturating_sub(1)
    }

    /// Where page `n` begins in the source.
    pub fn start_of(&self, n: usize) -> Pos {
        let n = n.min(self.starts.len() - 1);
        self.rows[self.starts[n]].pos
    }
}

/// Rows are cut into pages of `height`; a page never opens on blank rows.
fn paginate(rows: &[Row], height: usize) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let first = i;
        while i < rows.len() && rows[i].text.trim().is_empty() {
            i += 1;
        }
        if i == rows.len() {
            if starts.is_empty() {
                starts.push(first);
            }
            break;
        }
        starts.push(i);
        i += height;
    }
    starts
}

fn width_of(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// Characters that may not open a row (kinsoku).
const NO_START: &str = "、。，．・：；？！ー）」』】〕〉》’”…ぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮ";
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

/// Breaks one source line into rows no wider than `width`. Returns each row
/// with the character offset it starts at.
pub fn wrap(text: &str, width: usize, kind: Kind) -> Vec<(usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![(0, String::new())];
    }
    if kind == Kind::Pre {
        return wrap_hard(&chars, width);
    }

    let indent = chars.iter().take_while(|&&c| c == ' ').count();
    let hang = if indent * 2 <= width { indent } else { 0 };
    let mut out = Vec::new();
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
        let row: String = chars[i..end].iter().collect();
        out.push((i, " ".repeat(prefix) + row.trim_end()));
        i = end;
    }
    out
}

fn wrap_hard(chars: &[char], width: usize) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut row = String::new();
    let mut used = 0;
    for (i, &c) in chars.iter().enumerate() {
        let w = width_of(c);
        if used + w > width && !row.is_empty() {
            out.push((start, std::mem::take(&mut row)));
            start = i;
            used = 0;
        }
        row.push(c);
        used += w;
    }
    out.push((start, row));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Line;

    fn rows(text: &str, width: usize) -> Vec<String> {
        wrap(text, width, Kind::Body)
            .into_iter()
            .map(|(_, s)| s)
            .collect()
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
        assert_eq!(rows("  - one two three", 9), ["  - one", "  two", "  three"]);
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
    fn preformatted_keeps_spaces() {
        assert_eq!(
            wrap("    x  y", 5, Kind::Pre),
            [(0, "    x".into()), (5, "  y".into())]
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

    #[test]
    fn empty_document_has_one_page() {
        let l = Layout::new(&Document::default(), 10, 3);
        assert_eq!(l.page_count(), 1);
    }
}
