//! `Menu` and `MenuItem` (docs/re/vgui-controls-2.md §5.1–§5.2).
//!
//! GameUI, the server browser and the demo UI create only plain items (no cascading or
//! checkable ones), so the cascade arrow and check image paths are not modelled.

use std::any::Any;

use crate::controls::button::{Activation, ButtonState};
use crate::controls::label::Align;
use crate::controls::scroll_bar::ScrollBar;
use crate::keys::*;
use crate::msg::{Msg, Value, atoi};
use crate::scheme::Scheme;
use crate::surface::{Color, FontHandle, Surface};
use crate::ui::{Control, PanelId, Ui};

/// `vgui2::MenuItem`.
#[derive(Debug)]
pub struct MenuItem {
    pub st: ButtonState,
    pub user_data: Option<Msg>,
    /// A GameUI `CGameMenuItem`: its highlight label (armed → glow, disarmed → fade).
    pub game_highlight: Option<PanelId>,
}

/// GameUI's `CGameMenu` state on top of `Menu` (docs/re/gameui.md §4.4–§4.7).
#[derive(Debug, Clone, Copy, Default)]
pub struct GameMenuExt {
    /// `+0xf4`: restart the reveal animation.
    pub start: bool,
    animating: bool,
    start_time: f64,
    rate: f32,
}

impl MenuItem {
    fn menu(ui: &Ui, me: PanelId) -> Option<PanelId> {
        ui.parent(me)
    }

    fn content_width(&mut self, ui: &Ui, me: PanelId, s: &mut dyn Surface) -> i32 {
        self.st.label.content_size(ui, me, s).0
    }
}

