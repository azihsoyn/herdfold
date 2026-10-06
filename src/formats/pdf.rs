//! PDF, read for its text (`--format pdf-text`). A PDF is words put at
//! places on fixed pages; here the words are taken out and set again as
//! the book's own pages: lines joined into paragraphs (across the PDF's
//! pages too), words hyphenated at a line's end made whole, the running
//! heads and page numbers every page repeats left out. The chapters are
//! the PDF's outline, if it has one. Its own layout (columns, figures,
//! tables, formulas) is not kept.

use anyhow::{Context, Result};
use unicode_width::UnicodeWidthChar;

use crate::doc::{Chapter, Document, Kind, Line, Style};

pub fn load(bytes: &[u8]) -> Result<Document> {
    let pages = pdf_extract::extract_text_from_mem_by_pages(bytes)
        .context("reading the PDF's text (an encrypted or scanned PDF has none to read)")?;
    let pdf = lopdf::Document::load_mem(bytes).ok();
    let outline: Vec<(usize, String, usize)> = pdf
        .as_ref()
        .and_then(|d| d.get_toc().ok())
        .map(|t| {
            t.toc
                .into_iter()
                .map(|e| (e.level, e.title, e.page))
                .collect()
        })
        .unwrap_or_default();
    let title = pdf.as_ref().and_then(info_title).unwrap_or_default();
    Ok(set(&pages, &outline, title))
}

/// The title the PDF's information dictionary gives, if any.
fn info_title(pdf: &lopdf::Document) -> Option<String> {
    let info = pdf.trailer.get(b"Info").ok()?;
    let info = match info {
        lopdf::Object::Reference(id) => pdf.get_dictionary(*id).ok()?,
        lopdf::Object::Dictionary(d) => d,
        _ => return None,
    };
    let title = lopdf::decode_text_string(info.get(b"Title").ok()?).ok()?;
    let title = title.trim().to_string();
    (!title.is_empty()).then_some(title)
}

/// The book from the text of each page and the outline (level from 1,
/// title, page from 1).
fn set(pages: &[String], outline: &[(usize, String, usize)], title: String) -> Document {
    let pages: Vec<Vec<String>> = pages.iter().map(|p| lines_of(p)).collect();
    let pages = without_running_lines(pages);
    let mut doc = Document {
        title,
        ..Default::default()
    };
    // The line each PDF page's text begins in.
    let mut page_starts = Vec::with_capacity(pages.len());
    let mut para = String::new();
    let flush = |para: &mut String, doc: &mut Document| {
        if !para.is_empty() {
            doc.lines.push(Line::new(std::mem::take(para), Kind::Body));
            doc.lines.push(Line::new("", Kind::Body));
        }
    };
    for (n, page) in pages.iter().enumerate() {
        // A paragraph runs on to the next page unless it has ended there,
        // or the outline has a heading open the page.
        let ended = para
            .chars()
            .last()
            .is_some_and(|c| ".!?:。！？」』）)\"”".contains(c));
        let heading = outline.iter().any(|(_, _, p)| *p == n + 1);
        if ended || heading {
            flush(&mut para, &mut doc);
        }
        page_starts.push(doc.lines.len());
        for line in page {
            if line.is_empty() {
                flush(&mut para, &mut doc);
            } else {
                join(&mut para, line);
            }
        }
    }
    flush(&mut para, &mut doc);
    while doc.lines.last().is_some_and(|l| l.text.is_empty()) {
        doc.lines.pop();
    }
    for (level, title, page) in outline {
        let Some(p) = page.checked_sub(1).filter(|&p| p < page_starts.len()) else {
            continue;
        };
        let title = title.trim();
        let (from, to) = (
            page_starts[p],
            page_starts.get(p + 1).copied().unwrap_or(doc.lines.len()),
        );
        // The heading itself, on its page ("2 Background" for "Background"),
        // else the page's start.
        let found = (from..to.max(from + 1).min(doc.lines.len()))
            .find(|&i| is_heading(&doc.lines[i].text, title));
        let line = found.unwrap_or(from).min(doc.lines.len().saturating_sub(1));
        if found.is_some() {
            let l = &mut doc.lines[line];
            l.kind = Kind::Heading;
            l.style = Style {
                bold: true,
                accent: *level <= 2,
                ..Style::default()
            };
        }
        doc.chapters.push(Chapter {
            title: title.to_string(),
            level: (*level).clamp(1, 6) as u8,
            line,
        });
    }
    // In reading order, as the outline may not be.
    doc.chapters.sort_by_key(|c| c.line);
    doc
}

/// Whether `line` is the heading `title`: the title, perhaps after its
/// number ("3.2 Attention", "第3章 …"), and little else.
fn is_heading(line: &str, title: &str) -> bool {
    let fold = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let (line, title) = (fold(line), fold(title));
    if title.is_empty() || !line.ends_with(&title) {
        return false;
    }
    let number = &line[..line.len() - title.len()];
    number.chars().count() <= 8
        && number
            .chars()
            .all(|c| c.is_ascii_digit() || ".:-–第章節部編".contains(c) || c.is_ascii_uppercase())
}

/// `c`, with the radicals some fonts map kanji to (⽣ for 生, ⾨ for 門,
/// from the Kangxi and CJK radicals blocks) made the kanji again, so they
/// read and are found as the kanji they look like.
fn plain_kanji(c: char) -> char {
    use unicode_normalization::UnicodeNormalization;
    if matches!(c, '\u{2e80}'..='\u{2fdf}') {
        let mut n = std::iter::once(c).nfkc();
        if let (Some(k), None) = (n.next(), n.next()) {
            return k;
        }
    }
    c
}

