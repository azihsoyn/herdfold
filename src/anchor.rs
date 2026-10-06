//! Keeping places on their text. A bookmark, a note or a marker is kept as
//! a place in the text (a line and a character in it), which survives any
//! change to how the pages are set but not a change to the text itself: a
//! book edited or replaced, or read differently by a newer herdfold. So
//! each also keeps the words at its place and a few either side, and when
//! the book is opened, a place whose words are no longer there is found
//! again by them, as annotation tools on the web do. One that cannot be
//! found stays where it was, marked lost.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::doc::Document;
use crate::layout::Pos;
use crate::marks::Entry;

/// The words at a place, and a few either side to tell one occurrence
/// from another.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TextQuote {
    /// The words at the place: the marked text, or the start of the row.
    pub exact: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suffix: String,
}

/// Characters of context kept either side.
const CONTEXT: usize = 32;
/// Characters kept from a place that is a point (a page, a row).
const HEAD: usize = 48;
/// Longest stretch of marked text kept.
const MOST: usize = 2000;

/// The book's text as one string, lines joined by newlines, to look words up in.
pub struct Text {
    text: String,
    /// Where each line starts in `text`, in bytes.
    starts: Vec<usize>,
}

impl Text {
    pub fn new(doc: &Document) -> Self {
        let mut text = String::new();
        let mut starts = Vec::with_capacity(doc.lines.len());
        for (i, l) in doc.lines.iter().enumerate() {
            if i > 0 {
                text.push('\n');
            }
            starts.push(text.len());
            text.push_str(&l.text);
        }
        Self { text, starts }
    }

    /// `at` as a byte in the text, if it is in it.
    fn byte(&self, at: Pos) -> Option<usize> {
        let start = *self.starts.get(at.line)?;
        let end = self
            .starts
            .get(at.line + 1)
            .map_or(self.text.len(), |n| n - 1);
        let line = &self.text[start..end];
        let off = line
            .char_indices()
            .nth(at.offset)
            .map_or(line.len(), |(b, _)| b);
        Some(start + off)
    }

    /// The place of byte `b` of the text.
    fn pos(&self, b: usize) -> Pos {
        let line = self.starts.partition_point(|&s| s <= b).saturating_sub(1);
        let start = self.starts.get(line).copied().unwrap_or(0);
        Pos {
            line,
            offset: self.text[start..b].chars().count(),
        }
    }

    fn before(&self, b: usize) -> String {
        let chars: Vec<char> = self.text[..b].chars().rev().take(CONTEXT).collect();
        chars.into_iter().rev().collect()
    }

    fn after(&self, b: usize) -> String {
        self.text[b..].chars().take(CONTEXT).collect()
    }

    /// The words at a point, as a bookmark or a page note keeps them.
    pub fn quote_point(&self, at: Pos) -> Option<TextQuote> {
        let b = self.byte(at)?;
        // The words of its own line (after any blank lines it starts on):
        // what follows may change without the place moving.
        let rest = &self.text[b..];
        let blank = rest.len() - rest.trim_start_matches('\n').len();
        let line = rest[blank..].split('\n').next().unwrap_or("");
        let exact: String = rest[..blank]
            .chars()
            .chain(line.chars())
            .take(HEAD.max(blank + 1))
            .collect();
        let end = b + exact.len();
        Some(TextQuote {
            exact,
            prefix: self.before(b),
            suffix: self.after(end),
        })
    }

    /// The words from `at` to `end`, as a marker keeps them.
    pub fn quote_range(&self, at: Pos, end: Pos) -> Option<TextQuote> {
        let (b, e) = (self.byte(at)?, self.byte(end)?);
        let exact: String = self.text.get(b..e.max(b))?.chars().take(MOST).collect();
        let e = b + exact.len();
        Some(TextQuote {
            exact,
            prefix: self.before(b),
            suffix: self.after(e),
        })
    }

    /// Whether `quote`'s words are still at `at`.
    fn holds(&self, at: Pos, quote: &TextQuote) -> bool {
        self.byte(at)
            .is_some_and(|b| self.text[b..].starts_with(&quote.exact))
    }