impl Control for MenuItem {
    fn class(&self) -> &'static str {
        "MenuItem"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        let Some(menu) = Self::menu(ui, me) else { return self.st.on_message(ui, me, msg, s) };
        if msg.is("CursorEntered") || msg.is("CursorExited") {
            let name = if msg.is("CursorEntered") { "CursorEnteredMenuItem" } else { "CursorExitedMenuItem" };
            ui.post(menu, Msg::new(name).with("VPanel", Value::Panel(me)), None, 0.0);
        } else if msg.is("CursorMoved") {
            let keyboard = ui.ctrl::<Menu>(menu).is_some_and(|m| m.keyboard_mode);
            if keyboard {
                ui.post(menu, Msg::new("CursorEnteredMenuItem").with("VPanel", Value::Panel(me)), None, 0.0);
            }
            let (ax, ay) = ui.abs_pos(me);
            ui.call_parent(me, &Msg::new("OnCursorMoved").int("x", msg.get_int("xpos") - ax).int("y", msg.get_int("ypos") - ay), s);
        } else if msg.is("KeyCodeReleased") {
            let code = msg.get_int("code");
            if code == KEY_ESCAPE || code == KEY_F4 {
                ui.with_ctrl(menu, |c, ui| {
                    if let Some(m) = c.as_any_mut().downcast_mut::<Menu>() {
                        m.key_code_typed(ui, menu, code, s);
                    }
                });
            } else {
                self.st.on_message(ui, me, msg, s);
            }
        } else if msg.is("KeyModeSet") {
            ui.post(menu, Msg::new("KeyModeSet"), Some(me), 0.0);
        } else if msg.is("MenuClose") || msg.is("KillFocus") {
            ui.with_ctrl(menu, |c, ui| {
                if let Some(m) = c.as_any_mut().downcast_mut::<Menu>() {
                    m.on_kill_focus(ui, menu, s);
                }
            });
        } else if msg.is("ArmItem") || msg.is("DisarmItem") {
            let arm = msg.is("ArmItem");
            notify_highlight(ui, me, self.game_highlight, arm, s);
            if arm {
                self.st.set_armed(ui, me, true, s);
            } else if !self.st.is_key_down() {
                self.st.set_armed(ui, me, false, s);
            }
            let w = self.content_width(ui, me, s);
            Menu::force_calculate_width_from(ui, menu, Some((me, w)), s);
        } else {
            return self.st.on_message(ui, me, msg, s);
        }
        true
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        if self.game_highlight.is_some() {
            ui.set_proportional(me, true);
        }
        self.st.apply_scheme(ui, me, scheme, s);
        let (fg, bg) = ui.node(me).map_or((Color::WHITE, Color::WHITE), |n| (n.fg, n.bg));
        self.st.default_fg = scheme.color("Menu/FgColor", fg);
        self.st.default_bg = scheme.color("Menu/BgColor", bg);
        self.st.armed_fg = scheme.color("Menu/ArmedFgColor", fg);
        self.st.armed_bg = scheme.color("Menu/ArmedBgColor", bg);
        self.st.depressed_fg = self.st.armed_fg;
        self.st.depressed_bg = self.st.armed_bg;
        let inset = atoi(scheme.resource_string("Menu/TextInset"));
        self.st.label.set_text_inset(ui, me, inset, 0);
        if let Some(menu) = Self::menu(ui, me) {
            let w = self.content_width(ui, me, s);
            Menu::force_calculate_width_from(ui, menu, Some((me, w)), s);
        }
        if self.game_highlight.is_some() {
            // CGameMenuItem::ApplySchemeSettings.
            let menu_color = scheme.color("InGameDesktop/MenuColor", Color::WHITE);
            if let Some(n) = ui.node_mut(me) {
                n.fg = menu_color;
                n.bg = Color::TRANSPARENT;
                n.border = None;
            }
            self.st.default_fg = menu_color;
            self.st.default_bg = Color::TRANSPARENT;
            self.st.armed_fg = scheme.color("InGameDesktop/ArmedMenuColor", Color::WHITE);
            self.st.armed_bg = Color::TRANSPARENT;
            self.st.depressed_fg = scheme.color("InGameDesktop/DepressedMenuColor", Color::WHITE);
            self.st.depressed_bg = Color::TRANSPARENT;
            self.st.label.align = Align::West;
            self.st.set_borders(None, None);
            let (prop, hd) = (ui.is_proportional(me), ui.is_hd_proportional(me));
            self.st.label.text.font = s.font(&scheme.file, "MenuLarge", prop, hd);
            self.st.label.font_override = Some("MenuLarge".to_owned());
            let inset = ui.psv(me, 4);
            self.st.label.set_text_inset(ui, me, inset, 0);
            self.st.depressed_sound = Some("sound/UI/launch_selectmenu.wav".to_owned());
            self.st.activation = Activation::OnPressed;
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

/// `vgui2::Menu`.
#[derive(Debug)]
pub struct Menu {
    item_height: i32,
    fixed_width: i32,
    min_width: i32,
    num_visible: i32,
    blur_offset: i32,
    scroller: PanelId,
    /// `m_MenuItems`; the index is the item ID.
    items: Vec<PanelId>,
    /// `m_SortedItems`: item IDs in display order.
    sorted: Vec<usize>,
    sized_for_scroll: bool,
    recalc_width: bool,
    menu_wide: i32,
    current: i32,
    keyboard_mode: bool,
    check_image_width: i32,
    sb_size: i32,
    border_dark: Color,
    activated: i32,
    item_font: Option<FontHandle>,
    /// GameUI's `CGameMenu` behaviour, when this is the game menu.
    pub game: Option<GameMenuExt>,
}

impl Menu {
    /// `new Menu(parent, name)`: a hidden, non-proportional popup at z 1.
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str) -> PanelId {
        let placeholder = ui.embedded();
        let me = ui.create(
            parent,
            name,
            Box::new(Menu {
                item_height: 22,
                fixed_width: 0,
                min_width: 0,
                num_visible: 99,
                blur_offset: 0,
                scroller: placeholder,
                items: Vec::new(),
                sorted: Vec::new(),
                sized_for_scroll: false,
                recalc_width: true,
                menu_wide: 0,
                current: -1,
                keyboard_mode: false,
                check_image_width: 0,
                sb_size: 18,
                border_dark: Color::rgba(0, 0, 0, 0),
                activated: 0,
                item_font: None,
                game: None,
            }),
        );
        let sb = ScrollBar::create(ui, Some(me), "MenuScrollBar", true);
        ui.set_visible(sb, false);
        ui.add_action_signal_target(sb, me);
        ui.set_zpos(me, 1);
        ui.set_visible(me, false);
        ui.make_popup(me);
        ui.set_parent(me, parent);
        let sb_size = if ui.is_proportional(me) {
            let (sw, _) = ui.screen;
            let (bw, _) = ui.proportional_base_of(me);
            (sw as f32 / bw as f32 * 18.0) as i32
        } else {
            18
        };
        if let Some(m) = ui.ctrl::<Menu>(me) {
            m.scroller = sb;
            m.sb_size = sb_size;
        }
        me
    }

    /// `AddMenuItem(name, text, command, target, userData)` with a `Command{command}` message.
    pub fn add_item(ui: &mut Ui, menu: PanelId, name: &str, text: &str, command: &str, target: Option<PanelId>, user_data: Option<Msg>) -> i32 {
        Self::add_item_msg(ui, menu, name, text, Msg::new("Command").str("command", command), target, user_data)
    }

    /// `AddMenuItem(name, text, KeyValues* message, target, userData)`.
    pub fn add_item_msg(ui: &mut Ui, menu: PanelId, name: &str, text: &str, message: Msg, target: Option<PanelId>, user_data: Option<Msg>) -> i32 {
        let mut st = ButtonState::new(ui, None, text);
        st.activation = Activation::OnReleased;
        st.border_enabled = false;
        st.use_capture_mouse = false;
        st.label.align = Align::West;
        st.command = Some(message);
        st.select_notify = Some(menu);
        let item = ui.create(Some(menu), name, Box::new(MenuItem { st, user_data, game_highlight: None }));
        if let Some(t) = target {
            ui.add_action_signal_target(item, t);
        }
        let font = ui.ctrl::<Menu>(menu).and_then(|m| m.item_font);
        if let (Some(f), Some(it)) = (font, ui.ctrl::<MenuItem>(item)) {
            it.st.label.text.font = f;
        }
        ui.invalidate_layout(menu);
        let Some(m) = ui.ctrl::<Menu>(menu) else { return -1 };
        m.items.push(item);
        let id = m.items.len() - 1;
        m.sorted.push(id);
        m.recalc_width = true;
        id as i32
    }

    /// `UpdateMenuItem(id, text, message, userData)`: no relayout of the menu.
    pub fn update_item(ui: &mut Ui, menu: PanelId, id: i32, text: &str, message: Msg, user_data: Option<Msg>) {
        let Some(item) = Self::item(ui, menu, id) else { return };
        if let Some(it) = ui.ctrl::<MenuItem>(item) {
            it.st.command = Some(message);
            if user_data.is_some() {
                it.user_data = user_data;
            }
        }
        ui.with_ctrl(item, |c, ui| {
            if let Some(it) = c.as_any_mut().downcast_mut::<MenuItem>() {
                it.st.label.set_text(ui, text);
            }
        });
        ui.invalidate_layout(item);
    }

    /// The item panel of ID `id`.
    pub fn item(ui: &mut Ui, menu: PanelId, id: i32) -> Option<PanelId> {
        let m = ui.ctrl::<Menu>(menu)?;
        usize::try_from(id).ok().and_then(|i| m.items.get(i).copied())
    }

    pub fn item_count(ui: &mut Ui, menu: PanelId) -> i32 {
        ui.ctrl::<Menu>(menu).map_or(0, |m| m.items.len() as i32)
    }

    /// `GetActiveItem`.
    pub fn active_item(ui: &mut Ui, menu: PanelId) -> i32 {
        ui.ctrl::<Menu>(menu).map_or(0, |m| m.activated)
    }

    /// `GetMenuID(row)`.
    pub fn menu_id(ui: &mut Ui, menu: PanelId, row: i32) -> i32 {
        ui.ctrl::<Menu>(menu).and_then(|m| usize::try_from(row).ok().and_then(|r| m.sorted.get(r).copied())).map_or(-1, |id| id as i32)
    }

    /// `GetItemUserData`: disabled items return none.
    pub fn item_user_data(ui: &mut Ui, menu: PanelId, id: i32) -> Option<Msg> {
        let item = Self::item(ui, menu, id)?;
        if !ui.is_enabled(item) {
            return None;
        }
        ui.ctrl::<MenuItem>(item).and_then(|it| it.user_data.clone())
    }

    /// `SetItemEnabled(id)`.
    pub fn set_item_enabled(ui: &mut Ui, menu: PanelId, id: i32, state: bool) {
        if let Some(item) = Self::item(ui, menu, id) {
            ui.set_enabled(item, state);
        }
    }

    /// `SetItemVisible(id)` (no relayout).
    pub fn set_item_visible(ui: &mut Ui, menu: PanelId, id: i32, state: bool) {
        if let Some(item) = Self::item(ui, menu, id) {
            ui.set_visible(item, state);
        }
    }

    /// `DeleteAllItems`: the current and activated IDs are kept.
    pub fn delete_all_items(ui: &mut Ui, menu: PanelId) {
        let items = ui.ctrl::<Menu>(menu).map(|m| {
            m.sorted.clear();
            std::mem::take(&mut m.items)
        });
        for item in items.unwrap_or_default() {
            ui.mark_for_deletion(item);
        }
        ui.invalidate_layout(menu);
    }

    pub fn set_fixed_width(ui: &mut Ui, menu: PanelId, w: i32) {
        if let Some(m) = ui.ctrl::<Menu>(menu) {
            m.fixed_width = w;
        }
        ui.invalidate_layout(menu);
    }

    pub fn set_minimum_width(ui: &mut Ui, menu: PanelId, w: i32) {
        if let Some(m) = ui.ctrl::<Menu>(menu) {
            m.min_width = w;
        }
    }

    pub fn set_menu_item_height(ui: &mut Ui, menu: PanelId, h: i32) {
        if let Some(m) = ui.ctrl::<Menu>(menu) {
            m.item_height = h;
        }
    }

    pub fn set_number_of_visible_items(ui: &mut Ui, menu: PanelId, n: i32) {
        if let Some(m) = ui.ctrl::<Menu>(menu) {
            m.num_visible = n;
        }
        ui.invalidate_layout(menu);
    }

    /// `SetFont`: the item height follows the font (`tall + 2`).
    pub fn set_font(ui: &mut Ui, menu: PanelId, font: FontHandle, s: &mut dyn Surface) {
        let tall = s.font_tall(font);
        if let Some(m) = ui.ctrl::<Menu>(menu) {
            m.item_font = Some(font);
            m.item_height = tall + 2;
        }
        ui.invalidate_layout(menu);
    }

    /// `Menu::SetVisible`.
    pub fn set_visible(ui: &mut Ui, menu: PanelId, state: bool) {
        ui.with_ctrl(menu, |c, ui| {
            if let Some(m) = c.as_any_mut().downcast_mut::<Menu>() {
                m.set_visible_impl(ui, menu, state);
            }
        });
    }

    /// `ActivateItem(id)`.
    pub fn activate_item(ui: &mut Ui, menu: PanelId, id: i32, s: &mut dyn Surface) {
        ui.with_ctrl(menu, |c, ui| {
            if let Some(m) = c.as_any_mut().downcast_mut::<Menu>() {
                m.activate(ui, id, s);
            }
        });
    }

    /// `SetCurrentlyHighlightedItem(id)`.
    pub fn set_currently_highlighted_item(ui: &mut Ui, menu: PanelId, id: i32, s: &mut dyn Surface) {
        ui.with_ctrl(menu, |c, ui| {
            if let Some(m) = c.as_any_mut().downcast_mut::<Menu>() {
                m.set_highlighted(ui, menu, id, s);
            }
        });
    }

    /// `m_iCurrentlySelectedItemID`.
    pub fn current_item(ui: &mut Ui, menu: PanelId) -> i32 {
        ui.ctrl::<Menu>(menu).map_or(-1, |m| m.current)
    }

    /// `SetCurrentlySelectedItem(id)`.
    pub fn set_currently_selected_item(ui: &mut Ui, menu: PanelId, id: i32) {
        ui.with_ctrl(menu, |c, ui| {
            if let Some(m) = c.as_any_mut().downcast_mut::<Menu>() {
                m.set_selected(ui, menu, id);
            }
        });
    }

    /// `ForceCalculateWidth` when called from an item busy in its own handler: that item's
    /// content width is passed in.
    fn force_calculate_width_from(ui: &mut Ui, menu: PanelId, known: Option<(PanelId, i32)>, s: &mut dyn Surface) {
        ui.with_ctrl(menu, |c, ui| {
            if let Some(m) = c.as_any_mut().downcast_mut::<Menu>() {
                m.recalc_width = true;
                m.calculate_width(ui, known, s);
                m.layout(ui, menu, s);
            }
        });
    }

    fn item_id(&self, id: i32) -> Option<PanelId> {
        usize::try_from(id).ok().and_then(|i| self.items.get(i).copied())
    }

    /// `CalculateWidth` (hidden items included).
    fn calculate_width(&mut self, ui: &mut Ui, known: Option<(PanelId, i32)>, s: &mut dyn Surface) {
        if !self.recalc_width {
            return;
        }
        self.menu_wide = 0;
        if self.fixed_width == 0 {
            for &item in &self.items {
                let w = match known {
                    Some((k, w)) if k == item => Some(w),
                    _ => ui.with_ctrl(item, |c, ui| c.as_any_mut().downcast_mut::<MenuItem>().map(|it| it.content_width(ui, item, s))).flatten(),
                };
                if let Some(w) = w
                    && w > self.menu_wide - 8
                {
                    self.menu_wide = w + 8;
                }
            }
        }
        if self.menu_wide < self.min_width {
            self.menu_wide = self.min_width;
        }
        self.recalc_width = false;
    }

    fn force_calculate_width(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.recalc_width = true;
        self.calculate_width(ui, None, s);
        self.layout(ui, me, s);
    }

    fn inset(ui: &Ui, me: PanelId) -> [i32; 4] {
        ui.node(me).and_then(|n| n.border.as_ref().map(|b| b.inset)).unwrap_or([0; 4])
    }

    /// `MakeItemsVisibleInScrollRange`.
    fn make_items_visible_in_scroll_range(&mut self, ui: &mut Ui) {
        let count = self.sorted.len() as i32;
        let mut v = ScrollBar::value(ui, self.scroller);
        while v < count {
            let mut shown = 0;
            for (row, &id) in self.sorted.iter().enumerate() {
                let row = row as i32;
                let vis = row >= v && shown < self.num_visible;
                if vis {
                    shown += 1;
                }
                ui.set_visible(self.items[id], vis);
            }
            if shown >= self.num_visible {
                break;
            }
            v -= 1;
        }
    }

    /// `PerformLayout`.
    fn layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if self.sorted.len() as i32 > self.num_visible {
            ui.set_visible(self.scroller, true);
            self.sized_for_scroll = true;
            self.make_items_visible_in_scroll_range(ui);
        } else {
            ui.set_visible(self.scroller, false);
            self.sized_for_scroll = false;
        }
        // LayoutMenuBorder (CGameMenu overrides it with an empty function: no border).
        if self.game.is_none() {
            let border = ui.scheme_of(me).and_then(|sc| sc.border("MenuBorder"));
            if border.is_some()
                && let Some(n) = ui.node_mut(me)
            {
                n.border = border;
            }
        }
        let [_l1, _t1, r1, b1] = Self::inset(ui, me);
        let item_tall = ui.psv(me, self.item_height);
        let mut y = 0;
        for &id in &self.sorted {
            let item = self.items[id];
            if !ui.is_visible(item) {
                continue;
            }
            ui.set_pos(item, 0, y);
            if let (Some(f), Some(it)) = (self.item_font, ui.ctrl::<MenuItem>(item)) {
                it.st.label.text.font = f;
            }
            y += item_tall;
            if self.check_image_width > 0
                && let Some(it) = ui.ctrl::<MenuItem>(item)
            {
                it.st.label.text_inset = (self.check_image_width, 0);
            }
        }
        if self.fixed_width == 0 {
            self.calculate_width(ui, None, s);
        } else {
            self.menu_wide = self.fixed_width;
            if self.sized_for_scroll {
                self.menu_wide -= self.sb_size;
            }
        }
        let [l2, t2, _, _] = Self::inset(ui, me);
        let w = ui.psv(me, self.blur_offset) - l2 - t2 + self.menu_wide;
        let h = ui.psv(me, self.item_height);
        for &item in &self.items {
            ui.set_size(item, w, h);
        }
        let extra = if self.sized_for_scroll { self.sb_size } else { 0 };
        ui.set_size(me, self.menu_wide + extra + ui.psv(me, 5), r1 + y + b1);
        if ui.is_visible(self.scroller) {
            ui.set_enabled(self.scroller, false);
            ScrollBar::set_range_window(ui, self.scroller, self.num_visible);
            ScrollBar::set_range(ui, self.scroller, 0, self.items.len() as i32);
            ScrollBar::set_button_pressed_scroll_value(ui, self.scroller, 1);
            let (ww, hh) = ui.size(me);
            let [_, t, r, b] = Self::inset(ui, me);
            ui.set_pos(self.scroller, ww - t - self.sb_size, 1);
            ui.set_size(self.scroller, self.sb_size, hh - r - b);
        }
        for &item in &self.items {
            ui.invalidate_layout(item);
        }
    }

