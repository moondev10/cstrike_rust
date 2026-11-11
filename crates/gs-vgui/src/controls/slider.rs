//! `vgui2::Slider` (docs/re/vgui-controls-2.md S.6), GameUI's float32 arithmetic.

use std::any::Any;
use std::sync::Arc;

use gs_formats::keyvalues::Block;

use crate::border::Border;
use crate::controls::label::TextImage;
use crate::editable::{kv_int, kv_str};
use crate::keys::*;
use crate::msg::Msg;
use crate::scheme::Scheme;
use crate::surface::{Color, Surface};
use crate::ui::{Control, PanelId, Ui};

/// The slider state; GameUI's cvar slider embeds it.
#[derive(Debug)]
pub struct SliderState {
    dragging: bool,
    nob_pos: [i32; 2],
    nob_drag_start: [i32; 2],
    drag_start: (i32, i32),
    range: [i32; 2],
    value: i32,
    slider_border: Option<Arc<Border>>,
    inset_border: Option<Arc<Border>>,
    nob_size: f32,
    left_caption: Option<TextImage>,
    right_caption: Option<TextImage>,
    num_ticks: i32,
    track_color: Color,
    tick_color: Color,
    disabled1: Color,
    disabled2: Color,
    caption_font: crate::surface::FontHandle,
}

/// `(int)` of a float: truncation, NaN/out of range → INT_MIN (cvttss2si).
fn cvtt(f: f32) -> i32 {
    if (-2_147_483_648.0..2_147_483_648.0).contains(&f) { f as i32 } else { i32::MIN }
}

fn cvtt_d(f: f64) -> i32 {
    if (-2_147_483_648.0..2_147_483_648.0).contains(&f) { f as i32 } else { i32::MIN }
}

impl SliderState {
    pub fn new(ui: &Ui, me: PanelId) -> Self {
        let mut st = SliderState {
            dragging: false,
            nob_pos: [0; 2],
            nob_drag_start: [0; 2],
            drag_start: (0, 0),
            range: [0; 2],
            value: 0,
            slider_border: None,
            inset_border: None,
            nob_size: ui.psv(me, 8) as f32,
            left_caption: None,
            right_caption: None,
            num_ticks: 10,
            track_color: Color::WHITE,
            tick_color: Color::WHITE,
            disabled1: Color::WHITE,
            disabled2: Color::WHITE,
            caption_font: crate::surface::FontHandle(0),
        };
        st.recompute_nob_pos(ui, me);
        st
    }

    fn paint_size(ui: &Ui, me: PanelId) -> (i32, i32) {
        let (w, h) = ui.size(me);
        let [l, t, r, b] = ui.node(me).and_then(|n| n.border.as_ref().map(|b| b.inset)).unwrap_or([0; 4]);
        (w - l - r, h - t - b)
    }

    /// `GetTrackRect`.
    fn track_rect(&self, ui: &Ui, me: PanelId) -> (i32, i32, i32, i32) {
        let (wide, _) = Self::paint_size(ui, me);
        (0, ui.psv(me, 8), cvtt(wide as f32 - self.nob_size), ui.psv(me, 4))
    }

    /// `RecomputeNobPosFromValue`.
    fn recompute_nob_pos(&mut self, ui: &Ui, me: PanelId) {
        let (wide, _) = Self::paint_size(ui, me);
        let frac = (self.value - self.range[0]) as f32 / (self.range[1] - self.range[0]) as f32;
        let first = frac * (wide as f32 - self.nob_size) + 0.0;
        self.nob_pos[0] = cvtt(first);
        self.nob_pos[1] = cvtt(self.nob_size + first);
        if self.nob_pos[1] > wide {
            self.nob_pos[1] = wide;
            self.nob_pos[0] = wide - cvtt(self.nob_size);
        }
    }

    /// `RecomputeValueFromNobPos` (no clamp).
    fn recompute_value(&mut self, ui: &Ui, me: PanelId) {
        let (wide, _) = Self::paint_size(ui, me);
        let f = self.nob_pos[0] as f32 / (wide as f32 - self.nob_size) * (self.range[1] - self.range[0]) as f32 + self.range[0] as f32;
        self.value = cvtt_d(f64::from(f) + 0.5);
    }

    pub fn value(&self) -> i32 {
        self.value
    }

    pub fn range(&self) -> (i32, i32) {
        (self.range[0], self.range[1])
    }

    /// `SetValue(v, notify)`: clamped; `SliderMoved` on a change when notifying.
    pub fn set_value(&mut self, ui: &mut Ui, me: PanelId, v: i32, notify: bool) {
        let old = self.value;
        let v = if v < self.range[0] { self.range[0] } else { v };
        self.value = if v <= self.range[1] { v } else { self.range[1] };
        self.recompute_nob_pos(ui, me);
        if self.value != old && notify {
            self.send_moved(ui, me);
        }
    }

