//! EPUB: the spine gives the reading order, the book's own table of contents
//! (EPUB 3 nav or EPUB 2 NCX) gives the chapters. XHTML is reduced to plain
//! lines; only headings and preformatted text keep a kind of their own.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use anyhow::{Context, Result};
use zip::ZipArchive;

use super::xml::{self, Element, Node};
use crate::doc::{Chapter, Document, Kind, Line};

type Zip = ZipArchive<Cursor<Vec<u8>>>;

struct Item {
    href: String,
    media: String,
    props: String,
}

pub fn load(bytes: Vec<u8>) -> Result<Document> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).context("not an EPUB (not a zip archive)")?;
    let container = xml::parse(&read(&mut zip, "META-INF/container.xml")?);
    let opf_path = container
        .find("rootfile")
        .and_then(|e| e.attr("full-path"))
        .context("META-INF/container.xml names no package file")?
        .to_string();
    let opf = xml::parse(&read(&mut zip, &opf_path)?);
    let base = dir_of(&opf_path);

    let items: HashMap<&str, Item> = opf
        .find("manifest")
        .map(|m| {
            m.elements()
                .filter(|e| e.name == "item")
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            let item = Item {
                href: resolve(base, e.attr("href")?),
                media: e.attr("media-type").unwrap_or("").to_string(),
                props: e.attr("properties").unwrap_or("").to_string(),
            };
            Some((e.attr("id")?, item))
        })
        .collect();
    let spine = opf.find("spine").context("package has no spine")?;

    let mut doc = Document {
        title: opf.find("title").map(|e| e.text()).unwrap_or_default(),
        ..Default::default()
    };
    let mut starts: HashMap<String, usize> = HashMap::new();
    let mut anchors: HashMap<(String, String), usize> = HashMap::new();
    let mut headings = Vec::new();
    for itemref in spine.elements().filter(|e| e.name == "itemref") {
        let Some(item) = itemref.attr("idref").and_then(|id| items.get(id)) else {
            continue;
        };
        let Ok(src) = read(&mut zip, &item.href) else {
            continue;
        };
        let page = xml::parse(&src);
        starts.insert(item.href.clone(), doc.lines.len());
        let mut html = Html::new(&mut doc.lines);
        html.node(page.find("body").unwrap_or(&page));
        html.flush(false);
        html.blank();
        for (id, line) in html.ids {
            anchors.insert((item.href.clone(), id), line);
        }
        headings.extend(html.headings);
    }

    let toc = items
        .values()
        .find(|i| i.props.split_whitespace().any(|p| p == "nav"))
        .and_then(|i| Some((i, read(&mut zip, &i.href).ok()?)))
        .map(|(i, src)| nav_entries(&xml::parse(&src), dir_of(&i.href)))
        .filter(|t| !t.is_empty())
        .or_else(|| {
            let ncx = spine.attr("toc").and_then(|id| items.get(id)).or_else(|| {
                items
                    .values()
                    .find(|i| i.media == "application/x-dtbncx+xml")
            })?;
            let src = read(&mut zip, &ncx.href).ok()?;
            Some(ncx_entries(&xml::parse(&src), dir_of(&ncx.href)))
        })
        .unwrap_or_default();

    for (level, title, href) in toc {
        let (file, frag) = match href.split_once('#') {
            Some((f, id)) => (f.to_string(), Some(id.to_string())),
            None => (href.clone(), None),
        };
        let line = frag
            .and_then(|id| anchors.get(&(file.clone(), id)).copied())
            .or_else(|| starts.get(&file).copied());
        if let Some(line) = line {
            doc.chapters.push(Chapter { title, level, line });
        }
    }
    if doc.chapters.is_empty() {
        doc.chapters = headings;
    }
    while doc.lines.last().is_some_and(|l| l.text.is_empty()) {
        doc.lines.pop();
    }
    Ok(doc)
}

fn read(zip: &mut Zip, name: &str) -> Result<String> {
    let mut f = zip
        .by_name(name)
        .with_context(|| format!("{name} is missing from the EPUB"))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// Resolves `href` against the directory `base`, as a path inside the zip.
fn resolve(base: &str, href: &str) -> String {
    let href = percent_decode(href);
    let mut parts: Vec<&str> = if href.starts_with('/') {
        Vec::new()
    } else {
        base.split('/').filter(|p| !p.is_empty()).collect()
    };
    for p in href.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

type Entry = (u8, String, String);

fn nav_entries(root: &Element, dir: &str) -> Vec<Entry> {
    let navs: Vec<&Element> = root
        .descendants()
        .into_iter()
        .filter(|e| e.name == "nav")
        .collect();
    let nav = navs
        .iter()
        .find(|n| {
            n.attr("type")
                .is_some_and(|t| t.split_whitespace().any(|t| t == "toc"))
        })
        .or(navs.first());
    let mut out = Vec::new();
    if let Some(ol) = nav.and_then(|n| n.find("ol")) {
        nav_list(ol, 1, dir, &mut out);
    }
    out
}

fn nav_list(ol: &Element, level: u8, dir: &str, out: &mut Vec<Entry>) {
    for li in ol.elements().filter(|e| e.name == "li") {
        if let Some(a) = li.elements().find(|e| e.name == "a")
            && let Some(href) = a.attr("href")
        {
            out.push((level, a.text(), resolve(dir, href)));
        }
        if let Some(sub) = li.elements().find(|e| e.name == "ol") {
            nav_list(sub, level + 1, dir, out);
        }
    }
}

fn ncx_entries(root: &Element, dir: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    if let Some(map) = root.find("navmap") {
        ncx_points(map, 1, dir, &mut out);
    }
    out
}

fn ncx_points(parent: &Element, level: u8, dir: &str, out: &mut Vec<Entry>) {
    for p in parent.elements().filter(|e| e.name == "navpoint") {
        let label = p
            .elements()
            .find(|e| e.name == "navlabel")
            .map(|e| e.text());
        let src = p
            .elements()
            .find(|e| e.name == "content")
            .and_then(|e| e.attr("src"));
        if let (Some(label), Some(src)) = (label, src) {
            out.push((level, label, resolve(dir, src)));
        }
        ncx_points(p, level + 1, dir, out);
    }
}

const BLOCK: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "dd",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "tr",
    "ul",
];
/// Blocks followed by a blank line.
const PARAGRAPH: &[&str] = &[
    "blockquote",
    "dl",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "ol",
    "p",
    "pre",
    "table",
    "ul",
];

/// Flattens XHTML into lines.
struct Html<'a> {
    lines: &'a mut Vec<Line>,
    cur: String,
    kind: Kind,
    pre: usize,
    /// Item number of each open list; `None` for bullets.
    lists: Vec<Option<usize>>,
    ids: Vec<(String, usize)>,
    headings: Vec<Chapter>,
}

