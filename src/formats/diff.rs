use crate::doc::{Chapter, Document, Kind, Line};

/// A diff is kept exactly as written. The only structure taken from it is
/// where one file's changes begin; that header line is drawn as a heading.
pub fn load(src: &str) -> Document {
    let mut doc = Document::default();
    for (i, l) in src.lines().enumerate() {
        let mut kind = Kind::Pre;
        if let Some(rest) = l.strip_prefix("diff --git ") {
            doc.chapters.push(Chapter {
                title: file_name(rest),
                level: 1,
                line: i,
            });
            kind = Kind::Heading;
        }
        doc.lines.push(Line::new(l, kind));
    }
    doc
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
        let ch: Vec<_> = d.chapters.iter().map(|c| (c.title.as_str(), c.line)).collect();
        assert_eq!(ch, [("x.rs", 0), ("y z.rs", 2)]);
    }
}
