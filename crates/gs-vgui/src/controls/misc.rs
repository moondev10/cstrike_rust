//! Small controls: `ProgressBar`, `Divider`, `ImagePanel` (docs/re/vgui-controls.md §4.2, §5).

use std::any::Any;

use gs_formats::keyvalues::Block;

use crate::editable::{kv_int, kv_str};
use crate::msg::Msg;
use crate::scheme::Scheme;
use crate::surface::{Color, Surface, Texture};
use crate::ui::{Control, PanelId, Ui};

/// `vgui2::ProgressBar`: segments of `PSV(8)` with `PSV(4)` gaps, inset `PSV(4)`.
#[derive(Debug)]
pub struct ProgressBar {
    pub progress: f32,
    segment_wide: i32,
    segment_gap: i32,
    bar_inset: i32,
}

impl ProgressBar {
    /// `new ProgressBar(parent, name)`: the PSV values use the proportional state inherited now.
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str) -> PanelId {
        let me = ui.create(parent, name, Box::new(ProgressBar { progress: 0.0, segment_wide: 8, segment_gap: 4, bar_inset: 4 }));
        let (w, g, i) = (ui.psv(me, 8), ui.psv(me, 4), ui.psv(me, 4));
        if let Some(p) = ui.ctrl::<ProgressBar>(me) {
            p.segment_wide = w;
            p.segment_gap = g;
            p.bar_inset = i;
        }
        me
    }

    /// `SetProgress`: clamped to [0, 1] (NaN passes through, as in the original).
    pub fn set_progress(ui: &mut Ui, me: PanelId, f: f32) {
        if let Some(p) = ui.ctrl::<ProgressBar>(me) {
            p.progress = f.clamp(0.0, 1.0);
        }
    }

    /// `ProgressBar::ConstructTimeRemainingString`: the `#vgui_TimeLeft*` text for a transfer
    /// started at `start` with `fraction` done (last update `last`); `None` before a useful
    /// estimate exists.
    pub fn time_remaining(ui: &Ui, start: f32, now: f32, fraction: f32, last: f32, add_remaining: bool) -> Option<String> {
        if fraction <= 0.0 {
            return None;
        }
        let elapsed = last - start;
        if elapsed <= 0.0 {
            return None;
        }
        let total = elapsed / fraction;
        let remaining = total - (now - start);
        let secs = remaining.max(0.0) as i32;
        let (mins, secs) = (secs / 60, secs % 60);
        let loc = |token: &str| ui.localize.find(token).unwrap_or(token).to_owned();
        let text = if mins > 0 {
            let fmt = if add_remaining { loc("vgui_TimeLeftMinutesSeconds") } else { loc("vgui_TimeMinutesSeconds") };
            fmt.replace("%s1", &mins.to_string()).replace("%s2", &secs.to_string())
        } else {
            let fmt = if add_remaining { loc("vgui_TimeLeftSeconds") } else { loc("vgui_TimeSeconds") };
            fmt.replace("%s1", &secs.to_string())
        };
        Some(text)
    }
}

impl Control for ProgressBar {
    fn class(&self) -> &'static str {
        "ProgressBar"
    }
    fn on_message(&mut self, _ui: &mut Ui, _me: PanelId, msg: &Msg, _from: Option<PanelId>, _s: &mut dyn Surface) -> bool {
        msg.is("DialogVariables")
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, _s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let fg = scheme.color("BrightControlText", Color::WHITE);
        let bg = scheme.color("WindowBgColor", Color::WHITE);
        let border = scheme.border("ButtonDepressedBorder");
        if let Some(n) = ui.node_mut(me) {
            n.fg = fg;
            n.bg = bg;
            n.border = border;
        }
    }
    fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &Block, s: &mut dyn Surface) {
        if let Some(p) = kv_str(kv, "progress").and_then(|v| v.trim().parse::<f32>().ok()) {
            self.progress = p.clamp(0.0, 1.0);
        }
        ui.panel_apply_settings(me, kv, s);
    }
    /// `PaintBackground`: the bg fill, then `(w / (gap + seg)) * progress` segments.
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let (w, h) = ui.size(me);
        let (fg, bg) = ui.node(me).map_or((Color::WHITE, Color::WHITE), |n| (n.fg, n.bg));
        s.draw_set_color(bg);
        s.draw_filled_rect(0, 0, w, h);
        let step = self.segment_gap + self.segment_wide;
        if step <= 0 {
            return;
        }
        let n = ((w / step) as f32 * self.progress) as i32;
        s.draw_set_color(fg);
        let mut x = 0;
        for _ in 0..n {
            x += self.segment_gap;
            s.draw_filled_rect(x, self.bar_inset, x + self.segment_wide, self.bar_inset + h - 2 * self.bar_inset);
            x += self.segment_wide;
        }
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `vgui2::Divider`: a 128×2 panel with the depressed border.
#[derive(Debug, Default)]
pub struct Divider;

