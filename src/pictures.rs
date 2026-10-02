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
                    let Ok(bytes) = std::fs::read(&p.path) else {
                        continue;
                    };
                    let placed = herdr::set_picture(
                        &pane,
                        &layer(set),
                        &base64(&bytes),
                        p.size,
                        p.at,
                        p.cells,
                    );
                    if placed.is_ok() {
                        set += 1;
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
