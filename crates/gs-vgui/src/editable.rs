//! `EditablePanel`, `FocusNavGroup`, `BuildGroup` and `Panel::ApplySettings`
//! (docs/re/vgui-controls.md §1.7, §2).

use gs_formats::keyvalues::{Block, Value as KvValue};

use crate::keys::{KEY_ENTER, KEY_PAD_ENTER};
use crate::msg::{Msg, Value, atoi};
use crate::surface::Surface;
use crate::ui::{PanelId, Ui};

/// `FocusNavGroup`.
#[derive(Debug, Clone, Default)]
pub struct FocusNav {
    pub default_button: Option<PanelId>,
    pub current_default_button: Option<PanelId>,
    pub current_focus: Option<PanelId>,
    /// `_topLevelFocus` (Frame sets it: tabbing never leaves the frame).
    pub top_level: bool,
}

/// A `.res` string value (repeated keys: the last one counts).
pub fn kv_str<'a>(kv: &'a Block, key: &str) -> Option<&'a str> {
    kv.last_string(key)
}

/// `KeyValues::GetInt(key, def)`.
pub fn kv_int(kv: &Block, key: &str, def: i32) -> i32 {
    kv_str(kv, key).map_or(def, atoi)
}

/// A control the base `EditablePanel::CreateControlByName` builds (parentless; the build
/// group parents and names it).
pub type Factory<'a> = dyn FnMut(&mut Ui, &str, &mut dyn Surface) -> Option<PanelId> + 'a;

impl Ui {
    /// Turns `id` into an `EditablePanel` (base size = current size).
    pub fn make_editable(&mut self, id: PanelId) {
        let size = self.size(id);
        if let Some(n) = self.node_mut(id) {
            n.editable = true;
            n.base_size = size;
        }
    }

    fn nav(&self, main: PanelId) -> crate::editable::FocusNav {
        self.node(main).map(|n| n.nav.clone()).unwrap_or_default()
    }

    fn nav_mut(&mut self, main: PanelId) -> Option<&mut FocusNav> {
        self.node_mut(main).map(|n| &mut n.nav)
    }

    fn is_tab_candidate(&self, c: PanelId) -> bool {
        self.node(c).is_some_and(|n| n.tab_position != 0) && self.is_visible(c) && self.is_enabled(c)
    }

    fn tab_position(&self, id: PanelId) -> i32 {
        self.node(id).map_or(0, |n| n.tab_position)
    }

    fn answers_default_button(&mut self, id: PanelId) -> bool {
        self.with_ctrl(id, |c, _| c.can_be_default_button()).unwrap_or(false)
    }

    /// `RequestFocusNext(from)` (virtual).
    pub fn panel_request_focus_next(&mut self, id: PanelId, from: Option<PanelId>) -> bool {
        if let Some(r) = self.with_ctrl(id, |c, ui| c.request_focus_next(ui, id, from)).flatten() {
            return r;
        }
        self.panel_request_focus_next_base(id, from)
    }

    /// `Panel::RequestFocusNext(from)` (EditablePanel: its nav group).
    pub fn panel_request_focus_next_base(&mut self, id: PanelId, from: Option<PanelId>) -> bool {
        if self.node(id).is_some_and(|n| n.editable) {
            self.nav_step(id, from, true)
        } else if let Some(p) = self.parent(id) {
            self.panel_request_focus_next(p, Some(id))
        } else {
            false
        }
    }

    /// `RequestFocusPrev(from)` (virtual).
    pub fn panel_request_focus_prev(&mut self, id: PanelId, from: Option<PanelId>) -> bool {
        if let Some(r) = self.with_ctrl(id, |c, ui| c.request_focus_prev(ui, id, from)).flatten() {
            return r;
        }
        self.panel_request_focus_prev_base(id, from)
    }

    /// `Panel::RequestFocusPrev(from)`.
    pub fn panel_request_focus_prev_base(&mut self, id: PanelId, from: Option<PanelId>) -> bool {
        if self.node(id).is_some_and(|n| n.editable) {
            self.nav_step(id, from, false)
        } else if let Some(p) = self.parent(id) {
            self.panel_request_focus_prev(p, Some(id))
        } else {
            false
        }
    }