impl Divider {
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str) -> PanelId {
        let me = ui.create(parent, name, Box::new(Divider));
        ui.set_size(me, 128, 2);
        me
    }
}

impl Control for Divider {
    fn class(&self) -> &'static str {
        "Divider"
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, _s: &mut dyn Surface) {
        let border = scheme.border("ButtonDepressedBorder");
        if let Some(n) = ui.node_mut(me) {
            n.border = border;
        }
        ui.panel_apply_scheme(me, scheme);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `vgui2::ImagePanel`.
#[derive(Debug, Default)]
pub struct ImagePanel {
    image_name: Option<String>,
    image: Option<Texture>,
    scale: bool,
    fill: Color,
    /// `GetImage()->SetSize(w, h)`: the bitmap's own size, used instead of the texture's.
    pub image_size: Option<(i32, i32)>,
}

impl ImagePanel {
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str) -> PanelId {
        ui.create(parent, name, Box::new(ImagePanel::default()))
    }

    /// `SetImage` by name (resolved at the next scheme application).
    pub fn set_image_name(ui: &mut Ui, me: PanelId, name: &str) {
        if let Some(p) = ui.ctrl::<ImagePanel>(me) {
            p.image_name = Some(name.to_owned());
            p.image = None;
        }
        ui.reapply_scheme(me);
    }

    /// `GetImage()->SetSize(w, h)`.
    pub fn set_image_size(ui: &mut Ui, me: PanelId, w: i32, h: i32) {
        if let Some(p) = ui.ctrl::<ImagePanel>(me) {
            p.image_size = Some((w, h));
        }
    }
}

impl Control for ImagePanel {
    fn class(&self) -> &'static str {
        "ImagePanel"
    }
    fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &Block, s: &mut dyn Surface) {
        self.scale = kv_int(kv, "scaleImage", 0) != 0;
        if let Some(img) = kv_str(kv, "image").filter(|v| !v.is_empty()) {
            self.image_name = Some(img.to_owned());
            ui.invalidate_layout_ex(me, false, true, s);
        }
        if let Some(fc) = kv_str(kv, "fillcolor").filter(|v| !v.is_empty()) {
            let nums: Vec<i32> = fc.split_whitespace().map_while(|t| t.parse().ok()).collect();
            self.fill = if nums.len() <= 2 {
                ui.scheme_of(me).map_or(Color::TRANSPARENT, |sc| sc.color(fc, Color::TRANSPARENT))
            } else {
                let g = |i: usize| nums.get(i).copied().unwrap_or(0).clamp(0, 255) as u8;
                Color::rgba(g(0), g(1), g(2), g(3))
            };
        }
        if let Some(b) = kv_str(kv, "border").filter(|v| !v.is_empty()) {
            let border = ui.scheme_of(me).and_then(|sc| sc.border(b));
            if let Some(n) = ui.node_mut(me) {
                n.border = border;
            }
        }
        ui.panel_apply_settings(me, kv, s);
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        if let Some(name) = self.image_name.clone() {
            self.image = s.texture(&name, self.scale);
            if let Some(t) = self.image
                && !self.scale
            {
                let (iw, ih) = self.image_size.unwrap_or((t.width, t.height));
                ui.set_size(me, iw, ih);
            }
        }
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let (w, h) = ui.size(me);
        if self.fill.a() != 0 {
            s.draw_set_color(self.fill);
            s.draw_filled_rect(0, 0, w, h);
        }
        if let Some(t) = self.image {
            s.draw_set_color(Color::WHITE);
            let (iw, ih) = if self.scale { (w, h) } else { self.image_size.unwrap_or((t.width, t.height)) };
            s.draw_textured_rect(t, 0, 0, iw, ih);
        }
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
