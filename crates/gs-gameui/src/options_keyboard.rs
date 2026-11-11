//! Keyboard page — `COptionsSubKeyboard`, `VControlsListPanel`, `CInlineEditPanel`
//! (docs/re/gameui-dialogs.md §5).

use std::any::Any;

use gs_formats::keyvalues::Block;
use gs_vgui::controls::button::Button;
use gs_vgui::controls::message_box::MessageBox;
use gs_vgui::controls::property::{NoHooks, PageHooks, PropertyPage};
use gs_vgui::controls::sectioned::{COLUMN_BRIGHT, SectionedListPanel, with_sectioned};
use gs_vgui::keys::*;
use gs_vgui::scheme::Scheme;
use gs_vgui::{Color, Control, Msg, PanelId, Surface, Ui};

use crate::options::{load_res, set_page_hooks};
use crate::{Fs, Shared};

// ---------------------------------------------------------------------------------------------
// Key tables

/// `EngineKeyToSDLScancode@0x10033460`.
fn engine_key_to_scancode(k: i32) -> i32 {
    match k {
        9 => 43,
        13 => 40,
        27 => 41,
        32 => 44,
        39 => 52,
        44 => 54,
        45 => 45,
        46 => 55,
        47 => 56,
        48 => 39,
        49..=57 => k - 49 + 30,
        59 => 51,
        61 => 46,
        91 => 47,
        92 => 49,
        93 => 48,
        96 => 53,
        97..=122 => k - 97 + 4,
        127 => 42,
        128 => 82,
        129 => 81,
        130 => 80,
        131 => 79,
        132 => 226,
        133 => 224,
        134 => 225,
        135..=146 => k - 135 + 58,
        147 => 73,
        148 => 76,
        149 => 78,
        150 => 75,
        151 => 74,
        152 => 77,
        160 => 95,
        161 => 96,
        162 => 97,
        163 => 92,
        164 => 93,
        165 => 94,
        166 => 89,
        167 => 90,
        168 => 91,
        169 => 88,
        170 => 98,
        171 => 220,
        172 => 84,
        173 => 86,
        174 => 87,
        175 => 57,
        176 => 85,
        177 => 227,
        255 => 72,
        _ => 0,
    }
}

/// `SDLScancodeToEngineKey@0x100349C0`: the inverse plus the right-hand modifiers and keypad `.`.
fn scancode_to_engine_key(sc: i32) -> i32 {
    match sc {
        99 => 171,
        228 => 133,
        229 => 134,
        230 => 132,
        231 => 177,
        50 | 70 | 71 | 83 => 0,
        _ => (0..256).find(|&k| k != 0 && engine_key_to_scancode(k) == sc).unwrap_or(0),
    }
}

/// `SDL_GetKeyName(SDL_GetKeyFromScancode(sc))` (SDL2, US layout).
fn sdl_key_name(sc: i32) -> String {
    let fixed = match sc {
        40 => "Return",
        41 => "Escape",
        42 => "Backspace",
        43 => "Tab",
        44 => "Space",
        45 => "-",
        46 => "=",
        47 => "[",
        48 => "]",
        49 => "\\",
        51 => ";",
        52 => "'",
        53 => "`",
        54 => ",",
        55 => ".",
        56 => "/",
        57 => "CapsLock",
        70 => "PrintScreen",
        71 => "ScrollLock",
        72 => "Pause",
        73 => "Insert",
        74 => "Home",
        75 => "PageUp",
        76 => "Delete",
        77 => "End",
        78 => "PageDown",
        79 => "Right",
        80 => "Left",
        81 => "Down",
        82 => "Up",
        83 => "Numlock",
        84 => "Keypad /",
        85 => "Keypad *",
        86 => "Keypad -",
        87 => "Keypad +",
        88 => "Keypad Enter",
        98 => "Keypad 0",
        99 => "Keypad .",
        101 => "Application",
        220 => "Keypad Decimal",
        224 => "Left Ctrl",
        225 => "Left Shift",
        226 => "Left Alt",
        227 => "Left GUI",
        228 => "Right Ctrl",
        229 => "Right Shift",
        230 => "Right Alt",
        231 => "Right GUI",
        _ => "",
    };
    match sc {
        4..=29 => char::from(b'A' + (sc - 4) as u8).to_string(),
        30..=38 => char::from(b'1' + (sc - 30) as u8).to_string(),
        39 => "0".to_owned(),
        58..=69 => format!("F{}", sc - 57),
        89..=97 => format!("Keypad {}", sc - 88),
        _ => fixed.to_owned(),
    }
}

