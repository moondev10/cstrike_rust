//! `PropertySheet`, `PageTab`, `PropertyPage`, `PropertyDialog` (docs/re/vgui-controls-2.md
//! §8.4–§8.8).

use std::any::Any;

use crate::border::Break;
use crate::controls::button::{Button, ButtonState};
use crate::controls::frame::{FrameMsg, FrameState};
use crate::keys::*;
use crate::msg::{Msg, Value};
use crate::scheme::Scheme;
use crate::surface::{Color, Surface};
use crate::ui::{Control, PanelId, Ui};

// ---------------------------------------------------------------------------------------------
// PropertySheet

#[derive(Debug)]
pub struct PropertySheet {
    pages: Vec<PanelId>,
    tabs: Vec<PanelId>,
    active_page: Option<PanelId>,
    active_tab: Option<PanelId>,
    tab_height: i32,
    tab_width: i32,
    active_index: i32,
    tab_focus: bool,
}

/// Runs `f` on the property sheet `id`.
pub fn with_sheet<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut PropertySheet, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<PropertySheet>().map(|p| f(p, ui))).flatten()
}

impl PropertySheet {
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str) -> PanelId {
        let me = ui.create(parent, name, Box::new(PropertySheet { pages: Vec::new(), tabs: Vec::new(), active_page: None, active_tab: None, tab_height: 28, tab_width: 64, active_index: 0, tab_focus: false }));
        let (h, w) = (ui.psv(me, 28), ui.psv(me, 64));
        if let Some(p) = ui.ctrl::<PropertySheet>(me) {
            p.tab_height = h;
            p.tab_width = w;
        }
        me
    }

    /// `AddPage(page, title)`: a tab, `ResetData` posted; nothing activated until layout.
    pub fn add_page(&mut self, ui: &mut Ui, me: PanelId, page: PanelId, title: &str) {
        if self.pages.contains(&page) {
            return;
        }
        let tab = PageTab::create(ui, me, title, self.tab_width);
        ui.add_action_signal_target(tab, me);
        self.tabs.push(tab);
        self.pages.push(page);
        ui.set_parent(page, Some(me));
        ui.add_action_signal_target(page, me);
        ui.post(page, Msg::new("ResetData"), None, 0.0);
        ui.set_visible(page, false);
    }

    pub fn active_page(&self) -> Option<PanelId> {
        self.active_page
    }

    pub fn pages(&self) -> &[PanelId] {
        &self.pages
    }

    /// `SetTabWidth(w)` (existing tabs keep their minimum).
    pub fn set_tab_width(&mut self, ui: &mut Ui, me: PanelId, w: i32) {
        self.tab_width = w;
        ui.invalidate_layout(me);
    }

    /// `SetActivePage(page)`.
    pub fn set_active_page(&mut self, ui: &mut Ui, me: PanelId, page: PanelId) {
        if let Some(i) = self.pages.iter().position(|&p| p == page) {
            self.change_active_tab(ui, me, i as i32);
        }
    }

    /// `ResetAllData`.
    pub fn reset_all_data(&self, ui: &mut Ui, me: PanelId) {
        for &p in &self.pages {
            ui.post(p, Msg::new("ResetData"), Some(me), 0.0);
        }
    }

    /// `ApplyChanges`.
    pub fn apply_changes(&self, ui: &mut Ui, me: PanelId) {
        for &p in &self.pages {
            ui.post(p, Msg::new("ApplyChanges"), Some(me), 0.0);
        }
    }

    /// `SetPageEnabled(title, state)`: prefix match against each tab's text.
    pub fn set_page_enabled(&self, ui: &mut Ui, title: &str, state: bool) {
        for &t in &self.tabs {
            let text = gs_common::bytestr::ansi_cap(&ui.ctrl::<PageTab>(t).map(|p| p.st.label.text.text_string()).unwrap_or_default(), 50);
            let n = text.len();
            if title.get(..n).is_some_and(|head| head.eq_ignore_ascii_case(&text)) {
                ui.set_enabled(t, state);
            }
        }
    }

    /// `DeletePage(page)`.
    pub fn delete_page(&mut self, ui: &mut Ui, me: PanelId, page: PanelId, s: &mut dyn Surface) {
        let Some(i) = self.pages.iter().position(|&p| p == page) else { return };
        if self.active_page == Some(page) {
            self.change_active_tab(ui, me, i as i32 - 1);
        }
        let tab = self.tabs.remove(i);
        ui.remove_action_signal_target(tab, me);
        ui.mark_for_deletion(tab);
        self.pages.remove(i);
        ui.mark_for_deletion(page);
        self.perform_layout(ui, me, s);
    }

    /// `RemoveAllPages` (ServerBrowser.dll `sb@0x100372b0`): last to first, each tab is marked
    /// for deletion and each page unparented (not deleted); an active page falls back to the
    /// tab before it.
    pub fn remove_all_pages(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        for i in (0..self.pages.len()).rev() {
            let page = self.pages[i];
            let tab = self.tabs[i];
            ui.remove_action_signal_target(tab, me);
            if self.active_tab == Some(tab) {
                self.active_tab = None;
            }
            self.tabs.remove(i);
            ui.mark_for_deletion(tab);
            self.pages.remove(i);
            ui.set_parent(page, None);
            if self.active_page == Some(page) {
                self.active_page = None;
                self.change_active_tab(ui, me, (i as i32 - 1).max(0));
            }
        }
        self.perform_layout(ui, me, s);
    }

    /// `ChangeActiveTab(index)`: no wrap-around.
    pub fn change_active_tab(&mut self, ui: &mut Ui, me: PanelId, index: i32) {
        let Some(&page) = usize::try_from(index).ok().and_then(|i| self.pages.get(i)) else { return };
        if Some(page) == self.active_page {
            if let Some(t) = self.active_tab {
                ui.request_focus(t);
            }
            self.tab_focus = true;
            return;
        }
        if let Some(old) = self.active_page {
            ui.post(old, Msg::new("PageHide"), Some(me), 0.0);
            ui.post(old, Msg::new("PageTabActivated"), Some(me), 0.0);
            ui.set_visible(old, false);
        }
        self.tab_focus = match self.active_tab {
            Some(t) => {
                with_page_tab(ui, t, |pt, ui| pt.set_active(ui, t, false));
                ui.focus() == Some(t)
            }
            None => false,
        };
        let tab = self.tabs.get(index as usize).copied();
        self.active_page = Some(page);
        self.active_tab = tab;
        self.active_index = index;
        ui.set_visible(page, true);
        ui.move_to_front(page);
        if let Some(t) = tab {
            ui.set_visible(t, true);
            ui.move_to_front(t);
            with_page_tab(ui, t, |pt, ui| pt.set_active(ui, t, true));
        }
        match (self.tab_focus, tab) {
            (true, Some(t)) => ui.request_focus(t),
            _ => ui.request_focus(page),
        }
        ui.post(page, Msg::new("PageShow"), Some(me), 0.0);
        let mut activated = Msg::new("PageTabActivated");
        if let Some(t) = tab {
            activated = activated.with("panel", Value::Panel(t));
        }
        ui.post(page, activated, Some(me), 0.0);
        ui.post_action_signal(me, Msg::new("PageChanged"));
        ui.invalidate_layout(me);
    }
}

