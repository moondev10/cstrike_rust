//! Video modes and the game window (docs/re/engine-video.md §2, §4, §5.1): the windowed flag and
//! the resolution come from the registry and the command line, are matched against display 0's
//! modes and written back; the window has that size and is centred on the display.

use winit::monitor::{MonitorHandle, VideoModeHandle};
use winit::window::WindowAttributes;

use crate::{cmdline, registry};

/// `vmode_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub width: i32,
    pub height: i32,
    pub bpp: i32,
}

/// SDL display 0: the desktop mode (also its bounds) and the mode sizes in SDL's enumeration
/// order (width, height, bits per pixel, refresh rate, each descending).
pub struct Display {
    pub desktop: (i32, i32),
    sizes: Vec<(i32, i32)>,
}

impl Display {
    pub fn new(monitor: Option<&MonitorHandle>) -> Self {
        let Some(m) = monitor else {
            // SDL without a display: unreachable for a windowed game.
            return Self { desktop: (640, 480), sizes: Vec::new() };
        };
        let mut modes: Vec<_> = m.video_modes().collect();
        modes.sort_by(sdl_order);
        let size = m.size();
        Self { desktop: (size.width as i32, size.height as i32), sizes: modes.iter().map(|v| (v.size().width as i32, v.size().height as i32)).collect() }
    }
}

/// SDL2's display mode order: width, height, bits per pixel, refresh rate, each descending.
fn sdl_order(a: &VideoModeHandle, b: &VideoModeHandle) -> std::cmp::Ordering {
    let key = |v: &VideoModeHandle| (v.size().width, v.size().height, v.bit_depth(), v.refresh_rate_millihertz());
    key(b).cmp(&key(a))
}

/// `SDL_GetClosestDisplayMode` (the -nofbo exclusive switch, §5.3.2): the smallest mode at
/// least `size`; among its depths the first (deepest), among its refresh rates (highest first)
/// the last one not below the target (`-freq`, else the desktop's).
pub fn closest_display_mode(monitor: &MonitorHandle, size: (u32, u32), freq: Option<i32>) -> Option<VideoModeHandle> {
    let mut modes: Vec<_> = monitor.video_modes().collect();
    modes.sort_by(sdl_order);
    let target = match freq.filter(|&f| f > 0) {
        Some(f) => f as u32 * 1000,
        None => monitor.refresh_rate_millihertz().unwrap_or(0),
    };
    let mut best: Option<VideoModeHandle> = None;
    for m in modes {
        let s = m.size();
        if s.width < size.0 {
            break;
        }
        if s.height < size.1 {
            if s.width == size.0 {
                break;
            }
            continue;
        }
        let replace = match &best {
            None => true,
            Some(b) if s.width < b.size().width || s.height < b.size().height => true,
            Some(b) if m.bit_depth() != b.bit_depth() => false,
            Some(b) => m.refresh_rate_millihertz() != b.refresh_rate_millihertz() && m.refresh_rate_millihertz() >= target,
        };
        if replace {
            best = Some(m);
        }
    }
    best
}

/// `CVideoMode_OpenGL`: the mode list, the current mode and the windowed flag, fixed for the
/// engine run.
pub struct Video {
    modes: Vec<Mode>,
    current: usize,
    pub windowed: bool,
    fallback: Mode,
}

impl Video {
    /// `VideoMode_Create` and `CVideoMode_Common::Init`; `persist` writes the resolved values
    /// back to the registry (the windowless test harness keeps them out).
    pub fn init(display: &Display, persist: bool) -> Self {
        let mut windowed = registry::read_int("ScreenWindowed", 0) != 0;
        if ["-sw", "-startwindowed", "-windowed", "-window"].into_iter().any(cmdline::check_parm) {
            windowed = true;
        } else if cmdline::check_parm("-full") || cmdline::check_parm("-fullscreen") {
            windowed = false;
        }
        if persist {
            registry::write_int("ScreenWindowed", i32::from(windowed));
        }
        let mut w = registry::read_int("DisplayWidth", display.desktop.0);
        let mut h = registry::read_int("DisplayHeight", display.desktop.1);
        // -w overrides -width, -h overrides -height (CRT atoi of the next argument).
        for (parm, height) in [("-width", false), ("-w", false), ("-height", true), ("-h", true)] {
            if let Some(v) = cmdline::parm_value(parm) {
                *(if height { &mut h } else { &mut w }) = crt_atoi(&v);
            }
        }
        let force = cmdline::check_parm("-forceres");
        let nofbo = cmdline::check_parm("-nofbo");
        let video = Self::select(display, windowed, (w, h), windowed, force, nofbo);
        if persist {
            let m = video.current_mode();
            registry::write_int("DisplayWidth", m.width);
            registry::write_int("DisplayHeight", m.height);
            registry::write_int("DisplayBPP", m.bpp);
        }
        video
    }

    /// The test harness's `--size`: windowed at exactly that size (forced into the list, not
    /// clamped to the display), registry untouched.
    pub fn harness(display: &Display, size: (u32, u32)) -> Self {
        Self::select(display, true, (size.0 as i32, size.1 as i32), false, true, false)
    }