/// `ISystem` KeyCode → SDL key, as a scancode (US layout).
fn vgui_to_scancode(code: i32) -> i32 {
    match code {
        KEY_0 => 39,
        c if (KEY_0 + 1..=KEY_0 + 9).contains(&c) => c - (KEY_0 + 1) + 30,
        c if (KEY_A..=KEY_Z).contains(&c) => c - KEY_A + 4,
        KEY_PAD_0 => 98,
        c if (KEY_PAD_0 + 1..=KEY_PAD_0 + 9).contains(&c) => c - (KEY_PAD_0 + 1) + 89,
        KEY_PAD_DIVIDE => 84,
        KEY_PAD_MULTIPLY => 85,
        KEY_PAD_MINUS => 86,
        KEY_PAD_PLUS => 87,
        KEY_PAD_ENTER => 88,
        KEY_PAD_DECIMAL => 99,
        KEY_LBRACKET => 47,
        KEY_RBRACKET => 48,
        KEY_SEMICOLON => 51,
        KEY_APOSTROPHE => 52,
        KEY_BACKQUOTE => 53,
        KEY_COMMA => 54,
        KEY_PERIOD => 55,
        KEY_SLASH => 56,
        KEY_BACKSLASH => 49,
        KEY_MINUS => 45,
        KEY_EQUAL => 46,
        KEY_ENTER => 40,
        KEY_SPACE => 44,
        KEY_BACKSPACE => 42,
        KEY_TAB => 43,
        KEY_CAPSLOCK => 57,
        KEY_NUMLOCK => 83,
        KEY_ESCAPE => 41,
        KEY_SCROLLLOCK => 71,
        KEY_INSERT => 73,
        KEY_DELETE => 76,
        KEY_HOME => 74,
        KEY_END => 77,
        KEY_PAGEUP => 75,
        KEY_PAGEDOWN => 78,
        KEY_BREAK => 72,
        KEY_LSHIFT => 225,
        KEY_RSHIFT => 229,
        KEY_LALT => 226,
        KEY_RALT => 230,
        KEY_LCONTROL => 224,
        KEY_RCONTROL => 228,
        KEY_LWIN => 227,
        KEY_RWIN => 231,
        KEY_APP => 101,
        KEY_UP => 82,
        KEY_LEFT => 80,
        KEY_DOWN => 81,
        KEY_RIGHT => 79,
        c if (KEY_F1..=KEY_F1 + 11).contains(&c) => c - KEY_F1 + 58,
        _ => 0,
    }
}

/// The display string of engine key `k`: MOUSE1..5, the wheel names (ResetData), else SDL's.
fn key_display(k: i32, wheel_names: bool) -> String {
    match k {
        241..=245 => format!("MOUSE{}", k - 240),
        240 if wheel_names => "Mouse Wheel Up".to_owned(),
        239 if wheel_names => "Mouse Wheel Down".to_owned(),
        _ => sdl_key_name(engine_key_to_scancode(k)),
    }
}

/// `Key_NameForKey`.
fn key_name(k: i32) -> String {
    u8::try_from(k).map(gs_console::keys::key_name).unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// CInlineEditPanel

#[derive(Debug, Default)]
struct InlineEditPanel;

impl Control for InlineEditPanel {
    fn class(&self) -> &'static str {
        "CInlineEditPanel"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("KeyCodeTyped") || msg.is("MousePressed") {
            ui.call_parent(me, msg, s);
            return true;
        }
        false
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, _s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let b = scheme.border("DepressedButtonBorder");
        if let Some(n) = ui.node_mut(me) {
            n.border = b;
        }
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let (w, t) = ui.size(me);
        s.draw_set_color(Color::rgba(149, 136, 49, 255));
        s.draw_filled_rect(0, 0, w, t);
        s.draw_set_color(Color::rgba(45, 49, 40, 255));
        s.draw_filled_rect(-3, -3, w + 2, -2);
        s.draw_filled_rect(-2, -2, -3, t + 3);
        s.draw_set_color(Color::rgba(150, 159, 142, 255));
        s.draw_filled_rect(-2, t + 3, w + 3, t + 2);
        s.draw_filled_rect(w + 3, -3, w + 2, t + 2);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// VControlsListPanel

struct ControlsList {
    list: SectionedListPanel,
    capturing: bool,
    click_row: i32,
    inline: PanelId,
}

fn with_controls<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut ControlsList, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<ControlsList>().map(|l| f(l, ui))).flatten()
}

