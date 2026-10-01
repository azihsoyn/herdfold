//! Markdown, rendered: headings lose their `#` and gain weight and a rule,
//! emphasis and code are styled, lists get markers and hanging indents,
//! quotes and code blocks get a bar down their left side, tables are set in
//! columns. Headings are the chapters.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use unicode_width::UnicodeWidthStr;

use crate::doc::{Chapter, Document, Kind, Line, Run, Style};

pub fn load(src: &str) -> Document {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut r = Renderer::default();
    for event in Parser::new_ext(src, options) {
        r.event(event);
    }
    r.finish()
}

const BULLETS: [&str; 3] = ["• ", "◦ ", "▪ "];

#[derive(Default)]
struct Renderer {
    doc: Document,
    /// The line being built, its length in characters, and its styled runs.
    text: String,
    len: usize,
    runs: Vec<Run>,
    heading: bool,
    base: Style,
    hang: Option<usize>,
    /// Inline styles currently open (emphasis, links, ...).
    inline: Vec<Style>,
    quotes: usize,
    /// Next number of each open list; `None` for bullets.
    lists: Vec<Option<u64>>,
    /// Indent of each open list item's content.
    items: Vec<usize>,
    /// Text of the code block being read.
    code: Option<String>,
    table: Option<Table>,
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: String,
}

fn style(f: impl FnOnce(&mut Style)) -> Style {
    let mut s = Style::default();
    f(&mut s);
    s
}

