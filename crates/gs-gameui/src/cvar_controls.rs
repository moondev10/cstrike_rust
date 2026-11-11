//! GameUI's cvar-bound controls (docs/re/gameui-dialogs.md §2): `CCvarSlider`,
//! `CCvarToggleCheckButton`, `CCvarNegateCheckButton`, `CCvarTextEntry`,
//! `CLabeledCommandComboBox`. Each wraps the vgui control it derives from; the page calls the
//! non-virtual `apply_changes`/`reset` through the functions below.

use std::any::Any;

use gs_formats::keyvalues::Block;
use gs_vgui::controls::combo_box::ComboBox;
use gs_vgui::controls::menu::Menu;
use gs_vgui::controls::slider::{Slider, SliderState};
use gs_vgui::controls::text_entry::TextEntry;
use gs_vgui::controls::toggle::{ToggleButton, ToggleKind};
use gs_vgui::scheme::Scheme;
use gs_vgui::{Control, Msg, PanelId, Surface, Ui};

use crate::Shared;

/// The subclass part of a cvar control.
enum Kind {
    Slider(SliderExt),
    Check(CheckExt),
    Text(TextExt),
    Combo(ComboExt),
}

struct SliderExt {
    allow_oor: bool,
    modified_once: bool,
    start_f: f32,
    start_i: i32,
    last_i: i32,
    cur_f: f32,
    cvar: String,
}

struct CheckExt {
    cvar: Option<String>,
    start: bool,
    negate: bool,
}

struct TextExt {
    cvar: String,
    start: String,
}

struct ComboItem {
    name: String,
    command: String,
    id: i32,
}

struct ComboExt {
    items: Vec<ComboItem>,
    current: i32,
    start: i32,
}

/// A cvar control: the base vgui control plus the subclass state.
pub struct CvarControl {
    base: Box<dyn Control>,
    kind: Kind,
    host: Shared,
}

/// `(int)` truncation as `cvttss2si`.
fn trunc(f: f32) -> i32 {
    if (-2_147_483_648.0..2_147_483_648.0).contains(&f) { f as i32 } else { i32::MIN }
}

fn wrap(ui: &mut Ui, id: PanelId, kind: Kind, host: Shared) {
    ui.replace_ctrl(id, move |base| Box::new(CvarControl { base, kind, host }));
}

fn with_cvar<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut CvarControl, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<CvarControl>().map(|cc| f(cc, ui))).flatten()
}

fn slider_of(base: &mut Box<dyn Control>) -> Option<&mut SliderState> {
    base.as_any_mut().downcast_mut::<Slider>().map(|s| &mut s.st)
}

fn toggle_of(base: &mut Box<dyn Control>) -> Option<&mut ToggleButton> {
    base.as_any_mut().downcast_mut::<ToggleButton>()
}

fn entry_of(base: &mut Box<dyn Control>) -> Option<&mut TextEntry> {
    base.as_any_mut().downcast_mut::<TextEntry>()
}

/// The drop-down menu of a wrapped combo box.
fn combo_menu(ui: &mut Ui, id: PanelId) -> Option<PanelId> {
    with_cvar(ui, id, |c, _| c.base.as_any_mut().downcast_mut::<ComboBox>().map(|cb| cb.menu)).flatten()
}

/// `new CCvarSlider(parent, name, caption, min, max, cvar, allowOutOfRange)`.
#[allow(clippy::too_many_arguments)]
pub fn create_slider(ui: &mut Ui, parent: Option<PanelId>, name: &str, min: f32, max: f32, cvar: &str, allow_oor: bool, host: Shared) -> PanelId {
    let me = Slider::create(ui, parent, name);
    let v = host.borrow().cvar_float(cvar);
    let mut start_i = 0;
    ui.with_ctrl(me, |c, ui| {
        if let Some(sl) = c.as_any_mut().downcast_mut::<Slider>() {
            sl.st.set_range(trunc(min * 100.0), trunc(max * 100.0));
            sl.st.set_tick_captions(ui, me, Some(&format!("{min:.2}")), Some(&format!("{max:.2}")));
            sl.st.set_value(ui, me, trunc(v * 100.0), true);
            start_i = sl.st.value();
        }
    });
    let ext = SliderExt { allow_oor, modified_once: false, start_f: v, start_i, last_i: start_i, cur_f: v, cvar: cvar.to_owned() };
    wrap(ui, me, Kind::Slider(ext), host);
    me
}