impl ControlsList {
    fn create(ui: &mut Ui, parent: PanelId, name: &str) -> PanelId {
        let me = ui.create(Some(parent), name, Box::new(InlineEditPanel));
        let list = SectionedListPanel::new(ui, me);
        let inline = ui.create(None, "InlineEditPanel", Box::new(InlineEditPanel));
        ui.replace_ctrl(me, move |_| Box::new(ControlsList { list, capturing: false, click_row: 0, inline }));
        me
    }

    /// `StartCaptureMode(cursor)`.
    fn start_capture(&mut self, ui: &mut Ui, me: PanelId, cursor: i32) {
        self.capturing = true;
        let (row, inline) = (self.click_row, self.inline);
        self.list.enter_edit_mode(ui, me, row, 1, inline);
        ui.set_mouse_capture(Some(inline));
        if cursor == 1
            && let Some(n) = ui.node_mut(inline)
        {
            n.cursor = gs_vgui::Cursor::None;
        }
    }

    /// `EndCaptureMode(cursor)`.
    fn end_capture(&mut self, ui: &mut Ui, me: PanelId) {
        self.capturing = false;
        ui.set_mouse_capture(None);
        self.list.leave_edit_mode(ui);
        ui.request_focus(me);
        if let Some(n) = ui.node_mut(self.inline) {
            n.cursor = gs_vgui::Cursor::Arrow;
        }
    }
}

