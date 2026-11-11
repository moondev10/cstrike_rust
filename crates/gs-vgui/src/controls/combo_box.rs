//! `ComboBox` and its `ComboBoxButton` (docs/re/vgui-controls-2.md §5.4–§5.5).

use std::any::Any;

use gs_formats::keyvalues::Block;

use crate::controls::button::{Activation, ButtonState};
use crate::controls::label::{Align, LabelImage};
use crate::controls::menu::{Menu, MenuDirection, MenuItem};
use crate::controls::text_entry::TextEntry;
use crate::keys::*;
use crate::msg::Msg;
use crate::scheme::Scheme;
use crate::surface::{Color, Cursor, Surface};
use crate::ui::{Control, PanelId, Ui};

/// `vgui2::ComboBox`: a TextEntry with a drop-down Menu.
#[derive(Debug)]
pub struct ComboBox {
    pub te: TextEntry,
    pub menu: PanelId,
    pub button: PanelId,
    highlight: bool,
    pub direction: MenuDirection,
}

/// Runs `f` on the combo box `id`.
pub fn with_combo<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut ComboBox, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<ComboBox>().map(|cb| f(cb, ui))).flatten()
}

fn item_text(ui: &mut Ui, item: PanelId) -> String {
    ui.ctrl::<MenuItem>(item).map(|it| it.st.label.text.text_string()).unwrap_or_default()
}

