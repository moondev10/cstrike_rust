//! `CGameConsole` / `CGameConsoleDialog` (docs/re/gameui.md §5).

use std::any::Any;

use gs_vgui::controls::button::Button;
use gs_vgui::controls::frame::{FrameMsg, FrameState};
use gs_vgui::controls::menu::Menu;
use gs_vgui::controls::rich_text::{RichText, with_rt};
use gs_vgui::controls::text_entry::{TabCatching, TextEntry, with_te};
use gs_vgui::keys::*;
use gs_vgui::scheme::Scheme;
use gs_vgui::{Color, Control, Msg, PanelId, Surface, Ui};

use crate::{HostRequest, Shared};

/// `CompletionItem`.
#[derive(Debug, Clone)]
struct Completion {
    is_command: bool,
    text: String,
    extra: Option<String>,
}

impl Completion {
    /// `GetItemText`.
    fn item_text(&self) -> String {
        match &self.extra {
            Some(e) => format!("{} {}", self.text, e),
            None => self.text.clone(),
        }
    }
}

/// `CHistoryItem`.
#[derive(Debug, Clone)]
struct HistoryItem {
    text: String,
    extra: Option<String>,
}

/// `CGameConsoleDialog`.
#[derive(Debug)]
pub struct ConsoleDialog {
    pub fs: FrameState,
    host: Shared,
    pub history: PanelId,
    pub entry: PanelId,
    submit: PanelId,
    completion: PanelId,
    print_color: Color,
    dprint_color: Color,
    auto_complete_mode: bool,
    next_completion: i32,
    partial: String,
    completions: Vec<Completion>,
    items: Vec<HistoryItem>,
}

/// Runs `f` on the console dialog `id`.
pub fn with_console<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut ConsoleDialog, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<ConsoleDialog>().map(|d| f(d, ui))).flatten()
}

/// `GetText(buf, 256)`.
fn text256(ui: &mut Ui, entry: PanelId) -> String {
    gs_common::bytestr::ansi_cap(&with_te(ui, entry, |te, _| te.text()).unwrap_or_default(), 256)
}

impl ConsoleDialog {
    /// `new CGameConsoleDialog()` + `CGameConsole::Initialize`'s bounds; `parent` is the taskbar.
    pub fn create(ui: &mut Ui, host: Shared, parent: PanelId) -> PanelId {
        let placeholder = ui.embedded();
        let me = ui.create(
            None,
            "GameConsole",
            Box::new(ConsoleDialog {
                fs: FrameState::default(),
                host,
                history: placeholder,
                entry: placeholder,
                submit: placeholder,
                completion: placeholder,
                print_color: Color::TRANSPARENT,
                dprint_color: Color::TRANSPARENT,
                auto_complete_mode: false,
                next_completion: 0,
                partial: String::new(),
                completions: Vec::new(),
                items: Vec::new(),
            }),
        );
        FrameState::init(ui, me, false);
        ui.set_hd_proportional(me, true);
        ui.set_proportional(me, true);
        ui.make_popup(me);
        ui.set_visible(me, false);
        let m = ui.psv(me, 100);
        ui.set_min_size(me, m, m);
        crate::taskbar::center(ui, me);
        gs_vgui::controls::frame::with_frame(ui, me, |f, ui| f.set_title(ui, me, "#GameUI_Console"));
        let submit = Button::create(ui, Some(me), "ConsoleSubmit", "#GameUI_Submit", Some("submit"));
        let completion = Menu::create(ui, Some(me), "CompletionList");
        ui.make_popup(completion);
        Menu::set_visible(ui, completion, false);
        let entry = TextEntry::create(ui, Some(me), "ConsoleEntry");
        with_te(ui, entry, |te, _| {
            te.tab_catching = Some(TabCatching { completion_list: Some(completion), ignore_next_char: false });
            te.set_allow_non_ascii(true);
            te.send_new_line(true);
        });
        ui.add_action_signal_target(entry, me);
        if let Some(n) = ui.node_mut(entry) {
            n.tab_position = 1;
        }
        let history = RichText::create(ui, Some(me), "ConsoleHistory");
        with_rt(ui, history, |rt, ui| {
            rt.focus_panel = Some(entry);
            rt.set_vertical_scrollbar(ui, history, true);
        });
        with_console(ui, me, |d, _| {
            d.history = history;
            d.entry = entry;
            d.submit = submit;
            d.completion = completion;
        });
        ui.set_parent(me, Some(parent));
        let (sw, sh) = ui.screen;
        let off = ui.psv(me, 40);
        let w = ui.psv(me, 560).min(sw - 2 * off);
        let h = ui.psv(me, 400).min(sh - 2 * off);
        ui.set_bounds(me, off, off, w, h);
        me
    }

