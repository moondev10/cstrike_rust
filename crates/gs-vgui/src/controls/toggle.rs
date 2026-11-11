//! `ToggleButton`, `CheckButton`, `RadioButton` (docs/re/vgui-controls.md §7).

use std::any::Any;

use gs_formats::keyvalues::Block;

use crate::controls::button::{Activation, ButtonState};
use crate::controls::label::{Align, LabelImage, TextColorState};
use crate::editable::kv_int;
use crate::keys::*;
use crate::msg::{Msg, Value};
use crate::scheme::Scheme;
use crate::surface::{Color, Surface};
use crate::ui::{Control, PanelId, Ui};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleKind {
    Toggle,
    Check,
    Radio,
}

/// A toggle-style button: `ToggleButton`, `CheckButton` or `RadioButton`.
#[derive(Debug)]
pub struct ToggleButton {
    pub st: ButtonState,
    pub kind: ToggleKind,
    selected_color: Color,
    /// CheckButton's `m_bCheckButtonCheckable`.
    pub checkable: bool,
    /// RadioButton's `_oldTabPosition` / `_subTabPosition`.
    old_tab: i32,
    sub_tab: i32,
}

/// Runs `f` on the toggle button `id`.
pub fn with_toggle<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut ToggleButton, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<ToggleButton>().map(|t| f(t, ui))).flatten()
}

impl ToggleButton {
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str, text: &str, kind: ToggleKind) -> PanelId {
        let mut st = ButtonState::new(ui, parent, text);
        st.activation = Activation::OnPressed;
        st.custom_click = true;
        if kind != ToggleKind::Toggle {
            st.label.align = Align::West;
            st.label.set_text_image_index(1);
        }
        if kind == ToggleKind::Radio {
            st.label.proportional_scale_images = false;
        }
        let me = ui.create(parent, name, Box::new(ToggleButton { st, kind, selected_color: Color::TRANSPARENT, checkable: true, old_tab: 0, sub_tab: 0 }));
        if kind == ToggleKind::Radio {
            let reserve = (ui.psv(me, 20), ui.psv(me, 13));
            if let Some(t) = ui.ctrl::<ToggleButton>(me) {
                t.st.label.set_image_at(0, LabelImage::Pinned { tex: None, size: (0, 0), reserve }, 0);
            }
        }
        me
    }

    pub fn is_selected(&self) -> bool {
        self.st.is_selected()
    }

    /// `SetSelected` with the class's side effects.
    pub fn set_selected(&mut self, ui: &mut Ui, me: PanelId, state: bool, s: &mut dyn Surface) {
        match self.kind {
            ToggleKind::Toggle => self.st.set_selected(ui, me, state),
            ToggleKind::Check => {
                if !self.checkable {
                    return;
                }
                ui.post_action_signal(me, Msg::new("CheckButtonChecked").int("state", i32::from(state)));
                self.st.set_selected(ui, me, state);
                ui.reapply_scheme(me);
            }
            ToggleKind::Radio => {
                if state {
                    if !ui.is_enabled(me) {
                        return;
                    }
                    if let Some(n) = ui.node_mut(me) {
                        n.tab_position = self.old_tab;
                    }
                    if let Some(p) = ui.parent(me) {
                        let msg = Msg::new("RadioButtonChecked").with("panel", Value::Panel(me)).int("tabposition", self.old_tab);
                        for sib in ui.children(p).into_iter().filter(|&c| c != me) {
                            ui.post(sib, msg.clone(), Some(me), 0.0);
                        }
                    }
                    ui.request_focus(me);
                    ui.post_action_signal(me, Msg::new("RadioButtonChecked").with("panel", Value::Panel(me)).int("tabposition", self.old_tab));
                } else {
                    let tab = ui.node(me).map_or(0, |n| n.tab_position);
                    if tab != 0 {
                        self.old_tab = tab;
                    }
                    if let Some(n) = ui.node_mut(me) {
                        n.tab_position = 0;
                    }
                }
                ui.invalidate_layout(me);
                self.st.set_selected(ui, me, state);
            }
        }
        let _ = s;
    }

    /// `ToggleButton::DoClick`.
    fn do_click(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let sel = !self.st.is_selected();
        self.st.force_depressed(ui, me, sel);
        self.set_selected(ui, me, sel, s);
        if let Some(cmd) = self.st.command.clone() {
            ui.post_action_signal(me, cmd);
        }
        ui.post_action_signal(me, Msg::new("ButtonToggled").int("state", i32::from(self.st.is_selected())));
    }

    /// `FindBestRadioButton(dir)`: same group, next/previous sub-position (wrapping).
    fn find_best_radio(&self, ui: &mut Ui, me: PanelId, dir: i32) -> Option<PanelId> {
        let p = ui.parent(me)?;
        let group: Vec<(PanelId, i32)> = ui
            .children(p)
            .into_iter()
            .filter(|&c| c != me)
            .filter_map(|c| ui.ctrl::<ToggleButton>(c).filter(|t| t.kind == ToggleKind::Radio && t.old_tab == self.old_tab).map(|t| (c, t.sub_tab)))
            .collect();
        let want = self.sub_tab + dir;
        group.iter().find(|&&(_, s)| s == want).map(|&(c, _)| c).or_else(|| {
            if dir > 0 { group.iter().find(|&&(_, s)| s == 0).map(|&(c, _)| c) } else { group.iter().max_by_key(|&&(_, s)| s).map(|&(c, _)| c) }
        })
    }
}