impl Control for PropertySheet {
    fn class(&self) -> &'static str {
        "PropertySheet"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("TabPressed") {
            if let Some(p) = msg.get_panel("panel")
                && let Some(i) = self.tabs.iter().position(|&t| t == p)
            {
                self.change_active_tab(ui, me, i as i32);
            }
        } else if msg.is("TextChanged") {
            // Only for the (never created) page combo.
        } else if msg.is("OpenContextMenu") || msg.is("ApplyButtonEnable") {
            ui.post_action_signal(me, Msg::new(&msg.name));
        } else if msg.is("DefaultButtonSet") || msg.is("CurrentDefaultButtonSet") {
            if let Some(p) = ui.parent(me) {
                let mut m = Msg::new(&msg.name);
                if let Some(b) = msg.get_panel("button") {
                    m = m.with("button", Value::Panel(b));
                }
                ui.post(p, m, None, 0.0);
            }
        } else if msg.is("FindDefaultButton") {
            if let Some(p) = ui.parent(me) {
                ui.post(p, Msg::new("FindDefaultButton"), None, 0.0);
            }
        } else if msg.is("Command") {
            let cmd = msg.get_str("command");
            if cmd.eq_ignore_ascii_case("Close")
                && let Some(p) = ui.parent(me)
            {
                ui.call_parent(me, &Msg::new("Command").str("command", &cmd), s);
                let _ = p;
            }
        } else if msg.is("KeyCodeTyped") {
            let code = msg.get_int("code");
            let ctrl = ui.is_key_down(KEY_LCONTROL) || ui.is_key_down(KEY_RCONTROL);
            match code {
                KEY_LBRACKET => {
                    if ctrl {
                        self.change_active_tab(ui, me, self.active_index - 1);
                    }
                }
                KEY_RBRACKET => {
                    if ctrl {
                        self.change_active_tab(ui, me, self.active_index + 1);
                    }
                }
                _ => ui.panel_on_key_code_typed(me, code, s),
            }
        } else {
            return false;
        }
        true
    }
    fn has_hotkey(&self, _key: char) -> bool {
        false
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        if self.active_page.is_none() {
            self.change_active_tab(ui, me, 0);
            if let Some(p) = self.active_page {
                ui.request_focus(p);
            }
        }
        let (wide, tall) = ui.size(me);
        if let Some(p) = self.active_page {
            ui.set_bounds(p, 0, self.tab_height, wide, tall - self.tab_height);
            ui.invalidate_layout(p);
        }
        let mut xpos = 0;
        let (y2, y4, h27, h25) = (ui.psv(me, 2), ui.psv(me, 4), ui.psv(me, 27), ui.psv(me, 25));
        for &t in &self.tabs {
            let tw = ui.size(t).0;
            if Some(t) == self.active_tab {
                ui.set_bounds(t, xpos, y2, tw, h27);
            } else {
                ui.set_bounds(t, xpos, y4, tw, h25);
            }
            ui.set_visible(t, true);
            xpos += tw + 1;
        }
        if let Some(p) = self.active_page {
            ui.move_to_front(p);
        }
        if let Some(t) = self.active_tab {
            ui.move_to_front(t);
        }
    }
    fn request_focus_override(&mut self, ui: &mut Ui, _me: PanelId, dir: i32) -> bool {
        if dir != -1 && dir != 0 && self.active_tab.is_some() {
            if let Some(t) = self.active_tab {
                ui.request_focus_dir(t, dir);
            }
            self.tab_focus = true;
            return true;
        }
        if let Some(p) = self.active_page {
            ui.request_focus_dir(p, dir);
            self.tab_focus = false;
        }
        true
    }
    fn request_focus_next(&mut self, ui: &mut Ui, me: PanelId, from: Option<PanelId>) -> Option<bool> {
        if self.tab_focus
            && let (Some(p), Some(t)) = (self.active_page, self.active_tab)
        {
            let right = ui.ctrl::<PageTab>(t).is_some_and(|pt| std::mem::take(&mut pt.right_pressed));
            if right {
                self.change_active_tab(ui, me, self.active_index + 1);
                return Some(true);
            }
            ui.request_focus_dir(p, 1);
            self.tab_focus = false;
            return Some(true);
        }
        Some(ui.panel_request_focus_next_base(me, from))
    }
    fn request_focus_prev(&mut self, ui: &mut Ui, me: PanelId, from: Option<PanelId>) -> Option<bool> {
        if let Some(t) = self.active_tab {
            if self.tab_focus {
                let left = ui.ctrl::<PageTab>(t).is_some_and(|pt| std::mem::take(&mut pt.left_pressed));
                if left {
                    self.change_active_tab(ui, me, self.active_index - 1);
                    return Some(true);
                }
            } else {
                if let Some(p) = ui.parent(me) {
                    ui.post(p, Msg::new("FindDefaultButton"), None, 0.0);
                }
                ui.request_focus_dir(t, -1);
                self.tab_focus = true;
                return Some(true);
            }
        }
        self.tab_focus = false;
        Some(ui.panel_request_focus_prev_base(me, from))
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// PageTab

#[derive(Debug)]
pub struct PageTab {
    pub st: ButtonState,
    pub left_pressed: bool,
    pub right_pressed: bool,
    active: bool,
    text_color: Color,
    dim_text_color: Color,
    min_width: i32,
}

fn with_page_tab<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut PageTab, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<PageTab>().map(|p| f(p, ui))).flatten()
}