    /// `RequestFocus(direction)`: a class override first; an EditablePanel tabs into its
    /// children for ±1.
    pub fn request_focus_dir(&mut self, id: PanelId, dir: i32) {
        if self.with_ctrl(id, |c, ui| c.request_focus_override(ui, id, dir)).unwrap_or(false) {
            return;
        }
        self.request_focus_dir_base(id, dir);
    }

    /// `EditablePanel::RequestFocus(direction)`.
    pub fn request_focus_dir_base(&mut self, id: PanelId, dir: i32) {
        if self.node(id).is_some_and(|n| n.editable) && dir != 0 {
            self.nav_step(id, None, dir > 0);
        } else {
            self.request_focus_base(id);
        }
    }

    /// `FocusNavGroup::RequestFocusNext/Prev`.
    fn nav_step(&mut self, main: PanelId, from: Option<PanelId>, next: bool) -> bool {
        self.nav_depth += 1;
        if let Some(n) = self.nav_mut(main) {
            n.current_focus = None;
        }
        let none = if next { 0 } else { 9_999_999 };
        let mut cur = from.map_or(none, |f| self.tab_position(f));
        let mut best = None;
        for pass in 0..2 {
            let want = if next { cur + 1 } else { cur - 1 };
            if next || want > 0 {
                let mut best_pos = if next { 999_999 } else { -1 };
                for c in self.children(main) {
                    if !self.is_tab_candidate(c) {
                        continue;
                    }
                    let pos = self.tab_position(c);
                    if pos == want {
                        best = Some(c);
                        break;
                    }
                    let better = if next { pos > want && pos < best_pos } else { pos < want && pos > best_pos };
                    if better {
                        best = Some(c);
                        best_pos = pos;
                    }
                }
                if best.is_some() {
                    break;
                }
            }
            let top_level = self.nav(main).top_level;
            let parent = self.parent(main).filter(|&p| p != self.embedded());
            if pass == 0
                && !top_level
                && let Some(p) = parent
                && (!next || self.nav_depth <= 14)
            {
                let moved = if next { self.panel_request_focus_next(p, Some(main)) } else { self.panel_request_focus_prev(p, Some(main)) };
                if moved {
                    self.nav_set_current_default_button(main, None, true);
                    self.nav_depth -= 1;
                    return true;
                }
            }
            cur = none;
        }
        let Some(best) = best else {
            self.nav_depth -= 1;
            return false;
        };
        if let Some(n) = self.nav_mut(main) {
            n.current_focus = Some(best);
        }
        self.request_focus_dir(best, if next { 1 } else { -1 });
        if self.answers_default_button(best) {
            self.nav_set_current_default_button(main, Some(best), true);
        } else if let Some(d) = self.nav(main).default_button {
            self.nav_set_current_default_button(main, Some(d), true);
        } else {
            self.nav_set_current_default_button(main, None, true);
            if let Some(p) = self.parent(main) {
                self.post(p, Msg::new("FindDefaultButton"), Some(main), 0.0);
            }
        }
        self.nav_depth -= 1;
        true
    }

    /// `FocusNavGroup::SetCurrentFocus`: returns the default panel.
    pub(crate) fn nav_set_current_focus(&mut self, main: PanelId, focus: PanelId, default: Option<PanelId>) -> Option<PanelId> {
        if let Some(n) = self.nav_mut(main) {
            n.current_focus = Some(focus);
        }
        let mut default = default;
        if default.is_none() {
            if self.answers_default_button(focus) {
                default = Some(focus);
            } else if let Some(d) = self.nav(main).default_button {
                default = Some(d);
            }
        }
        self.nav_set_current_default_button(main, default, true);
        default
    }