    /// `SetRange(min, max)`: no nob recompute, no message.
    pub fn set_range(&mut self, min: i32, max: i32) {
        let max = if max >= min { max } else { min };
        self.range = [min, max];
        if self.value < min {
            self.value = min;
        } else if self.value > max {
            self.value = max;
        }
    }

    pub fn set_num_ticks(&mut self, n: i32) {
        self.num_ticks = n;
    }

    pub fn set_thumb_width(&mut self, w: i32) {
        self.nob_size = w as f32;
    }

    /// `SetTickCaptions(l, r)`: a `None` side stays unchanged.
    pub fn set_tick_captions(&mut self, ui: &mut Ui, me: PanelId, l: Option<&str>, r: Option<&str>) {
        for (cap, text) in [(&mut self.left_caption, l), (&mut self.right_caption, r)] {
            if let Some(t) = text {
                cap.get_or_insert_with(TextImage::default).set_text(ui, t);
            }
        }
        ui.invalidate_layout(me);
    }

    fn send_moved(&self, ui: &mut Ui, me: PanelId) {
        ui.post_action_signal(me, Msg::new("SliderMoved").int("position", self.value));
    }

    fn local_cursor(ui: &Ui, me: PanelId) -> (i32, i32) {
        let (cx, cy) = ui.cursor_pos();
        let (ax, ay) = ui.abs_pos(me);
        (cx - ax, cy - ay)
    }

