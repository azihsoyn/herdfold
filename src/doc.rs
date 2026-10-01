use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a line is broken into rows. The book never looks at what a line means;
/// the input format says which of these it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Prose: broken at spaces (or between wide characters), hanging indent kept.
    Body,
    /// Same breaking as `Body`, drawn bold.
    Heading,
    /// Preformatted: whitespace kept, broken hard at the edge.
    Pre,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub kind: Kind,
}

impl Line {
    pub fn new(text: impl Into<String>, kind: Kind) -> Self {
        Self {
            text: expand_tabs(&text.into()),
            kind,
        }
    }
}

/// A division the input gave us. The book never finds these on its own.
#[derive(Clone, Debug, PartialEq)]
pub struct Chapter {
    pub title: String,
    pub level: u8,
    pub line: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Document {
    pub title: String,
    pub lines: Vec<Line>,
    pub chapters: Vec<Chapter>,
}

fn expand_tabs(s: &str) -> String {
    if !s.contains('\t') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 8);
    let mut col = 0;
    for c in s.chars() {
        if c == '\t' {
            let n = 4 - col % 4;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}
