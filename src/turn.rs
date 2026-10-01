//! Turning a page, drawn. The free edge of the turning sheet (`┃`) crosses
//! the pane, the next page appearing behind it. Turning forward the edge
//! runs right to left, first over the right page and then over the left;
//! turning back it runs the other way. Each pane times its own part from
//! the moment it learns of the turn, so no frames cross the socket.

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

use crate::view::{self, PageView, Side};

/// How long the edge takes to cross one pane.
const CROSSING: Duration = Duration::from_millis(160);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Turn {
    Forward,
    Backward,
}

/// A turn under way in one pane: what was showing, and since when.
pub struct Turning {
    turn: Turn,
    from: Option<PageView>,
    started: Instant,
    /// 0 if this pane is crossed first, 1 if second.
    order: u32,
}

impl Turning {
    pub fn new(turn: Turn, from: Option<PageView>, side: Side) -> Self {
        let order = match (side, turn) {
            (Side::Single, _) | (Side::Right, Turn::Forward) | (Side::Left, Turn::Backward) => 0,
            _ => 1,
        };
        Self {
            turn,
            from,
            started: Instant::now(),
            order,
        }
    }

    /// How far the edge has crossed this pane, 0 to 1; `None` once it is over.
    fn progress(&self) -> Option<f32> {
        let elapsed =
            self.started.elapsed().as_secs_f32() - self.order as f32 * CROSSING.as_secs_f32();
        let t = elapsed / CROSSING.as_secs_f32();
        if t >= 1.0 {
            return None;
        }
        let t = t.clamp(0.0, 1.0);
        // Ease in and out: the sheet lifts slowly, swings, and settles.
        Some(t * t * (3.0 - 2.0 * t))
    }

    pub fn done(&self) -> bool {
        self.progress().is_none()
    }

    /// Draws the pane mid-turn, from the page that was showing to `to`.
    /// Returns false (drawing nothing) once the turn is over.
    pub fn render(&self, buf: &mut Buffer, area: Rect, to: Option<&PageView>) -> bool {
        let Some(p) = self.progress() else {
            return false;
        };
        let mut from_buf = Buffer::empty(area);
        let mut to_buf = Buffer::empty(area);
        view::render(&mut from_buf, area, self.from.as_ref());
        view::render(&mut to_buf, area, to);

        let crossed = (p * area.width as f32).round() as u16;
        // Columns on the far side of the edge show the next page.
        let (edge, shows_next): (u16, Box<dyn Fn(u16) -> bool>) = match self.turn {
            Turn::Forward => {
                let edge = area.right().saturating_sub(crossed);
                (edge, Box::new(move |x| x > edge))
            }
            Turn::Backward => {
                let edge = area.x + crossed;
                (edge, Box::new(move |x| x < edge))
            }
        };
        // The sheet in motion is drawn faint: the old page lifting away while
        // this pane is crossed first, the new one landing when second.
        let faint_next = self.order == 1;
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let next = shows_next(x);
                let mut cell = if next {
                    to_buf[(x, y)].clone()
                } else {
                    from_buf[(x, y)].clone()
                };
                // A wide character cut by the edge would spill across it.
                if cell.symbol().width() > 1 && x + 1 < area.right() && shows_next(x + 1) != next {
                    cell.set_symbol(" ");
                }
                if next == faint_next && x != edge {
                    cell.modifier.insert(Modifier::DIM);
                }
                if x == edge && crossed > 0 && crossed < area.width {
                    cell.reset();
                    cell.set_symbol("┃");
                }
                buf[(x, y)] = cell;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_crosses_the_right_page_first() {
        assert_eq!(Turning::new(Turn::Forward, None, Side::Right).order, 0);
        assert_eq!(Turning::new(Turn::Forward, None, Side::Left).order, 1);
        assert_eq!(Turning::new(Turn::Backward, None, Side::Left).order, 0);
        assert_eq!(Turning::new(Turn::Backward, None, Side::Right).order, 1);
        assert_eq!(Turning::new(Turn::Backward, None, Side::Single).order, 0);
    }

    #[test]
    fn a_turn_ends() {
        let mut t = Turning::new(Turn::Forward, None, Side::Left);
        assert!(!t.done());
        t.started -= CROSSING * 2;
        assert!(t.done());
    }
}