    fn select(display: &Display, windowed: bool, (mut w, mut h): (i32, i32), clamp: bool, force: bool, nofbo: bool) -> Self {
        let fallback = Mode { width: display.desktop.0, height: display.desktop.1, bpp: 32 };
        // AddMode: the first 32 distinct sizes in SDL order, then CompareModes (ascending).
        let mut modes: Vec<Mode> = Vec::new();
        for &(width, height) in &display.sizes {
            if modes.len() >= 32 {
                break;
            }
            if !modes.iter().any(|m| m.width == width && m.height == height) {
                modes.push(Mode { width, height, bpp: 32 });
            }
        }
        modes.sort_by_key(|m| (m.width, m.height));
        if clamp {
            w = w.min(display.desktop.0);
            h = h.min(display.desktop.1);
        }
        if w > 0 && h < 0 {
            h = (f64::from(w) * 3.0 * 0.25) as i32;
        }
        // The exact size, else the last (tallest) mode of that width, else the smallest.
        let mut current = modes.iter().position(|m| m.width == w && m.height == h).or_else(|| modes.iter().rposition(|m| m.width == w)).unwrap_or(0);
        if (windowed || !nofbo) && (force || modes.len() < 2) {
            if modes.is_empty() {
                modes.push(Mode { width: w, height: h, bpp: 32 });
                current = 0;
            } else {
                modes[current].width = w;
                modes[current].height = h;
            }
        }
        Self { modes, current, windowed, fallback }
    }

    /// `GetCurrentMode`: the chosen mode, or the desktop when the list is empty.
    pub fn current_mode(&self) -> Mode {
        self.modes.get(self.current).copied().unwrap_or(self.fallback)
    }

    /// `GetVideoModes`.
    pub fn modes(&self) -> &[Mode] {
        &self.modes
    }

    /// `AdjustWindowForMode`'s window placement in windowed mode: the mode's size, centred on
    /// display 0 (`max(0, (bounds - size) / 2)`).
    pub fn windowed_rect(&self, display: &Display) -> ((u32, u32), (i32, i32)) {
        let m = self.current_mode();
        let pos = (((display.desktop.0 - m.width) / 2).max(0), ((display.desktop.1 - m.height) / 2).max(0));
        ((m.width.max(1) as u32, m.height.max(1) as u32), pos)
    }
}

/// The window's icons from the executable's icon resource 1 (`icon.rc`: the CS 1.6 icon with the
/// Rust logo), where the engine sets the mod's `game.tga` after `SDL_CreateWindow`
/// (deviations.md). The process is DPI-unaware: the caption takes 16×16, the taskbar the
/// system's default size.
pub fn with_app_icon(attrs: WindowAttributes) -> WindowAttributes {
    #[cfg(windows)]
    let attrs = {
        use winit::dpi::PhysicalSize;
        use winit::platform::windows::{IconExtWindows, WindowAttributesExtWindows};
        use winit::window::Icon;
        let load = |size| Icon::from_resource(1, size).map_err(|err| log::warn!("window icon: {err}")).ok();
        attrs.with_window_icon(load(Some(PhysicalSize::new(16, 16)))).with_taskbar_icon(load(None))
    };
    attrs
}

/// The CRT's `atoi`: leading white space, an optional sign, decimal digits (saturating).
pub(crate) fn crt_atoi(s: &str) -> i32 {
    let t = s.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let mut v: i64 = 0;
    for b in digits.bytes().take_while(u8::is_ascii_digit) {
        v = (v * 10 + i64::from(b - b'0')).min(i64::from(i32::MAX) + 1);
    }
    let v = if neg { -v } else { v };
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(sizes: &[(i32, i32)]) -> Display {
        Display { desktop: (1920, 1080), sizes: sizes.to_vec() }
    }

    /// engine-video.md §4.4: the list deduplicated and ascending; an absent height of an
    /// existing width takes the tallest of that width, an absent width the smallest mode.
    #[test]
    fn modes_match_like_the_worked_example() {
        let d = display(&[(1920, 1080), (1920, 1080), (1680, 1050), (1600, 900), (1440, 900), (1366, 768), (1280, 1024), (1280, 720), (1024, 768), (800, 600), (640, 480)]);
        let v = Video::select(&d, false, (1280, 900), false, false, false);
        assert_eq!(v.modes().len(), 10);
        assert_eq!(v.current_mode(), Mode { width: 1280, height: 1024, bpp: 32 });
        let v = Video::select(&d, false, (1300, 700), false, false, false);
        assert_eq!(v.current_mode(), Mode { width: 640, height: 480, bpp: 32 });
        let v = Video::select(&d, true, (1280, 900), true, true, false);
        assert_eq!(v.current_mode(), Mode { width: 1280, height: 900, bpp: 32 });
    }

    /// Windowed: the registry size is clamped to the display before matching.
    #[test]
    fn windowed_size_is_clamped_to_the_display() {
        let d = display(&[(1920, 1080), (1280, 720)]);
        let v = Video::select(&d, true, (2560, 1440), true, false, false);
        assert_eq!(v.current_mode(), Mode { width: 1920, height: 1080, bpp: 32 });
        assert_eq!(v.windowed_rect(&d), ((1920, 1080), (0, 0)));
    }

    #[test]
    fn crt_atoi_reads_like_the_crt() {
        assert_eq!(crt_atoi(" 800"), 800);
        assert_eq!(crt_atoi("-12x"), -12);
        assert_eq!(crt_atoi(" "), 0);
        assert_eq!(crt_atoi("99999999999"), i32::MAX);
    }
}
