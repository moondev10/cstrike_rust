//! `ScrollBar`, `ScrollBarSlider` and the arrow buttons (docs/re/vgui-controls-2.md §4 S.2–S.4).

use std::any::Any;
use std::sync::Arc;

use crate::border::Border;
use crate::controls::button::{Activation, ButtonState};
use crate::controls::label::{Align, LabelImage};
use crate::msg::{Msg, Value};
use crate::scheme::Scheme;
use crate::surface::{Color, Surface};
use crate::ui::{Control, PanelId, Ui};

/// `vgui2::ScrollBarSlider`.
#[derive(Debug)]
pub struct ScrollBarSlider {
    vertical: bool,
    dragging: bool,
    nob: [i32; 2],
    nob_drag_start: [i32; 2],
    drag_start: (i32, i32),
    range: [i32; 2],
    value: i32,
    window: i32,
    button_offset: i32,
    border: Option<Arc<Border>>,
}

impl ScrollBarSlider {
    fn new(vertical: bool) -> Self {
        Self { vertical, dragging: false, nob: [0; 2], nob_drag_start: [0; 2], drag_start: (0, 0), range: [0; 2], value: 0, window: 0, button_offset: 0, border: None }
    }

    /// `RecomputeNobPosFromValue` (float32 throughout).
    fn recompute_nob(&mut self, (wide, tall): (i32, i32)) {
        let mut range = (self.range[1] - self.range[0]) as f32;
        let window = self.window as f32;
        let frac = (self.value - self.range[0]) as f32 / (range - window);
        if window > 0.0 {
            if range.is_nan() || range <= 0.0 {
                range = 1.0;
            }
            let (length, thick) = if self.vertical { (tall as f32 - 1.0, wide as f32) } else { (wide as f32, tall as f32 - 1.0) };
            let raw = window / range * length;
            // maxss: the second operand when either is NaN.
            let nob = if thick > raw { thick } else { raw };
            let free = length - nob;
            let first = free * frac;
            self.nob = [cvtt(first), cvtt(first + nob)];
            if self.nob[1] as f32 > length {
                self.nob = [cvtt(free), cvtt(length)];
            }
        }
    }

    /// `RecomputeValueFromNobPos`.
    fn recompute_value(&mut self, (wide, tall): (i32, i32)) {
        let mut range = (self.range[1] - self.range[0]) as f32;
        let window = self.window as f32;
        let off = if window > 0.0 {
            if range.is_nan() || range <= 0.0 {
                range = 1.0;
            }
            let (length, thick) = if self.vertical { (tall as f32, wide as f32) } else { (wide as f32, tall as f32) };
            let raw = window / range * length;
            let nob = if thick > raw { thick } else { raw };
            self.nob[0] as f32 / (length - nob) * (range - window)
        } else {
            (self.value - self.range[0]) as f32
        };
        let eps = range * 0.01;
        let top = self.range[1] - self.window;
        if f64::from(eps) > (f64::from(window + off - self.range[1] as f32)).abs() {
            self.value = top;
        } else {
            let v = (f64::from(self.range[0] as f32 + off) + 0.5) as i32;
            self.value = v.min(top);
        }
    }

    /// `SetValue`: lower clamp, then upper; `ScrollBarSliderMoved` when it changed.
    fn set_value(&mut self, ui: &mut Ui, me: PanelId, v: i32) {
        let old = self.value;
        let v = v.max(self.range[0]);
        let v = if v <= self.range[1] - self.window { v } else { self.range[1] - self.window };
        self.value = v;
        self.recompute_nob(ui.size(me));
        if self.value != old {
            ui.post_action_signal(me, Msg::new("ScrollBarSliderMoved").int("position", self.value));
        }
    }
}

/// `(int)` of a float32 (`cvttss2si`: NaN/out of range → `i32::MIN`).
fn cvtt(f: f32) -> i32 {
    if (-2_147_483_648.0..2_147_483_648.0).contains(&f) { f as i32 } else { i32::MIN }
}