/// `new CCvarToggleCheckButton(parent, name, text, cvar)`; `negate`: CCvarNegateCheckButton
/// (checked ⇔ cvar < 0).
#[allow(clippy::too_many_arguments)]
pub fn create_check(ui: &mut Ui, parent: Option<PanelId>, name: &str, text: &str, cvar: Option<&str>, negate: bool, host: Shared, s: &mut dyn Surface) -> PanelId {
    let me = ToggleButton::create(ui, parent, name, text, ToggleKind::Check);
    let mut start = false;
    if let Some(c) = cvar {
        let v = host.borrow().cvar_float(c);
        start = if negate { 0.0 > v } else { v > 0.0 };
        gs_vgui::controls::toggle::with_toggle(ui, me, |t, ui| t.set_selected(ui, me, start, s));
    }
    ui.add_action_signal_target(me, me);
    wrap(ui, me, Kind::Check(CheckExt { cvar: cvar.map(str::to_owned), start, negate }), host);
    me
}

/// `new CCvarTextEntry(parent, name, cvar)`.
pub fn create_text(ui: &mut Ui, parent: Option<PanelId>, name: &str, cvar: &str, host: Shared, s: &mut dyn Surface) -> PanelId {
    let me = TextEntry::create(ui, parent, name);
    let mut start = String::new();
    let value = host.borrow().cvar_string(cvar).map(str::to_owned);
    if let Some(v) = value.filter(|v| !v.is_empty()) {
        gs_vgui::controls::text_entry::with_te(ui, me, |te, ui| te.set_text(ui, me, &v, s));
        start = v.chars().take(63).collect();
    }
    ui.add_action_signal_target(me, me);
    wrap(ui, me, Kind::Text(TextExt { cvar: cvar.to_owned(), start }), host);
    me
}

/// `new CLabeledCommandComboBox(parent, name)`.
pub fn create_combo(ui: &mut Ui, parent: Option<PanelId>, name: &str, host: Shared) -> PanelId {
    let me = ComboBox::create(ui, parent, name, 6, false);
    ui.add_action_signal_target(me, me);
    wrap(ui, me, Kind::Combo(ComboExt { items: Vec::new(), current: -1, start: -1 }), host);
    me
}

/// `ApplyChanges` of any cvar control (`immediate` only matters for CCvarTextEntry).
pub fn apply_changes(ui: &mut Ui, id: PanelId, immediate: bool) {
    with_cvar(ui, id, |c, _| {
        let host = c.host.clone();
        match &mut c.kind {
            Kind::Slider(e) => {
                if !e.modified_once {
                    return;
                }
                e.start_i = slider_of(&mut c.base).map_or(0, |st| st.value());
                e.start_f = if e.allow_oor { e.cur_f } else { e.start_i as f32 / 100.0 };
                host.borrow_mut().cvar_set_value(&e.cvar, e.start_f);
            }
            Kind::Check(e) => {
                let sel = toggle_of(&mut c.base).is_some_and(|t| t.is_selected());
                let name = e.cvar.clone().unwrap_or_default();
                if e.negate {
                    if name.is_empty() {
                        return;
                    }
                    let mut a = host.borrow().cvar_float(&name).abs();
                    if f64::from(a) < 1e-5 {
                        a = 0.022;
                    }
                    e.start = sel;
                    host.borrow_mut().cvar_set_value(&name, if sel { -a } else { a });
                } else {
                    e.start = sel;
                    host.borrow_mut().cvar_set_value(&name, if sel { 1.0 } else { 0.0 });
                }
            }
            Kind::Text(e) => {
                let text = gs_common::bytestr::ansi_cap(&entry_of(&mut c.base).map(|te| te.text()).unwrap_or_default(), 64);
                if text.is_empty() {
                    return;
                }
                if immediate {
                    host.borrow_mut().cvar_set(&e.cvar, &text);
                } else {
                    host.borrow_mut().client_cmd(&format!("{} \"{}\"", e.cvar, text));
                }
                e.start = text;
            }
            Kind::Combo(e) => {
                if e.current == -1 || e.items.is_empty() {
                    return;
                }
                if let Some(it) = e.items.get(e.current as usize) {
                    host.borrow_mut().client_cmd(&it.command);
                }
                e.start = e.current;
            }
        }
    });
}

