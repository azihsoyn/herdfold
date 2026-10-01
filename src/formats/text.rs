use crate::doc::{Document, Kind, Line};

pub fn load(src: &str) -> Document {
    Document {
        lines: src.lines().map(|l| Line::new(l, Kind::Body)).collect(),
        ..Default::default()
    }
}
