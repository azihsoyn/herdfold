use crate::doc::{Chapter, Document, Ink, Kind, Line, Run, Style};
use crate::highlight::Highlighter;

/// A diff is kept exactly as written. The only structure taken from it is
/// where one file's changes begin; that header line is drawn as a heading.
/// Added and removed lines are marked by colour, and their code highlighted
/// by the file's language where it is known, as the old and the new file
/// each read.
pub fn load(src: &str) -> Document {
    let mut doc = Document::default();
    let mut file = String::new();
    // Highlighters for the old and the new file, from the hunk's start.
    let mut old: Option<Highlighter> = None;
    let mut new: Option<Highlighter> = None;
    let mut in_hunk = false;
    for (i, l) in src.lines().enumerate() {
        if let Some(rest) = l.strip_prefix("diff --git ") {
            file = file_name(rest);
            doc.chapters.push(Chapter {
                title: file.clone(),
                level: 1,
                line: i,
            });
            in_hunk = false;
            doc.lines.push(Line::new(l, Kind::Heading));
            continue;
        }
        let mut line = Line::new(l, Kind::Pre);
        if l.starts_with("@@") {
            in_hunk = true;
            let lang = language_of(&file);
            (old, new) = (Highlighter::new(&lang), Highlighter::new(&lang));
            line.style = palette(CYAN);
        } else if !in_hunk {
            // `index`, `---`, `+++` and the like.
            line.style.dim = true;
        } else {
            let (sign, rest) = l.split_at(l.chars().next().map_or(0, char::len_utf8));
            let (color, highlighter) = match sign {
                "+" => (Some(GREEN), new.as_mut()),
                "-" => (Some(RED), old.as_mut()),
                _ => {
                    // Context reads in both files.
                    if let Some(h) = old.as_mut() {
                        h.line(rest);
                    }
                    (None, new.as_mut())
                }
            };
            match (highlighter, color) {
                (Some(h), _) => {
                    let text: String = line.text.chars().skip(1).collect();
                    line.runs = h
                        .line(&text)
                        .into_iter()
                        .map(|r| Run {
                            start: r.start + 1,
                            end: r.end + 1,
                            ..r
                        })
                        .collect();
                    if let Some(c) = color {
                        line.runs.insert(
                            0,
                            Run {
                                start: 0,
                                end: 1,
                                style: Style {
                                    bold: true,
                                    ..palette(c)
                                },
                            },
                        );
                    }
                    line.style.dim = sign == "-";
                }
                // No grammar for the file: the whole line in its colour.
                (None, Some(c)) => line.style = palette(c),
                (None, None) => {}
            }
        }
        doc.lines.push(line);
    }
    doc
}

const RED: u8 = 1;
const GREEN: u8 = 2;
const CYAN: u8 = 6;

fn palette(c: u8) -> Style {
    Style {
        color: Some(Ink::Palette(c)),
        ..Style::default()
    }
}

/// The language a file is in, as its extension (or, without one, its name)
/// tells.
fn language_of(file: &str) -> String {
    let path = std::path::Path::new(file);
    path.extension()
        .or_else(|| path.file_name())
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `a/src/x.rs b/src/x.rs` -> `src/x.rs`
fn file_name(rest: &str) -> String {
    match rest.rsplit_once(" b/") {
        Some((_, b)) => b.to_string(),
        None => rest.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_file_is_a_chapter() {
        let d = load("diff --git a/x.rs b/x.rs\n+1\ndiff --git a/y z.rs b/y z.rs\n-2\n");
        let ch: Vec<_> = d
            .chapters
            .iter()
            .map(|c| (c.title.as_str(), c.line))
            .collect();
        assert_eq!(ch, [("x.rs", 0), ("y z.rs", 2)]);
    }

    #[test]
    fn changes_are_coloured_and_their_code_highlighted() {
        let d = load(
            "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-fn old() {}\n+fn new() {}\ndiff --git a/n.zzz b/n.zzz\n@@ -1 +1 @@\n+plain\n",
        );
        assert!(d.lines[1].style.dim, "file headers are quiet");
        assert_eq!(d.lines[3].style.color, Some(Ink::Palette(CYAN)));
        let (removed, added) = (&d.lines[4], &d.lines[5]);
        assert_eq!(removed.style_at(0).color, Some(Ink::Palette(RED)));
        assert_eq!(added.style_at(0).color, Some(Ink::Palette(GREEN)));
        assert!(removed.style.dim && !added.style.dim);
        // `fn` is highlighted as Rust, in some colour other than the sign's.
        assert!(
            added
                .style_at(1)
                .color
                .is_some_and(|c| c != Ink::Palette(GREEN))
        );
        // A file of no known language: the whole line in its colour.
        let plain = d.lines.last().unwrap();
        assert_eq!(plain.style.color, Some(Ink::Palette(GREEN)));
    }
}