    /// `FocusNavGroup::SetCurrentDefaultButton`.
    pub fn nav_set_current_default_button(&mut self, main: PanelId, panel: Option<PanelId>, send: bool) {
        let old = self.nav(main).current_default_button;
        if old == panel {
            return;
        }
        if send && let Some(o) = old {
            self.post(o, Msg::new("SetAsCurrentDefaultButton").int("state", 0), None, 0.0);
        }
        if let Some(n) = self.nav_mut(main) {
            n.current_default_button = panel;
        }
        if send && let Some(p) = panel {
            self.post(p, Msg::new("SetAsCurrentDefaultButton").int("state", 1), None, 0.0);
        }
    }

    /// `FocusNavGroup::SetDefaultButton`.
    pub fn nav_set_default_button(&mut self, main: PanelId, panel: Option<PanelId>) {
        if let Some(n) = self.nav_mut(main) {
            n.default_button = panel;
        }
        self.nav_set_current_default_button(main, panel, true);
    }

    /// `FocusNavGroup::GetDefaultPanel`: the first child with tab position 1.
    pub fn nav_default_panel(&self, main: PanelId) -> Option<PanelId> {
        self.children(main).into_iter().find(|&c| self.tab_position(c) == 1)
    }

    /// `FocusNavGroup::FindPanelByHotkey(key)`: first child answering, visible and enabled.
    pub fn nav_find_panel_by_hotkey(&mut self, main: PanelId, key: char) -> Option<PanelId> {
        let key = key.to_lowercase().next().unwrap_or(key);
        self.children(main).into_iter().find(|&c| self.with_ctrl(c, |ctrl, _| ctrl.has_hotkey(key)).unwrap_or(false) && self.is_visible(c) && self.is_enabled(c))
    }

    /// `EditablePanel`'s message handling; `true` when handled.
    pub(crate) fn editable_default_message(&mut self, id: PanelId, msg: &Msg, _s: &mut dyn Surface) -> bool {
        if msg.is("KeyCodeTyped") {
            let code = msg.get_int("code");
            if code != KEY_ENTER && code != KEY_PAD_ENTER {
                return false;
            }
            match self.nav(id).current_default_button {
                Some(b) if self.is_visible(b) && self.is_enabled(b) => {
                    self.post(b, Msg::new("Hotkey"), Some(id), 0.0);
                    true
                }
                _ => false,
            }
        } else if msg.is("SetFocus") {
            let f = self.nav(id).current_focus;
            match f {
                Some(f) if f != id => self.request_focus(f),
                _ => {
                    if let Some(d) = self.nav_default_panel(id) {
                        self.request_focus(d);
                        self.post(d, Msg::new("SetFocus"), None, 0.0);
                    }
                }
            }
            true
        } else if msg.is("DefaultButtonSet") {
            let b = msg.get_panel("button");
            self.nav_set_default_button(id, b);
            true
        } else if msg.is("CurrentDefaultButtonSet") {
            let b = msg.get_panel("button");
            self.nav_set_current_default_button(id, b, false);
            if let (Some(p), Some(b)) = (self.parent(id), b) {
                self.post(p, Msg::new("CurrentDefaultButtonSet").with("button", Value::Panel(b)), Some(id), 0.0);
            }
            true
        } else if msg.is("FindDefaultButton") {
            if let Some(d) = self.nav(id).default_button {
                self.nav_set_current_default_button(id, Some(d), true);
            } else if let Some(p) = self.parent(id) {
                self.post(p, Msg::new("FindDefaultButton"), Some(id), 0.0);
            }
            true
        } else {
            false
        }
    }