/// A page's lines, trimmed, with control characters (form feeds and the
/// like) taken out and radicals made kanji; runs of blank lines kept as one.
fn lines_of(page: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in page.lines() {
        let l: String = l
            .chars()
            .filter(|c| !c.is_control())
            .map(plain_kanji)
            .collect();
        let l = l.trim().to_string();
        if l.is_empty() && out.last().is_none_or(String::is_empty) {
            continue;
        }
        out.push(l);
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

/// A line with its digits all alike, so "Chapter 3 · 41" and
/// "Chapter 3 · 42" count as the same running head.
fn shape(line: &str) -> String {
    line.chars()
        .map(|c| if c.is_ascii_digit() { '#' } else { c })
        .collect()
}

/// The pages without what they repeat at their head and foot: a line that
/// opens (or closes) at least two pages in five, digits aside, and page
/// numbers alone on their line.
fn without_running_lines(mut pages: Vec<Vec<String>>) -> Vec<Vec<String>> {
    let count = |pick: fn(&Vec<String>) -> Option<&String>| {
        let mut n = std::collections::HashMap::<String, usize>::new();
        for p in &pages {
            if let Some(l) = pick(p) {
                *n.entry(shape(l)).or_default() += 1;
            }
        }
        n
    };
    let heads = count(|p| p.iter().find(|l| !l.is_empty()));
    let feet = count(|p| p.iter().rev().find(|l| !l.is_empty()));
    let often =
        |n: Option<&usize>| pages.len() >= 4 && n.is_some_and(|&n| n * 5 >= pages.len() * 2);
    let number = |l: &str| {
        let l = l.trim_matches(|c: char| c == '-' || c == '–' || c.is_whitespace());
        !l.is_empty() && l.len() <= 6 && l.chars().all(|c| c.is_ascii_digit())
    };
    let (heads_often, feet_often): (Vec<String>, Vec<String>) = (
        heads
            .iter()
            .filter(|(_, n)| often(Some(n)))
            .map(|(k, _)| k.clone())
            .collect(),
        feet.iter()
            .filter(|(_, n)| often(Some(n)))
            .map(|(k, _)| k.clone())
            .collect(),
    );
    for p in &mut pages {
        if let Some(i) = p.iter().position(|l| !l.is_empty())
            && (heads_often.contains(&shape(&p[i])) || number(&p[i]))
        {
            p.remove(i);
        }
        if let Some(i) = p.iter().rposition(|l| !l.is_empty())
            && (feet_often.contains(&shape(&p[i])) || number(&p[i]))
        {
            p.remove(i);
        }
    }
    pages
}

fn wide(c: char) -> bool {
    c.width().unwrap_or(0) == 2
}

/// Adds `line` to the paragraph being built: a word broken by a hyphen at
/// the end of the line is made whole, wide (CJK) text runs on without a
/// space, and other lines are joined by one.
fn join(para: &mut String, line: &str) {
    if para.is_empty() {
        para.push_str(line);
        return;
    }
    let last = para.chars().last().unwrap_or(' ');
    let before = para.chars().rev().nth(1).unwrap_or(' ');
    let first = line.chars().next().unwrap_or(' ');
    if last == '-' && before.is_alphabetic() && first.is_lowercase() {
        para.pop();
    } else if !(wide(last) && wide(first)) {
        para.push(' ');
    }
    para.push_str(line);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(d: &Document) -> Vec<&str> {
        d.lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn lines_become_paragraphs_across_pages() {
        let pages = [
            "A first para-\ngraph runs on\n\nthe second starts here and".to_string(),
            "runs onto the next page.\n\n日本語の文が\n改行されても続く。".to_string(),
        ];
        let d = set(&pages, &[], "T".into());
        assert_eq!(
            texts(&d),
            [
                "A first paragraph runs on",
                "",
                "the second starts here and runs onto the next page.",
                "",
                "日本語の文が改行されても続く。"
            ]
        );
    }

    #[test]
    fn radicals_are_read_as_the_kanji_they_look_like() {
        let d = set(&["羅⽣⾨の下⼈".to_string()], &[], String::new());
        assert_eq!(d.lines[0].text, "羅生門の下人");
    }

    #[test]
    fn running_heads_and_page_numbers_are_left_out() {
        let pages: Vec<String> = (1..=5)
            .map(|n| format!("A Book · Chapter {n}\n\nWords on page {n}.\n\n{n}"))
            .collect();
        let d = set(&pages, &[], String::new());
        assert!(d.lines.iter().all(|l| !l.text.starts_with("A Book")));
        assert!(d.lines.iter().all(|l| l.text.parse::<u32>().is_err()));
        assert_eq!(d.lines[0].text, "Words on page 1.");
    }

    #[test]
    fn the_outline_gives_the_chapters() {
        let pages = [
            "Intro\n\nSome words.".to_string(),
            "Part Two\n\nMore words.".to_string(),
        ];
        let outline = [(1, "Intro".to_string(), 1), (1, "Part Two".to_string(), 2)];
        assert!(is_heading("2 Background", "Background"));
        assert!(is_heading(
            "3.2.1 Scaled Dot-Product Attention",
            "Scaled Dot-Product Attention"
        ));
        assert!(is_heading("第3章 序論", "序論"));
        assert!(!is_heading(
            "We give the background of it all",
            "Background"
        ));
        let d = set(&pages, &outline, String::new());
        let ch: Vec<(&str, usize)> = d
            .chapters
            .iter()
            .map(|c| (c.title.as_str(), c.line))
            .collect();
        assert_eq!(ch, [("Intro", 0), ("Part Two", 4)]);
        assert_eq!(d.lines[4].kind, Kind::Heading);
    }
}