/// `Reset` of any cvar control.
pub fn reset(ui: &mut Ui, id: PanelId, s: &mut dyn Surface) {
    let mut activate = None;
    with_cvar(ui, id, |c, ui| {
        let host = c.host.clone();
        match &mut c.kind {
            Kind::Slider(e) => {
                let v = host.borrow().cvar_float(&e.cvar);
                e.start_f = v;
                e.cur_f = v;
                if let Some(st) = slider_of(&mut c.base) {
                    st.set_value(ui, id, trunc(v * 100.0), true);
                    e.start_i = st.value();
                }
                e.last_i = e.start_i;
            }
            Kind::Check(e) => {
                let name = e.cvar.clone().unwrap_or_default();
                let v = host.borrow().cvar_float(&name);
                let b = if e.negate { 0.0 > v } else { v > 0.0 };
                e.start = b;
                if let Some(t) = toggle_of(&mut c.base) {
                    t.set_selected(ui, id, b, s);
                }
            }
            Kind::Text(_) => {}
            Kind::Combo(e) => {
                if e.start != -1 {
                    activate = Some(e.start);
                }
            }
        }
    });
    if let Some(i) = activate {
        combo_activate_item(ui, id, i, s);
    }
}

/// `CCvarSlider::GetSliderValue`.
pub fn slider_value(ui: &mut Ui, id: PanelId) -> f32 {
    with_cvar(ui, id, |c, _| {
        let v = slider_of(&mut c.base).map_or(0, |st| st.value());
        match &c.kind {
            Kind::Slider(e) if e.allow_oor => e.cur_f,
            Kind::Slider(_) => v as f32 / 100.0,
            _ => 0.0,
        }
    })
    .unwrap_or(0.0)
}

/// `CCvarSlider::SetSliderValue(f)`.
pub fn set_slider_value(ui: &mut Ui, id: PanelId, f: f32) {
    with_cvar(ui, id, |c, ui| {
        if let Kind::Slider(e) = &mut c.kind
            && let Some(st) = slider_of(&mut c.base)
        {
            st.set_value(ui, id, trunc(f * 100.0), false);
            e.last_i = st.value();
            if e.cur_f != f {
                e.cur_f = f;
                e.modified_once = true;
            }
        }
    });
}

/// `CCvarSlider::HasBeenModified`.
pub fn has_been_modified(ui: &mut Ui, id: PanelId) -> bool {
    with_cvar(ui, id, |c, _| {
        let v = slider_of(&mut c.base).map_or(0, |st| st.value());
        match &mut c.kind {
            Kind::Slider(e) => {
                if v != e.start_i {
                    e.modified_once = true;
                }
                e.modified_once
            }
            _ => false,
        }
    })
    .unwrap_or(false)
}

/// `CLabeledCommandComboBox::AddItem(text, command)`: the item's name is the localized text.
pub fn combo_add_item(ui: &mut Ui, id: PanelId, text: &str, command: &str) {
    let Some(menu) = combo_menu(ui, id) else { return };
    let cid = Menu::add_item_msg(ui, menu, text, text, Msg::new("SetText").str("text", text), Some(id), None);
    let name = match text.strip_prefix('#').and_then(|k| ui.localize.find(k)) {
        Some(w) => w.to_owned(),
        None => text.to_owned(),
    };
    with_cvar(ui, id, |c, _| {
        if let Kind::Combo(e) = &mut c.kind {
            e.items.push(ComboItem { name, command: command.to_owned(), id: cid });
        }
    });
}

/// `CLabeledCommandComboBox::ActivateItem(index)`.
pub fn combo_activate_item(ui: &mut Ui, id: PanelId, index: i32, s: &mut dyn Surface) {
    let cid = with_cvar(ui, id, |c, _| match &mut c.kind {
        Kind::Combo(e) if (0..e.items.len() as i32).contains(&index) => {
            e.current = index;
            Some(e.items[index as usize].id)
        }
        _ => None,
    })
    .flatten();
    if let (Some(cid), Some(menu)) = (cid, combo_menu(ui, id)) {
        Menu::activate_item(ui, menu, cid, s);
    }
}