    /// Where `quote`'s words are now, and where they end: of every place
    /// they appear, the one whose surroundings match the most, then the
    /// nearest to `hint`.
    pub fn locate(&self, quote: &TextQuote, hint: Pos) -> Option<(Pos, Pos)> {
        if quote.exact.is_empty() {
            return None;
        }
        let near = self.byte(hint).unwrap_or(self.text.len());
        let score = |b: usize| {
            let before = common_suffix(&self.text[..b], &quote.prefix);
            let after = common_prefix(&self.text[b + quote.exact.len()..], &quote.suffix);
            (before + after, std::cmp::Reverse(b.abs_diff(near)))
        };
        let best = self
            .text
            .match_indices(quote.exact.as_str())
            .map(|(b, _)| b)
            .max_by_key(|&b| score(b))?;
        Some((self.pos(best), self.pos(best + quote.exact.len())))
    }
}

impl Text {
    /// Where a point whose own words were reworded is now: just after the
    /// words that came before it, if they are still there (else just
    /// before the words that came after it), the nearest to `hint`.
    pub fn locate_near(&self, quote: &TextQuote, hint: Pos) -> Option<Pos> {
        let near = self.byte(hint).unwrap_or(self.text.len());
        let nearest = |found: Vec<usize>| found.into_iter().min_by_key(|b| b.abs_diff(near));
        let after_prefix = (quote.prefix.chars().count() >= 8)
            .then(|| {
                nearest(
                    self.text
                        .match_indices(quote.prefix.as_str())
                        .map(|(b, _)| b + quote.prefix.len())
                        .collect(),
                )
            })
            .flatten();
        let before_suffix = || {
            (quote.suffix.chars().count() >= 8)
                .then(|| {
                    nearest(
                        self.text
                            .match_indices(quote.suffix.as_str())
                            .map(|(b, _)| b)
                            .collect(),
                    )
                })
                .flatten()
        };
        let b = after_prefix.or_else(before_suffix)?;
        // A place is the start of a row: the start of the line it is in.
        let pos = self.pos(b);
        Some(Pos {
            line: pos.line,
            offset: if after_prefix.is_some() {
                pos.offset
            } else {
                0
            },
        })
    }
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

fn common_suffix(a: &str, b: &str) -> usize {
    a.chars()
        .rev()
        .zip(b.chars().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

/// What checking a book's places found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Checked {
    /// Found again somewhere else.
    pub moved: usize,
    /// Not found: left where they were, marked lost.
    pub lost: usize,
}

/// Checks every bookmark and note in `entry` against `text`: each whose
/// words have moved is put where they are now, each whose words are gone
/// is marked lost, and each kept before its words were (by herdfold 0.4
/// or earlier) takes the words at its place.
pub fn check(text: &Text, entry: &mut Entry) -> Checked {
    let mut checked = Checked::default();
    let mut found = |at: &mut Pos, end: Option<&mut Pos>, quote: &mut Option<TextQuote>| {
        let Some(q) = quote else {
            *quote = match &end {
                Some(end) => text.quote_range(*at, **end),
                None => text.quote_point(*at),
            };
            return true;
        };
        if text.holds(*at, q) {
            return true;
        }
        match text.locate(q, *at) {
            Some((start, stop)) => {
                *at = start;
                if let Some(end) = end {
                    *end = stop;
                }
                checked.moved += 1;
                true
            }
            // A point whose own words were reworded is found by the words
            // around it; marked text is not, lest other words be marked.
            None if end.is_none() => match text.locate_near(q, *at) {
                Some(near) => {
                    *at = near;
                    *q = text.quote_point(near).unwrap_or_default();
                    checked.moved += 1;
                    true
                }
                None => false,
            },
            None => false,
        }
    };
    for m in &mut entry.marks {
        m.lost = !found(&mut m.at, None, &mut m.quote);
    }
    for n in &mut entry.notes {
        let mut end = n.end;
        n.lost = !found(&mut n.at, end.as_mut(), &mut n.quote);
        n.end = end;
    }
    if entry.place.is_some() {
        let mut place = entry.place.take();
        if found(&mut entry.at, None, &mut place) {
            entry.place = place;
        }
    }
    checked.lost = entry.marks.iter().filter(|m| m.lost).count()
        + entry.notes.iter().filter(|n| n.lost).count();
    entry.marks.sort_by_key(|m| m.at);
    entry.notes.sort_by_key(|n| n.at);
    checked
}

/// A digest of a book's bytes, to know it again under another name:
/// FNV-1a, 64 bits, in hex.
pub fn digest(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Kind, Line};
    use crate::marks::{Mark, Note, Ribbon};

    fn doc(lines: &[&str]) -> Document {
        Document {
            lines: lines.iter().map(|l| Line::new(*l, Kind::Body)).collect(),
            ..Default::default()
        }
    }

    fn at(line: usize, offset: usize) -> Pos {
        Pos { line, offset }
    }

    #[test]
    fn places_follow_their_words_when_the_text_moves() {
        let before = Text::new(&doc(&["one", "the cat sat", "on the mat"]));
        let mut entry = Entry {
            marks: vec![Mark {
                at: at(2, 0),
                color: Ribbon::Red,
                quote: None,
                lost: false,
            }],
            notes: vec![Note {
                at: at(1, 4),
                end: Some(at(1, 7)),
                anchor: crate::marks::Anchor::Range,
                text: String::new(),
                by: crate::marks::Author::Reader,
                question: None,
                color: None,
                quote: None,
                lost: false,
            }],
            ..Entry::default()
        };
        // Kept before words were: they take them now.
        assert_eq!(check(&before, &mut entry), Checked::default());
        assert_eq!(entry.notes[0].quote.as_ref().unwrap().exact, "cat");
        // Two lines are put in front, and the marker's line is reworded.
        let after = Text::new(&doc(&[
            "new",
            "lines",
            "one",
            "a black cat sat",
            "on the mat",
        ]));
        assert_eq!(check(&after, &mut entry), Checked { moved: 2, lost: 0 });
        assert_eq!(entry.marks[0].at, at(4, 0));
        assert_eq!(
            (entry.notes[0].at, entry.notes[0].end),
            (at(3, 8), Some(at(3, 11)))
        );
        // The words gone: lost, and left where they were.
        let gone = Text::new(&doc(&["nothing", "here"]));
        assert_eq!(check(&gone, &mut entry), Checked { moved: 0, lost: 2 });
        assert!(entry.marks[0].lost && entry.notes[0].lost);
    }

    #[test]
    fn a_point_keeps_only_its_own_line() {
        let t = Text::new(&doc(&["Title", "", "First paragraph."]));
        assert_eq!(t.quote_point(at(0, 0)).unwrap().exact, "Title");
        assert_eq!(t.quote_point(at(1, 0)).unwrap().exact, "\nFirst paragraph.");
        // Text put after the title does not move a bookmark on it.
        let mut entry = Entry {
            marks: vec![Mark {
                at: at(0, 0),
                color: Ribbon::Red,
                quote: t.quote_point(at(0, 0)),
                lost: false,
            }],
            ..Entry::default()
        };
        let edited = Text::new(&doc(&["Title", "", "Inserted.", "", "First paragraph."]));
        assert_eq!(check(&edited, &mut entry), Checked::default());
    }

    #[test]
    fn a_reworded_row_is_found_by_its_neighbours_but_marked_text_is_not() {
        let t = Text::new(&doc(&["Before it.", "The row noted.", "After it."]));
        let mut entry = Entry {
            notes: vec![
                Note {
                    at: at(1, 0),
                    end: None,
                    anchor: crate::marks::Anchor::Line,
                    text: "n".into(),
                    by: crate::marks::Author::Reader,
                    question: None,
                    color: None,
                    quote: t.quote_point(at(1, 0)),
                    lost: false,
                },
                Note {
                    at: at(1, 8),
                    end: Some(at(1, 13)),
                    anchor: crate::marks::Anchor::Range,
                    text: String::new(),
                    by: crate::marks::Author::Reader,
                    question: None,
                    color: None,
                    quote: t.quote_range(at(1, 8), at(1, 13)),
                    lost: false,
                },
            ],
            ..Entry::default()
        };
        let edited = Text::new(&doc(&[
            "New first.",
            "Before it.",
            "A row reworded.",
            "After it.",
        ]));
        assert_eq!(check(&edited, &mut entry), Checked { moved: 1, lost: 1 });
        let row = entry.notes.iter().find(|n| n.end.is_none()).unwrap();
        assert_eq!((row.at, row.lost), (at(2, 0), false));
        assert!(entry.notes.iter().find(|n| n.end.is_some()).unwrap().lost);
    }

    #[test]
    fn of_several_occurrences_the_one_in_the_same_words_is_taken() {
        let t = Text::new(&doc(&["the cat ran.", "the cat sat."]));
        let q = t.quote_range(at(1, 4), at(1, 7)).unwrap();
        let moved = Text::new(&doc(&["x", "the cat ran.", "the cat sat."]));
        assert_eq!(moved.locate(&q, at(0, 0)), Some((at(2, 4), at(2, 7))));
    }

    #[test]
    fn digests_are_stable() {
        assert_eq!(digest(b""), "cbf29ce484222325");
        assert_eq!(digest(b"a"), "af63dc4c8601ec8c");
    }
}