    /// `SetVisible` (`CGameMenu::SetVisible`: always shown; hiding only moves it to the back).
    fn set_visible_impl(&mut self, ui: &mut Ui, me: PanelId, state: bool) {
        if self.game.is_some() {
            self.set_visible_base(ui, me, true);
            if !state {
                ui.move_to_back(me);
            }
            return;
        }
        self.set_visible_base(ui, me, state);
    }

    fn set_visible_base(&mut self, ui: &mut Ui, me: PanelId, state: bool) {
        if state == ui.is_visible(me) {
            return;
        }
        if !state {
            ui.post_action_signal(me, Msg::new("MenuClose"));
            self.set_selected(ui, me, -1);
        } else {
            ui.move_to_front(me);
            ui.request_focus(me);
        }
        ui.set_visible(me, state);
        self.sized_for_scroll = false;
    }

    /// `SetCurrentlySelectedItem`: disarms the old item, `MenuItemHighlight{itemID}`.
    fn set_selected(&mut self, ui: &mut Ui, me: PanelId, id: i32) {
        if id == self.current {
            return;
        }
        if let Some(item) = self.item_id(self.current) {
            ui.post(item, Msg::new("DisarmItem"), None, 0.0);
        }
        ui.post_action_signal(me, Msg::new("MenuItemHighlight").int("itemID", id));
        self.current = id;
    }