impl Control for ControlsList {
    fn class(&self) -> &'static str {
        "VControlsListPanel"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("MousePressed") && self.capturing {
            ui.call_parent(me, msg, s);
            return true;
        }
        if msg.is("MouseDoublePressed") {
            let sel = self.list.selected_item(ui);
            if self.list.is_item_id_valid(sel) {
                ui.call_parent(me, &Msg::new("KeyCodePressed").int("code", KEY_ENTER), s);
            }
            return true;
        }
        self.list.on_message(ui, me, msg, s)
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, _s: &mut dyn Surface) {
        self.list.apply_scheme(ui, me, scheme);
    }
    fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &Block, s: &mut dyn Surface) {
        self.list.apply_settings(ui, me, kv, s);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.list.perform_layout(ui, me, s);
    }
    fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId, _w: i32, _h: i32) {
        self.list.on_size_changed(ui, me);
    }
    fn sectioned(&mut self) -> Option<&mut SectionedListPanel> {
        Some(&mut self.list)
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// COptionsSubKeyboard

struct KeyboardPage {
    host: Shared,
    fs: Fs,
    list: PanelId,
    change_key: PanelId,
    clear_key: PanelId,
    bindings: Vec<Option<String>>,
    keys_to_unbind: Vec<String>,
}

pub fn create_keyboard_page(ui: &mut Ui, dlg: PanelId, host: &Shared, fs: &Fs, s: &mut dyn Surface) -> PanelId {
    let page = PropertyPage::create(ui, Some(dlg), "", true, Box::new(NoHooks));
    let list = ControlsList::create(ui, page, "listpanel_keybindlist");
    let bindings: Vec<Option<String>> = (0..256).map(|i| host.borrow().bindings.get(i).filter(|b| !b.is_empty()).cloned()).collect();
    let mut kb = KeyboardPage { host: host.clone(), fs: fs.clone(), list, change_key: list, clear_key: list, bindings, keys_to_unbind: Vec::new() };
    kb.parse_action_descriptions(ui);
    kb.change_key = Button::create(ui, Some(page), "ChangeKeyButton", "", None);
    kb.clear_key = Button::create(ui, Some(page), "ClearKeyButton", "", None);
    load_res(ui, page, fs.as_ref(), "Resource/OptionsSubKeyboard.res", s);
    ui.set_enabled(kb.change_key, false);
    ui.set_enabled(kb.clear_key, false);
    set_page_hooks(ui, page, Box::new(kb));
    page
}

/// `engine.COM_ParseFile` tokens of a text file.
fn tokens(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut p = data;
    while let Some((t, rest)) = gs_common::token::com_parse(p, false) {
        out.push(String::from_utf8_lossy(&t).into_owned());
        p = rest;
    }
    out
}

impl KeyboardPage {
    /// `ParseActionDescriptions`: `gfx/shell/kb_act.lst`.
    fn parse_action_descriptions(&mut self, ui: &mut Ui) {
        let Some(data) = self.fs.read("gfx/shell/kb_act.lst") else { return };
        let toks = tokens(&data);
        let list = self.list;
        let mut section = 0;
        let mut it = toks.into_iter();
        let w226 = ui.psv(list, 226);
        let w128 = ui.psv(list, 128);
        while let Some(binding) = it.next().filter(|t| !t.is_empty()) {
            let Some(desc) = it.next().filter(|t| !t.is_empty()) else { break };
            if desc.starts_with('=') {
                continue;
            }
            with_sectioned(ui, list, |l, ui| {
                if binding.eq_ignore_ascii_case("blank") {
                    section += 1;
                    l.add_section(ui, list, section, &desc, None);
                    l.add_column(ui, section, "Action", &desc, COLUMN_BRIGHT, w226);
                    l.add_column(ui, section, "Code", "#GameUI_KeyButton", COLUMN_BRIGHT, w128);
                    l.add_column(ui, section, "AltCode", "#GameUI_Alternate", COLUMN_BRIGHT, w128);
                } else {
                    let kv = Msg::new("Item").str("Action", &desc).str("Binding", &binding).str("Key", "").str("AltKey", "").str("Code", "").str("AltCode", "");
                    l.add_item(ui, list, section, &kv);
                }
            });
        }
    }

    fn rows(&self, ui: &mut Ui) -> Vec<(i32, Msg)> {
        let list = self.list;
        with_sectioned(ui, list, |l, ui| (0..l.item_count()).filter_map(|r| {
            let id = l.item_id_from_row(ui, r);
            l.item_data(ui, id).map(|d| (id, d))
        }).collect()).unwrap_or_default()
    }

    /// `GetItemForBinding`.
    fn item_for_binding(&self, ui: &mut Ui, binding: &str) -> Option<i32> {
        self.rows(ui).into_iter().find(|(_, d)| d.get_str("Binding").eq_ignore_ascii_case(binding)).map(|(id, _)| id)
    }

    fn set(&self, ui: &mut Ui, id: i32, key: &str, value: &str) {
        let list = self.list;
        with_sectioned(ui, list, |l, ui| l.set_item_value(ui, id, key, value));
    }

    fn get(&self, ui: &mut Ui, id: i32, key: &str) -> String {
        let list = self.list;
        with_sectioned(ui, list, |l, ui| l.item_data(ui, id)).flatten().map(|d| d.get_str(key)).unwrap_or_default()
    }

    fn invalidate_item(&self, ui: &mut Ui, id: i32) {
        let list = self.list;
        with_sectioned(ui, list, |l, ui| l.invalidate_item(ui, id));
    }

    /// `AddBinding(item, key, code)`.
    fn add_binding(&self, ui: &mut Ui, id: i32, key: &str, code: &str) {
        if !self.get(ui, id, "Key").eq_ignore_ascii_case(key) {
            self.remove_key_from_bind_items(ui, key);
            let (k, c) = (self.get(ui, id, "Key"), self.get(ui, id, "Code"));
            self.set(ui, id, "AltKey", &k);
            self.set(ui, id, "Key", key);
            self.set(ui, id, "AltCode", &c);
            self.set(ui, id, "Code", code);
        }
    }

    /// `RemoveKeyFromBindItems(key)`.
    fn remove_key_from_bind_items(&self, ui: &mut Ui, key: &str) {
        if key.is_empty() {
            return;
        }
        for (id, d) in self.rows(ui) {
            if key.eq_ignore_ascii_case(&d.get_str("AltKey")) {
                self.set(ui, id, "AltKey", "");
                self.set(ui, id, "AltCode", "");
                self.invalidate_item(ui, id);
            }
            if key.eq_ignore_ascii_case(&self.get(ui, id, "Key")) {
                self.set(ui, id, "Key", "");
                self.set(ui, id, "Code", "");
                self.invalidate_item(ui, id);
                let alt = self.get(ui, id, "AltKey");
                if !alt.is_empty() {
                    let ac = self.get(ui, id, "AltCode");
                    self.set(ui, id, "Key", &alt);
                    self.set(ui, id, "Code", &ac);
                    self.set(ui, id, "AltKey", "");
                    self.set(ui, id, "AltCode", "");
                }
            }
        }
        ui.invalidate_layout(self.list);
    }

    /// `ClearBindItems`.
    fn clear_bind_items(&mut self, ui: &mut Ui) {
        self.keys_to_unbind.clear();
        for (id, _) in self.rows(ui) {
            for k in ["Key", "AltKey", "Code", "AltCode"] {
                self.set(ui, id, k, "");
            }
            self.invalidate_item(ui, id);
        }
        ui.invalidate_layout(self.list);
    }

    /// `FindKeyByName(name)`.
    fn find_key_by_name(name: &str) -> i32 {
        (0..256)
            .find(|&i| {
                let n = key_name(i);
                !n.is_empty() && if name.len() == 1 { n == name } else { n.eq_ignore_ascii_case(name) }
            })
            .unwrap_or(-1)
    }

    /// `FillInDefaultBindings`: `gfx/shell/kb_def.lst`.
    fn fill_in_default_bindings(&mut self, ui: &mut Ui, me: PanelId) {
        let Some(data) = self.fs.read("gfx/shell/kb_def.lst") else { return };
        self.clear_bind_items(ui);
        let mut it = tokens(&data).into_iter();
        while let Some(key) = it.next().filter(|t| !t.is_empty()) {
            let Some(binding) = it.next().filter(|t| !t.is_empty()) else { break };
            let Some(item) = self.item_for_binding(ui, &binding) else { continue };
            let k = gs_console::keys::key_number(&key).map_or(-1, i32::from);
            let code = key_display(k, false);
            self.add_binding(ui, item, &key, &code);
        }
        ui.post_action_signal(me, Msg::new("ApplyButtonEnable"));
        if let Some(item) = self.item_for_binding(ui, "toggleconsole") {
            self.add_binding(ui, item, "`", "`");
        }
        if let Some(item) = self.item_for_binding(ui, "cancelselect") {
            self.add_binding(ui, item, "ESCAPE", "ESCAPE");
        }
    }

    /// `Finish(engineKey, mouseMask, code)`: the end of a key capture.
    fn finish(&mut self, ui: &mut Ui, me: PanelId, engine_key: i32, mouse: i32, code: &str) {
        let list = self.list;
        let id = with_controls(ui, list, |l, ui| {
            let id = l.click_row;
            l.end_capture(ui, list);
            id
        })
        .unwrap_or(-1);
        let has = with_sectioned(ui, list, |l, ui| l.item_data(ui, id)).flatten().is_some();
        if has {
            if mouse != 0 {
                let n = (0..5).find(|b| mouse & (1 << b) != 0).map_or(1, |b| b + 1);
                self.add_binding(ui, id, &format!("MOUSE{n}"), code);
                ui.post_action_signal(me, Msg::new("ApplyButtonEnable"));
            } else if engine_key != 0 && engine_key != 27 {
                self.add_binding(ui, id, &key_name(engine_key), code);
                ui.post_action_signal(me, Msg::new("ApplyButtonEnable"));
            }
            self.invalidate_item(ui, id);
        }
        ui.set_enabled(self.change_key, true);
        ui.set_enabled(self.clear_key, true);
    }

    fn capturing(&self, ui: &mut Ui) -> bool {
        with_controls(ui, self.list, |l, _| l.capturing).unwrap_or(false)
    }
}

impl PageHooks for KeyboardPage {
    /// `OnResetData`.
    fn on_reset_data(&mut self, ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        self.clear_bind_items(ui);
        let bindings = self.host.borrow().bindings.clone();
        for k in 0..256 {
            let Some(b) = bindings.get(k as usize).filter(|b| !b.is_empty()) else { continue };
            let code = key_display(k, true);
            let Some(item) = self.item_for_binding(ui, b) else { continue };
            let name = key_name(k);
            self.add_binding(ui, item, &name, &code);
            self.keys_to_unbind.push(name);
        }
        let list = self.list;
        with_sectioned(ui, list, |l, ui| l.set_selected_item_id(ui, list, 0));
    }

    /// `ApplyAllBindings`.
    fn on_apply_changes(&mut self, ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        let mut host = self.host.borrow_mut();
        for k in self.keys_to_unbind.drain(..) {
            host.client_cmd(&format!("unbind \"{k}\"\n"));
        }
        drop(host);
        self.bindings.iter_mut().for_each(|b| *b = None);
        for (_, d) in self.rows(ui) {
            let b = d.get_str("Binding");
            if b.is_empty() {
                continue;
            }
            for key in [d.get_str("Key"), d.get_str("AltKey")] {
                if !key.is_empty() {
                    self.host.borrow_mut().client_cmd(&format!("bind \"{key}\" \"{b}\"\n"));
                    let i = Self::find_key_by_name(&key);
                    if i != -1 {
                        self.bindings[i as usize] = Some(b.clone());
                    }
                }
            }
        }
        self.host.borrow_mut().client_cmd("exec userconfig.cfg\n");
    }

    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) -> bool {
        let list = self.list;
        if msg.is("ItemSelected") {
            let id = msg.get_int("itemID");
            with_controls(ui, list, |l, _| l.click_row = id);
            let valid = with_sectioned(ui, list, |l, _| l.is_item_id_valid(id)).unwrap_or(false);
            if valid {
                ui.set_enabled(self.change_key, true);
                if let Some(d) = with_sectioned(ui, list, |l, ui| l.item_data(ui, id)).flatten() {
                    ui.set_enabled(self.clear_key, !d.get_str("Key").is_empty());
                    if d.get_int("Header") != 0 {
                        ui.set_enabled(self.change_key, false);
                    }
                }
            } else {
                ui.set_enabled(self.change_key, false);
                ui.set_enabled(self.clear_key, false);
            }
            return true;
        }
        if msg.is("Command") {
            let cmd = msg.get_str("command");
            let capturing = self.capturing(ui);
            if cmd.eq_ignore_ascii_case("Defaults") {
                let qb = MessageBox::create_query(ui, "#GameUI_KeyboardSettings", "#GameUI_KeyboardSettingsText", None);
                ui.add_action_signal_target(qb, me);
                MessageBox::set_commands(ui, qb, Some(Msg::new("Command").str("command", "DefaultsOK")), None);
                gs_vgui::controls::frame::with_frame(ui, qb, |f, ui| f.do_modal(ui, qb));
                return true;
            }
            if cmd.eq_ignore_ascii_case("DefaultsOK") {
                self.fill_in_default_bindings(ui, me);
                ui.request_focus(list);
                return true;
            }
            if cmd.eq_ignore_ascii_case("ChangeKey") && !capturing {
                with_controls(ui, list, |l, ui| l.start_capture(ui, list, 1));
                return true;
            }
            if cmd.eq_ignore_ascii_case("ClearKey") && !capturing {
                self.key_code_pressed(ui, KEY_DELETE, s);
                ui.request_focus(list);
                return true;
            }
            return false;
        }
        let capturing = self.capturing(ui);
        if msg.is("KeyCodePressed") && !capturing {
            return self.key_code_pressed(ui, msg.get_int("code"), s);
        }
        if !capturing {
            return false;
        }
        if msg.is("KeyCodeTyped") {
            let sc = vgui_to_scancode(msg.get_int("code"));
            let name = sdl_key_name(sc);
            self.finish(ui, me, scancode_to_engine_key(sc), 0, &name);
            return true;
        }
        if msg.is("MousePressed") || msg.is("MouseDoublePressed") {
            let code = msg.get_int("code");
            if (0..=4).contains(&code) {
                self.finish(ui, me, 0, 1 << code, &format!("MOUSE{}", code + 1));
            } else {
                self.finish(ui, me, 0, 1, "MOUSE1");
            }
            return true;
        }
        if msg.is("MouseWheeled") {
            if msg.get_int("delta") > 0 {
                self.finish(ui, me, Self::find_key_by_name("MWHEELUP"), 0, "MWHEELUP");
            } else {
                self.finish(ui, me, Self::find_key_by_name("MWHEELDOWN"), 0, "MWHEELDOWN");
            }
            return true;
        }
        false
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl KeyboardPage {
    /// `OnKeyCodePressed`: Delete clears the item of interest's keys.
    fn key_code_pressed(&mut self, ui: &mut Ui, code: i32, s: &mut dyn Surface) -> bool {
        let list = self.list;
        let id = with_controls(ui, list, |l, _| l.click_row).unwrap_or(-1);
        let has_cell = with_sectioned(ui, list, |l, ui| l.cell_bounds(ui, id, 1, s).is_some()).unwrap_or(false);
        if has_cell && code == KEY_DELETE {
            let alt = self.get(ui, id, "AltKey");
            if !alt.is_empty() {
                self.remove_key_from_bind_items(ui, &alt);
            }
            let key = self.get(ui, id, "Key");
            if !key.is_empty() {
                self.remove_key_from_bind_items(ui, &key);
            }
            ui.set_enabled(self.clear_key, false);
            self.invalidate_item(ui, id);
            return true;
        }
        false
    }
}
