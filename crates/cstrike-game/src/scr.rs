//! The engine's own 2D layers drawn by `SCR_UpdateScreen` after the VGUI2 tree
//! (docs/re/engine-misc.md §1.5, §2, §8.1–§8.2): the FPS counter, the PAUSED plaque and the
//! center print.

use gs_draw::{Image, Screen};
use gs_fs::FileSystem;

/// `SCR_CenterPrint` state.
#[derive(Debug, Default)]
pub struct CenterPrint {
    /// `centerstring`: at most 1023 bytes.
    text: Vec<u8>,
    /// `center_lines` (counted over the untruncated text).
    lines: i32,
    /// `centertime_off`, counting down.
    time_off: f32,
    /// `centertime_start` (`cl.time`).
    time_start: f32,
}

impl CenterPrint {
    /// `SCR_CenterPrint(s)` (`svc_centerprint`, `svc_finale`, `svc_cutscene`,
    /// `pfnCenterPrint`).
    pub fn print(&mut self, s: &str, centertime: f32, cl_time: f64) {
        let b = s.as_bytes();
        self.text = b[..b.len().min(1023)].to_vec();
        self.time_off = centertime;
        self.time_start = cl_time as f32;
        self.lines = 1 + b.iter().filter(|&&c| c == b'\n').count() as i32;
    }

    /// `SCR_CheckDrawCenterString` where its drawing is covered anyway (the pre-VGUI pass of
    /// a finale or cutscene): only the countdown.
    pub fn countdown(&mut self, frametime: f64) {
        self.time_off = (f64::from(self.time_off) - frametime) as f32;
    }

    /// `SCR_CheckDrawCenterString`: the countdown by the host frame time, then the text while
    /// it lasts (always during an intermission) unless chat input is open.
    #[allow(clippy::too_many_arguments)]
    pub fn check_draw(&mut self, screen: &mut Screen, frametime: f64, intermission: u8, chat_open: bool, cl_time: f64, printspeed: f32, con_color: Option<[u8; 3]>) {
        self.time_off = (f64::from(self.time_off) - frametime) as f32;
        if (self.time_off > 0.0 || intermission != 0) && !chat_open {
            self.draw(screen, intermission, cl_time, printspeed, con_color);
        }
    }

    /// `SCR_DrawCenterString`: centred lines of at most 40 bytes (the rest of a longer line is
    /// dropped), drawn byte by byte; in an intermission a typewriter count of
    /// `scr_printspeed` characters per second.
    fn draw(&self, screen: &mut Screen, intermission: u8, cl_time: f64, printspeed: f32, con_color: Option<[u8; 3]>) {
        let mut remaining: i32 = if intermission != 0 { ((cl_time - f64::from(self.time_start)) * f64::from(printspeed)) as i32 } else { 9999 };
        let font = screen.font("EngineFont");
        let lh = screen.font_tall(font).max(12);
        if let Some([r, g, b]) = con_color {
            screen.set_text_color(f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0);
        }
        let (w, h) = (screen.width as i32, screen.height as i32);
        let mut y = if self.lines > 4 { 4 * lh } else { (f64::from(h) * 0.35) as i32 };
        let mut p = self.text.as_slice();
        loop {
            let n = p.iter().take(40).position(|&c| c == b'\n' || c == 0).unwrap_or(p.len().min(40));
            let line = &p[..n];
            // Measured as text (UTF-8, localized when a `#` token), drawn as sign-extended bytes.
            let x0 = (w - screen.text_width(font, &String::from_utf8_lossy(line))) / 2;
            let mut x = x0;
            for &byte in line {
                let ch = char::from_u32(u32::from(byte as i8 as i16 as u16)).unwrap_or(' ');
                x += screen.vgui2_draw_character(x, y, ch, font);
                let r = remaining;
                remaining = remaining.wrapping_sub(1);
                if r == 0 {
                    return;
                }
            }
            y += lh;
            match p.iter().position(|&c| c == b'\n' || c == 0) {
                Some(i) if p[i] == b'\n' => p = &p[i + 1..],
                _ => return,
            }
        }
    }
}

