//! The book as it is read aloud: its text in sentences, without the
//! breaks the pages put in it, readings given for the text they read, and
//! each sentence with where it is in the book. A voice reads them one
//! after another; a screen reader is handed the open pages' worth.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::doc::{Document, Kind, Line};
use crate::layout::Pos;

/// What of a ruby is read: its reading (漢字（かんじ） as かんじ), the text
/// it reads (漢字), or both as they are set.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, clap::ValueEnum,
)]
#[serde(rename_all = "snake_case")]
pub enum RubyReading {
    #[default]
    Reading,
    Base,
    Both,
}

/// One sentence, or a line of code, a table's row, a picture's description.
#[derive(Clone, Debug, PartialEq)]
pub struct Sentence {
    pub at: Pos,
    /// Where it ends (exclusive).
    pub end: Pos,
    pub kind: Kind,
    pub text: String,
}

/// The sentences that start from `from` up to `to` (exclusive); the last
/// may run on past `to`, so none is cut short.
pub fn sentences(doc: &Document, from: Pos, to: Pos, ruby: RubyReading) -> Vec<Sentence> {
    let mut out = Vec::new();
    for (i, line) in doc.lines.iter().enumerate().skip(from.line) {
        if i > to.line || (i == to.line && to.offset == 0) {
            break;
        }
        let start = if i == from.line { from.offset } else { 0 };
        let len = line.text.chars().count();
        let pos = |offset| Pos { line: i, offset };
        let whole = |text: String| {
            let text = tidy(&text);
            (!text.is_empty()).then(|| Sentence {
                at: pos(start),
                end: pos(len),
                kind: line.kind,
                text,
            })
        };
        match line.kind {
            Kind::Rule => {}
            Kind::Pre | Kind::Image if start < len => {
                let text: String = line.text.chars().skip(start).collect();
                out.extend(whole(text));
            }
            // A table's row as its cells, one after another.
            Kind::Table if start == 0 && !line.cells.is_empty() => {
                let chars: Vec<char> = line.text.chars().collect();
                let cells: Vec<String> = line
                    .cells
                    .iter()
                    .map(|c| tidy(&said(line, &chars, c.start, c.end.min(len), ruby)))
                    .filter(|c| !c.is_empty())
                    .collect();
                out.extend(whole(cells.join(", ")));
            }
            Kind::Body | Kind::Heading => {
                let chars: Vec<char> = line.text.chars().collect();
                for (a, b) in split(&chars, start) {
                    if i == to.line && a >= to.offset {
                        break;
                    }
                    let text = tidy(&said(line, &chars, a, b, ruby));
                    if !text.is_empty() {
                        out.push(Sentence {
                            at: pos(a),
                            end: pos(b),
                            kind: line.kind,
                            text,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Where the sentences of `chars` are, from `start`: each up to the stop
/// that ends it (。！？, or . ! ? before a space) and the brackets and
/// quotes that close after it; the line's end ends the last.
fn split(chars: &[char], start: usize) -> Vec<(usize, usize)> {
    const STOPS: &str = "。！？．!?";
    const CLOSERS: &str = "」』）)】〕\"”’'";
    let mut out = Vec::new();
    let skip_spaces = |mut i: usize| {
        while chars.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        i
    };
    let mut a = skip_spaces(start);
    let mut i = a;
    while i < chars.len() {
        let c = chars[i];
        let stop =
            STOPS.contains(c) || (c == '.' && chars.get(i + 1).is_none_or(|n| n.is_whitespace()));
        i += 1;
        if stop {
            while i < chars.len() && (STOPS.contains(chars[i]) || CLOSERS.contains(chars[i])) {
                i += 1;
            }
            out.push((a, i));
            i = skip_spaces(i);
            a = i;
        }
    }
    if a < chars.len() {
        out.push((a, chars.len()));
    }
    out
}

/// The characters `a..b` of the line as they are read: a ruby's reading in
/// place of its text, or its text alone, as `ruby` says.
fn said(line: &Line, chars: &[char], a: usize, b: usize, ruby: RubyReading) -> String {
    let left_out = |k: usize| {
        line.ruby.iter().any(|r| {
            let bracket = k + 1 == r.reading.start || k == r.reading.end;
            match ruby {
                RubyReading::Reading => r.base.contains(&k) || bracket,
                RubyReading::Base => r.reading.contains(&k) || bracket,
                RubyReading::Both => false,
            }
        })
    };
    (a..b.min(chars.len()))
        .filter(|&k| !left_out(k))
        .map(|k| chars[k])
        .collect()
}

/// Runs of spaces as one, and none at the ends.
fn tidy(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Ruby;

    fn doc(lines: &[(&str, Kind)]) -> Document {
        Document {
            lines: lines.iter().map(|(t, k)| Line::new(*t, *k)).collect(),
            ..Default::default()
        }
    }

    fn texts(s: &[Sentence]) -> Vec<&str> {
        s.iter().map(|s| s.text.as_str()).collect()
    }

    const END: Pos = Pos {
        line: usize::MAX,
        offset: 0,
    };

    #[test]
    fn lines_are_read_in_sentences() {
        let d = doc(&[
            ("A turn is drawn. It takes 0.3 seconds! Really?", Kind::Body),
            ("", Kind::Body),
            (
                "　ある日の暮方の事である。「下人が、雨やみを待っていた。」広い門の下には",
                Kind::Body,
            ),
        ]);
        let s = sentences(&d, Pos::default(), END, RubyReading::Reading);
        assert_eq!(
            texts(&s),
            [
                "A turn is drawn.",
                "It takes 0.3 seconds!",
                "Really?",
                "ある日の暮方の事である。",
                "「下人が、雨やみを待っていた。」",
                "広い門の下には"
            ]
        );
        assert_eq!(
            (s[1].at, s[1].end),
            (
                Pos {
                    line: 0,
                    offset: 17
                },
                Pos {
                    line: 0,
                    offset: 38
                }
            )
        );
    }

    #[test]
    fn readings_are_read_in_place_of_their_text() {
        let mut d = doc(&[("羅生門（らしょうもん）の下で", Kind::Body)]);
        d.lines[0].ruby = vec![Ruby {
            base: 0..3,
            reading: 4..10,
        }];
        let read = |r| sentences(&d, Pos::default(), END, r)[0].text.clone();
        assert_eq!(read(RubyReading::Reading), "らしょうもんの下で");
        assert_eq!(read(RubyReading::Base), "羅生門の下で");
        assert_eq!(read(RubyReading::Both), "羅生門（らしょうもん）の下で");
    }

    #[test]
    fn sentences_start_within_the_range_and_run_on_past_it() {
        let d = doc(&[("One. Two three. Four.", Kind::Body), ("Five.", Kind::Body)]);
        let s = sentences(
            &d,
            Pos { line: 0, offset: 5 },
            Pos { line: 0, offset: 7 },
            RubyReading::Reading,
        );
        assert_eq!(texts(&s), ["Two three."]);
    }

    #[test]
    fn code_tables_and_rules_are_told_apart() {
        let mut d = doc(&[
            ("fn turn() {}", Kind::Pre),
            ("───", Kind::Rule),
            ("Key  What", Kind::Table),
        ]);
        d.lines[2].cells = vec![0..3, 5..9];
        let s = sentences(&d, Pos::default(), END, RubyReading::Reading);
        assert_eq!(texts(&s), ["fn turn() {}", "Key, What"]);
        assert_eq!((s[0].kind, s[1].kind), (Kind::Pre, Kind::Table));
    }
}