    /// Arms an item directly (`MenuItem::ArmItem` invoked by the menu).
    fn arm(&mut self, ui: &mut Ui, me: PanelId, id: i32, s: &mut dyn Surface) {
        let Some(item) = self.item_id(id) else { return };
        ui.with_ctrl(item, |c, ui| {
            if let Some(it) = c.as_any_mut().downcast_mut::<MenuItem>() {
                notify_highlight(ui, item, it.game_highlight, true, s);
                it.st.set_armed(ui, item, true, s);
            }
        });
        self.force_calculate_width(ui, me, s);
    }

    fn disarm(&mut self, ui: &mut Ui, me: PanelId, id: i32, s: &mut dyn Surface) {
        let Some(item) = self.item_id(id) else { return };
        ui.with_ctrl(item, |c, ui| {
            if let Some(it) = c.as_any_mut().downcast_mut::<MenuItem>() {
                notify_highlight(ui, item, it.game_highlight, false, s);
                if !it.st.is_key_down() {
                    it.st.set_armed(ui, item, false, s);
                }
            }
        });
        self.force_calculate_width(ui, me, s);
    }

    fn set_highlighted(&mut self, ui: &mut Ui, me: PanelId, id: i32, s: &mut dyn Surface) {
        self.set_selected(ui, me, id);
        let Some(row) = self.sorted.iter().position(|&x| x as i32 == id) else { return };
        let row = row as i32;
        if ui.is_visible(self.scroller) {
            let v = ScrollBar::value(ui, self.scroller);
            if row > v + self.num_visible - 1 || row < v {
                ScrollBar::set_value(ui, self.scroller, row);
            }
        }
        if let Some(item) = self.item_id(self.current) {
            let armed = ui.ctrl::<MenuItem>(item).is_some_and(|it| it.st.is_armed());
            if !armed {
                self.arm(ui, me, self.current, s);
            }
        }
    }