impl Control for ToggleButton {
    fn class(&self) -> &'static str {
        match self.kind {
            ToggleKind::Toggle => "ToggleButton",
            ToggleKind::Check => "CheckButton",
            ToggleKind::Radio => "RadioButton",
        }
    }
    fn can_be_default_button(&self) -> bool {
        false
    }
    fn has_hotkey(&self, key: char) -> bool {
        self.st.label.hotkey == Some(key)
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        let handled = if msg.is("RadioButtonChecked") && self.kind == ToggleKind::Radio {
            if msg.get_panel("panel") != Some(me) && msg.get_int("tabposition") == self.old_tab {
                self.set_selected(ui, me, false, s);
            }
            true
        } else if msg.is("MouseDoublePressed") {
            self.st.on_message(ui, me, &Msg::new("MousePressed").int("code", msg.get_int("code")), s)
        } else if msg.is("KeyCodePressed") && msg.get_int("code") == KEY_SPACE && self.kind == ToggleKind::Toggle {
            self.do_click(ui, me, s);
            true
        } else if msg.is("KeyCodeTyped") {
            let code = msg.get_int("code");
            if self.kind == ToggleKind::Radio {
                match code {
                    KEY_ENTER | KEY_SPACE if !self.st.is_selected() => self.set_selected(ui, me, true, s),
                    KEY_TAB => {
                        if let Some(b) = self.find_best_radio(ui, me, 1) {
                            ui.request_focus(b);
                        }
                    }
                    KEY_UP | KEY_LEFT | KEY_DOWN | KEY_RIGHT => {
                        let dir = if code == KEY_UP || code == KEY_LEFT { -1 } else { 1 };
                        if let Some(b) = self.find_best_radio(ui, me, dir) {
                            ui.request_focus(b);
                            with_toggle(ui, b, |t, ui| t.set_selected(ui, b, true, s));
                        }
                    }
                    _ => ui.panel_on_key_code_typed(me, code, s),
                }
            } else if code != KEY_ENTER && code != KEY_SPACE {
                ui.panel_on_key_code_typed(me, code, s);
            }
            true
        } else {
            self.st.on_message(ui, me, msg, s)
        };
        if self.st.take_click() {
            self.do_click(ui, me, s);
        }
        handled
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.st.apply_scheme(ui, me, scheme, s);
        self.selected_color = scheme.color("BrightControlText", Color::WHITE);
        match self.kind {
            ToggleKind::Toggle => {}
            ToggleKind::Check => {
                let bg = ui.node(me).map_or(Color::WHITE, |n| n.bg);
                self.st.default_fg = scheme.color("FgColor", Color::WHITE);
                self.st.default_bg = bg;
                self.selected_color = scheme.color("BrightControlText", scheme.color("ControlText", Color::WHITE));
                if let Some(n) = ui.node_mut(me) {
                    n.paint_background_enabled = false;
                }
                self.st.label.set_text_image_index(1);
                let (name, hw) = if self.st.is_selected() { ("resource/icon_checked", true) } else { ("resource/icon_emptybox", false) };
                let tex = s.texture(name, hw);
                let size = tex.map_or((0, 0), |t| (t.width, t.height));
                self.st.label.set_image_at(0, LabelImage::Texture { name: name.to_owned(), tex, size, color: Color::WHITE }, 0);
            }
            ToggleKind::Radio => {
                let fg = scheme.color("FgColor", Color::WHITE);
                if let Some(n) = ui.node_mut(me) {
                    n.fg = fg;
                    n.paint_background_enabled = false;
                }
                self.selected_color = scheme.color("BrightControlText", scheme.color("ControlText", Color::WHITE));
                self.st.label.align = Align::West;
                let name = if self.st.is_selected() { "resource/icon_radiosel" } else { "resource/icon_radiounsel" };
                let tex = s.texture(name, true);
                let size = tex.map_or((0, 0), |t| (ui.psv(me, t.width), ui.psv(me, t.height)));
                let reserve = (ui.psv(me, 20), ui.psv(me, 13));
                self.st.label.set_image_at(0, LabelImage::Pinned { tex, size, reserve }, 0);
            }
        }
        if let Some(n) = ui.node_mut(me)
            && self.kind != ToggleKind::Toggle
        {
            n.border = None;
        }
        self.st.set_borders(None, None);
    }
    fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &Block, s: &mut dyn Surface) {
        self.st.label.apply_settings(ui, me, kv, s);
        self.st.label.color_state = if self.kind == ToggleKind::Radio { TextColorState::Normal } else { TextColorState::Light };
        if let Some(cmd) = crate::editable::kv_str(kv, "command").filter(|c| !c.is_empty()) {
            self.st.set_command(cmd);
        }
        if self.kind == ToggleKind::Radio {
            self.sub_tab = kv_int(kv, "SubTabPosition", 0);
            self.old_tab = ui.node(me).map_or(0, |n| n.tab_position);
        }
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.st.perform_layout(ui, me, s);
        if self.st.is_selected() && self.kind != ToggleKind::Toggle {
            self.st.label.text.color = self.selected_color;
        }
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if self.st.is_selected() && self.kind == ToggleKind::Toggle {
            self.st.label.text.color = self.selected_color;
        }
        self.st.paint(ui, me, s);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
