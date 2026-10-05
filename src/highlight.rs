//! Code highlighted by its language, as bat highlights it: the grammars bat
//! bundles, and bat's `ansi` theme, whose colours are the terminal's own
//! palette, so highlighted code suits a light terminal as well as a dark one.

use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::doc::{Run, Style};

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        two_face::theme::extra()
            .get(two_face::theme::EmbeddedThemeName::Ansi)
            .clone()
    })
}

/// The grammar for `lang`, a code block's language (`rust`, `py`, `ts`...)
/// or a file name's extension.
fn syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let lang = lang.trim().split([' ', ',', '{']).next()?.to_lowercase();
    if lang.is_empty() {
        return None;
    }
    let set = syntaxes();
    set.find_syntax_by_token(&lang)
        .or_else(|| set.find_syntax_by_extension(&lang))
        .filter(|s| s.name != "Plain Text")
}

/// Whether there is a grammar for `lang`.
pub fn knows(lang: &str) -> bool {
    syntax(lang).is_some()
}

/// A highlighter that reads lines one after another, carrying what it has
/// read (an open string, a block comment) from each line to the next.
pub struct Highlighter {
    lines: HighlightLines<'static>,
}

impl Highlighter {
    pub fn new(lang: &str) -> Option<Self> {
        Some(Self {
            lines: HighlightLines::new(syntax(lang)?, theme()),
        })
    }

    /// The styled stretches of `line`, in characters.
    pub fn line(&mut self, line: &str) -> Vec<Run> {
        let with_newline = format!("{line}\n");
        let Ok(parts) = self.lines.highlight_line(&with_newline, syntaxes()) else {
            return Vec::new();
        };
        let mut runs = Vec::new();
        let mut at = 0;
        for (style, text) in parts {
            let text = text.strip_suffix('\n').unwrap_or(text);
            let len = text.chars().count();
            let s = Style {
                color: palette(style.foreground),
                bold: style.font_style.contains(FontStyle::BOLD),
                italic: style.font_style.contains(FontStyle::ITALIC),
                underline: style.font_style.contains(FontStyle::UNDERLINE),
                ..Style::default()
            };
            if len > 0 && s != Style::default() {
                runs.push(Run {
                    start: at,
                    end: at + len,
                    style: s,
                });
            }
            at += len;
        }
        runs
    }
}

/// The palette colour bat's `ansi` theme means by `c`: alpha 0 names a
/// palette entry in the red channel; alpha 1, the terminal's own text colour.
fn palette(c: syntect::highlighting::Color) -> Option<u8> {
    (c.a == 0).then_some(c.r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_coloured_from_the_palette() {
        let mut h = Highlighter::new("rust").unwrap();
        let runs = h.line("fn main() { let s = \"hi\"; }");
        assert!(!runs.is_empty());
        assert!(runs.iter().all(|r| r.style.color.is_none_or(|c| c < 16)));
        // `fn` and the string are coloured differently.
        let at = |i: usize| {
            runs.iter()
                .find(|r| r.start <= i && i < r.end)
                .and_then(|r| r.style.color)
        };
        assert!(at(0).is_some() && at(21).is_some() && at(0) != at(21));
    }

    #[test]
    fn languages_are_found_by_name_or_extension() {
        assert!(knows("rust") && knows("rs") && knows("py") && knows("toml") && knows("ts"));
        assert!(knows("rust ignore"));
        assert!(!knows("") && !knows("no-such-language"));
    }
}