impl PageTab {
    fn create(ui: &mut Ui, sheet: PanelId, title: &str, min_width: i32) -> PanelId {
        let mut st = ButtonState::new(ui, Some(sheet), title);
        st.command = Some(Msg::new("TabPressed"));
        let tab = PageTab { st, left_pressed: false, right_pressed: false, active: false, text_color: Color::TRANSPARENT, dim_text_color: Color::TRANSPARENT, min_width };
        ui.create(Some(sheet), "tab", Box::new(tab))
    }

    fn set_active(&mut self, ui: &mut Ui, me: PanelId, state: bool) {
        self.active = state;
        ui.invalidate_layout(me);
    }
}

impl Control for PageTab {
    fn class(&self) -> &'static str {
        "PageTab"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("MousePressed") {
            let code = msg.get_int("code");
            if code == MOUSE_RIGHT {
                ui.post_action_signal(me, Msg::new("OpenContextMenu").int("itemID", -1));
                return true;
            }
            if ui.is_enabled(me) && self.st.click_enabled(code) {
                ui.request_focus(me);
                if let Some(cmd) = self.st.command.clone() {
                    ui.post_action_signal(me, cmd);
                }
                self.st.set_selected(ui, me, true);
                ui.set_mouse_capture(Some(me));
            }
            return true;
        }
        if msg.is("MouseReleased") {
            ui.set_mouse_capture(None);
            self.st.set_selected(ui, me, false);
            return true;
        }
        if msg.is("KeyCodeTyped") {
            let code = msg.get_int("code");
            if code == KEY_LEFT {
                self.left_pressed = true;
            }
            if code == KEY_RIGHT {
                self.right_pressed = true;
            }
            ui.panel_on_key_code_typed(me, code, s);
            return true;
        }
        self.st.on_message(ui, me, msg, s)
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.st.apply_scheme(ui, me, scheme, s);
        let fg = ui.node(me).map_or(Color::WHITE, |n| n.fg);
        self.text_color = scheme.color("BrightControlText", fg);
        self.dim_text_color = scheme.color("FgColorDim", fg);
        let h = ui.size(me).1;
        let (cw, _) = self.st.label.content_size(ui, me, s);
        let w = self.min_width.max(cw + ui.psv(me, 10));
        ui.set_size(me, w, h);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let border = ui.scheme_of(me).and_then(|sc| sc.border(if self.active { "TabActiveBorder" } else { "TabBorder" }));
        let fg = if self.active { self.text_color } else { self.dim_text_color };
        let bg = self.st.state_bg();
        self.st.perform_layout_with(ui, me, border, fg, bg, s);
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.st.paint(ui, me, s);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// PropertyPage

/// A GameUI page's own behaviour on top of `PropertyPage` (its virtual overrides and message
/// handlers).
pub trait PageHooks: Any {
    /// `OnResetData` (slot 158).
    fn on_reset_data(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {}
    /// `OnApplyChanges` (slot 159).
    fn on_apply_changes(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {}
    /// `OnPageShow` (slot 160).
    fn on_page_show(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {}
    /// `OnPageHide` (slot 161).
    fn on_page_hide(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {}
    /// The page's other messages, before PropertyPage's; `true` when handled.
    fn on_message(&mut self, _ui: &mut Ui, _me: PanelId, _msg: &Msg, _s: &mut dyn Surface) -> bool {
        false
    }
    fn apply_scheme(&mut self, _ui: &mut Ui, _me: PanelId, _scheme: &Scheme, _s: &mut dyn Surface) {}
    fn perform_layout(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {}
    fn paint(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {}
    fn think(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {}
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// Pages without own behaviour.
#[derive(Debug, Default)]
pub struct NoHooks;

impl PageHooks for NoHooks {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct PropertyPage {
    pub paint_border: bool,
    page_tab: Option<PanelId>,
    pub hooks: Box<dyn PageHooks>,
}

impl std::fmt::Debug for PropertyPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PropertyPage").field("paint_border", &self.paint_border).field("page_tab", &self.page_tab).finish_non_exhaustive()
    }
}

/// Runs `f` on the hooks of page `id` downcast to `T`.
pub fn with_page<T: PageHooks, R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut T, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<PropertyPage>().and_then(|p| p.hooks.as_any_mut().downcast_mut::<T>()).map(|h| f(h, ui))).flatten()
}

impl PropertyPage {
    /// `new PropertyPage(parent, name, paintBorder)`: an EditablePanel.
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str, paint_border: bool, hooks: Box<dyn PageHooks>) -> PanelId {
        let me = ui.create(parent, name, Box::new(PropertyPage { paint_border, page_tab: None, hooks }));
        ui.make_editable(me);
        me
    }
}

impl Control for PropertyPage {
    fn class(&self) -> &'static str {
        "PropertyPage"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("ResetData") {
            self.hooks.on_reset_data(ui, me, s);
        } else if msg.is("ApplyChanges") {
            self.hooks.on_apply_changes(ui, me, s);
        } else if msg.is("PageShow") {
            self.hooks.on_page_show(ui, me, s);
        } else if msg.is("PageHide") {
            self.hooks.on_page_hide(ui, me, s);
        } else if msg.is("PageTabActivated") {
            self.page_tab = msg.get_panel("panel");
        } else if self.hooks.on_message(ui, me, msg, s) {
        } else if msg.is("KeyCodeTyped") && matches!(msg.get_int("code"), KEY_LEFT | KEY_RIGHT) {
            if self.page_tab.is_some_and(|t| ui.exists(t) && ui.focus() == Some(t)) {
                return false;
            }
        } else {
            return false;
        }
        true
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        if self.paint_border {
            let b = scheme.border("ButtonBorder");
            if let Some(n) = ui.node_mut(me) {
                n.border = b;
            }
        }
        self.hooks.apply_scheme(ui, me, scheme, s);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.hooks.perform_layout(ui, me, s);
    }
    fn think(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.hooks.think(ui, me, s);
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.hooks.paint(ui, me, s);
    }
    /// The top border is broken under the page's tab.
    fn paint_border(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let border = ui.node(me).and_then(|n| n.border.clone());
        if let Some(b) = border {
            let (w, h) = ui.size(me);
            match self.page_tab.filter(|&t| self.paint_border && ui.exists(t)) {
                Some(t) => {
                    let (tx, _) = ui.pos(t);
                    let tw = ui.size(t).0;
                    b.paint(s, 0, 0, w, h, Some(Break { side: 1, start: tx + 1, end: tx + tw - 1 }));
                }
                None => b.paint(s, 0, 0, w, h, None),
            }
        }
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// PropertyDialog

/// A GameUI dialog's own behaviour on top of `PropertyDialog`.
pub trait DialogHooks: Any {
    /// Messages before PropertyDialog's; `true` when handled.
    fn on_message(&mut self, _ui: &mut Ui, _me: PanelId, _msg: &Msg, _s: &mut dyn Surface) -> bool {
        false
    }
    /// `OnCommand` before PropertyDialog's; `true` when handled.
    fn on_command(&mut self, _ui: &mut Ui, _me: PanelId, _cmd: &str, _s: &mut dyn Surface) -> bool {
        false
    }
    /// After `Frame::Activate` (`Activate` override).
    fn on_activate(&mut self, _ui: &mut Ui, _me: PanelId, _d: &mut PropertyDialogParts) {}
    /// `OnOK` overrides: runs after PropertyDialog's (`PropertyDialog::OnOK(); …`).
    fn on_ok(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {}
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// Dialogs without own behaviour.
#[derive(Debug, Default)]
pub struct NoDialogHooks;

impl DialogHooks for NoDialogHooks {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// The children of a PropertyDialog.
#[derive(Debug, Clone, Copy)]
pub struct PropertyDialogParts {
    pub sheet: PanelId,
    pub ok: PanelId,
    pub cancel: PanelId,
    pub apply: PanelId,
}

pub struct PropertyDialog {
    pub fs: FrameState,
    pub parts: PropertyDialogParts,
    pub hooks: Box<dyn DialogHooks>,
}

impl std::fmt::Debug for PropertyDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PropertyDialog").field("parts", &self.parts).finish_non_exhaustive()
    }
}

/// Runs `f` on the PropertyDialog `id`.
pub fn with_property_dialog<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut PropertyDialog, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<PropertyDialog>().map(|d| f(d, ui))).flatten()
}

impl PropertyDialog {
    /// `PropertyDialog(parent, name)`.
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str, hooks: Box<dyn DialogHooks>) -> PanelId {
        let placeholder = ui.embedded();
        let parts = PropertyDialogParts { sheet: placeholder, ok: placeholder, cancel: placeholder, apply: placeholder };
        let me = ui.create(parent, name, Box::new(PropertyDialog { fs: FrameState::default(), parts, hooks }));
        FrameState::init(ui, me, false);
        let sheet = PropertySheet::create(ui, Some(me), "Sheet");
        ui.add_action_signal_target(sheet, me);
        let mk = |ui: &mut Ui, name: &str, text: &str, tab: i32, cmd: &str| {
            let b = Button::create(ui, Some(me), name, text, Some(cmd));
            ui.add_action_signal_target(b, me);
            if let Some(n) = ui.node_mut(b) {
                n.tab_position = tab;
            }
            b
        };
        if let Some(n) = ui.node_mut(sheet) {
            n.tab_position = 1;
        }
        let ok = mk(ui, "OKButton", "#PropertyDialog_OK", 2, "OK");
        ui.nav_set_default_button(me, Some(ok));
        let cancel = mk(ui, "CancelButton", "#PropertyDialog_Cancel", 3, "Close");
        let apply = mk(ui, "ApplyButton", "#PropertyDialog_Apply", 4, "Apply");
        ui.set_visible(apply, false);
        ui.set_enabled(apply, false);
        FrameState::set_sizeable(ui, me, false);
        with_property_dialog(ui, me, |d, _| d.parts = PropertyDialogParts { sheet, ok, cancel, apply });
        me
    }

    /// `AddPage(page, title)`.
    pub fn add_page(ui: &mut Ui, me: PanelId, page: PanelId, title: &str) {
        let Some(sheet) = with_property_dialog(ui, me, |d, _| d.parts.sheet) else { return };
        with_sheet(ui, sheet, |sh, ui| sh.add_page(ui, sheet, page, title));
    }

    /// `OnOK`: `ApplyChanges` to every page, then the action signal.
    fn on_ok(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let sheet = self.parts.sheet;
        with_sheet(ui, sheet, |sh, ui| sh.apply_changes(ui, sheet));
        ui.post_action_signal(me, Msg::new("ApplyChanges"));
        self.hooks.on_ok(ui, me, s);
    }

    fn command(&mut self, ui: &mut Ui, me: PanelId, cmd: &str, s: &mut dyn Surface) {
        if self.hooks.on_command(ui, me, cmd, s) {
            return;
        }
        if cmd.eq_ignore_ascii_case("OK") {
            self.on_ok(ui, me, s);
            self.command(ui, me, "Close", s);
        } else if cmd.eq_ignore_ascii_case("Cancel") {
            self.command(ui, me, "Close", s);
        } else if cmd.eq_ignore_ascii_case("Apply") {
            self.on_ok(ui, me, s);
            ui.set_enabled(self.parts.apply, false);
            ui.invalidate_layout(me);
        } else {
            self.fs.on_command(ui, me, cmd);
        }
    }

    /// `Activate` (Frame's, then the subclass's additions).
    pub fn activate(&mut self, ui: &mut Ui, me: PanelId) {
        FrameState::activate(ui, me);
        // Frame::Activate's RequestFocus(0) is PropertyDialog's: the sheet, then its page.
        ui.request_focus_dir(self.parts.sheet, 0);
        let mut parts = self.parts;
        self.hooks.on_activate(ui, me, &mut parts);
    }
}

impl Control for PropertyDialog {
    fn class(&self) -> &'static str {
        "PropertyDialog"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if self.hooks.on_message(ui, me, msg, s) {
            return true;
        }
        if msg.is("Activate") {
            self.activate(ui, me);
            return true;
        }
        if msg.is("ApplyButtonEnable") {
            if !ui.is_enabled(self.parts.apply) {
                ui.set_enabled(self.parts.apply, true);
                ui.invalidate_layout(me);
            }
            return true;
        }
        match self.fs.on_message(ui, me, msg, s) {
            FrameMsg::Handled => true,
            FrameMsg::Unhandled => false,
            FrameMsg::Command(c) => {
                self.command(ui, me, &c, s);
                true
            }
        }
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.fs.apply_scheme(ui, me, scheme, s);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.fs.perform_layout(ui, me);
        let (x, y, wide, tall) = self.fs.client_area(ui, me, s);
        let p = self.parts;
        ui.set_bounds(p.sheet, x, y, wide, tall - ui.psv(me, 32));
        let (s72, s24, s80) = (ui.psv(me, 72), ui.psv(me, 24), ui.psv(me, 80));
        let mut bx = x + wide - s80;
        let by = y + tall - ui.psv(me, 28);
        if ui.is_visible(p.apply) {
            ui.set_bounds(p.apply, bx, by, s72, s24);
            bx -= s80;
        }
        if ui.is_visible(p.cancel) {
            ui.set_bounds(p.cancel, bx, by, s72, s24);
            bx -= s80;
        }
        ui.set_bounds(p.ok, bx, by, s72, s24);
        ui.invalidate_layout(p.sheet);
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.fs.paint_background(ui, me, s);
    }
    fn request_focus_override(&mut self, ui: &mut Ui, _me: PanelId, dir: i32) -> bool {
        ui.request_focus_dir(self.parts.sheet, dir);
        true
    }
    fn frame(&mut self) -> Option<&mut FrameState> {
        Some(&mut self.fs)
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
