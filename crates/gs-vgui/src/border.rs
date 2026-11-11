//! Scheme borders (`vgui2::Border`, docs/re/vgui2-core.md §4.6): an inset and coloured lines per
//! side, each line one pixel wide with start/end offsets.

use crate::surface::{Color, Surface};

/// One line of a side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    pub color: Color,
    pub start: i32,
    pub end: i32,
}

/// A border. Sides are Left, Top, Right, Bottom.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Border {
    pub name: String,
    /// `l t r b`.
    pub inset: [i32; 4],
    pub sides: [Vec<Line>; 4],
}

/// The side a `Paint` call breaks (to leave room for a tab, e.g.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Break {
    pub side: i32,
    pub start: i32,
    pub end: i32,
}

impl Border {
    /// `Paint(VPANEL)`: the panel's full rectangle, no break.
    pub fn paint_panel(&self, s: &mut dyn Surface, w: i32, h: i32) {
        self.paint(s, 0, 0, w, h, None);
    }

    /// `Paint(x0, y0, x1, y1, breakSide, breakStart, breakEnd)`. The coordinate arithmetic is C
    /// `int` (a slider nob at `INT_MIN` wraps).
    pub fn paint(&self, s: &mut dyn Surface, x0: i32, y0: i32, x1: i32, y1: i32, brk: Option<Break>) {
        let brk = brk.unwrap_or(Break { side: -1, start: 0, end: 0 });
        for (i, l) in self.sides[0].iter().enumerate() {
            s.draw_set_color(l.color);
            let x = x0.wrapping_add(i as i32);
            if brk.side == 0 {
                if brk.start > 0 {
                    s.draw_filled_rect(x, y0.wrapping_add(l.start), x.wrapping_add(1), y0.wrapping_add(brk.start));
                }
                if brk.end < y1.wrapping_sub(l.end) {
                    s.draw_filled_rect(x, y0.wrapping_add(brk.end).wrapping_add(1), x.wrapping_add(1), y1.wrapping_sub(l.end));
                }
            } else {
                s.draw_filled_rect(x, y0.wrapping_add(l.start), x.wrapping_add(1), y1.wrapping_sub(l.end));
            }
        }
        for (i, l) in self.sides[1].iter().enumerate() {
            s.draw_set_color(l.color);
            let y = y0.wrapping_add(i as i32);
            if brk.side == 1 {
                if brk.start > 0 {
                    s.draw_filled_rect(x0.wrapping_add(l.start), y, x0.wrapping_add(brk.start), y.wrapping_add(1));
                }
                if brk.end < x1.wrapping_sub(l.end) {
                    s.draw_filled_rect(x0.wrapping_add(brk.end).wrapping_add(1), y, x1.wrapping_sub(l.end), y.wrapping_add(1));
                }
            } else {
                s.draw_filled_rect(x0.wrapping_add(l.start), y, x1.wrapping_sub(l.end), y.wrapping_add(1));
            }
        }
        for (i, l) in self.sides[2].iter().enumerate() {
            s.draw_set_color(l.color);
            let x = x1.wrapping_sub(i as i32);
            s.draw_filled_rect(x.wrapping_sub(1), y0.wrapping_add(l.start), x, y1.wrapping_sub(l.end));
        }
        for (i, l) in self.sides[3].iter().enumerate() {
            s.draw_set_color(l.color);
            let y = y1.wrapping_sub(i as i32);
            s.draw_filled_rect(x0.wrapping_add(l.start), y.wrapping_sub(1), x1.wrapping_sub(l.end), y);
        }
    }
}