    /// `EditablePanel::OnSizeChanged`: pin corners and auto-resize of the children.
    pub(crate) fn editable_size_changed(&mut self, id: PanelId, w: i32, h: i32) {
        self.invalidate_layout(id);
        let (bw, bh) = self.node(id).map_or((w, h), |n| n.base_size);
        let (dx, dy) = (w - bw, h - bh);
        for c in self.children(id) {
            let Some(n) = self.node(c) else { continue };
            let (pin, resize) = (n.pin_corner, n.auto_resize);
            let (mut x, mut y) = self.pos(c);
            let (mut cw, mut ct) = self.size(c);
            if pin == 1 || pin == 3 {
                x += dx;
            }
            if pin == 2 || pin == 3 {
                y += dy;
            }
            if resize == 1 || resize == 3 {
                cw += dx;
            }
            if resize == 2 || resize == 3 {
                ct += dy;
            }
            if x + cw > w {
                cw = w - x;
                if cw < 0 {
                    continue;
                }
            }
            if y + ct > h {
                ct = h - y;
                if ct < 0 {
                    continue;
                }
            }
            self.set_bounds(c, x, y, cw, ct);
            self.invalidate_layout(c);
        }
        if let Some(n) = self.node_mut(id) {
            n.base_size = (w, h);
        }
    }

    /// `Panel::ApplySettings`: position (`r`/`c` prefixes), size (always re-scaled), flags.
    pub fn panel_apply_settings(&mut self, id: PanelId, kv: &Block, s: &mut dyn Surface) {
        let (sw, sh) = self.screen;
        let (mut x, mut y) = self.pos(id);
        let coord = |ui: &Ui, v: &str, screen: i32| -> i32 {
            let mut chars = v.chars();
            match chars.next() {
                Some('r' | 'R') => screen - ui.psv(id, atoi(chars.as_str())),
                Some('c' | 'C') => screen / 2 + ui.psv(id, atoi(chars.as_str())),
                _ => ui.psv(id, atoi(v)),
            }
        };
        if let Some(v) = kv_str(kv, "xpos") {
            x = coord(self, v, sw);
        }
        if let Some(v) = kv_str(kv, "ypos") {
            y = coord(self, v, sh);
        }
        self.set_pos(id, x, y);
        if kv_str(kv, "zpos").is_some() {
            self.set_zpos(id, kv_int(kv, "zpos", 0));
        }
        let (w, h) = self.size(id);
        let w = self.psv(id, kv_int(kv, "wide", w));
        let h = self.psv(id, kv_int(kv, "tall", h));
        self.set_size(id, w, h);
        if let Some(n) = self.node_mut(id) {
            n.auto_resize = kv_int(kv, "AutoResize", 0);
            n.pin_corner = kv_int(kv, "PinCorner", 0);
        }
        if kv_int(kv, "IgnoreScheme", 0) != 0 {
            self.invalidate_layout_ex(id, false, true, s);
        }
        match kv_int(kv, "visible", 1) {
            0 => self.set_visible(id, false),
            1 => self.set_visible(id, true),
            _ => {}
        }
        self.set_enabled(id, kv_int(kv, "enabled", 1) != 0);
        let tab = kv_int(kv, "tabPosition", 0);
        if let Some(n) = self.node_mut(id) {
            n.tab_position = tab;
        }
        let pb = kv_int(kv, "paintbackground", -1);
        let pr = kv_int(kv, "paintborder", -1);
        let field = kv_str(kv, "fieldName").map(str::to_owned);
        if let Some(n) = self.node_mut(id) {
            if pb >= 0 {
                n.paint_background_enabled = pb != 0;
            }
            if pr >= 0 {
                n.paint_border_enabled = pr != 0;
            }
            if let Some(f) = field {
                n.name = f;
            }
        }
    }

    /// `EditablePanel::ApplySettings`: Panel's, then the build group, then the new base size.
    pub fn editable_apply_settings(&mut self, id: PanelId, kv: &Block, factory: &mut Factory<'_>, s: &mut dyn Surface) {
        self.panel_apply_settings(id, kv, s);
        self.build_group_apply_settings(id, kv, factory, s);
        let size = self.size(id);
        if let Some(n) = self.node_mut(id) {
            n.base_size = size;
        }
    }

