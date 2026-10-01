//! A small, forgiving XML tree, enough to read an EPUB's container, package,
//! table of contents and XHTML. Malformed input yields whatever was parsed
//! before the error rather than nothing.

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

#[derive(Debug, Default)]
pub struct Element {
    /// Local name, lowercased, without a namespace prefix.
    pub name: String,
    /// Attributes as written (`epub:type`, `href`, ...).
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

#[derive(Debug)]
pub enum Node {
    Element(Element),
    Text(String),
}

impl Element {
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key || k.rsplit(':').next() == Some(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    /// Every element below this one, depth first, in document order.
    pub fn descendants(&self) -> Vec<&Element> {
        let mut out = Vec::new();
        fn walk<'a>(e: &'a Element, out: &mut Vec<&'a Element>) {
            for c in e.elements() {
                out.push(c);
                walk(c, out);
            }
        }
        walk(self, &mut out);
        out
    }

    pub fn find(&self, name: &str) -> Option<&Element> {
        self.descendants().into_iter().find(|e| e.name == name)
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        fn walk(e: &Element, s: &mut String) {
            for c in &e.children {
                match c {
                    Node::Text(t) => s.push_str(t),
                    Node::Element(e) => walk(e, s),
                }
            }
        }
        walk(self, &mut s);
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

const VOID: &[&str] = &["br", "img", "hr", "meta", "link", "input", "col", "wbr"];

pub fn parse(src: &str) -> Element {
    let mut reader = Reader::from_str(src);
    let config = reader.config_mut();
    config.trim_text(false);
    config.check_end_names = false;
    config.allow_unmatched_ends = true;

    let mut stack = vec![Element::default()];
    loop {
        let event = match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(e) => e,
        };
        match event {
            Event::Start(s) => {
                let e = element(&s);
                if VOID.contains(&e.name.as_str()) {
                    append(&mut stack, Node::Element(e));
                } else {
                    stack.push(e);
                }
            }
            Event::Empty(s) => append(&mut stack, Node::Element(element(&s))),
            Event::End(end) => {
                let name = local(end.local_name().as_ref());
                if let Some(depth) = stack.iter().rposition(|e| e.name == name)
                    && depth > 0
                {
                    while stack.len() > depth {
                        let e = stack.pop().unwrap();
                        append(&mut stack, Node::Element(e));
                    }
                }
            }
            Event::Text(t) => push_text(&mut stack, &t.decode().unwrap_or_default()),
            Event::CData(t) => push_text(&mut stack, &t.decode().unwrap_or_default()),
            Event::GeneralRef(r) => {
                let name = r.decode().unwrap_or_default();
                let c = match r.resolve_char_ref() {
                    Ok(Some(c)) => Some(c.to_string()),
                    _ => entity(&name).map(str::to_string),
                };
                push_text(&mut stack, &c.unwrap_or_else(|| format!("&{name};")));
            }
            _ => {}
        }
    }
    while stack.len() > 1 {
        let e = stack.pop().unwrap();
        append(&mut stack, Node::Element(e));
    }
    stack.pop().unwrap()
}

fn local(name: &[u8]) -> String {
    String::from_utf8_lossy(name).to_ascii_lowercase()
}

fn element(s: &BytesStart) -> Element {
    let attrs = s
        .attributes()
        .with_checks(false)
        .flatten()
        .map(|a| {
            let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
            let value = match a.normalized_value(XmlVersion::default()) {
                Ok(v) => v.into_owned(),
                Err(_) => String::from_utf8_lossy(&a.value).into_owned(),
            };
            (key, value)
        })
        .collect();
    Element {
        name: local(s.local_name().as_ref()),
        attrs,
        children: Vec::new(),
    }
}

fn append(stack: &mut [Element], node: Node) {
    stack.last_mut().unwrap().children.push(node);
}

fn push_text(stack: &mut [Element], text: &str) {
    let children = &mut stack.last_mut().unwrap().children;
    if let Some(Node::Text(t)) = children.last_mut() {
        t.push_str(text);
    } else {
        children.push(Node::Text(text.to_string()));
    }
}

fn entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "mdash" => "—",
        "ndash" => "–",
        "hellip" => "…",
        "lsquo" => "‘",
        "rsquo" => "’",
        "ldquo" => "“",
        "rdquo" => "”",
        "laquo" => "«",
        "raquo" => "»",
        "middot" => "·",
        "bull" => "•",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "deg" => "°",
        "times" => "×",
        "shy" => "",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_elements_and_entities() {
        let root = parse(r#"<a x="1"><b>hi &amp; &nbsp;there&#33;</b><br></a>"#);
        let a = root.find("a").unwrap();
        assert_eq!(a.attr("x"), Some("1"));
        assert_eq!(a.find("b").unwrap().text(), "hi & there!");
        assert!(a.find("br").is_some());
    }

    #[test]
    fn survives_mismatched_tags() {
        let root = parse("<a><b>one</a><c>two</c>");
        assert_eq!(root.find("b").unwrap().text(), "one");
        assert_eq!(root.find("c").unwrap().text(), "two");
    }

    #[test]
    fn namespaced_names_are_local() {
        let root = parse(r#"<dc:title>T</dc:title><nav epub:type="toc"/>"#);
        assert_eq!(root.find("title").unwrap().text(), "T");
        assert_eq!(root.find("nav").unwrap().attr("type"), Some("toc"));
    }
}