    /// `ActivateItem`.
    fn activate(&mut self, ui: &mut Ui, id: i32, s: &mut dyn Surface) {
        let Some(item) = self.item_id(id) else { return };
        if !ui.is_enabled(item) {
            return;
        }
        ui.with_ctrl(item, |c, ui| {
            if let Some(it) = c.as_any_mut().downcast_mut::<MenuItem>() {
                it.st.do_click(ui, item, s);
            }
        });
        self.activated = id;
    }

    /// `OnKillFocus`.
    fn on_kill_focus(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        let f = ui.focus();
        if let Some(f) = f
            && ui.has_parent(f, me)
        {
            return;
        }
        if !ui.is_keyboard_input_enabled(me) && f.is_none() {
            return;
        }
        ui.post_action_signal(me, Msg::new("MenuClose"));
        self.set_visible_impl(ui, me, false);
        if self.game.is_some() {
            ui.move_popup_to_back(me);
        }
    }

    /// `MoveAlongMenuItemList(dir, loop)`.
    fn move_along(&mut self, ui: &mut Ui, me: PanelId, dir: i32, depth: i32) {
        let count = self.sorted.len() as i32;
        if count == 0 {
            return;
        }
        let sb_visible = ui.is_visible(self.scroller);
        let mut row = self.sorted.iter().position(|&x| x as i32 == self.current).map_or(-1, |r| r as i32) + dir;
        if row > count - 1 {
            row = if sb_visible { count - 1 } else { 0 };
        } else if row < 0 {
            row = if sb_visible { ScrollBar::value(ui, self.scroller) } else { count - 1 };
        }
        if sb_visible {
            let v = ScrollBar::value(ui, self.scroller);
            if row > v + self.num_visible - 1 || row < v {
                ScrollBar::set_value(ui, self.scroller, v + dir);
                ui.invalidate_layout(me);
            }
            let v = ScrollBar::value(ui, self.scroller);
            if row > v + self.num_visible - 1 || row < v {
                ScrollBar::set_value(ui, self.scroller, row);
            }
        }
        self.set_selected(ui, me, self.sorted[row as usize] as i32);
        if depth < self.items.len() as i32
            && let Some(item) = self.item_id(self.current)
        {
            let blank = ui.ctrl::<MenuItem>(item).is_some_and(|it| it.st.label.text.text.is_empty());
            if blank || !ui.is_visible(item) {
                self.move_along(ui, me, dir, depth + 1);
            }
        }
    }

