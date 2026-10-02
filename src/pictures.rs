//! Pictures on the open page, set over the rows the layout left for them
//! through herdr's graphics API. herdr draws a terminal's pane itself, so
//! the terminal's own image protocols do not reach the screen from inside
//! it; herdr's API does. A worker thread does the setting, so the reader
//! never waits on it, and only the latest wish is carried out.

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread;

use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

use crate::herdr;
use crate::view::{self, PageView};

/// A picture on a page: on rows `row..row + rows` (counted as the page's
/// rows are), from column `col` of the text column, `cols` wide.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PagePicture {
    pub row: usize,
    pub col: usize,
    pub cols: u16,
    pub rows: u16,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

/// A picture placed in a pane, in the pane's cells.
#[derive(Clone, Debug, PartialEq)]
struct Placed {
    path: PathBuf,
    size: (u32, u32),
    at: (u16, u16),
    cells: (u16, u16),
}

pub struct Pictures {
    shown: Vec<Placed>,
    worker: Option<(Sender<Vec<Placed>>, thread::JoinHandle<()>)>,
}

/// How many picture layers herdr keeps for a pane.
const LAYERS: usize = 16;

/// The layer the `k`th picture of a page is set on.
fn layer(k: usize) -> String {
    format!("herdfold-{k}")
}

impl Pictures {
    /// For the pane this process draws in; inert outside herdr.
    pub fn new() -> Self {
        let Some(pane) = std::env::var("HERDR_PANE_ID").ok() else {
            return Self {
                shown: Vec::new(),
                worker: None,
            };
        };
        let (tx, rx) = mpsc::channel::<Vec<Placed>>();
        let worker = thread::spawn(move || {
            // The cell's size, to send no more pixels than the cells show.
            let cell = herdr::cell_size(&pane).unwrap_or((10, 20));
            // Layers set and not yet taken out, each cleared by name: a
            // clear without one does not reach them. The first clear sweeps
            // every layer herdr allows, for what a reader that did not close
            // cleanly left behind.
            let mut set = LAYERS;
            let clear = |set: &mut usize| {
                for k in 0..*set {
                    herdr::clear_picture(&pane, &layer(k));
                }
                *set = 0;
            };
            while let Ok(mut wanted) = rx.recv() {
                // Only the latest wish matters.
                while let Ok(newer) = rx.try_recv() {
                    wanted = newer;
                }
                clear(&mut set);
                for p in &wanted {
                    // Too large for herdr: try again at half the size.
                    for shrink in 0..4 {
                        let Some((data, size)) = prepare(p, cell, shrink) else {
                            break;
                        };
                        match herdr::set_picture(
                            &pane,
                            &layer(set),
                            "png",
                            &data,
                            size,
                            p.at,
                            p.cells,
                        ) {
                            Ok(()) => {
                                set += 1;
                                break;
                            }
                            Err(e) if e.contains("too large") => continue,
                            Err(_) => break,
                        }
                    }
                }
            }
            // The reader is closing: leave nothing behind.
            clear(&mut set);
        });
        Self {
            shown: Vec::new(),
            worker: Some((tx, worker)),
        }
    }

    /// Shows the pictures of `page` drawn in `area`, or none.
    pub fn show(&mut self, area: Rect, page: Option<&PageView>) {
        let wanted: Vec<Placed> = page
            .map(|v| {
                let (x, _) = view::column(area, v.width);
                v.pictures
                    .iter()
                    .map(|p| Placed {
                        path: p.path.clone(),
                        size: (p.width, p.height),
                        at: (x + p.col as u16, view::TOP + p.row as u16),
                        cells: (p.cols, p.rows),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if wanted == self.shown {
            return;
        }
        if let Some((tx, _)) = &self.worker {
            let _ = tx.send(wanted.clone());
        }
        self.shown = wanted;
    }
}

/// A picture ready for herdr, as a PNG: its data in base64 and its size in
/// pixels. A PNG that already fits its cells goes as it is; anything else
/// is decoded, scaled down to the cells (and halved `shrink` times more),
/// and encoded as a PNG. herdr refuses large raw pixel data, and takes the
/// same picture compressed.
fn prepare(p: &Placed, (cw, ch): (u32, u32), shrink: u32) -> Option<(String, (u32, u32))> {
    let bytes = std::fs::read(&p.path).ok()?;
    let fits = (
        (p.cells.0 as u32 * cw) >> shrink,
        (p.cells.1 as u32 * ch) >> shrink,
    );
    let png = bytes.starts_with(b"\x89PNG\r\n\x1a\n");
    if png && shrink == 0 && p.size.0 <= fits.0 && p.size.1 <= fits.1 {
        return Some((base64(&bytes), p.size));
    }
    let picture = image::load_from_memory(&bytes).ok()?;
    let picture = if picture.width() > fits.0 || picture.height() > fits.1 {
        picture.resize(
            fits.0.max(1),
            fits.1.max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        picture
    };
    let mut out = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some((base64(out.get_ref()), (picture.width(), picture.height())))
}

/// Where pictures taken out of books are kept:
/// `$XDG_CACHE_HOME/herdfold/pictures` (default `~/.cache/herdfold/pictures`).
pub fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join(crate::NAME).join("pictures"))
}

impl Drop for Pictures {
    /// Waits for the worker to take the pictures out before the reader goes.
    fn drop(&mut self) {
        if let Some((tx, worker)) = self.worker.take() {
            drop(tx);
            let _ = worker.join();
        }
    }
}

pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, format: image::ImageFormat, w: u32, h: u32) -> PathBuf {
        let path = std::env::temp_dir().join(format!("herdfold-{}-{name}", std::process::id()));
        image::RgbImage::new(w, h)
            .save_with_format(&path, format)
            .unwrap();
        path
    }

    fn placed(path: PathBuf, size: (u32, u32), cells: (u16, u16)) -> Placed {
        Placed {
            path,
            size,
            at: (0, 0),
            cells,
        }
    }

    #[test]
    fn a_small_png_goes_as_it_is() {
        let path = file("small.png", image::ImageFormat::Png, 20, 10);
        let (data, size) = prepare(&placed(path.clone(), (20, 10), (4, 1)), (10, 20), 0).unwrap();
        assert_eq!(size, (20, 10));
        assert_eq!(data, base64(&std::fs::read(&path).unwrap()));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn other_pictures_are_scaled_to_their_cells_and_sent_as_png() {
        let path = file("big.jpg", image::ImageFormat::Jpeg, 800, 400);
        // 10 x 4 cells of 10 x 20 pixels show at most 100 x 80.
        let p = placed(path.clone(), (800, 400), (10, 4));
        let (data, size) = prepare(&p, (10, 20), 0).unwrap();
        assert_eq!(size, (100, 50));
        assert!(data.starts_with(&base64(b"\x89PNG")[..4]));
        // Asked to shrink, it halves again.
        assert_eq!(prepare(&p, (10, 20), 1).unwrap().1, (50, 25));
        let _ = std::fs::remove_file(path);
    }
}