    /// `Print`.
    pub fn print(&mut self, ui: &mut Ui, text: &str) {
        let (h, c) = (self.history, self.print_color);
        with_rt(ui, h, |rt, ui| {
            rt.insert_color_change(c);
            rt.insert_string(ui, h, text);
        });
    }

    /// `DPrint`.
    pub fn dprint(&mut self, ui: &mut Ui, text: &str) {
        let (h, c) = (self.history, self.dprint_color);
        with_rt(ui, h, |rt, ui| {
            rt.insert_color_change(c);
            rt.insert_string(ui, h, text);
        });
    }

    /// `Activate`.
    pub fn activate(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        FrameState::activate(ui, me);
        ui.request_focus(self.entry);
        with_te(ui, self.entry, |te, _| {
            if let Some(tc) = &mut te.tab_catching {
                tc.ignore_next_char = false;
            }
        });
        ui.invalidate_layout_ex(self.history, false, true, s);
    }

    /// `Hide`: `Frame::Close`, completion index reset.
    pub fn hide(&mut self, ui: &mut Ui, me: PanelId) {
        self.fs.close(ui, me);
        self.next_completion = 0;
    }

    /// The history text for `condump`.
    pub fn history_text(&self, ui: &mut Ui) -> String {
        with_rt(ui, self.history, |rt, _| rt.text()).unwrap_or_default()
    }

