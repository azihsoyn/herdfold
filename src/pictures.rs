//! Pictures on the open page, set over the rows the layout left for them
//! with the Kitty graphics protocol, which herdr draws in its panes. A
//! worker thread reads and scales them, so the reader never waits on it,
//! and only the latest wish is carried out.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

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
    /// Which wish the pictures on their way are for; older ones are dropped.
    wish: u64,
    worker: Option<Worker>,
}

struct Worker {
    wishes: Sender<(u64, Vec<Placed>)>,
    ready: Receiver<(u64, Vec<u8>)>,
}

/// The size of a character cell in pixels, if pictures can be set in this
/// pane: inside herdr, which draws the Kitty graphics its panes write, and
/// only when the pane says how many pixels it has.
pub fn cell_size() -> Option<(u32, u32)> {
    std::env::var_os("HERDR_PANE_ID")?;
    let size = crossterm::terminal::window_size().ok()?;
    let (w, h) = (
        size.width as u32 / size.columns.max(1) as u32,
        size.height as u32 / size.rows.max(1) as u32,
    );
    (w > 0 && h > 0).then_some((w, h))
}

impl Pictures {
    /// For the pane this process draws in; inert where pictures cannot be
    /// set.
    pub fn new() -> Self {
        let Some(cell) = cell_size() else {
            return Self {
                shown: Vec::new(),
                wish: 0,
                worker: None,
            };
        };
        // The worker reads and scales the pictures, so the reader never
        // waits on it; what it makes is written to the terminal between
        // frames, never in the middle of one.
        let (wishes, wished) = mpsc::channel::<(u64, Vec<Placed>)>();
        let (made, ready) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(mut wanted) = wished.recv() {
                // Only the latest wish matters.
                while let Ok(newer) = wished.try_recv() {
                    wanted = newer;
                }
                let (wish, pictures) = wanted;
                let mut out = Vec::new();
                for (k, p) in pictures.iter().enumerate() {
                    if let Some((png, _)) = prepare(p, cell) {
                        out.extend(kitty_place(k as u32 + 1, &png, p.at, p.cells));
                    }
                }
                if made.send((wish, out)).is_err() {
                    break;
                }
            }
        });
        // Sweep what a reader that did not close cleanly left behind.
        write(KITTY_CLEAR);
        Self {
            shown: Vec::new(),
            wish: 0,
            worker: Some(Worker { wishes, ready }),
        }
    }

    /// Shows the pictures of `page` drawn in `area`, or none. Called after
    /// each frame, it also sets the pictures the worker has made ready.
    pub fn show(&mut self, area: Rect, page: Option<&PageView>) {
        let Some(worker) = &self.worker else {
            return;
        };
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
        if wanted != self.shown {
            // The page's pictures go as soon as it does.
            write(KITTY_CLEAR);
            self.wish += 1;
            if !wanted.is_empty() {
                let _ = worker.wishes.send((self.wish, wanted.clone()));
            }
            self.shown = wanted;
        }
        while let Ok((wish, out)) = worker.ready.try_recv() {
            if wish == self.wish && !out.is_empty() {
                write(&out);
            }
        }
    }
}

/// Takes every picture this pane set out, and the data kept for them.
const KITTY_CLEAR: &[u8] = b"\x1b_Ga=d,d=A,q=2\x1b\\";

/// The Kitty graphics that set `png` (base64) as picture `id` over `cells`
/// from (`col`, `row`): the cursor taken there and back, so the frame drawn
/// around it is left as it was, and the data sent in pieces of the size
/// the protocol takes.
fn kitty_place(id: u32, png: &str, (col, row): (u16, u16), (cols, rows): (u16, u16)) -> Vec<u8> {
    const PIECE: usize = 4096;
    let mut out = format!("\x1b7\x1b[{};{}H", row + 1, col + 1).into_bytes();
    let pieces: Vec<&[u8]> = png.as_bytes().chunks(PIECE).collect();
    for (k, piece) in pieces.iter().enumerate() {
        let more = u8::from(k + 1 < pieces.len());
        if k == 0 {
            out.extend(
                format!("\x1b_Ga=T,f=100,t=d,i={id},c={cols},r={rows},C=1,q=2,m={more};")
                    .as_bytes(),
            );
        } else {
            out.extend(format!("\x1b_Gm={more};").as_bytes());
        }
        out.extend_from_slice(piece);
        out.extend_from_slice(b"\x1b\\");
    }
    out.extend_from_slice(b"\x1b8");
    out
}

/// Writes `bytes` to the terminal at once.
fn write(bytes: &[u8]) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(bytes).and_then(|()| out.flush());
}

/// A picture ready to be set, as a PNG: its data in base64 and its size in
/// pixels. A PNG that already fits
/// its cells goes as it is; anything else is decoded, scaled down to the
/// cells, and encoded as a PNG.
fn prepare(p: &Placed, (cw, ch): (u32, u32)) -> Option<(String, (u32, u32))> {
    let bytes = std::fs::read(&p.path).ok()?;
    let fits = (p.cells.0 as u32 * cw, p.cells.1 as u32 * ch);
    let png = bytes.starts_with(b"\x89PNG\r\n\x1a\n");
    if png && p.size.0 <= fits.0 && p.size.1 <= fits.1 {
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
    /// Takes the pictures out before the reader goes.
    fn drop(&mut self) {
        if self.worker.take().is_some() {
            write(KITTY_CLEAR);
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
        let (data, size) = prepare(&placed(path.clone(), (20, 10), (4, 1)), (10, 20)).unwrap();
        assert_eq!(size, (20, 10));
        assert_eq!(data, base64(&std::fs::read(&path).unwrap()));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn other_pictures_are_scaled_to_their_cells_and_sent_as_png() {
        let path = file("big.jpg", image::ImageFormat::Jpeg, 800, 400);
        // 10 x 4 cells of 10 x 20 pixels show at most 100 x 80.
        let p = placed(path.clone(), (800, 400), (10, 4));
        let (data, size) = prepare(&p, (10, 20)).unwrap();
        assert_eq!(size, (100, 50));
        assert!(data.starts_with(&base64(b"\x89PNG")[..4]));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_picture_is_set_in_pieces_with_the_cursor_put_back() {
        let png = "A".repeat(5000);
        let out = String::from_utf8(kitty_place(2, &png, (3, 1), (10, 4))).unwrap();
        assert!(out.starts_with("\x1b7\x1b[2;4H\x1b_Ga=T,f=100,t=d,i=2,c=10,r=4,C=1,q=2,m=1;AAAA"));
        assert!(out.contains(&format!("A\x1b\\\x1b_Gm=0;{}\x1b\\", "A".repeat(904))));
        assert!(out.ends_with("\x1b\\\x1b8"));
    }
}
