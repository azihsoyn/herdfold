use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a line is broken into rows. The input format says which it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Prose: broken at spaces (or between wide characters), hanging indent kept.
    Body,
    /// Breaks like `Body`; kept off the foot of a page.
    Heading,
    /// Preformatted: whitespace kept, broken hard at the edge.
    Pre,
    /// A rule across the column, drawn with the line's one character.
    Rule,
    /// A picture, set on rows of its own; the line's text is its
    /// description, shown in its place where pictures cannot be.
    Image,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// How a stretch of text is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Style {
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub strike: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub dim: bool,
    /// Headings and the rules under them.
    #[serde(default, skip_serializing_if = "is_false")]
    pub accent: bool,
    /// Inline code.
    #[serde(default, skip_serializing_if = "is_false")]
    pub code: bool,
    /// Under a highlighter marker, of this colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker: Option<crate::marks::Ribbon>,
    /// In the selection being made.
    #[serde(default, skip_serializing_if = "is_false")]
    pub selected: bool,
    /// A match for the search.
    #[serde(default, skip_serializing_if = "is_false")]
    pub found: bool,
}

impl Style {
    /// Both styles at once.
    pub fn with(self, o: Self) -> Self {
        Self {
            bold: self.bold || o.bold,
            italic: self.italic || o.italic,
            underline: self.underline || o.underline,
            strike: self.strike || o.strike,
            dim: self.dim || o.dim,
            accent: self.accent || o.accent,
            code: self.code || o.code,
            marker: o.marker.or(self.marker),
            selected: self.selected || o.selected,
            found: self.found || o.found,
        }
    }
}

/// Text in one style, as drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Styled {
    pub text: String,
    #[serde(default)]
    pub style: Style,
}

/// `style` over the characters `start..end` of a line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub kind: Kind,
    /// Style of the whole line; `runs` add to it.
    pub style: Style,
    pub runs: Vec<Run>,
    /// Indent of wrapped rows. `None` repeats the line's own leading spaces.
    pub hang: Option<usize>,
    /// Drawn (dim) at the start of every row of the line, e.g. a quote bar.
    pub gutter: String,
    /// For a picture, the picture.
    pub image: Option<Picture>,
}

/// A picture to set in the book: an image file and its size in pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub path: std::path::PathBuf,
    pub width: u32,
    pub height: u32,
}

impl Picture {
    /// The picture at `path`, if it is one herdfold can read (PNG, JPEG,
    /// GIF, WebP, BMP). Only the header is read here, for the size; the
    /// picture itself is decoded when its page is shown.
    pub fn open(path: std::path::PathBuf) -> Option<Self> {
        let (width, height) = image::ImageReader::open(&path)
            .ok()?
            .with_guessed_format()
            .ok()?
            .into_dimensions()
            .ok()?;
        (width > 0 && height > 0).then_some(Self {
            path,
            width,
            height,
        })
    }
}

impl Line {
    pub fn new(text: impl Into<String>, kind: Kind) -> Self {
        Self {
            text: expand_tabs(&text.into()),
            kind,
            style: Style::default(),
            runs: Vec::new(),
            hang: None,
            gutter: String::new(),
            image: None,
        }
    }

    /// The style at character `i`.
    pub fn style_at(&self, i: usize) -> Style {
        self.runs
            .iter()
            .filter(|r| r.start <= i && i < r.end)
            .fold(self.style, |s, r| s.with(r.style))
    }
}

/// A division the input gave us. The book never finds these on its own.
#[derive(Clone, Debug, PartialEq)]
pub struct Chapter {
    pub title: String,
    pub level: u8,
    pub line: usize,
}

/// A link from one place in the book to another, as the input gave it: a
/// note reference to its note, a cross-reference to a heading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    pub line: usize,
    /// Byte offsets of the link's text in the line.
    pub start: usize,
    pub end: usize,
    /// The line it leads to.
    pub target: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Document {
    pub title: String,
    pub lines: Vec<Line>,
    pub chapters: Vec<Chapter>,
    /// Links inside the book, in the order they appear.
    pub links: Vec<Link>,
    /// The book says its pages run right to left (bound on the right).
    pub rtl: bool,
}

pub fn expand_tabs(s: &str) -> String {
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