    /// `OnKeyCodeTyped`.
    fn key_code_typed(&mut self, ui: &mut Ui, me: PanelId, code: i32, s: &mut dyn Surface) {
        let ctrl = ui.is_key_down(KEY_LCONTROL) || ui.is_key_down(KEY_RCONTROL);
        if ui.is_key_down(KEY_LALT) || ui.is_key_down(KEY_RALT) {
            ui.call_parent(me, &Msg::new("KeyCodeTyped").int("code", code), s);
            ui.post_action_signal(me, Msg::new("MenuClose"));
        }
        match code {
            KEY_ENTER => {
                if self.item_id(self.current).is_some() {
                    self.activate(ui, self.current, s);
                } else {
                    ui.call_parent(me, &Msg::new("KeyCodeTyped").int("code", code), s);
                }
            }
            KEY_ESCAPE => self.set_visible_impl(ui, me, false),
            KEY_UP | KEY_DOWN => {
                self.move_along(ui, me, if code == KEY_UP { -1 } else { 1 }, 0);
                self.arm(ui, me, self.current, s);
            }
            // No parent menu item: LEFT and RIGHT (no cascades) go to the base class.
            KEY_LEFT | KEY_RIGHT => ui.call_parent(me, &Msg::new("KeyCodeTyped").int("code", code), s),
            KEY_F4 if ctrl => self.set_visible_impl(ui, me, false),
            _ => {}
        }
    }

    /// `OnKeyTyped`: first-letter type-ahead by item ID.
    fn key_typed(&mut self, ui: &mut Ui, me: PanelId, ch: char, s: &mut dyn Surface) {
        let count = self.items.len() as i32;
        if count == 0 {
            return;
        }
        let lower = |c: char| c.to_lowercase().next().unwrap_or(c);
        let mut start = self.current.max(0);
        let mut i = if start + 1 < count { start + 1 } else { 0 };
        while i != start {
            let first = ui.ctrl::<MenuItem>(self.items[i as usize]).and_then(|it| it.st.label.text.text.first().copied());
            if first.is_some_and(|c| lower(c) == lower(ch)) {
                start = i;
                break;
            }
            i = if i + 1 < count { i + 1 } else { 0 };
        }
        self.set_highlighted(ui, me, start, s);
        ui.invalidate_layout(me);
    }
}