    /// `BuildGroup::ApplySettings`: sub-blocks go to the build-group panel of that name, or
    /// create a control through the factory.
    pub fn build_group_apply_settings(&mut self, id: PanelId, kv: &Block, factory: &mut Factory<'_>, s: &mut dyn Surface) {
        for (name, value) in &kv.entries {
            let KvValue::Block(sub) = value else { continue };
            let group = self.node(id).map(|n| n.build_group.clone()).unwrap_or_default();
            let live: Vec<PanelId> = group.into_iter().filter(|&p| self.exists(p)).collect();
            if let Some(n) = self.node_mut(id) {
                n.build_group.clone_from(&live);
            }
            let found = live.into_iter().find(|&p| self.node(p).is_some_and(|n| n.name.eq_ignore_ascii_case(name)));
            if let Some(p) = found {
                // A panel loading its own layout from one of its methods (the loading dialog's
                // dual-progress, VAC, HTTP and error layouts) has its control taken out; its
                // block still applies, as Panel::ApplySettings (what Frame-derived dialogs run).
                if self.with_ctrl(p, |c, ui| c.apply_settings(ui, p, sub, s)).is_none() && self.exists(p) {
                    self.panel_apply_settings(p, sub, s);
                }
                continue;
            }
            let cls = kv_str(sub, "ControlName").unwrap_or("");
            if let Some(p) = factory(self, cls, s) {
                self.set_parent(p, Some(id));
                if let Some(n) = self.node_mut(id)
                    && !n.build_group.contains(&p)
                {
                    n.build_group.push(p);
                }
                self.set_pos(p, 0, 0);
                if let Some(n) = self.node_mut(p) {
                    n.name = name.clone();
                }
                self.with_ctrl(p, |c, ui| c.apply_settings(ui, p, sub, s));
                self.add_action_signal_target(p, id);
            }
        }
    }

    /// `EditablePanel::LoadControlSettings` on parsed `.res` text.
    pub fn load_control_settings(&mut self, id: PanelId, res_text: &str, factory: &mut Factory<'_>, s: &mut dyn Surface) {
        let root = gs_formats::keyvalues::parse(res_text);
        // The file's single root block (its name is the file name).
        let Some((_, kv)) = root.blocks().next() else { return };
        let kv = kv.clone();
        self.build_group_apply_settings(id, &kv, factory, s);
        self.invalidate_layout(id);
    }
}

/// The base `EditablePanel::CreateControlByName` for the classes implemented here.
pub fn create_control_by_name(ui: &mut Ui, name: &str, _s: &mut dyn Surface) -> Option<PanelId> {
    use crate::controls::misc::{Divider, ImagePanel, ProgressBar};
    use crate::controls::{button::Button, label::Label, menu::Menu, rich_text::RichText, scroll_bar::ScrollBar, text_entry::TextEntry};
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "label" => Some(Label::create(ui, None, "", "Label")),
        "button" => Some(Button::create(ui, None, "", "Button", None)),
        "panel" => Some(ui.create(None, "", Box::new(crate::ui::PlainPanel))),
        "editablepanel" => {
            let p = ui.create(None, "", Box::new(crate::ui::PlainPanel));
            ui.make_editable(p);
            Some(p)
        }
        "progressbar" => Some(ProgressBar::create(ui, None, "")),
        "divider" => Some(Divider::create(ui, None, "Divider")),
        "imagepanel" => Some(ImagePanel::create(ui, None, "ResourceImagePanel")),
        "textentry" => Some(TextEntry::create(ui, None, "")),
        "richtext" => Some(RichText::create(ui, None, "")),
        "menu" => Some(Menu::create(ui, None, "Menu")),
        "scrollbar" => Some(ScrollBar::create(ui, None, "", false)),
        "checkbutton" => Some(crate::controls::toggle::ToggleButton::create(ui, None, "", "CheckButton", crate::controls::toggle::ToggleKind::Check)),
        "radiobutton" => Some(crate::controls::toggle::ToggleButton::create(ui, None, "", "RadioButton", crate::controls::toggle::ToggleKind::Radio)),
        "togglebutton" => Some(crate::controls::toggle::ToggleButton::create(ui, None, "", "ToggleButton", crate::controls::toggle::ToggleKind::Toggle)),
        _ => None,
    }
}