impl<'a> Html<'a> {
    fn new(lines: &'a mut Vec<Line>) -> Self {
        Self {
            lines,
            cur: String::new(),
            kind: Kind::Body,
            pre: 0,
            lists: Vec::new(),
            ids: Vec::new(),
            headings: Vec::new(),
        }
    }

    fn node(&mut self, e: &Element) {
        if let Some(id) = e.attr("id") {
            self.ids.push((id.to_string(), self.lines.len()));
        }
        let name = e.name.as_str();
        match name {
            "head" | "script" | "style" | "title" => return,
            "br" => return self.flush(true),
            "hr" => {
                self.flush(false);
                return self.blank();
            }
            "img" | "image" => {
                let alt = e.attr("alt").unwrap_or("").trim();
                let s = if alt.is_empty() {
                    "[image]".to_string()
                } else {
                    format!("[image: {alt}]")
                };
                return self.text(&s);
            }
            "td" | "th" => self.text(" "),
            _ => {}
        }
        let block = BLOCK.contains(&name);
        if block {
            self.flush(false);
        }
        let level = match name.as_bytes() {
            [b'h', d @ b'1'..=b'6'] => Some(d - b'0'),
            _ => None,
        };
        if let Some(level) = level {
            self.kind = Kind::Heading;
            let title = e.text();
            if !title.is_empty() {
                let line = self.lines.len();
                self.headings.push(Chapter { title, level, line });
            }
        }
        match name {
            "pre" => self.pre += 1,
            "ul" => self.lists.push(None),
            "ol" => self.lists.push(Some(0)),
            "li" => {
                let depth = self.lists.len().saturating_sub(1);
                let mark = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{n}. ")
                    }
                    _ => "• ".to_string(),
                };
                self.cur = "  ".repeat(depth) + &mark;
            }
            _ => {}
        }

        for c in &e.children {
            match c {
                Node::Text(t) => self.text(t),
                Node::Element(c) => self.node(c),
            }
        }

        match name {
            "pre" => self.pre -= 1,
            "ul" | "ol" => {
                self.lists.pop();
            }
            _ => {}
        }
        if block {
            self.flush(false);
            if PARAGRAPH.contains(&name) && self.lists.is_empty() {
                self.blank();
            }
        }
    }

    fn text(&mut self, t: &str) {
        if self.pre > 0 {
            for (i, part) in t.split('\n').enumerate() {
                if i > 0 {
                    self.flush(true);
                }
                self.cur.push_str(&part.replace('\u{a0}', " "));
            }
            return;
        }
        for c in t.chars() {
            if c == '\u{a0}' {
                self.cur.push(' ');
            } else if c.is_whitespace() {
                if !self.cur.is_empty() && !self.cur.ends_with(' ') {
                    self.cur.push(' ');
                }
            } else {
                self.cur.push(c);
            }
        }
    }

    fn flush(&mut self, force: bool) {
        let s = self.cur.trim_end();
        let only_marker = !self.cur.is_empty() && s.trim().is_empty();
        if (!s.is_empty() && !only_marker) || force {
            let kind = if self.pre > 0 { Kind::Pre } else { self.kind };
            self.lines.push(Line::new(s, kind));
        }
        self.cur.clear();
        self.kind = Kind::Body;
    }

    fn blank(&mut self) {
        if self.lines.last().is_some_and(|l| !l.text.is_empty()) {
            self.lines.push(Line::new("", Kind::Body));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flatten(src: &str) -> Vec<String> {
        let mut lines = Vec::new();
        let root = xml::parse(src);
        let mut h = Html::new(&mut lines);
        h.node(&root);
        h.flush(false);
        lines.into_iter().map(|l| l.text).collect()
    }

    #[test]
    fn paragraphs_are_lines_with_blank_lines_between() {
        assert_eq!(
            flatten("<body><p>one\n  two</p><p>three <em>four</em></p></body>"),
            ["one two", "", "three four", ""]
        );
    }

    #[test]
    fn preformatted_text_keeps_its_lines() {
        assert_eq!(flatten("<pre>a\n  b</pre>"), ["a", "  b", ""]);
    }

    #[test]
    fn lists_get_markers() {
        assert_eq!(
            flatten("<ol><li>x</li><li>y<ul><li>z</li></ul></li></ol>"),
            ["1. x", "2. y", "  • z", ""]
        );
    }

    #[test]
    fn resolves_relative_paths() {
        assert_eq!(
            resolve("OEBPS/text", "../img/a%20b.png"),
            "OEBPS/img/a b.png"
        );
        assert_eq!(resolve("", "ch1.xhtml"), "ch1.xhtml");
    }
}