impl Control for Menu {
    fn class(&self) -> &'static str {
        "Menu"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("MenuItemSelected") {
            self.set_visible_impl(ui, me, false);
            ui.set_visible(self.scroller, false);
            if let Some(panel) = msg.get_panel("panel")
                && let Some(id) = self.items.iter().position(|&p| p == panel)
            {
                self.activated = id as i32;
            }
            if let Some(p) = ui.parent(me) {
                ui.post(p, msg.clone(), Some(me), 0.0);
            }
        } else if msg.is("ScrollBarSliderMoved") {
            ui.invalidate_layout(me);
        } else if msg.is("KeyModeSet") {
            self.keyboard_mode = true;
        } else if msg.is("CursorEnteredMenuItem") || msg.is("CursorExitedMenuItem") {
            if self.keyboard_mode {
                return true;
            }
            let Some(item) = msg.get_panel("VPanel") else { return true };
            let Some(id) = self.items.iter().position(|&p| p == item) else { return true };
            if msg.is("CursorEnteredMenuItem") {
                self.arm(ui, me, id as i32, s);
                self.set_selected(ui, me, id as i32);
            } else {
                self.disarm(ui, me, id as i32, s);
            }
        } else if msg.is("Command") {
            let cmd = msg.get_str("command");
            if let Some(g) = &mut self.game
                && cmd.eq_ignore_ascii_case("Open")
            {
                g.start = true;
                ui.move_to_front(me);
                ui.request_focus(me);
            } else {
                ui.post_action_signal(me, Msg::new("Command").str("command", &cmd));
            }
        } else if msg.is("KillFocus") || msg.is("MenuClose") {
            self.on_kill_focus(ui, me, s);
        } else if msg.is("CursorMoved") {
            self.keyboard_mode = false;
            let (ax, ay) = ui.abs_pos(me);
            ui.call_parent(me, &Msg::new("OnCursorMoved").int("x", msg.get_int("xpos") - ax).int("y", msg.get_int("ypos") - ay), s);
            ui.request_focus(me);
            ui.invalidate_layout(me);
        } else if msg.is("OnCursorMoved") {
            // From an item's OnCursorMoved: the menu's own handler (Panel → menu → its parent).
            self.keyboard_mode = false;
            ui.call_parent(me, msg, s);
            ui.request_focus(me);
            ui.invalidate_layout(me);
        } else if msg.is("MouseWheeled") {
            if ui.is_visible(self.scroller) {
                let v = ScrollBar::value(ui, self.scroller) - msg.get_int("delta");
                ScrollBar::set_value(ui, self.scroller, v);
                ui.invalidate_layout(me);
            }
        } else if msg.is("KeyCodePressed") {
            self.keyboard_mode = true;
            if let Some(p) = ui.parent(me) {
                ui.post(p, Msg::new("KeyModeSet"), Some(me), 0.0);
            }
        } else if msg.is("KeyCodeTyped") {
            self.key_code_typed(ui, me, msg.get_int("code"), s);
        } else if msg.is("KeyTyped") {
            let ch = char::from_u32(msg.get_int("unichar") as u32).unwrap_or('\0');
            self.key_typed(ui, me, ch, s);
        } else {
            return false;
        }
        true
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let fg = scheme.color("Menu/FgColor", Color::WHITE);
        let bg = scheme.color("Menu/BgColor", Color::WHITE);
        if let Some(n) = ui.node_mut(me) {
            n.fg = fg;
            n.bg = bg;
        }
        self.border_dark = scheme.color("BorderDark", Color::rgba(255, 255, 255, 0));
        self.recalc_width = true;
        self.calculate_width(ui, None, s);
        ui.invalidate_layout(me);
        if let Some(g) = &mut self.game {
            // CGameMenu::ApplySchemeSettings.
            ui.set_proportional(me, true);
            self.item_height = atoi(scheme.resource_string("InGameDesktop/MenuItemHeight"));
            if let Some(n) = ui.node_mut(me) {
                n.bg = Color::TRANSPARENT;
                n.border = None;
            }
            let rate = scheme.resource_string("InGameDesktop/MenuItemVisibilityRate").trim().parse::<f32>().unwrap_or(0.0);
            g.rate = if f64::from(rate) < 1e-7 { 1e-7 } else { rate };
        }
    }
    /// `CGameMenu::OnThink`: the reveal animation, one child every `rate` seconds from the
    /// last child upward; at the end the first visible enabled item is highlighted.
    fn think(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let Some(mut g) = self.game else { return };
        let now = f64::from(ui.time_ms) / 1000.0;
        if g.start {
            self.set_selected(ui, me, -1);
            g.animating = true;
            g.start_time = now;
            g.start = false;
        }
        if g.animating {
            let children = ui.children(me);
            let count = children.len() as i32;
            let mut n = ((now - g.start_time) / f64::from(g.rate)) as i32;
            if n >= count {
                n = count;
                g.animating = false;
            }
            for (i, &c) in children.iter().enumerate() {
                if let Some(node) = ui.node_mut(c) {
                    node.paint_enabled = i as i32 >= count - n;
                }
            }
            if !g.animating
                && let Some(id) = children.iter().find_map(|&c| {
                    let is_item = ui.ctrl::<MenuItem>(c).is_some();
                    (is_item && ui.is_visible(c) && ui.is_enabled(c)).then(|| self.items.iter().position(|&p| p == c)).flatten()
                })
            {
                self.set_highlighted(ui, me, id as i32, s);
            }
            ui.invalidate_layout(me);
        }
        self.game = Some(g);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.layout(ui, me, s);
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if ui.is_visible(self.scroller) {
            let (w, h) = ui.size(me);
            s.draw_set_color(self.border_dark);
            s.draw_filled_rect(w - self.sb_size, -1, w - self.sb_size + 1, h);
        }
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `Menu::MenuDirection_e`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuDirection {
    Left,
    Right,
    Up,
    Down,
    Cursor,
    AlignWithParent,
}

/// `vgui2::MenuButton`: opens its menu on mouse press.
#[derive(Debug)]
pub struct MenuButton {
    pub st: ButtonState,
    pub menu: Option<PanelId>,
    pub direction: MenuDirection,
    pub open_offset_y: i32,
}

impl MenuButton {
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str, text: &str) -> PanelId {
        let mut st = ButtonState::new(ui, parent, text);
        st.use_capture_mouse = false;
        st.activation = Activation::OnPressed;
        st.custom_click = true;
        ui.create(parent, name, Box::new(MenuButton { st, menu: None, direction: MenuDirection::Down, open_offset_y: 0 }))
    }

    /// `SetMenu`.
    pub fn set_menu(ui: &mut Ui, button: PanelId, menu: PanelId) {
        if let Some(b) = ui.ctrl::<MenuButton>(button) {
            b.menu = Some(menu);
        }
        Menu::set_visible(ui, menu, false);
        ui.add_action_signal_target(menu, button);
        ui.set_parent(menu, Some(button));
    }

    /// `HideMenu`.
    pub fn hide_menu(&mut self, ui: &mut Ui, me: PanelId) {
        if let Some(m) = self.menu {
            Menu::set_visible(ui, m, false);
            self.st.force_depressed(ui, me, false);
        }
    }

    /// `MenuButton::DoClick`.
    fn do_click(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let Some(menu) = self.menu else { return };
        if ui.is_visible(menu) {
            self.hide_menu(ui, me);
            return;
        }
        if !ui.is_enabled(menu) {
            return;
        }
        ui.perform_layout(menu, s);
        let (_, tall) = ui.size(me);
        let (x, y) = match self.direction {
            MenuDirection::Cursor => ui.cursor_pos(),
            MenuDirection::AlignWithParent if ui.parent(me).is_some() => {
                let (px, py) = ui.parent(me).map_or((0, 0), |p| ui.abs_pos(p));
                (px - 1, py + tall + self.open_offset_y)
            }
            _ => {
                let (ax, ay) = ui.abs_pos(me);
                (ax, ay + tall)
            }
        };
        let (_, mt) = ui.size(menu);
        let (_, bt) = ui.size(me);
        match self.direction {
            MenuDirection::Up => ui.set_pos(menu, x, y - bt - mt - 1),
            MenuDirection::Down => ui.set_pos(menu, x, y + 1),
            _ => ui.set_pos(menu, x + 1, y + 1),
        }
        let (mut mx, my) = ui.pos(menu);
        let (mw, mh) = ui.size(menu);
        let (ww, _) = ui.screen;
        if mx < 0 {
            mx = 0;
        }
        if mx + mw > ww {
            mx = ww - mw;
        }
        ui.set_bounds(menu, mx, my, mw, mh);
        ui.move_to_front(me);
        self.st.force_depressed(ui, me, true);
        Menu::set_visible(ui, menu, true);
        ui.request_focus(menu);
    }
}

