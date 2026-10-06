//! Aozora Bunko text (青空文庫): Shift_JIS (or UTF-8) plain text with its
//! own markup. The title and author open the file; ruby is written
//! `｜漢字《かんじ》` (the bar may be left out before a run of kanji) and
//! set as the reading in brackets after its text; notes in `［＃…］` are
//! dropped, except those naming headings (`［＃「第一章」は中見出し］`),
//! which give the chapters. The block explaining the markup is left out.

use crate::doc::{Chapter, Document, Kind, Line, Run, Style};

pub fn load(bytes: &[u8]) -> Document {
    let text = match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::SHIFT_JIS.decode(bytes).0.into_owned(),
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut doc = Document::default();
    let mut lines = text.lines().peekable();
    // The title, then the author (and maybe a translator), then a blank.
    let mut head = Vec::new();
    while let Some(l) = lines.next_if(|l| !l.trim().is_empty()) {
        head.push(l.trim().to_string());
    }
    doc.title = head.first().cloned().unwrap_or_default();
    for h in &head {
        let mut line = Line::new(h.as_str(), Kind::Heading);
        line.style = Style {
            bold: true,
            accent: true,
            ..Style::default()
        };
        doc.lines.push(line);
    }
    let mut in_rules = false;
    for raw in lines {
        // The block between two rules of hyphens explains the markup.
        if raw.starts_with("-----") {
            in_rules = !in_rules;
            continue;
        }
        if in_rules {
            continue;
        }
        let heading = heading_of(raw);
        let (text, runs) = set(raw);
        // Blank lines either side of the left-out block come to one.
        let blank = text.trim().is_empty();
        if blank && doc.lines.last().is_some_and(|l| l.text.is_empty()) {
            continue;
        }
        let mut line = Line::new(if blank { "" } else { text.trim_end() }, Kind::Body);
        line.runs = runs;
        if let Some((title, level)) = heading {
            line.kind = Kind::Heading;
            line.style = Style {
                bold: true,
                accent: level <= 2,
                ..Style::default()
            };
            doc.chapters.push(Chapter {
                title,
                level,
                line: doc.lines.len(),
            });
        }
        doc.lines.push(line);
    }
    while doc.lines.last().is_some_and(|l| l.text.is_empty()) {
        doc.lines.pop();
    }
    doc
}

/// The heading a line's notes name, with its level: 大見出し 1, 中 2, 小 3.
fn heading_of(line: &str) -> Option<(String, u8)> {
    for (word, level) in [("大見出し", 1), ("中見出し", 2), ("小見出し", 3)] {
        let tail = format!("」は{word}］");
        if let Some(end) = line.find(&tail) {
            let start = line[..end].rfind("［＃「")? + "［＃「".len();
            return Some((plain(&line[start..end]), level));
        }
    }
    None
}

/// Text with its markup taken out, readings and all.
fn plain(s: &str) -> String {
    set(s).0
}

/// Whether `c` is kanji, as a ruby written without the bar reads back over.
fn is_kanji(c: char) -> bool {
    matches!(c, '\u{4e00}'..='\u{9fff}' | '\u{3400}'..='\u{4dbf}' | '\u{f900}'..='\u{faff}' | '々' | '〆' | 'ヶ' | '〇')
}

/// A line set as it reads: notes dropped, each ruby's reading in brackets
/// after its text, dimmed.
fn set(line: &str) -> (String, Vec<Run>) {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<char> = Vec::new();
    let mut runs = Vec::new();
    // Where in `out` the text a bar marks for ruby starts.
    let mut bar: Option<usize> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '［' if chars.get(i + 1) == Some(&'＃') => {
                // A note: skipped to its close.
                let mut depth = 0;
                while i < chars.len() {
                    match chars[i] {
                        '［' => depth += 1,
                        '］' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            '｜' => bar = Some(out.len()),
            '《' => {
                let close = chars[i..].iter().position(|&c| c == '》').map(|n| i + n);
                let Some(close) = close else {
                    out.push(c);
                    i += 1;
                    continue;
                };
                // Without a bar, the reading is for the kanji just before.
                let base = bar.take().unwrap_or_else(|| {
                    let n = out.iter().rev().take_while(|&&c| is_kanji(c)).count();
                    out.len() - n
                });
                if base < out.len() {
                    let start = out.len();
                    out.push('（');
                    out.extend(&chars[i + 1..close]);
                    out.push('）');
                    runs.push(Run {
                        start,
                        end: out.len(),
                        style: Style {
                            dim: true,
                            ..Style::default()
                        },
                    });
                } else {
                    out.extend(&chars[i..=close]);
                }
                i = close;
            }
            c => out.push(c),
        }
        i += 1;
    }
    (out.into_iter().collect(), runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruby_is_set_after_its_text() {
        let (text, runs) = set("吾輩《わがはい》は｜猫である《ねこである》。");
        assert_eq!(text, "吾輩（わがはい）は猫である（ねこである）。");
        assert_eq!((runs[0].start, runs[0].end), (2, 8));
    }

    #[test]
    fn notes_are_dropped_and_headings_found() {
        let src = "吾輩は猫である\n夏目漱石\n\n-------\n【テキスト中に現れる記号について】\n《》：ルビ\n-------\n\n［＃５字下げ］一［＃「一」は中見出し］\n\n　吾輩は猫である。※［＃「てへん＋劣」、第3水準1-84-77］\n\n底本：「吾輩は猫である」";
        let d = load(src.as_bytes());
        assert_eq!(d.title, "吾輩は猫である");
        let texts: Vec<&str> = d.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "吾輩は猫である",
                "夏目漱石",
                "",
                "一",
                "",
                "　吾輩は猫である。※",
                "",
                "底本：「吾輩は猫である」"
            ]
        );
        assert_eq!(d.chapters.len(), 1);
        assert_eq!(
            (d.chapters[0].title.as_str(), d.chapters[0].line),
            ("一", 3)
        );
    }

    #[test]
    fn shift_jis_is_read() {
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode("題\n著者\n\n本文《ほんぶん》");
        let d = load(&bytes);
        assert_eq!(d.lines.last().unwrap().text, "本文（ほんぶん）");
    }
}
