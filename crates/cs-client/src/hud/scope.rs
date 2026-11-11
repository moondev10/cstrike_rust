//! `CHudSniperScope` (docs/re/client-hud.md §5.11): the hardware scope overlay, one
//! `sprites/scope_arc` texture mirrored into the four quadrants, black cross lines and borders.

use super::{Hud, HudEngine};

const BLACK: [u8; 4] = [0, 0, 0, 255];

/// Drawn while not observing (or in-eye) and zoomed to 44° or less.
pub(super) fn draw(hud: &Hud, eng: &mut dyn HudEngine) {
    if !matches!(hud.state.user1, 0 | 4) || hud.fov > 44 {
        return;
    }
    let arc = eng.load_image("sprites/scope_arc");
    let screen = eng.screen();
    let (w, h) = (screen.width as i32, screen.height as i32);
    let (y1, y2) = (h / 16, h - h / 16);
    let x1 = (w - h) / 2 + h / 16;
    let x2 = w - x1;
    let (cx, cy) = (w / 2, h / 2);
    if let Some(arc) = arc {
        // The texture is the lower-right quarter; 1/512 inset on every edge.
        let (lo, hi) = (0.001_953_125, 0.998_046_9);
        screen.draw_image_uv(arc, x1, y1, cx - x1, cy - y1, [hi, hi, lo, lo], BLACK);
        screen.draw_image_uv(arc, cx, y1, x2 - cx, cy - y1, [lo, hi, hi, lo], BLACK);
        screen.draw_image_uv(arc, x1, cy, cx - x1, y2 - cy, [hi, lo, lo, hi], BLACK);
        screen.draw_image_uv(arc, cx, cy, x2 - cx, y2 - cy, [lo, lo, hi, hi], BLACK);
    }
    let fill = |screen: &mut gs_draw::Screen, x0: i32, y0: i32, x1: i32, y1: i32| screen.fill_rgba_blend(x0, y0, x1 - x0, y1 - y0, 0, 0, 0, 255);
    // GetProportionalScaledValue(10).
    let opening = 10 * h / 480;
    fill(screen, cx, 0, cx + 1, cy - opening);
    fill(screen, cx, cy + opening, cx + 1, h);
    fill(screen, 0, cy + 1, cx - opening, cy + 2);
    fill(screen, cx + opening, cy + 1, w, cy + 2);
    fill(screen, 0, 0, w, y1 + 1);
    fill(screen, 0, y2 - 1, w, h);
    fill(screen, 0, y1, x1 + 1, h);
    fill(screen, x2 - 1, y1, w, h);
}