    /// Slider's messages; `false` for the ones it does not handle.
    pub fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) -> bool {
        if msg.is("MousePressed") || msg.is("MouseDoublePressed") {
            if !ui.is_enabled(me) {
                return true;
            }
            let (x, y) = Self::local_cursor(ui, me);
            ui.request_focus(me);
            if x >= self.nob_pos[0] && x < self.nob_pos[1] {
                self.dragging = true;
                ui.set_mouse_capture(Some(me));
                self.nob_drag_start = self.nob_pos;
                self.drag_start = (x, y);
            } else {
                let (min, max) = self.range();
                let w = ui.size(me).0;
                self.set_value(ui, me, cvtt((max - min) as f32 / w as f32 * x as f32) + min, true);
            }
        } else if msg.is("MouseReleased") {
            self.dragging = false;
            ui.set_mouse_capture(None);
        } else if msg.is("CursorMoved") {
            if !self.dragging {
                return true;
            }
            let (x, _) = Self::local_cursor(ui, me);
            let (wide, _) = Self::paint_size(ui, me);
            let d = x - self.drag_start.0;
            self.nob_pos = [self.nob_drag_start[0] + d, self.nob_drag_start[1] + d];
            if self.nob_pos[1] > wide {
                self.nob_pos[0] += wide - self.nob_pos[1];
                self.nob_pos[1] = wide;
            }
            if self.nob_pos[0] < 0 {
                self.nob_pos[1] -= self.nob_pos[0];
                self.nob_pos[0] = 0;
            }
            self.recompute_value(ui, me);
            self.send_moved(ui, me);
        } else if msg.is("KeyCodeTyped") {
            let code = msg.get_int("code");
            let v = self.value;
            let (min, max) = self.range();
            let rng = max as f32 - min as f32;
            let step = cvtt(rng * 0.02);
            match code {
                KEY_ENTER => {}
                KEY_HOME => self.set_value(ui, me, min, true),
                KEY_END => self.set_value(ui, me, max, true),
                KEY_PAGEUP => self.set_value(ui, me, v + cvtt(rng / self.num_ticks as f32), true),
                KEY_PAGEDOWN => self.set_value(ui, me, v - cvtt(rng / self.num_ticks as f32), true),
                KEY_LEFT => self.set_value(ui, me, v - step, true),
                KEY_RIGHT => self.set_value(ui, me, v + step, true),
                _ => ui.panel_on_key_code_typed(me, code, s),
            }
        } else {
            return false;
        }
        true
    }

    pub fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let fg = scheme.color("Slider/SliderFgColor", Color::WHITE);
        if let Some(n) = ui.node_mut(me) {
            n.fg = fg;
        }
        self.slider_border = scheme.border("ButtonBorder");
        self.inset_border = scheme.border("ButtonDepressedBorder");
        self.track_color = scheme.color("SliderTrackColor", Color::WHITE);
        self.tick_color = scheme.color("SliderTickColor", Color::WHITE);
        self.disabled1 = scheme.color("DisabledFgColor1", Color::WHITE);
        self.disabled2 = scheme.color("DisabledFgColor2", Color::WHITE);
        let (prop, hd) = (ui.is_proportional(me), ui.is_hd_proportional(me));
        self.caption_font = s.font(&scheme.file, "DefaultVerySmall", prop, hd);
        for cap in [&mut self.left_caption, &mut self.right_caption].into_iter().flatten() {
            cap.font = self.caption_font;
        }
    }

    pub fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &Block, s: &mut dyn Surface) {
        ui.panel_apply_settings(me, kv, s);
        let t = kv_int(kv, "thumbwidth", 0);
        if t != 0 {
            self.set_thumb_width(t);
        }
        let (l, r) = (kv_str(kv, "leftText").map(str::to_owned), kv_str(kv, "rightText").map(str::to_owned));
        self.set_tick_captions(ui, me, l.as_deref(), r.as_deref());
    }

    pub fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.recompute_nob_pos(ui, me);
        for cap in [&mut self.left_caption, &mut self.right_caption].into_iter().flatten() {
            let (w, h) = cap.content_size(s);
            cap.set_size(w, h);
        }
    }

    pub fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        ui.panel_paint_background(me, s);
        let (x, y, w, h) = self.track_rect(ui, me);
        s.draw_set_color(self.track_color);
        s.draw_filled_rect(x, y, x + w, y + h);
        if let Some(b) = &self.inset_border {
            b.paint(s, x, y, x + w, y + h, None);
        }
    }

    pub fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let enabled = ui.is_enabled(me);
        let (_, ty, _, th) = self.track_rect(ui, me);
        let (wide, _) = Self::paint_size(ui, me);
        // DrawTicks.
        let half = self.nob_size * 0.5;
        let spacing = (wide as f32 - self.nob_size) / self.num_ticks as f32;
        let y = ty + cvtt(self.nob_size);
        let (tall, p1, p2) = (ui.psv(me, 5), ui.psv(me, 1), ui.psv(me, 2));
        // `for i = 0 .. numTicks` inclusive (none when negative).
        let xs = (0..=self.num_ticks).map(|i| cvtt(i as f32 * spacing + half));
        if enabled {
            s.draw_set_color(self.tick_color);
            for xp in xs {
                s.draw_filled_rect(xp, y, xp + p1, y + tall);
            }
        } else {
            s.draw_set_color(self.disabled1);
            for xp in xs.clone() {
                s.draw_filled_rect(xp + p1, y + p1, xp + p2, y + p1 + tall);
            }
            s.draw_set_color(self.disabled2);
            for xp in xs {
                s.draw_filled_rect(xp, y, xp + p1, y + tall);
            }
        }
        // DrawTickLabels.
        let ly = ty + ui.psv(me, 12);
        let col = if enabled { self.tick_color } else { self.disabled1 };
        s.draw_set_text_color(col);
        if let Some(cap) = &mut self.left_caption {
            cap.pos = (0, ly);
            cap.color = col;
            cap.paint(s);
        }
        if let Some(cap) = &mut self.right_caption {
            cap.pos = (wide - cap.size.0, ly);
            cap.color = col;
            cap.paint(s);
        }
        // DrawNob.
        let fg = ui.node(me).map_or(Color::WHITE, |n| n.fg);
        s.draw_set_color(fg);
        let nh = ui.psv(me, 16);
        let cy = ty + th / 2;
        let (y0, y1) = (cy - nh / 2, cy + nh / 2);
        s.draw_filled_rect(self.nob_pos[0], y0, self.nob_pos[1], y1);
        if let Some(b) = &self.slider_border {
            b.paint(s, self.nob_pos[0], y0, self.nob_pos[1], y1, None);
        }
    }

    pub fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId) {
        self.recompute_nob_pos(ui, me);
    }
}

/// A plain Slider panel (`SliderMoved` also goes to the slider itself, which ignores it).
#[derive(Debug)]
pub struct Slider {
    pub st: SliderState,
}

impl Slider {
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str) -> PanelId {
        let placeholder = ui.embedded();
        let st = SliderState::new(ui, placeholder);
        let me = ui.create(parent, name, Box::new(Slider { st }));
        ui.add_action_signal_target(me, me);
        me
    }
}

impl Control for Slider {
    fn class(&self) -> &'static str {
        "Slider"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("SliderMoved") {
            return true;
        }
        self.st.on_message(ui, me, msg, s)
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.st.apply_scheme(ui, me, scheme, s);
    }
    fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &Block, s: &mut dyn Surface) {
        self.st.apply_settings(ui, me, kv, s);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.st.perform_layout(ui, me, s);
    }
    fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId, _w: i32, _h: i32) {
        self.st.on_size_changed(ui, me);
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.st.paint_background(ui, me, s);
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.st.paint(ui, me, s);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