/// `SCR_Init`'s `Draw_PicFromWad("paused")`: the gfx.wad qpic, palette index 255 transparent.
pub fn load_paused(fs: &FileSystem, renderer: &mut gs_render::Renderer) -> Option<Image> {
    let wad = fs.read("gfx.wad").ok().and_then(|d| gs_formats::wad::Wad::parse(d).ok())?;
    let lump = wad.find("paused")?;
    let d = wad.lump_data(lump);
    let dim = |o: usize| d.get(o..o + 4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (w, h) = (dim(0)?, dim(4)?);
    let n = usize::try_from(w).ok()? * usize::try_from(h).ok()?;
    let pixels = d.get(8..8 + n)?;
    let pal = d.get(8 + n + 2..8 + n + 2 + 768)?;
    let rgba: Vec<u8> = pixels
        .iter()
        .flat_map(|&i| {
            let c = &pal[usize::from(i) * 3..usize::from(i) * 3 + 3];
            [c[0], c[1], c[2], if i == 255 { 0 } else { 255 }]
        })
        .collect();
    let texture = renderer.upload_overlay_image("gfx.wad/paused", w as u32, h as u32, &rgba);
    Some(Image { texture, width: w as u32, height: h as u32 })
}

/// The PAUSED plaque, centred with integer halves.
pub fn draw_paused(screen: &mut Screen, pic: Image) {
    let (w, h) = (screen.width as i32, screen.height as i32);
    let (pw, ph) = (pic.width as i32, pic.height as i32);
    screen.draw_pic(pic, w / 2 - pw / 2, h / 2 - ph / 2, pw, ph);
}

/// `SCR_DrawConnectMsg`: nothing while `scr_connectmsg` starts with `0`; else an opaque black
/// box near the bottom of the view and the three lines (`scr_connectmsg`, `…1`, `…2`; one
/// starting with `0` is skipped without taking its line) in grey console text.
pub fn draw_connect_msg(screen: &mut Screen, lines: [&str; 3]) {
    if lines[0].starts_with('0') {
        return;
    }
    let (w, h) = (screen.width as i32, screen.height as i32);
    let lh = if h > 14 { 14 } else { h - 2 };
    let widest = lines.iter().map(|l| screen.draw_console_string_len(l).0).max().unwrap_or(0);
    let tw = if w <= 270 { w - 2 } else { widest.min(w - 2) };
    let tx = 1 + (w - tw) / 2;
    let mut ty = h - 5 * lh - lh / 2;
    screen.fill_rgba_blend(tx - 5, ty - lh / 2, tw + 10, 4 * lh, 0, 0, 0, 255);
    for line in lines.into_iter().filter(|l| !l.starts_with('0')) {
        screen.set_text_color(192.0 / 255.0, 192.0 / 255.0, 192.0 / 255.0);
        screen.draw_console_string(tx, ty, line);
        ty += lh;
    }
}

/// `SCR_DrawFPS`'s running average (`cl_showfps`).
#[derive(Debug, Default)]
pub struct Fps {
    avg: f64,
}

impl Fps {
    /// White `"%d fps"` at (2, 2) in EngineFont, then the colour reverts to `con_color`.
    pub fn draw(&mut self, screen: &mut Screen, frametime: f64) {
        if frametime <= 0.0 {
            return;
        }
        self.avg = frametime * 0.4 + self.avg * 0.6;
        let fps = (1.0 / self.avg).floor() as i32;
        screen.set_text_color(1.0, 1.0, 1.0);
        screen.draw_console_string(2, 2, &format!("{fps} fps"));
    }
}

/// `con_color` as `"%i %i %i"`: only when three values parse.
pub fn con_color(s: &str) -> Option<[u8; 3]> {
    let mut it = s.split_whitespace().map(|v| v.parse::<i32>().ok());
    let mut c = [0u8; 3];
    for slot in &mut c {
        *slot = it.next().flatten()?.clamp(0, 255) as u8;
    }
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_print_counts_lines_over_the_full_text_and_keeps_1023_bytes() {
        let mut c = CenterPrint::default();
        let long = format!("{}\n\n", "x".repeat(1100));
        c.print(&long, 2.0, 5.0);
        assert_eq!((c.text.len(), c.lines, c.time_off, c.time_start), (1023, 3, 2.0, 5.0));
    }

    #[test]
    fn con_color_needs_three_numbers() {
        assert_eq!(con_color("255 180 30"), Some([255, 180, 30]));
        assert_eq!(con_color("255 180"), None);
        assert_eq!(con_color("300 -5 7"), Some([255, 0, 7]));
    }
}