/// `CLabeledCommandComboBox::SetInitialItem(index)`.
pub fn combo_set_initial_item(ui: &mut Ui, id: PanelId, index: i32, s: &mut dyn Surface) {
    let ok = with_cvar(ui, id, |c, _| match &mut c.kind {
        Kind::Combo(e) if (0..e.items.len() as i32).contains(&index) => {
            e.start = index;
            true
        }
        _ => false,
    })
    .unwrap_or(false);
    if ok {
        combo_activate_item(ui, id, index, s);
    }
}

/// `CLabeledCommandComboBox::GetActiveItemCommand`.
pub fn combo_active_command(ui: &mut Ui, id: PanelId) -> Option<String> {
    with_cvar(ui, id, |c, _| match &c.kind {
        Kind::Combo(e) if e.current >= 0 => e.items.get(e.current as usize).map(|i| i.command.clone()),
        _ => None,
    })
    .flatten()
}

/// `CLabeledCommandComboBox::HasItem(name)` (exact compare).
pub fn combo_has_item(ui: &mut Ui, id: PanelId, name: &str) -> bool {
    with_cvar(ui, id, |c, _| matches!(&c.kind, Kind::Combo(e) if e.items.iter().any(|i| i.name == name))).unwrap_or(false)
}

/// `CLabeledCommandComboBox::DeleteAllItems`.
pub fn combo_delete_all(ui: &mut Ui, id: PanelId) {
    if let Some(menu) = combo_menu(ui, id) {
        Menu::delete_all_items(ui, menu);
    }
    with_cvar(ui, id, |c, _| {
        if let Kind::Combo(e) = &mut c.kind {
            e.items.clear();
        }
    });
}

/// `GetText` of a wrapped combo box.
pub fn combo_text(ui: &mut Ui, id: PanelId) -> String {
    with_cvar(ui, id, |c, _| c.base.as_any_mut().downcast_mut::<ComboBox>().map(|cb| cb.te.text())).flatten().unwrap_or_default()
}

/// `ComboBox::GetActiveItem` of a wrapped combo box (the menu's active item ID).
pub fn combo_active_item(ui: &mut Ui, id: PanelId) -> i32 {
    combo_menu(ui, id).map_or(0, |m| Menu::active_item(ui, m))
}

/// `IsSelected` of a cvar check button.
pub fn check_selected(ui: &mut Ui, id: PanelId) -> bool {
    with_cvar(ui, id, |c, _| toggle_of(&mut c.base).is_some_and(|t| t.is_selected())).unwrap_or(false)
}

/// `SetEnabled`-independent text of a cvar text entry (`GetText`).
pub fn entry_text(ui: &mut Ui, id: PanelId) -> String {
    with_cvar(ui, id, |c, _| entry_of(&mut c.base).map(|te| te.text())).flatten().unwrap_or_default()
}

/// `Slider::GetValue` of a cvar slider.
pub fn slider_position(ui: &mut Ui, id: PanelId) -> i32 {
    with_cvar(ui, id, |c, _| slider_of(&mut c.base).map_or(0, |st| st.value())).unwrap_or(0)
}

/// `Slider::SetValue(v, true)` of a cvar slider.
pub fn set_slider_position(ui: &mut Ui, id: PanelId, v: i32) {
    with_cvar(ui, id, |c, ui| {
        if let Some(st) = slider_of(&mut c.base) {
            st.set_value(ui, id, v, true);
        }
    });
}

impl CvarControl {
    /// The wrapped vgui control.
    pub fn base_mut(&mut self) -> &mut dyn Control {
        self.base.as_mut()
    }

    /// `CCvarSlider::OnSliderMoved`.
    fn slider_moved(e: &mut SliderExt, v: i32, ui: &mut Ui, me: PanelId) {
        if v != e.start_i {
            e.modified_once = true;
        } else if !e.modified_once {
            return;
        }
        if v != e.last_i {
            e.last_i = v;
            e.cur_f = e.last_i as f32 / 100.0;
            ui.post_action_signal(me, Msg::new("ControlModified"));
        }
    }
}