impl Control for ScrollBarSlider {
    fn class(&self) -> &'static str {
        "ScrollBarSlider"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("MousePressed") || msg.is("MouseDoublePressed") {
            let (cx, cy) = ui.cursor_pos();
            let (ax, ay) = ui.abs_pos(me);
            let (x, y) = (cx - ax, cy - ay);
            let p = if self.vertical { y } else { x };
            if p < self.nob[0] {
                let v = self.value - self.window;
                self.set_value(ui, me, v);
            } else if p >= self.nob[1] {
                let v = self.value + self.window;
                self.set_value(ui, me, v);
            } else {
                self.dragging = true;
                ui.set_mouse_capture(Some(me));
                self.nob_drag_start = self.nob;
                self.drag_start = (x, y);
            }
        } else if msg.is("MouseReleased") {
            self.dragging = false;
            ui.set_mouse_capture(None);
        } else if msg.is("CursorMoved") {
            if !self.dragging {
                return true;
            }
            let (ax, ay) = ui.abs_pos(me);
            let (x, y) = (msg.get_int("xpos") - ax, msg.get_int("ypos") - ay);
            let (wide, tall) = ui.size(me);
            if self.vertical {
                let d = y - self.drag_start.1;
                self.nob = [self.nob_drag_start[0] + d, self.nob_drag_start[1] + d];
                if self.nob[1] > tall {
                    self.nob = [self.nob[0] - self.nob[1] + tall, tall];
                    let v = self.range[1] - self.window;
                    self.set_value(ui, me, v);
                }
            } else {
                let d = x - self.drag_start.0;
                self.nob = [self.nob_drag_start[0] + d, self.nob_drag_start[1] + d];
                if self.nob[1] > wide {
                    self.nob = [self.nob[0] - self.nob[1] + wide, wide];
                }
            }
            if self.nob[0] < 0 {
                self.nob[1] -= self.nob[0];
                self.nob[0] = 0;
                self.set_value(ui, me, 0);
            }
            ui.invalidate_layout(me);
            self.recompute_value((wide, tall));
            ui.post_action_signal(me, Msg::new("ScrollBarSliderMoved").int("position", self.value));
        } else if msg.is("MouseFocusTicked") {
            // Panel::OnMouseFocusTicked: "OnMouseFocusTicked" to the parent (the ScrollBar).
            ui.call_parent(me, &Msg::new("OnMouseFocusTicked"), s);
        } else {
            return false;
        }
        true
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, _s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let fg = scheme.color("ScrollBarSlider/ScrollBarSliderFgColor", Color::WHITE);
        let bg = scheme.color("ScrollBarSlider/ScrollBarSliderBgColor", Color::WHITE);
        if let Some(n) = ui.node_mut(me) {
            n.fg = fg;
            n.bg = bg;
        }
        self.border = scheme.border("ButtonBorder");
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        self.recompute_nob(ui.size(me));
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let (w, h) = ui.size(me);
        s.draw_set_color(ui.node(me).map_or(Color::WHITE, |n| n.bg));
        s.draw_filled_rect(0, 0, w - 1, h - 1);
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let (w, h) = ui.size(me);
        let r = self.range[1] - self.range[0];
        if r > 0 && r > self.window {
            s.draw_set_color(ui.node(me).map_or(Color::WHITE, |n| n.fg));
            if self.vertical {
                s.draw_filled_rect(0, self.nob[0], w - 1, self.nob[1]);
                if let Some(b) = &self.border {
                    b.paint(s, 0, self.nob[0], w - 1, self.nob[1], None);
                }
            } else {
                s.draw_filled_rect(self.nob[0], 0, self.nob[1], h);
                if let Some(b) = &self.border {
                    b.paint(s, self.nob[0] - 1, 1, self.nob[1], h, None);
                }
            }
        }
    }
    fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId, w: i32, h: i32) {
        ui.invalidate_layout(me);
        self.recompute_nob((w, h));
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// The arrow `Button` of a ScrollBar (`ScrollBarButton`).
#[derive(Debug)]
pub struct ScrollBarButton {
    pub st: ButtonState,
    image: String,
}

impl Control for ScrollBarButton {
    fn class(&self) -> &'static str {
        "ScrollBarButton"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("MousePressed") || msg.is("MouseDoublePressed") {
            if ui.is_enabled(me) && msg.get_int("code") == crate::keys::MOUSE_LEFT && self.st.use_capture_mouse {
                self.st.set_selected(ui, me, true);
                ui.set_mouse_capture(Some(me));
            }
            true
        } else if msg.is("MouseReleased") {
            if ui.is_enabled(me) && msg.get_int("code") == crate::keys::MOUSE_LEFT && self.st.use_capture_mouse {
                self.st.set_selected(ui, me, false);
                ui.set_mouse_capture(None);
            }
            true
        } else if msg.is("MouseFocusTicked") {
            ui.call_parent(me, &Msg::new("MouseFocusTicked"), s);
            true
        } else {
            self.st.on_message(ui, me, msg, s)
        }
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.st.apply_scheme(ui, me, scheme, s);
        let b_default = scheme.border("ScrollBarButtonBorder");
        let b_depressed = scheme.border("ScrollBarButtonDepressedBorder");
        self.st.set_borders(b_default, b_depressed);
        let (fg, bg) = ui.node(me).map_or((Color::WHITE, Color::WHITE), |n| (n.fg, n.bg));
        self.st.armed_fg = fg;
        self.st.armed_bg = bg;
        self.st.depressed_fg = fg;
        self.st.depressed_bg = bg;
        self.st.default_fg = scheme.color("DimBaseText", Color::WHITE);
        self.st.default_bg = bg;
        self.st.label.align = Align::Center;
        self.st.label.add_image(LabelImage::Texture { name: self.image.clone(), tex: s.texture(&self.image, true), size: (0, 0), color: Color::WHITE }, 0);
        if let Some(LabelImage::Texture { tex: Some(t), size, .. }) = self.st.label.last_image_mut() {
            *size = (t.width, t.height);
        }
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.st.perform_layout(ui, me, s);
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.st.paint(ui, me, s);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `vgui2::ScrollBar`.
#[derive(Debug)]
pub struct ScrollBar {
    pub slider: PanelId,
    pub buttons: [PanelId; 2],
    button_pressed_scroll_value: i32,
    scroll_delay: u32,
    respond: bool,
}

impl ScrollBar {
    /// `new ScrollBar(parent, name, vertical)`.
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str, vertical: bool) -> PanelId {
        let me = ui.create(parent, name, Box::new(ScrollBar { slider: ui.embedded(), buttons: [ui.embedded(); 2], button_pressed_scroll_value: 20, scroll_delay: 400, respond: true }));
        let slider = ui.create(Some(me), "", Box::new(ScrollBarSlider::new(vertical)));
        ui.add_action_signal_target(slider, me);
        let icons = if vertical { ["resource/icon_up", "resource/icon_down"] } else { ["resource/icon_left", "resource/icon_right"] };
        let mut buttons = [me; 2];
        for (i, icon) in icons.iter().enumerate() {
            let mut st = ButtonState::new(ui, None, "");
            st.activation = Activation::OnPressed;
            st.command = Some(Msg::new("ScrollButtonPressed").with("index", Value::Int(i as i32)));
            let b = ui.create(Some(me), "", Box::new(ScrollBarButton { st, image: (*icon).to_owned() }));
            ui.add_action_signal_target(b, me);
            if vertical {
                let inset = ui.psv(b, -8);
                if let Some(c) = ui.ctrl::<ScrollBarButton>(b) {
                    c.st.label.text_inset = (inset, 0);
                }
            }
            buttons[i] = b;
        }
        let offset: i32 = buttons.iter().map(|&b| if vertical { ui.size(b).1 } else { ui.size(b).0 }).sum();
        if let Some(sl) = ui.ctrl::<ScrollBarSlider>(slider) {
            sl.button_offset = offset;
        }
        if let Some(sb) = ui.ctrl::<ScrollBar>(me) {
            sb.slider = slider;
            sb.buttons = buttons;
        }
        if let Some(n) = ui.node_mut(me) {
            n.paint_border_enabled = true;
            n.paint_background_enabled = false;
            n.paint_enabled = true;
        }
        me
    }

    fn slider(ui: &mut Ui, sb: PanelId) -> Option<PanelId> {
        ui.ctrl::<ScrollBar>(sb).map(|c| c.slider)
    }

    pub fn value(ui: &mut Ui, sb: PanelId) -> i32 {
        Self::slider(ui, sb).and_then(|s| ui.ctrl::<ScrollBarSlider>(s).map(|c| c.value)).unwrap_or(0)
    }

    pub fn set_value(ui: &mut Ui, sb: PanelId, v: i32) {
        let Some(sl) = Self::slider(ui, sb) else { return };
        ui.with_ctrl(sl, |c, ui| {
            if let Some(c) = c.as_any_mut().downcast_mut::<ScrollBarSlider>() {
                c.set_value(ui, sl, v);
            }
        });
    }

    /// `SetRange`: no clamp, no message.
    pub fn set_range(ui: &mut Ui, sb: PanelId, min: i32, max: i32) {
        let Some(sl) = Self::slider(ui, sb) else { return };
        if let Some(c) = ui.ctrl::<ScrollBarSlider>(sl) {
            c.range = [min, max.max(min)];
        }
        ui.invalidate_layout(sl);
    }

    pub fn range(ui: &mut Ui, sb: PanelId) -> (i32, i32) {
        Self::slider(ui, sb).and_then(|s| ui.ctrl::<ScrollBarSlider>(s).map(|c| (c.range[0], c.range[1]))).unwrap_or((0, 0))
    }

    pub fn set_range_window(ui: &mut Ui, sb: PanelId, n: i32) {
        let Some(sl) = Self::slider(ui, sb) else { return };
        if let Some(c) = ui.ctrl::<ScrollBarSlider>(sl) {
            c.window = n;
        }
    }

    pub fn range_window(ui: &mut Ui, sb: PanelId) -> i32 {
        Self::slider(ui, sb).and_then(|s| ui.ctrl::<ScrollBarSlider>(s).map(|c| c.window)).unwrap_or(0)
    }

    pub fn set_button_pressed_scroll_value(ui: &mut Ui, sb: PanelId, v: i32) {
        if let Some(c) = ui.ctrl::<ScrollBar>(sb) {
            c.button_pressed_scroll_value = v;
        }
    }

    /// `HasFullRange`.
    pub fn has_full_range(ui: &mut Ui, sb: PanelId) -> bool {
        let Some(sl) = Self::slider(ui, sb) else { return false };
        let (w, h) = ui.size(sl);
        ui.ctrl::<ScrollBarSlider>(sl).is_some_and(|c| {
            let len = if c.vertical { h } else { w };
            (c.window as f32) > 0.0 && len as f32 + c.button_offset as f32 >= c.window as f32
        })
    }

    /// `OnMouseFocusTicked`: arrow auto-repeat.
    fn mouse_focus_ticked(&mut self, ui: &mut Ui, me: PanelId) {
        let down = |b: PanelId| ui.node(b).is_some_and(|n| n.depressed);
        let dir = if down(self.buttons[0]) {
            -1
        } else if down(self.buttons[1]) {
            1
        } else {
            self.scroll_delay = ui.time_ms.wrapping_add(400);
            self.respond = true;
            return;
        };
        if self.respond {
            let v = Self::value(ui, me) + self.button_pressed_scroll_value * dir;
            let sl = self.slider;
            ui.with_ctrl(sl, |c, ui| {
                if let Some(c) = c.as_any_mut().downcast_mut::<ScrollBarSlider>() {
                    c.set_value(ui, sl, v);
                }
            });
            ui.post_action_signal(me, Msg::new("ScrollBarSliderMoved").int("position", v));
        }
        if self.scroll_delay < ui.time_ms {
            self.scroll_delay = ui.time_ms.wrapping_add(50);
            self.respond = true;
        } else {
            self.respond = false;
        }
    }
}

impl Control for ScrollBar {
    fn class(&self) -> &'static str {
        "ScrollBar"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, _s: &mut dyn Surface) -> bool {
        if msg.is("ScrollBarSliderMoved") {
            ui.post_action_signal(me, Msg::new("ScrollBarSliderMoved").int("position", msg.get_int("position")));
        } else if msg.is("MouseFocusTicked") || msg.is("OnMouseFocusTicked") {
            self.mouse_focus_ticked(ui, me);
        } else {
            return false;
        }
        true
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, _s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let bg = scheme.color("ScrollBar/BgColor", Color::WHITE);
        if let Some(n) = ui.node_mut(me) {
            n.bg = bg;
        }
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        let (wide, tall) = ui.size(me);
        let vertical = ui.ctrl::<ScrollBarSlider>(self.slider).is_some_and(|c| c.vertical);
        if vertical {
            ui.set_bounds(self.slider, 0, wide, wide, tall - 2 * wide + 1);
            ui.set_bounds(self.buttons[0], 0, 0, wide - 1, wide);
            ui.set_bounds(self.buttons[1], 0, tall - wide, wide - 1, wide);
        } else {
            ui.set_bounds(self.slider, tall, -1, wide - 2 * tall + 1, tall + 1);
            ui.set_bounds(self.buttons[0], 0, 0, tall, tall);
            ui.set_bounds(self.buttons[1], wide - tall, 0, tall, tall);
        }
        ui.invalidate_layout(self.slider);
    }
    fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId, _w: i32, _h: i32) {
        ui.invalidate_layout(me);
        ui.invalidate_layout(self.slider);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