impl ComboBox {
    /// `new ComboBox(parent, name, numLines, allowEdit)`.
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str, num_lines: i32, allow_edit: bool) -> PanelId {
        let prop = parent.is_some_and(|p| ui.is_proportional(p));
        let mut te = TextEntry::new(ui, prop);
        te.set_editable(allow_edit);
        te.set_horizontal_scrolling(false);
        let placeholder = ui.embedded();
        let me = ui.create(parent, name, Box::new(ComboBox { te, menu: placeholder, button: placeholder, highlight: false, direction: MenuDirection::Down }));
        let menu = Menu::create(ui, Some(me), "");
        ui.add_action_signal_target(menu, me);
        let mut st = ButtonState::new(ui, Some(me), "");
        st.activation = Activation::OnPressed;
        st.set_command("ButtonClicked");
        let button = ui.create(Some(me), "", Box::new(ComboBoxButton { st, disabled_bg: Color::TRANSPARENT }));
        ui.add_action_signal_target(button, me);
        Menu::set_number_of_visible_items(ui, menu, num_lines);
        if let Some(c) = ui.ctrl::<ComboBox>(me) {
            c.menu = menu;
            c.button = button;
        }
        me
    }

    /// `AddItem(text, userData)`: the item posts `SetText{text}` back to the combo.
    pub fn add_item(ui: &mut Ui, me: PanelId, text: &str, user_data: Option<Msg>) -> i32 {
        let Some(menu) = ui.ctrl::<ComboBox>(me).map(|c| c.menu) else { return -1 };
        Menu::add_item_msg(ui, menu, text, text, Msg::new("SetText").str("text", text), Some(me), user_data)
    }

    /// `UpdateItem(id, text, userData)`.
    pub fn update_item(ui: &mut Ui, me: PanelId, id: i32, text: &str, user_data: Option<Msg>) -> bool {
        let Some(menu) = ui.ctrl::<ComboBox>(me).map(|c| c.menu) else { return false };
        if Menu::item(ui, menu, id).is_none() {
            return false;
        }
        Menu::update_item(ui, menu, id, text, Msg::new("SetText").str("text", text), user_data);
        ui.invalidate_layout(me);
        true
    }

    pub fn delete_all_items(ui: &mut Ui, me: PanelId) {
        if let Some(menu) = ui.ctrl::<ComboBox>(me).map(|c| c.menu) {
            Menu::delete_all_items(ui, menu);
        }
    }

    pub fn item_count(ui: &mut Ui, me: PanelId) -> i32 {
        ui.ctrl::<ComboBox>(me).map(|c| c.menu).map_or(0, |m| Menu::item_count(ui, m))
    }

    /// `ActivateItem(id)`: the item's click fires `SetText`.
    pub fn activate_item(ui: &mut Ui, me: PanelId, id: i32, s: &mut dyn Surface) {
        if let Some(menu) = ui.ctrl::<ComboBox>(me).map(|c| c.menu) {
            Menu::activate_item(ui, menu, id, s);
        }
    }

    /// `ActivateItemByRow(row)`.
    pub fn activate_item_by_row(ui: &mut Ui, me: PanelId, row: i32, s: &mut dyn Surface) {
        if let Some(menu) = ui.ctrl::<ComboBox>(me).map(|c| c.menu) {
            let id = Menu::menu_id(ui, menu, row);
            if id >= 0 {
                Menu::activate_item(ui, menu, id, s);
            }
        }
    }

    /// `GetActiveItem` (initially 0).
    pub fn active_item(ui: &mut Ui, me: PanelId) -> i32 {
        ui.ctrl::<ComboBox>(me).map(|c| c.menu).map_or(0, |m| Menu::active_item(ui, m))
    }

    /// `GetActiveItemUserData`.
    pub fn active_item_user_data(ui: &mut Ui, me: PanelId) -> Option<Msg> {
        let menu = ui.ctrl::<ComboBox>(me).map(|c| c.menu)?;
        let id = Menu::active_item(ui, menu);
        Menu::item_user_data(ui, menu, id)
    }

    /// `GetItemUserData(id)`.
    pub fn item_user_data(ui: &mut Ui, me: PanelId, id: i32) -> Option<Msg> {
        let menu = ui.ctrl::<ComboBox>(me).map(|c| c.menu)?;
        Menu::item_user_data(ui, menu, id)
    }

    /// The current text.
    pub fn text(ui: &mut Ui, me: PanelId) -> String {
        ui.ctrl::<ComboBox>(me).map(|c| c.te.text()).unwrap_or_default()
    }

    /// `DoMenuLayout`: below the combo (or above/at the cursor), fixed to the combo's width.
    fn do_menu_layout(&self, ui: &mut Ui, me: PanelId) {
        let (_, tall) = ui.size(me);
        let (x, y) = match self.direction {
            MenuDirection::Cursor => ui.cursor_pos(),
            MenuDirection::AlignWithParent if ui.parent(me).is_some() => {
                let (px, py) = ui.parent(me).map_or((0, 0), |p| ui.abs_pos(p));
                (px - ui.psv(me, 1), py + tall)
            }
            _ => {
                let (ax, ay) = ui.abs_pos(me);
                (ax, ay + tall)
            }
        };
        let (_, mt) = ui.size(self.menu);
        let (_, bt) = ui.size(me);
        let one = ui.psv(me, 1);
        match self.direction {
            MenuDirection::Up => ui.set_pos(self.menu, x, y - (bt + mt) - one),
            MenuDirection::Down => ui.set_pos(self.menu, x, y + one),
            _ => ui.set_pos(self.menu, x + one, y + one),
        }
        let w = ui.size(me).0;
        Menu::set_fixed_width(ui, self.menu, w);
    }

    /// `HideMenu`.
    fn hide_menu(&self, ui: &mut Ui) {
        Menu::set_visible(ui, self.menu, false);
    }

    /// `DoClick`: toggles the drop-down, highlighting the row whose text matches.
    fn do_click(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if ui.is_visible(self.menu) {
            self.hide_menu(ui);
            return;
        }
        if !ui.is_enabled(self.menu) {
            return;
        }
        ui.perform_layout(self.menu, s);
        let cur = gs_common::bytestr::ansi_cap(&self.te.text(), 255);
        let count = Menu::item_count(ui, self.menu);
        for row in 0..count {
            let id = Menu::menu_id(ui, self.menu, row);
            if let Some(item) = Menu::item(ui, self.menu, id)
                && item_text(ui, item) == cur
            {
                Menu::set_currently_highlighted_item(ui, self.menu, row, s);
                break;
            }
        }
        self.do_menu_layout(ui, me);
        ui.move_to_front(me);
        Menu::set_visible(ui, self.menu, true);
        ui.request_focus(self.menu);
        self.te.select_no_text();
        ui.with_ctrl(self.button, |c, ui| {
            if let Some(b) = c.as_any_mut().downcast_mut::<ComboBoxButton>() {
                b.st.set_armed(ui, self.button, true, s);
            }
        });
    }

    /// `OnSetText`: a `#token` is localized; a change posts `TextChanged{text}`.
    fn on_set_text(&mut self, ui: &mut Ui, me: PanelId, text: &str, s: &mut dyn Surface) {
        let shown = text.strip_prefix('#').and_then(|k| ui.localize.find(k)).map_or_else(|| text.to_owned(), str::to_owned);
        let cur = gs_common::bytestr::ansi_cap(&self.te.text(), 254);
        if cur != shown {
            self.te.set_text(ui, me, &shown, s);
            ui.post_action_signal(me, Msg::new("TextChanged").str("text", &shown));
        }
        self.hide_menu(ui);
    }

    /// `MoveAlongMenuItemList(dir)` (a row used as an item ID).
    fn move_along(&mut self, ui: &mut Ui, me: PanelId, dir: i32, s: &mut dyn Surface) {
        let cur = gs_common::bytestr::ansi_cap(&self.te.text(), 254);
        let count = Menu::item_count(ui, self.menu);
        let mut texts = Vec::new();
        for row in 0..count {
            let id = Menu::menu_id(ui, self.menu, row);
            texts.push(Menu::item(ui, self.menu, id).map(|i| item_text(ui, i)).unwrap_or_default());
        }
        let Some(row) = texts.iter().position(|t| *t == cur) else { return };
        let j = (row as i32 + dir).clamp(0, count - 1);
        let t = texts[j as usize].clone();
        self.on_set_text(ui, me, &t, s);
        self.te.select_all_text(false);
        Menu::activate_item(ui, self.menu, j, s);
    }
}