    /// `AddToHistory(cmd, extra)`.
    fn add_to_history(&mut self, ui: &mut Ui, cmd: &str, extra: Option<&str>) {
        while self.items.len() >= 100 {
            self.items.remove(0);
        }
        let command = cmd.strip_suffix(' ').unwrap_or(cmd).to_owned();
        let ext = extra.map(|e| e.trim_end_matches(' ').to_owned());
        let mut i = self.items.len();
        while i > 0 {
            i -= 1;
            let it = &self.items[i];
            let same_extra = match (&ext, &it.extra) {
                (None, None) => true,
                (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                _ => false,
            };
            if it.text.eq_ignore_ascii_case(&command) && same_extra {
                self.items.remove(i);
            }
        }
        self.items.push(HistoryItem { text: command, extra: ext });
        self.next_completion = 0;
        let partial = self.partial.clone();
        self.rebuild_completion_list(&partial);
        let _ = ui;
    }

    /// `RebuildCompletionList(text)`: the history for an empty entry, else command and cvar
    /// names with that prefix (engine list order, no sort).
    fn rebuild_completion_list(&mut self, text: &str) {
        self.completions.clear();
        if text.is_empty() {
            for h in &self.items {
                self.completions.push(Completion { is_command: false, text: h.text.clone(), extra: h.extra.clone() });
            }
            return;
        }
        if text.contains(' ') {
            return;
        }
        let host = self.host.borrow();
        let n = text.len();
        let prefix = |name: &str| name.len() >= n && name.as_bytes()[..n].eq_ignore_ascii_case(text.as_bytes());
        for name in host.command_names.iter().chain(host.cvar_names.iter()) {
            if prefix(name) {
                self.completions.push(Completion { is_command: true, text: name.clone(), extra: None });
            }
        }
    }

    /// `OnAutoComplete(reverse)`.
    fn on_auto_complete(&mut self, ui: &mut Ui, reverse: bool, s: &mut dyn Surface) {
        let count = self.completions.len() as i32;
        let mut idx = if !self.auto_complete_mode {
            self.next_completion = 0;
            self.auto_complete_mode = true;
            0
        } else {
            self.next_completion
        };
        if reverse {
            idx = self.next_completion - 2;
            self.next_completion = idx;
            if idx < 0 {
                idx = count - 1;
                self.next_completion = idx;
            }
        }
        if idx < 0 || idx >= count {
            idx = 0;
            self.next_completion = 0;
            if count <= 0 {
                return;
            }
        }
        let item = &self.completions[idx as usize];
        let text = if item.is_command { item.text.clone() } else { item.item_text() };
        let mut buf: String = text.chars().take(254).collect();
        if !buf.contains(' ') {
            buf.push(' ');
        }
        let entry = self.entry;
        with_te(ui, entry, |te, ui| {
            te.set_text(ui, entry, &buf, s);
            te.select_none();
            te.goto_text_end(ui, entry, s);
        });
        self.next_completion += 1;
    }

    /// `OnTextChanged(panel)`.
    fn on_text_changed(&mut self, ui: &mut Ui, me: PanelId, panel: Option<PanelId>, s: &mut dyn Surface) {
        if panel != Some(self.entry) {
            return;
        }
        self.partial = text256(ui, self.entry);
        if !self.auto_complete_mode && self.partial.ends_with(' ') {
            self.next_completion = 0;
            self.on_auto_complete(ui, false, s);
        }
        self.auto_complete_mode = false;
        let partial = self.partial.clone();
        self.rebuild_completion_list(&partial);
        let list = self.completion;
        if self.completions.is_empty() {
            Menu::set_visible(ui, list, false);
        } else {
            Menu::set_visible(ui, list, true);
            Menu::delete_all_items(ui, list);
            for i in 0..self.completions.len().min(7) {
                let text = if i == 6 { "...".to_owned() } else { self.completions[i].item_text() };
                Menu::add_item_msg(ui, list, &text, &text, Msg::new("CompletionCommand").str("command", &text), Some(me), None);
            }
            self.position_completion_list(ui, me);
        }
        ui.request_focus(me);
        ui.request_focus(self.entry);
    }

    /// `PositionCompletionList`: the list below the entry, the entry's focus, then the console
    /// (`this->MoveToFront`, not the list) to the front. As the top popup it keeps the key focus
    /// in the entry; the list, under the console in the popup order, shows below its bottom edge
    /// (vgui-surface §6.4.1).
    fn position_completion_list(&self, ui: &mut Ui, me: PanelId) {
        let (dx, dy) = ui.pos(me);
        let (ex, ey) = ui.pos(self.entry);
        let off = ui.psv(me, 32);
        ui.set_pos(self.completion, dx + ex, dy + ey + off);
        ui.request_focus(self.entry);
        ui.move_to_front(me);
    }

    /// `OnCommand("Submit")`.
    fn submit(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let buf = text256(ui, self.entry);
        self.host.borrow_mut().requests.push(HostRequest::ClientCmd(buf.clone()));
        let (h, c) = (self.history, self.print_color);
        with_rt(ui, h, |rt, ui| {
            for piece in ["] ", buf.as_str(), "\n"] {
                rt.insert_color_change(c);
                rt.insert_string(ui, h, piece);
            }
        });
        let entry = self.entry;
        with_te(ui, entry, |te, ui| te.set_text(ui, entry, "", s));
        self.on_text_changed(ui, me, Some(entry), s);
        with_rt(ui, h, |rt, ui| rt.goto_text_end(ui));
        let (cmd, extra) = match buf.split_once(' ') {
            Some((c, e)) => (c.to_owned(), Some(e.to_owned())),
            None => (buf.clone(), None),
        };
        self.add_to_history(ui, &cmd, extra.as_deref());
        Menu::set_visible(ui, self.completion, false);
    }

    /// `CGameConsoleDialog::OnKeyCodeTyped` after Frame's.
    fn key_code_typed(&mut self, ui: &mut Ui, me: PanelId, code: i32, s: &mut dyn Surface) {
        let toggle_key = self.host.borrow().toggle_console_key;
        let shift = ui.is_key_down(KEY_LSHIFT) || ui.is_key_down(KEY_RSHIFT);
        if ui.focus() == Some(self.entry) {
            match code {
                KEY_TAB => {
                    self.on_auto_complete(ui, shift, s);
                    ui.request_focus(self.entry);
                }
                KEY_DOWN => {
                    self.on_auto_complete(ui, false, s);
                    ui.request_focus(self.entry);
                }
                KEY_UP => {
                    self.on_auto_complete(ui, true, s);
                    ui.request_focus(self.entry);
                }
                _ => {}
            }
        }
        self.partial = text256(ui, self.entry);
        let ctrl = ui.is_key_down(KEY_LCONTROL) || ui.is_key_down(KEY_RCONTROL);
        if self.partial.is_empty() && ctrl {
            if code == KEY_C || code == KEY_X {
                ui.send(self.history, &Msg::new("DoCopySelected"), Some(me), s);
            }
            if code == KEY_A {
                with_rt(ui, self.history, |rt, _| rt.select_all_text());
            }
        }
        if !shift && toggle_key != 0 && code == toggle_key {
            let entry = self.entry;
            with_te(ui, entry, |te, ui| {
                te.set_text(ui, entry, "", s);
                if let Some(tc) = &mut te.tab_catching {
                    tc.ignore_next_char = true;
                }
            });
            ui.post(me, Msg::new("Close"), None, 0.0);
            self.host.borrow_mut().requests.push(HostRequest::HideGameUiFromConsole);
        }
    }
}

impl Control for ConsoleDialog {
    fn class(&self) -> &'static str {
        "CGameConsoleDialog"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("Activate") {
            self.activate(ui, me, s);
            return true;
        }
        if msg.is("TextChanged") {
            let p = msg.get_panel("panel");
            self.on_text_changed(ui, me, p, s);
            return true;
        }
        if msg.is("CloseCompletionList") {
            Menu::set_visible(ui, self.completion, false);
            return true;
        }
        if msg.is("CompletionCommand") {
            let cmd = msg.get_str("command");
            if cmd.contains("...") {
                Menu::set_visible(ui, self.completion, true);
            } else {
                let entry = self.entry;
                with_te(ui, entry, |te, ui| {
                    te.set_text(ui, entry, &cmd, s);
                    te.goto_text_end(ui, entry, s);
                    te.insert_char(ui, entry, ' ', s);
                    te.goto_text_end(ui, entry, s);
                });
            }
            return true;
        }
        if msg.is("KeyCodeTyped") {
            let r = self.fs.on_message(ui, me, msg, s);
            if let FrameMsg::Command(c) = r {
                self.fs.on_command(ui, me, &c);
            }
            self.key_code_typed(ui, me, msg.get_int("code"), s);
            return true;
        }
        match self.fs.on_message(ui, me, msg, s) {
            FrameMsg::Handled => true,
            FrameMsg::Unhandled => false,
            FrameMsg::Command(c) => {
                if c.eq_ignore_ascii_case("Submit") {
                    self.submit(ui, me, s);
                } else {
                    self.fs.on_command(ui, me, &c);
                }
                true
            }
        }
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.fs.apply_scheme(ui, me, scheme, s);
        self.print_color = ui.node(me).map_or(Color::WHITE, |n| n.fg);
        self.dprint_color = scheme.color("BrightControlText", Color::WHITE);
        let mono = self.host.borrow().con_mono > 0.0;
        with_rt(ui, self.history, |rt, _| rt.mono = mono);
        ui.invalidate_layout(me);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        self.fs.perform_layout(ui, me);
        ui.nav_set_default_button(me, Some(self.submit));
        let border = ui.scheme_of(me).and_then(|sc| sc.border("DepressedButtonBorder"));
        for p in [self.entry, self.history] {
            if let Some(n) = ui.node_mut(p) {
                n.border = border.clone();
            }
        }
        let (wide, tall) = ui.size(me);
        let (o, top, y4, eh, sw) = (ui.psv(me, 8), ui.psv(me, 28), ui.psv(me, 4), ui.psv(me, 24), ui.psv(me, 64));
        ui.set_pos(self.history, o, o + top);
        ui.set_size(self.history, wide - 2 * o, tall - 2 * (y4 + o) - top - eh);
        ui.set_pos(self.entry, o, tall - 2 * y4 - eh);
        ui.set_size(self.entry, wide - 3 * o - sw, eh);
        ui.set_pos(self.submit, wide - sw - o, tall - 2 * y4 - eh);
        ui.set_size(self.submit, sw, eh);
        self.position_completion_list(ui, me);
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.fs.paint_background(ui, me, s);
    }
    fn frame(&mut self) -> Option<&mut FrameState> {
        Some(&mut self.fs)
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

