use crate::doc::{Chapter, Document, Kind, Line};

/// Markdown is shown as written. Headings become chapters (and are drawn
/// bold); fenced code is kept unbroken by words.
pub fn load(src: &str) -> Document {
    let mut doc = Document::default();
    let mut fence: Option<&str> = None;
    for l in src.lines() {
        let trimmed = l.trim_start();
        if let Some(f) = fence {
            if trimmed.starts_with(f) {
                fence = None;
            }
            doc.lines.push(Line::new(l, Kind::Pre));
            continue;
        }
        if let Some(f) = ["```", "~~~"].into_iter().find(|f| trimmed.starts_with(f)) {
            fence = Some(f);
            doc.lines.push(Line::new(l, Kind::Pre));
            continue;
        }
        if let Some((level, title)) = atx(l) {
            push_heading(&mut doc, level, title);
            doc.lines.push(Line::new(l, Kind::Heading));
            continue;
        }
        if let Some(level) = setext(l)
            && let Some(prev) = doc.lines.last_mut()
            && prev.kind == Kind::Body
            && !prev.text.trim().is_empty()
        {
            prev.kind = Kind::Heading;
            let title = prev.text.trim().to_string();
            let line = doc.lines.len() - 1;
            doc.chapters.push(Chapter { title, level, line });
            doc.lines.push(Line::new(l, Kind::Heading));
            continue;
        }
        if let Some(prev) = doc.lines.last_mut()
            && continues(&prev.text, prev.kind, l)
        {
            prev.text.push(' ');
            prev.text.push_str(l.trim_start());
            continue;
        }
        doc.lines.push(Line::new(l, Kind::Body));
    }
    doc.title = doc
        .chapters
        .iter()
        .find(|c| c.level == 1)
        .map(|c| c.title.clone())
        .unwrap_or_default();
    doc
}

/// Markdown joins the lines of a paragraph (a soft line break), so a file
/// wrapped at 80 columns can be set to the page's own measure. A line
/// continues the one before unless either is blank, the earlier one ends in
/// a hard break, or the later one opens a block of its own.
fn continues(prev: &str, kind: Kind, line: &str) -> bool {
    if kind != Kind::Body || prev.trim().is_empty() || line.trim().is_empty() {
        return false;
    }
    if prev.ends_with("  ") || prev.ends_with('\\') || prev.trim_start().starts_with('|') {
        return false;
    }
    let t = line.trim_start();
    let ordered = t
        .split_once(['.', ')'])
        .is_some_and(|(n, rest)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && rest.starts_with(' '));
    let opens_block = ["- ", "* ", "+ ", "> ", "|", "<", "#"].iter().any(|m| t.starts_with(m))
        || ordered
        || setext(line).is_some();
    !opens_block
}

fn push_heading(doc: &mut Document, level: u8, title: &str) {
    doc.chapters.push(Chapter {
        title: title.to_string(),
        level,
        line: doc.lines.len(),
    });
}

fn atx(l: &str) -> Option<(u8, &str)> {
    if l.starts_with("    ") {
        return None;
    }
    let t = l.trim_start();
    let level = t.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &t[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let title = rest.trim().trim_end_matches('#').trim_end();
    (!title.is_empty()).then_some((level as u8, title))
}

fn setext(l: &str) -> Option<u8> {
    let t = l.trim();
    if t.len() >= 2 && t.chars().all(|c| c == '=') {
        Some(1)
    } else if t.len() >= 2 && t.chars().all(|c| c == '-') {
        Some(2)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chapters(src: &str) -> Vec<(u8, String, usize)> {
        load(src)
            .chapters
            .into_iter()
            .map(|c| (c.level, c.title, c.line))
            .collect()
    }

    #[test]
    fn atx_headings_are_chapters() {
        assert_eq!(
            chapters("# Top\ntext\n## Sub ##\n#nope\n"),
            [(1, "Top".into(), 0), (2, "Sub".into(), 2)]
        );
    }

    #[test]
    fn headings_inside_fences_are_not() {
        assert_eq!(chapters("```\n# comment\n```\n# Real\n"), [(1, "Real".into(), 3)]);
    }

    #[test]
    fn setext_headings_are_chapters() {
        assert_eq!(
            chapters("Title\n=====\n\nPart\n----\n"),
            [(1, "Title".into(), 0), (2, "Part".into(), 3)]
        );
    }

    #[test]
    fn a_rule_after_a_blank_line_is_not_a_heading() {
        assert!(chapters("text\n\n---\n").is_empty());
    }

    fn texts(src: &str) -> Vec<String> {
        load(src).lines.into_iter().map(|l| l.text).collect()
    }

    #[test]
    fn paragraph_lines_are_joined() {
        assert_eq!(texts("one\ntwo\n\nthree"), ["one two", "", "three"]);
    }

    #[test]
    fn list_items_and_hard_breaks_end_a_line() {
        assert_eq!(
            texts("- a\n  more\n- b\nend  \nnext\n1. x\n> q"),
            ["- a more", "- b end  ", "next", "1. x", "> q"]
        );
    }

    #[test]
    fn code_is_not_joined() {
        assert_eq!(texts("text\n```\na\nb\n```"), ["text", "```", "a", "b", "```"]);
    }

    #[test]
    fn title_is_the_first_top_heading() {
        assert_eq!(load("## a\n# b\n").title, "b");
    }
}
