//! Code highlighted by its language, as bat highlights it: the grammars and
//! themes bat bundles. The theme is `ansi` unless one is named under
//! `[highlight]` in config.toml (or with `--theme`); `ansi` takes its colours
//! from the terminal's own palette, so it suits a light terminal as well as
//! a dark one.

use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::doc::{Ink, Run, Style};

/// The theme used unless another is named.
pub const DEFAULT_THEME: &str = "ansi";

/// The theme named for this run (`--theme`), over the one in config.toml.
static ASKED: OnceLock<String> = OnceLock::new();

/// Names the theme for this run.
pub fn use_theme(name: &str) {
    let _ = ASKED.set(name.to_string());
}

/// The names of the themes there are.
pub fn theme_names() -> Vec<&'static str> {
    two_face::theme::EmbeddedLazyThemeSet::theme_names()
        .iter()
        .map(|t| t.as_name())
        .collect()
}

/// The theme called `name` (case aside), if there is one.
fn find_theme(name: &str) -> Option<two_face::theme::EmbeddedThemeName> {
    two_face::theme::EmbeddedLazyThemeSet::theme_names()
        .iter()
        .copied()
        .find(|t| t.as_name().eq_ignore_ascii_case(name.trim()))
}

/// The theme named in config.toml's `[highlight]` table, if any.
pub fn configured_theme() -> Option<String> {
    let text = std::fs::read_to_string(crate::keys::config_path()?).ok()?;
    let table: toml::Table = text.parse().ok()?;
    Some(table.get("highlight")?.get("theme")?.as_str()?.to_string())
}

/// Whether `name` is a theme there is.
pub fn is_theme(name: &str) -> bool {
    find_theme(name).is_some()
}

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let name = ASKED
            .get()
            .cloned()
            .or_else(configured_theme)
            .and_then(|n| find_theme(&n))
            .unwrap_or(two_face::theme::EmbeddedThemeName::Ansi);
        two_face::theme::extra().get(name).clone()
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

/// The colour a theme means by `c`. bat's palette themes say alpha 0 for
/// a palette entry, named in the red channel, and alpha 1 for the
/// terminal's own text colour; any other is an exact colour.
fn palette(c: syntect::highlighting::Color) -> Option<Ink> {
    match c.a {
        0 => Some(Ink::Palette(c.r)),
        1 => None,
        _ => Some(Ink::Rgb([c.r, c.g, c.b])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_coloured_from_the_palette() {
        let mut h = Highlighter::new("rust").unwrap();
        let runs = h.line("fn main() { let s = \"hi\"; }");
        assert!(!runs.is_empty());
        assert!(runs.iter().all(|r| {
            r.style
                .color
                .is_none_or(|c| matches!(c, Ink::Palette(0..16)))
        }));
        // `fn` and the string are coloured differently.
        let at = |i: usize| {
            runs.iter()
                .find(|r| r.start <= i && i < r.end)
                .and_then(|r| r.style.color)
        };
        assert!(at(0).is_some() && at(21).is_some() && at(0) != at(21));
    }

    #[test]
    fn themes_are_found_by_name() {
        assert!(is_theme("ansi") && is_theme("Monokai Extended") && is_theme("nord"));
        assert!(!is_theme("no such theme"));
        assert!(theme_names().contains(&"OneHalfDark"));
    }

    #[test]
    fn languages_are_found_by_name_or_extension() {
        assert!(knows("rust") && knows("rs") && knows("py") && knows("toml") && knows("ts"));
        assert!(knows("rust ignore"));
        assert!(!knows("") && !knows("no-such-language"));
    }
}