impl Control for CvarControl {
    fn class(&self) -> &'static str {
        match &self.kind {
            Kind::Slider(_) => "CCvarSlider",
            Kind::Check(e) if e.negate => "CCvarNegateCheckButton",
            Kind::Check(_) => "CCvarToggleCheckButton",
            Kind::Text(_) => "CCvarTextEntry",
            Kind::Combo(_) => "CLabeledCommandComboBox",
        }
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        match &mut self.kind {
            Kind::Slider(e) if msg.is("SliderMoved") => {
                let v = slider_of(&mut self.base).map_or(0, |st| st.value());
                Self::slider_moved(e, v, ui, me);
                return true;
            }
            Kind::Check(e) if msg.is("CheckButtonChecked") => {
                let sel = toggle_of(&mut self.base).is_some_and(|t| t.is_selected());
                if sel != e.start {
                    ui.post_action_signal(me, Msg::new("ControlModified"));
                }
                return true;
            }
            Kind::Text(e) if msg.is("TextChanged") => {
                let text = gs_common::bytestr::ansi_cap(&entry_of(&mut self.base).map(|te| te.text()).unwrap_or_default(), 64);
                if !text.eq_ignore_ascii_case(&e.start) {
                    ui.post_action_signal(me, Msg::new("ControlModified"));
                }
                return true;
            }
            Kind::Combo(e) if msg.is("TextChanged") => {
                let text = msg.get_str("text");
                if let Some(i) = e.items.iter().position(|it| it.name.eq_ignore_ascii_case(&text)) {
                    e.current = i as i32;
                }
                if e.start != e.current {
                    ui.post_action_signal(me, Msg::new("ControlModified"));
                }
                return true;
            }
            _ => {}
        }
        self.base.on_message(ui, me, msg, from, s)
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.base.apply_scheme(ui, me, scheme, s);
        if matches!(self.kind, Kind::Text(_))
            && let Some(te) = entry_of(&mut self.base)
        {
            let n = te.maximum_char_count();
            if !(0..=64).contains(&n) {
                te.set_maximum_char_count(63);
            }
        }
    }
    fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &Block, s: &mut dyn Surface) {
        self.base.apply_settings(ui, me, kv, s);
    }
    fn think(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.base.think(ui, me, s);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.base.perform_layout(ui, me, s);
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.base.paint_background(ui, me, s);
    }
    /// The slider and the check buttons re-sync from the cvar every paint (exact compare).
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let host = self.host.clone();
        match &mut self.kind {
            Kind::Slider(e) => {
                let v = host.borrow().cvar_float(&e.cvar);
                if v != e.start_f {
                    e.start_f = v;
                    e.cur_f = v;
                    if let Some(st) = slider_of(&mut self.base) {
                        st.set_value(ui, me, trunc(v * 100.0), true);
                        e.start_i = st.value();
                    }
                }
            }
            Kind::Check(e) => {
                if let Some(name) = e.cvar.as_deref().filter(|n| !n.is_empty()) {
                    let v = host.borrow().cvar_float(name);
                    let b = if e.negate { v < 0.0 } else { v > 0.0 };
                    if b != e.start {
                        e.start = b;
                        if let Some(t) = toggle_of(&mut self.base) {
                            t.set_selected(ui, me, b, s);
                        }
                    }
                }
            }
            _ => {}
        }
        self.base.paint(ui, me, s);
    }
    fn paint_border(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.base.paint_border(ui, me, s);
    }
    fn post_child_paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.base.post_child_paint(ui, me, s);
    }
    fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId, w: i32, h: i32) {
        self.base.on_size_changed(ui, me, w, h);
    }
    fn can_be_default_button(&self) -> bool {
        self.base.can_be_default_button()
    }
    fn has_hotkey(&self, key: char) -> bool {
        self.base.has_hotkey(key)
    }
    fn request_focus_override(&mut self, ui: &mut Ui, me: PanelId, dir: i32) -> bool {
        self.base.request_focus_override(ui, me, dir)
    }
    fn request_focus_next(&mut self, ui: &mut Ui, me: PanelId, from: Option<PanelId>) -> Option<bool> {
        self.base.request_focus_next(ui, me, from)
    }
    fn request_focus_prev(&mut self, ui: &mut Ui, me: PanelId, from: Option<PanelId>) -> Option<bool> {
        self.base.request_focus_prev(ui, me, from)
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