impl Control for MenuButton {
    fn class(&self) -> &'static str {
        "MenuButton"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        let handled = if msg.is("MenuClose") {
            self.hide_menu(ui, me);
            ui.post_action_signal(me, Msg::new("MenuClose"));
            true
        } else if msg.is("KillFocus") {
            if self.menu.is_some_and(|m| ui.focus() != Some(m)) {
                self.hide_menu(ui, me);
            }
            true
        } else if msg.is("KeyCodeTyped") {
            let mods = [KEY_LSHIFT, KEY_RSHIFT, KEY_LCONTROL, KEY_RCONTROL, KEY_LALT, KEY_RALT].iter().any(|&k| ui.is_key_down(k));
            if !mods && msg.get_int("code") == KEY_ENTER {
                self.do_click(ui, me, s);
            }
            ui.call_parent(me, msg, s);
            true
        } else if msg.is("CursorEntered") {
            self.st.on_message(ui, me, msg, s);
            if let Some(p) = ui.parent(me) {
                ui.post(p, Msg::new("CursorEnteredMenuButton").with("VPanel", Value::Panel(me)), None, 0.0);
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
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.st.perform_layout(ui, me, s);
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        // DrawFocusBorder is empty: the label only.
        self.st.label.paint(ui, me, s);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `CGameMenuItem::OnArmed` / `OnDisarmed`: the highlight label glows over the item, or starts
/// fading.
fn notify_highlight(ui: &mut Ui, item: PanelId, hl: Option<PanelId>, armed: bool, s: &mut dyn Surface) {
    let Some(hl) = hl else { return };
    let msg = if armed {
        let (x, y) = ui.pos(item);
        let (w, h) = ui.size(item);
        Msg::new("GameItemArmed").int("x", x).int("y", y).int("w", w).int("h", h)
    } else {
        Msg::new("GameItemDisarmed")
    };
    ui.send(hl, &msg, Some(item), s);
}

/// Turns `menu` into GameUI's `CGameMenu` (`Menu+0x8c` blur offset 12).
pub fn make_game_menu(ui: &mut Ui, menu: PanelId) {
    if let Some(m) = ui.ctrl::<Menu>(menu) {
        m.game = Some(GameMenuExt::default());
        m.blur_offset = 12;
    }
}

/// Makes item `item` a `CGameMenuItem` with highlight label `hl`; items start unpainted (the
/// reveal animation paints them).
pub fn make_game_menu_item(ui: &mut Ui, item: PanelId, hl: PanelId) {
    if let Some(it) = ui.ctrl::<MenuItem>(item) {
        it.game_highlight = Some(hl);
    }
    if let Some(n) = ui.node_mut(item) {
        n.paint_enabled = false;
    }
}

/// `+0xf4 = 1` on the game menu (`OnGameUIActivated`, the in-game change).
pub fn restart_reveal(ui: &mut Ui, menu: PanelId) {
    if let Some(g) = ui.ctrl::<Menu>(menu).and_then(|m| m.game.as_mut()) {
        g.start = true;
    }
}