impl Control for ComboBox {
    fn class(&self) -> &'static str {
        "ComboBox"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("SetText") {
            let t = msg.get_str("text");
            self.on_set_text(ui, me, &t, s);
        } else if msg.is("ActivateItem") {
            Menu::activate_item(ui, self.menu, msg.get_int("itemID"), s);
        } else if msg.is("MenuClose") {
            self.hide_menu(ui);
            if ui.focus() == Some(me) {
                self.te.select_all_text(false);
            } else if self.highlight {
                self.highlight = false;
                ui.request_focus(me);
            } else if ui.mouse_over().is_some_and(|o| o == me || ui.has_parent(o, me)) {
                self.te.select_all_text(false);
                ui.with_ctrl(self.button, |c, ui| {
                    if let Some(b) = c.as_any_mut().downcast_mut::<ComboBoxButton>() {
                        b.st.set_armed(ui, self.button, true, s);
                    }
                });
                ui.request_focus(me);
            } else {
                ui.with_ctrl(self.button, |c, ui| {
                    if let Some(b) = c.as_any_mut().downcast_mut::<ComboBoxButton>() {
                        b.st.set_armed(ui, self.button, false, s);
                    }
                });
            }
        } else if msg.is("MenuItemSelected") {
            self.highlight = true;
        } else if msg.is("Command") {
            if msg.get_str("command").eq_ignore_ascii_case("ButtonClicked") {
                self.do_click(ui, me, s);
            }
        } else if msg.is("MousePressed") || msg.is("MouseDoublePressed") {
            if !ui.is_enabled(me) {
                return true;
            }
            let over = ui.mouse_over().is_some_and(|o| o == me);
            if !over {
                self.hide_menu(ui);
                return true;
            }
            if self.te.is_editable(ui, me) {
                self.te.handle(ui, me, msg, s);
                if msg.is("MousePressed") {
                    self.hide_menu(ui);
                }
                return true;
            }
            ui.request_focus(me);
            self.do_click(ui, me, s);
        } else if msg.is("CursorEntered") {
            ui.with_ctrl(self.button, |c, ui| {
                if let Some(b) = c.as_any_mut().downcast_mut::<ComboBoxButton>()
                    && ui.is_enabled(self.button)
                {
                    b.st.set_armed(ui, self.button, true, s);
                }
            });
            self.te.handle(ui, me, msg, s);
        } else if msg.is("CursorExited") {
            if !ui.is_visible(self.menu) {
                ui.with_ctrl(self.button, |c, ui| {
                    if let Some(b) = c.as_any_mut().downcast_mut::<ComboBoxButton>() {
                        b.st.set_armed(ui, self.button, false, s);
                    }
                });
                self.te.handle(ui, me, msg, s);
            }
        } else if msg.is("SetFocus") {
            self.te.handle(ui, me, msg, s);
            self.te.goto_text_end(ui, me, s);
            self.te.select_all_text(false);
        } else if msg.is("KillFocus") {
            self.te.select_no_text();
        } else if msg.is("KeyCodeTyped") {
            let code = msg.get_int("code");
            match code {
                KEY_UP | KEY_DOWN if ui.is_visible(self.menu) => self.move_along(ui, me, if code == KEY_UP { -1 } else { 1 }, s),
                KEY_ENTER | KEY_SPACE => self.do_click(ui, me, s),
                _ => return self.te.handle(ui, me, msg, s),
            }
        } else if msg.is("KeyTyped") {
            if self.te.is_editable(ui, me) {
                return self.te.handle(ui, me, msg, s);
            }
            let ch = char::from_u32(msg.get_int("unichar") as u32).unwrap_or('\0');
            let lower = |c: char| c.to_lowercase().next().unwrap_or(c);
            let count = Menu::item_count(ui, self.menu);
            if count <= 0 {
                return true;
            }
            let start = Menu::active_item(ui, self.menu).max(0);
            let text_of = |ui: &mut Ui, row: i32| {
                let id = Menu::menu_id(ui, self.menu, row);
                Menu::item(ui, self.menu, id).map(|i| item_text(ui, i)).unwrap_or_default()
            };
            let mut i = if start + 1 < count { start + 1 } else { 0 };
            let mut found = None;
            while i != start {
                if text_of(ui, i).chars().next().is_some_and(|c| lower(c) == lower(ch)) {
                    found = Some(i);
                    break;
                }
                i = if i + 1 < count { i + 1 } else { 0 };
            }
            let r = found.unwrap_or(start);
            if (0..count).contains(&r) {
                let t = text_of(ui, r);
                self.on_set_text(ui, me, &t, s);
                self.te.select_all_text(false);
                Menu::activate_item(ui, self.menu, r, s);
            }
        } else {
            return self.te.handle(ui, me, msg, s);
        }
        true
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.te.apply_scheme_te(ui, me, scheme, s);
        let border = scheme.border("ComboBoxBorder");
        if let Some(n) = ui.node_mut(me) {
            n.border = border;
        }
    }
    fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &Block, s: &mut dyn Surface) {
        self.te.apply_settings(ui, me, kv, s);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let (w, h) = ui.size(me);
        let [l, t, r, b] = ui.node(me).and_then(|n| n.border.as_ref().map(|b| b.inset)).unwrap_or([0; 4]);
        let (wide, tall) = (w - l - r, h - t - b);
        self.te.perform_layout(ui, me, s);
        let (bx, by, bw) = (wide - ui.psv(me, 20), ui.psv(me, 2), ui.psv(me, 16));
        let bh = tall - ui.psv(me, 2);
        ui.set_bounds(self.button, bx, by, bw, bh);
        let editable = self.te.is_editable(ui, me);
        if let Some(n) = ui.node_mut(me) {
            n.cursor = if editable { Cursor::IBeam } else { Cursor::Arrow };
        }
        let enabled = ui.is_enabled(me);
        ui.set_enabled(self.button, enabled);
        self.do_menu_layout(ui, me);
    }
    /// `OnSizeChanged`: TextEntry's, then `PerformLayout` (the arrow button gets its PSV(16)
    /// width), then `SetDrawWidth(w - button width)`.
    fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId, w: i32, h: i32) {
        self.te.on_size_changed(ui, me, w, h);
        let [l, t, r, b] = ui.node(me).and_then(|n| n.border.as_ref().map(|b| b.inset)).unwrap_or([0; 4]);
        let (wide, tall) = (w - l - r, h - t - b);
        let (bx, by, bw) = (wide - ui.psv(me, 20), ui.psv(me, 2), ui.psv(me, 16));
        ui.set_bounds(self.button, bx, by, bw, tall - ui.psv(me, 2));
        self.te.set_draw_width(w - ui.size(self.button).0);
        ui.invalidate_layout(me);
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.te.paint_te(ui, me, s);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `ComboBoxButton`: the `icon_down` bitmap; no border; stays armed while the menu is open.
#[derive(Debug)]
pub struct ComboBoxButton {
    pub st: ButtonState,
    disabled_bg: Color,
}

impl Control for ComboBoxButton {
    fn class(&self) -> &'static str {
        "ComboBoxButton"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("CursorExited") {
            ui.call_parent(me, &Msg::new("CursorExited"), s);
            return true;
        }
        self.st.on_message(ui, me, msg, s)
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.st.apply_scheme(ui, me, scheme, s);
        let armed = scheme.color("MenuButton/ArmedArrowColor", Color::WHITE);
        let bg = scheme.color("MenuButton/ButtonBgColor", Color::WHITE);
        self.st.armed_fg = armed;
        self.st.armed_bg = bg;
        self.st.depressed_fg = armed;
        self.st.depressed_bg = bg;
        self.st.default_fg = scheme.color("MenuButton/ButtonArrowColor", Color::WHITE);
        self.st.default_bg = bg;
        self.disabled_bg = scheme.color("ControlBG", Color::WHITE);
        self.st.set_borders(None, None);
        let tex = s.texture("resource/icon_down", false);
        let size = tex.map_or((0, 0), |t| (t.width, t.height));
        self.st.label.align = Align::West;
        self.st.label.add_image(LabelImage::Texture { name: "resource/icon_down".to_owned(), tex, size, color: Color::WHITE }, 0);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.st.perform_layout(ui, me, s);
        if !ui.is_enabled(me)
            && let Some(n) = ui.node_mut(me)
        {
            n.bg = self.disabled_bg;
        }
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.st.label.paint(ui, me, s);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