impl Renderer {
    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => match &mut self.code {
                Some(code) => code.push_str(&t),
                None => self.push(&t),
            },
            Event::Code(t) => {
                self.inline.push(style(|s| s.code = true));
                self.push(&t);
                self.inline.pop();
            }
            Event::InlineMath(t) | Event::DisplayMath(t) => self.push(&t),
            Event::SoftBreak => self.push(" "),
            Event::HardBreak => self.line_break(),
            Event::Rule => {
                self.flush();
                self.rule('─', style(|s| s.dim = true));
                self.blank();
            }
            Event::Html(t) => {
                if !t.trim_start().starts_with("<!--") {
                    for l in t.lines() {
                        self.raw(l, style(|s| s.dim = true));
                    }
                }
            }
            Event::InlineHtml(t) => {
                if t.starts_with("<br") {
                    self.line_break();
                }
            }
            Event::FootnoteReference(t) => self.push(&format!("[^{t}]")),
            Event::TaskListMarker(done) => {
                // Replace the bullet just written with a box.
                for _ in 0..2 {
                    self.text.pop();
                }
                self.len -= 2;
                self.push(if done { "☑ " } else { "☐ " });
            }
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                if let Some(&indent) = self.items.last()
                    && self.text.is_empty()
                {
                    self.push(&" ".repeat(indent));
                    self.hang = Some(indent);
                }
            }
            Tag::Heading { level, .. } => {
                self.blank();
                self.heading = true;
                self.base = style(|s| {
                    s.bold = true;
                    s.accent = level <= HeadingLevel::H3;
                });
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.quotes += 1;
            }
            Tag::CodeBlock(_) => {
                self.flush();
                self.code = Some(String::new());
            }
            Tag::List(start) => {
                self.flush();
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let depth = self.lists.len().max(1);
                let indent = "  ".repeat(depth - 1);
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}. ", *n - 1)
                    }
                    _ => BULLETS[(depth - 1) % BULLETS.len()].to_string(),
                };
                let hang = indent.width() + marker.width();
                self.push(&(indent + &marker));
                self.hang = Some(hang);
                self.items.push(hang);
            }
            Tag::Table(_) => {
                self.flush();
                self.table = Some(Table::default());
            }
            Tag::Emphasis => self.inline.push(style(|s| s.italic = true)),
            Tag::Strong => self.inline.push(style(|s| s.bold = true)),
            Tag::Strikethrough => self.inline.push(style(|s| s.strike = true)),
            Tag::Link { .. } => self.inline.push(style(|s| s.underline = true)),
            Tag::Image { .. } => {
                self.inline.push(style(|s| s.dim = true));
                self.push("[image: ");
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Heading(level) => {
                let title = self.text.trim().to_string();
                if !title.is_empty() {
                    self.doc.chapters.push(Chapter {
                        title,
                        level: level as u8,
                        line: self.doc.lines.len(),
                    });
                }
                self.flush();
                let accent = style(|s| s.accent = true);
                match level {
                    HeadingLevel::H1 => self.rule('━', accent),
                    HeadingLevel::H2 => self.rule('─', accent),
                    _ => {}
                }
                self.blank();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                while self.doc.lines.last().is_some_and(|l| l.text.is_empty()) {
                    self.doc.lines.pop();
                }
                self.quotes -= 1;
                self.blank();
            }
            TagEnd::CodeBlock => {
                let code = self.code.take().unwrap_or_default();
                let indent = " ".repeat(self.items.last().copied().unwrap_or(0));
                let gutter = format!("{indent}{}▏ ", self.quote_bar());
                for l in code.lines() {
                    let mut line = Line::new(l, Kind::Pre);
                    line.gutter = gutter.clone();
                    self.doc.lines.push(line);
                }
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Item => {
                self.flush();
                self.items.pop();
            }
            TagEnd::TableCell => {
                if let Some(t) = &mut self.table {
                    let cell = std::mem::take(&mut t.cell);
                    t.row.push(cell.trim().to_string());
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(t) = &mut self.table {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.set_table(t);
                }
                self.blank();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                self.inline.pop();
            }
            TagEnd::Image => {
                self.push("]");
                self.inline.pop();
            }
            _ => {}
        }
    }

    fn quote_bar(&self) -> String {
        "┃ ".repeat(self.quotes)
    }

    /// Adds text to the line being built, in the open inline styles.
    fn push(&mut self, s: &str) {
        if let Some(t) = &mut self.table {
            t.cell.push_str(s);
            return;
        }
        let s = s.replace('\t', "    ");
        let style = self.inline.iter().fold(Style::default(), |a, &b| a.with(b));
        let start = self.len;
        self.text.push_str(&s);
        self.len += s.chars().count();
        if style == Style::default() || start == self.len {
            return;
        }
        match self.runs.last_mut() {
            Some(r) if r.end == start && r.style == style => r.end = self.len,
            _ => self.runs.push(Run {
                start,
                end: self.len,
                style,
            }),
        }
    }

    /// Ends the line being built, if any.
    fn flush(&mut self) {
        if self.text.is_empty() {
            return;
        }
        let kind = if self.heading {
            Kind::Heading
        } else {
            Kind::Body
        };
        let mut line = Line::new(std::mem::take(&mut self.text), kind);
        line.style = std::mem::take(&mut self.base);
        line.runs = std::mem::take(&mut self.runs);
        line.hang = self.hang.take();
        line.gutter = self.quote_bar();
        self.doc.lines.push(line);
        self.len = 0;
        self.heading = false;
    }

    /// A hard break: a new line in the same block, keeping its indent.
    fn line_break(&mut self) {
        let (heading, base, hang) = (self.heading, self.base, self.hang);
        self.flush();
        (self.heading, self.base, self.hang) = (heading, base, hang);
        if let Some(&indent) = self.items.last() {
            self.push(&" ".repeat(indent));
        }
    }

    /// One blank line, unless there is nothing above or the last line
    /// already is one.
    fn blank(&mut self) {
        self.flush();
        if self.doc.lines.last().is_some_and(|l| !l.text.is_empty()) {
            let mut line = Line::new("", Kind::Body);
            line.gutter = self.quote_bar().trim_end().to_string();
            self.doc.lines.push(line);
        }
    }

    fn rule(&mut self, c: char, style: Style) {
        let mut line = Line::new(c.to_string(), Kind::Rule);
        line.style = style;
        line.gutter = self.quote_bar();
        self.doc.lines.push(line);
    }

    fn raw(&mut self, text: &str, style: Style) {
        let mut line = Line::new(text, Kind::Pre);
        line.style = style;
        line.gutter = self.quote_bar();
        self.doc.lines.push(line);
    }

    /// Sets a table in columns, the head row bold over a rule.
    fn set_table(&mut self, t: Table) {
        let cols = t.rows.iter().map(Vec::len).max().unwrap_or(0);
        let widths: Vec<usize> = (0..cols)
            .map(|c| {
                t.rows
                    .iter()
                    .filter_map(|r| r.get(c))
                    .map(|s| s.width())
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        for (i, row) in t.rows.iter().enumerate() {
            let cells: Vec<String> = (0..cols)
                .map(|c| {
                    let s = row.get(c).map(String::as_str).unwrap_or("");
                    format!("{s}{}", " ".repeat(widths[c] - s.width()))
                })
                .collect();
            let head = i == 0;
            self.raw(cells.join(" │ ").trim_end(), style(|s| s.bold = head));
            if head {
                let rule: Vec<String> = widths.iter().map(|&w| "─".repeat(w)).collect();
                self.raw(&rule.join("─┼─"), style(|s| s.dim = true));
            }
        }
    }

    fn finish(mut self) -> Document {
        self.flush();
        while self.doc.lines.last().is_some_and(|l| l.text.is_empty()) {
            self.doc.lines.pop();
        }
        self.doc.title = self
            .doc
            .chapters
            .iter()
            .find(|c| c.level == 1)
            .map(|c| c.title.clone())
            .unwrap_or_default();
        self.doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(src: &str) -> Vec<String> {
        load(src).lines.into_iter().map(|l| l.text).collect()
    }

    #[test]
    fn headings_lose_their_marks_and_gain_rules() {
        let d = load("# Top\n\ntext\n\n## Sub ##\n\n### Small\n");
        let lines: Vec<_> = d.lines.iter().map(|l| (l.text.as_str(), l.kind)).collect();
        assert_eq!(
            lines,
            [
                ("Top", Kind::Heading),
                ("━", Kind::Rule),
                ("", Kind::Body),
                ("text", Kind::Body),
                ("", Kind::Body),
                ("Sub", Kind::Heading),
                ("─", Kind::Rule),
                ("", Kind::Body),
                ("Small", Kind::Heading),
            ]
        );
        let ch: Vec<_> = d
            .chapters
            .iter()
            .map(|c| (c.level, c.title.as_str(), c.line))
            .collect();
        assert_eq!(ch, [(1, "Top", 0), (2, "Sub", 5), (3, "Small", 8)]);
        assert_eq!(d.title, "Top");
    }

    #[test]
    fn paragraph_lines_are_joined() {
        assert_eq!(texts("one\ntwo\n\nthree"), ["one two", "", "three"]);
    }

    #[test]
    fn inline_styles_become_runs() {
        let d = load("a **b** *c* `d` [e](http://x)");
        let l = &d.lines[0];
        assert_eq!(l.text, "a b c d e");
        let at = |i| l.style_at(i);
        assert!(at(2).bold && at(4).italic && at(6).code && at(8).underline);
        assert_eq!(at(0), Style::default());
    }

    #[test]
    fn lists_get_markers_and_hang_after_them() {
        let d = load("- one\n- two\n  - deep\n\n1. first\n2. second\n\n- [x] done\n");
        let lines: Vec<_> = d.lines.iter().map(|l| (l.text.as_str(), l.hang)).collect();
        assert_eq!(
            lines,
            [
                ("• one", Some(2)),
                ("• two", Some(2)),
                ("  ◦ deep", Some(4)),
                ("", None),
                ("1. first", Some(3)),
                ("2. second", Some(3)),
                ("", None),
                ("☑ done", Some(2)),
            ]
        );
    }

    #[test]
    fn code_blocks_keep_their_lines_behind_a_bar() {
        let d = load("```rust\nfn x() {\n    1\n}\n```\n# Real\n");
        let code: Vec<_> = d
            .lines
            .iter()
            .filter(|l| l.kind == Kind::Pre)
            .map(|l| (l.gutter.as_str(), l.text.as_str()))
            .collect();
        assert_eq!(code, [("▏ ", "fn x() {"), ("▏ ", "    1"), ("▏ ", "}")]);
        assert_eq!(d.chapters.len(), 1);
    }

    #[test]
    fn quotes_carry_a_bar() {
        let d = load("> said\n> so\n\nafter");
        let lines: Vec<_> = d
            .lines
            .iter()
            .map(|l| (l.gutter.as_str(), l.text.as_str()))
            .collect();
        assert_eq!(lines, [("┃ ", "said so"), ("", ""), ("", "after")]);
    }

    #[test]
    fn tables_are_set_in_columns() {
        assert_eq!(
            texts("| a | bb |\n|---|---|\n| ccc | d |\n"),
            ["a   │ bb", "────┼───", "ccc │ d"]
        );
    }
}
