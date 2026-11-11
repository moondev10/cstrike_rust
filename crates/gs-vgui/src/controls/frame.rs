//! `Frame` with its title-bar buttons, system menu button and resize/caption grips
//! (docs/re/vgui-controls.md §8). A frame-derived class embeds [`FrameState`], returns it from
//! [`Control::frame`] and routes its messages through [`FrameState::on_message`].

use std::any::Any;
use std::sync::Arc;

use crate::border::Border;
use crate::controls::button::{Activation, ButtonState};
use crate::controls::label::{Align, LabelImage, TextImage};
use crate::controls::menu::{Menu, MenuButton, MenuDirection};
use crate::keys::*;
use crate::msg::Msg;
use crate::scheme::Scheme;
use crate::surface::{Color, Cursor, Surface};
use crate::ui::{Control, PanelId, Ui};

/// What a frame-derived class does with a message after [`FrameState::on_message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameMsg {
    Handled,
    Unhandled,
    /// The virtual `OnCommand(cmd)`: the class handles it or falls back to
    /// [`FrameState::on_command`].
    Command(String),
}

/// The `Frame` part of a frame-derived panel.
#[derive(Debug)]
pub struct FrameState {
    pub title: TextImage,
    pub moveable: bool,
    pub sizeable: bool,
    has_focus: bool,
    pub draw_title_bar: bool,
    pub delete_self_on_close: bool,
    prev_modal: Option<PanelId>,
    title_fg: Color,
    title_bg: Color,
    title_dis_fg: Color,
    title_dis_bg: Color,
    grips: Vec<PanelId>,
    caption: Option<PanelId>,
    minimize: Option<PanelId>,
    maximize: Option<PanelId>,
    systray: Option<PanelId>,
    close: Option<PanelId>,
    menu_button: Option<PanelId>,
    /// `SetCloseButtonSound`: played by `OnCloseFrameButtonPressed` before `Close`.
    pub close_sound: Option<String>,
    /// The `SetTitle` text before dialog-variable substitution.
    title_src: String,
    /// `EditablePanel::m_pDialogVariables` (`SetDialogVariable`): `%name%` in the title.
    dialog_vars: Vec<(String, String)>,
}

impl Default for FrameState {
    fn default() -> Self {
        Self {
            title: TextImage::default(),
            moveable: true,
            sizeable: true,
            has_focus: false,
            draw_title_bar: true,
            delete_self_on_close: false,
            prev_modal: None,
            title_fg: Color::TRANSPARENT,
            title_bg: Color::TRANSPARENT,
            title_dis_fg: Color::TRANSPARENT,
            title_dis_bg: Color::TRANSPARENT,
            grips: Vec::new(),
            caption: None,
            minimize: None,
            maximize: None,
            systray: None,
            close: None,
            menu_button: None,
            close_sound: None,
            title_src: String::new(),
            dialog_vars: Vec::new(),
        }
    }
}

/// `frame`'s [`FrameState`] (through [`Control::frame`]).
pub fn with_frame<R>(ui: &mut Ui, frame: PanelId, f: impl FnOnce(&mut FrameState, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(frame, |c, ui| c.frame().map(|fs| f(fs, ui))).flatten()
}

impl FrameState {
    /// The `Frame` constructor's work on an already created panel `me` (whose control embeds
    /// a default `FrameState`).
    pub fn init(ui: &mut Ui, me: PanelId, force_proportional: bool) {
        ui.make_editable(me);
        if force_proportional {
            ui.set_proportional(me, true);
        }
        ui.set_visible(me, false);
        ui.make_popup(me);
        if let Some(n) = ui.node_mut(me) {
            n.build_group.push(me);
            n.nav.top_level = true;
        }
        let (mw, mh) = (ui.psv(me, 128), ui.psv(me, 66));
        ui.set_min_size(me, mw, mh);
        let mults = [(0, -1), (0, 1), (-1, 0), (1, 0), (-1, -1), (1, -1), (-1, 1), (1, 1)];
        let mut grips = Vec::new();
        for (i, &m) in mults.iter().enumerate() {
            let g = ui.create(Some(me), "", Box::new(GripPanel { frame: me, mult: m, caption: false, image: i == 7, dragging: false, drag_start: (0, 0), org_pos: (0, 0), org_size: (0, 0) }));
            if let Some(n) = ui.node_mut(g) {
                n.paint_enabled = i == 7;
                n.paint_background_enabled = i == 7;
                n.paint_border_enabled = false;
            }
            grips.push(g);
        }
        let caption = ui.create(Some(me), "", Box::new(GripPanel { frame: me, mult: (0, 0), caption: true, image: false, dragging: false, drag_start: (0, 0), org_pos: (0, 0), org_size: (0, 0) }));
        if let Some(n) = ui.node_mut(caption) {
            n.cursor = Cursor::Arrow;
            n.paint_enabled = false;
            n.paint_background_enabled = false;
            n.paint_border_enabled = false;
        }
        // Minimize and close: `SetCommand(new KeyValues(name))`, the message itself; the
        // sys-tray button: `SetCommand("MinimizeToSysTray")`, a `Command`.
        let minimize = FrameButton::create(ui, me, Some("resource/icon_minimize"), Some(Msg::new("Minimize")));
        let maximize = FrameButton::create(ui, me, Some("resource/icon_maximize"), None);
        ui.set_visible(maximize, false);
        let systray = FrameButton::create(ui, me, None, Some(Msg::new("Command").str("command", "MinimizeToSysTray")));
        ui.set_visible(systray, false);
        let close = FrameButton::create(ui, me, Some("resource/icon_close"), Some(Msg::new("CloseFrameButtonPressed")));
        // The engine surface lacks FRAME_MINIMIZE_MAXIMIZE.
        ui.set_visible(minimize, false);
        ui.set_visible(maximize, false);
        let menu_button = FrameSystemButton::create(ui, me);
        let fs = FrameState { grips, caption: Some(caption), minimize: Some(minimize), maximize: Some(maximize), systray: Some(systray), close: Some(close), menu_button: Some(menu_button), ..FrameState::default() };
        with_frame(ui, me, |f, _| *f = fs);
        with_frame(ui, me, |f, ui| f.set_title(ui, me, "Untitled"));
        Self::setup_resize_cursors(ui, me);
    }

    fn setup_resize_cursors(ui: &mut Ui, me: PanelId) {
        let Some((grips, sizeable)) = with_frame(ui, me, |f, _| (f.grips.clone(), f.sizeable)) else { return };
        let cursors = [Cursor::SizeNs, Cursor::SizeNs, Cursor::SizeWe, Cursor::SizeWe, Cursor::SizeNwSe, Cursor::SizeNeSw, Cursor::SizeNeSw, Cursor::SizeNwSe];
        for (i, &g) in grips.iter().enumerate() {
            if let Some(n) = ui.node_mut(g) {
                n.cursor = if sizeable { cursors[i] } else { Cursor::Arrow };
                if i == 7 {
                    n.paint_enabled = sizeable;
                    n.paint_background_enabled = sizeable;
                }
            }
        }
    }

    /// `SetTitle(title, surfaceTitle)`.
    pub fn set_title(&mut self, ui: &mut Ui, _me: PanelId, title: &str) {
        self.title_src = title.to_owned();
        self.refresh_title(ui);
    }

    /// `SetDialogVariable(name, value)`: the title's `%name%` takes the value.
    pub fn set_dialog_variable(&mut self, ui: &mut Ui, name: &str, value: &str) {
        match self.dialog_vars.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
            Some(v) => v.1 = value.to_owned(),
            None => self.dialog_vars.push((name.to_owned(), value.to_owned())),
        }
        self.refresh_title(ui);
    }

    fn refresh_title(&mut self, ui: &mut Ui) {
        if self.dialog_vars.is_empty() {
            self.title.set_text(ui, &self.title_src);
            return;
        }
        let mut t = crate::controls::label::localized(ui, &self.title_src);
        for (k, v) in &self.dialog_vars {
            t = t.replace(&format!("%{k}%"), v);
        }
        self.title.set_text_raw(&t);
    }

    pub fn set_sizeable(ui: &mut Ui, me: PanelId, state: bool) {
        with_frame(ui, me, |f, _| f.sizeable = state);
        Self::setup_resize_cursors(ui, me);
    }

    pub fn set_moveable(&mut self, state: bool) {
        self.moveable = state;
    }

    /// `SetTitleBarVisible`.
    pub fn set_title_bar_visible(&mut self, ui: &mut Ui, state: bool) {
        self.draw_title_bar = state;
        for b in [self.menu_button, self.minimize, self.maximize, self.close].into_iter().flatten() {
            ui.set_visible(b, state);
        }
    }

    pub fn set_close_button_visible(&self, ui: &mut Ui, state: bool) {
        if let Some(b) = self.close {
            ui.set_visible(b, state);
        }
    }

    pub fn set_menu_button_visible(&self, ui: &mut Ui, state: bool) {
        if let Some(b) = self.menu_button {
            ui.set_visible(b, state);
        }
    }

    /// `ApplySchemeSettings`.
    pub fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        self.title_fg = scheme.color("TitleBarFgColor", Color::WHITE);
        self.title_bg = scheme.color("TitleBarBgColor", Color::WHITE);
        self.title_dis_fg = scheme.color("TitleBarDisabledFgColor", Color::WHITE);
        self.title_dis_bg = scheme.color("TitleBarDisabledBgColor", Color::WHITE);
        let (prop, hd) = (ui.is_proportional(me), ui.is_hd_proportional(me));
        self.title.font = s.font(&scheme.file, "Default", prop, hd);
        let (w, h) = self.title.content_size(s);
        self.title.set_size(w, h);
    }

    /// `s` of PaintBackground / PerformLayout / LayoutProportional: the screen height over the
    /// tall of `Panel::GetProportionalBase` (the HD base in an HD-proportional frame).
    fn scale(ui: &Ui, me: PanelId) -> f32 {
        if ui.is_proportional(me) { ui.screen.1 as f32 / ui.proportional_base_of(me).1 as f32 } else { 1.0 }
    }

    /// `PerformLayout`.
    pub fn perform_layout(&mut self, ui: &mut Ui, me: PanelId) {
        let (w, h) = ui.size(me);
        let (g5, g8, br) = (ui.psv(me, 5), ui.psv(me, 8), ui.psv(me, 18));
        if let [top, bottom, left, right, tl, tr, bl, brp] = self.grips[..] {
            ui.set_bounds(top, g8, 0, w - 2 * g8, g5);
            ui.set_bounds(left, 0, g8, g5, h - 2 * g8);
            ui.set_bounds(tl, 0, 0, g8, g8);
            ui.set_bounds(tr, w - g8, 0, g8, g8);
            ui.set_bounds(bl, 0, h - g8, g8, g8);
            ui.set_bounds(bottom, g8, h - g5, w - (g8 + br), g5);
            ui.set_bounds(right, w - g5, g8, g5, h - (g8 + br));
            ui.set_bounds(brp, w - br, h - br, br, br);
        }
        if let Some(c) = self.caption {
            let (cw, ch) = (w - ui.psv(me, 10), ui.psv(me, 23));
            ui.set_size(c, cw, ch);
        }
        // The eight resize grips and four title buttons; the caption grip (+0xe8) and the close
        // button stay where they are, so the buttons created after the caption grip stay on it.
        let mut front: Vec<PanelId> = self.grips.clone();
        front.extend([self.maximize, self.menu_button, self.minimize, self.systray].into_iter().flatten());
        for p in front {
            ui.move_to_front(p);
        }
        if let Some(m) = self.menu_button {
            let (a, b, c) = (ui.psv(me, 7), ui.psv(me, 8), ui.psv(me, 18));
            ui.set_bounds(m, a, b, c, c);
        }
        let prop = ui.is_proportional(me);
        let s = Self::scale(ui, me);
        let (bw, bx, by) = if prop { ((20.0 * s) as i32, (5.0 * s) as i32, (8.0 * s) as i32) } else { (20, 5, 8) };
        let layout_prop = |ui: &mut Ui, b: PanelId| {
            let (size, inset) = if prop { ((18.0 * s) as i32, ((2.0 * s).ceil() as i32, s.ceil() as i32)) } else { (18, (2, 1)) };
            ui.set_size(b, size, size);
            if let Some(fb) = ui.ctrl::<FrameButton>(b) {
                fb.st.label.text_inset = inset;
            }
        };
        let mut used = bw;
        if let Some(c) = self.close.filter(|&c| ui.is_visible(c)) {
            ui.set_pos(c, w - bx - bw, by);
            layout_prop(ui, c);
            used = 2 * bw;
        }
        if let Some(t) = self.systray.filter(|&t| ui.is_visible(t)) {
            ui.set_pos(t, w - bx - used, by);
            layout_prop(ui, t);
            used += bw;
        }
        if let Some(m) = self.maximize.filter(|&m| ui.is_visible(m)) {
            ui.set_pos(m, w - bx - used, by);
            layout_prop(ui, m);
            used += bw;
        }
        if let Some(m) = self.minimize.filter(|&m| ui.is_visible(m)) {
            ui.set_pos(m, w - bx - used, by);
            layout_prop(ui, m);
        }
    }

    /// `GetClientArea` → `(x, y, w, h)`.
    pub fn client_area(&self, ui: &Ui, me: PanelId, s: &dyn Surface) -> (i32, i32, i32, i32) {
        let x = ui.psv(me, 5);
        let (mut w, mut t) = ui.size(me);
        let mut y = 0;
        if self.draw_title_bar {
            y = s.font_tall(self.title.font) + ui.psv(me, 5) + ui.psv(me, 8);
            t -= ui.psv(me, 5) + y;
        }
        w -= ui.psv(me, 5) + x;
        (x, y, w, t)
    }

    /// `PaintBackground`.
    pub fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let wide = ui.size(me).0;
        let tall = s.font_tall(self.title.font);
        ui.panel_paint_background(me, s);
        let has_focus = ui.focus().is_some_and(|f| ui.has_parent(f, me));
        let title_bg = if has_focus { self.title_bg } else { self.title_dis_bg };
        if has_focus != self.has_focus {
            self.has_focus = has_focus;
            for b in [self.minimize, self.maximize, self.close, self.systray].into_iter().flatten() {
                if let Some(fb) = ui.ctrl::<FrameButton>(b) {
                    fb.disabled_look = !has_focus;
                    fb.apply_look();
                }
                ui.invalidate_layout(b);
            }
            if let Some(m) = self.menu_button {
                ui.set_enabled(m, has_focus);
                FrameSystemButton::update_look(ui, m, has_focus);
                ui.invalidate_layout(m);
            }
        }
        if let Some(b) = ui.scheme_of(me).and_then(|sc| sc.border("FrameBorder")) {
            let (w, h) = ui.size(me);
            b.paint_panel(s, w, h);
        }
        if self.draw_title_bar {
            let sc = Self::scale(ui, me);
            s.draw_set_color(title_bg);
            s.draw_filled_rect((5.0 * sc) as i32, (5.0 * sc) as i32, (wide as f32 - 5.0 * sc) as i32, (28.0 * sc) as i32);
            self.title.color = if has_focus { self.title_fg } else { self.title_dis_fg };
            self.title.pos = ((28.0 * sc) as i32, (9.0 * sc) as i32);
            self.title.set_size((wide as f32 - 72.0 * sc) as i32, tall);
            self.title.paint(s);
        }
    }

    /// `Activate`: front, focus, visible, enabled (no fade).
    pub fn activate(ui: &mut Ui, me: PanelId) {
        ui.move_to_front(me);
        ui.request_focus(me);
        ui.set_visible(me, true);
        ui.set_enabled(me, true);
    }

    /// `Close` = `OnClose` (with `EditablePanel::OnClose`'s `SaveUserConfig`).
    pub fn close(&mut self, ui: &mut Ui, me: PanelId) {
        ui.save_frame_user_config(me, self);
        if ui.app_modal() == Some(me) {
            ui.set_app_modal(self.prev_modal.take());
        }
        ui.set_visible(me, false);
        if self.delete_self_on_close {
            ui.mark_for_deletion(me);
        }
    }

    /// `DoModal`.
    pub fn do_modal(&mut self, ui: &mut Ui, me: PanelId) {
        Self::move_to_center_of_screen(ui, me);
        ui.invalidate_layout(me);
        Self::activate(ui, me);
        self.prev_modal = ui.app_modal();
        ui.set_app_modal(Some(me));
    }

    /// `CloseModal`: release the modal surface, then `Close` to self.
    pub fn close_modal(&mut self, ui: &mut Ui, me: PanelId) {
        if ui.app_modal() == Some(me) {
            ui.set_app_modal(self.prev_modal.take());
        }
        ui.post(me, Msg::new("Close"), None, 0.0);
    }

    /// `MoveToCenterOfScreen` (workspace origin ignored).
    pub fn move_to_center_of_screen(ui: &mut Ui, me: PanelId) {
        let (ww, wt) = ui.screen;
        let (w, h) = ui.size(me);
        ui.set_pos(me, (ww - w) / 2, (wt - h) / 2);
    }

    /// `Frame::OnCommand` defaults; `false` for commands Frame ignores.
    pub fn on_command(&mut self, ui: &mut Ui, me: PanelId, cmd: &str) -> bool {
        if cmd.eq_ignore_ascii_case("Close") {
            self.close(ui, me);
        } else if cmd.eq_ignore_ascii_case("CloseModal") {
            self.close_modal(ui, me);
        } else if cmd.eq_ignore_ascii_case("Minimize") {
            ui.set_visible(me, false);
        } else {
            return cmd.eq_ignore_ascii_case("MinimizeToSysTray");
        }
        true
    }

    /// Frame's message handling (before EditablePanel's/Panel's defaults).
    pub fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) -> FrameMsg {
        if msg.is("Command") {
            return FrameMsg::Command(msg.get_str("command"));
        }
        if msg.is("CloseFrameButtonPressed") {
            if let Some(snd) = &self.close_sound {
                s.play_sound(snd);
            }
            return FrameMsg::Command("Close".to_owned());
        }
        if msg.is("Close") {
            self.close(ui, me);
        } else if msg.is("Minimize") {
            ui.set_visible(me, false);
        } else if msg.is("KeyCodePressed") {
            if msg.get_int("code") == KEY_ESCAPE {
                ui.post(me, Msg::new("CloseFrameButtonPressed"), None, 0.0);
                ui.post(me, Msg::new("Command").str("command", "Cancel"), None, 0.0);
            }
        } else if msg.is("KeyCodeTyped") {
            let code = msg.get_int("code");
            let ctrl = ui.is_key_down(KEY_LCONTROL) || ui.is_key_down(KEY_RCONTROL);
            if ctrl && code == KEY_F4 {
                ui.post(me, Msg::new("CloseFrameButtonPressed"), None, 0.0);
            } else if code == KEY_ENTER {
                let b = ui.node(me).and_then(|n| n.nav.current_default_button);
                if let Some(b) = b.filter(|&b| ui.is_visible(b) && ui.is_enabled(b)) {
                    ui.post(b, Msg::new("Hotkey"), Some(me), 0.0);
                }
            } else if code == KEY_ESCAPE && ui.app_modal() == Some(me) {
                self.close_modal(ui, me);
            }
        } else if msg.is("KeyTyped") {
            let ch = char::from_u32(msg.get_int("unichar") as u32).unwrap_or('\0');
            if let Some(p) = ui.nav_find_panel_by_hotkey(me, ch) {
                ui.post(p, Msg::new("Hotkey"), Some(me), 0.0);
            }
        } else if msg.is("MousePressed") {
            let inside = ui.focus().is_some_and(|f| ui.has_parent(f, me));
            if !inside {
                ui.request_focus(me);
            }
        } else if msg.is("FlashWindow") {
            // The OS taskbar flash has no in-game effect.
        } else {
            let _ = s;
            return FrameMsg::Unhandled;
        }
        FrameMsg::Handled
    }
}

/// A plain `Frame` (no subclass behaviour).
#[derive(Debug, Default)]
pub struct Frame {
    pub fs: FrameState,
}

impl Frame {
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str) -> PanelId {
        let me = ui.create(parent, name, Box::new(Frame::default()));
        FrameState::init(ui, me, false);
        me
    }
}

impl Control for Frame {
    fn class(&self) -> &'static str {
        "Frame"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        match self.fs.on_message(ui, me, msg, s) {
            FrameMsg::Handled => true,
            FrameMsg::Unhandled => false,
            FrameMsg::Command(c) => {
                self.fs.on_command(ui, me, &c);
                true
            }
        }
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.fs.apply_scheme(ui, me, scheme, s);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        self.fs.perform_layout(ui, me);
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

/// A title-bar button (`FrameButton`): an icon, 18×18, no focus.
#[derive(Debug)]
pub struct FrameButton {
    pub st: ButtonState,
    icon: Option<String>,
    pub disabled_look: bool,
    en_fg: Color,
    en_bg: Color,
    dis_fg: Color,
    dis_bg: Color,
    bright: Option<Arc<Border>>,
    depressed: Option<Arc<Border>>,
    disabled: Option<Arc<Border>>,
}

impl FrameButton {
    fn create(ui: &mut Ui, frame: PanelId, icon: Option<&str>, command: Option<Msg>) -> PanelId {
        let mut st = ButtonState::new(ui, Some(frame), "");
        st.label.align = Align::NorthWest;
        st.label.text_inset = (2, 1);
        st.command = command;
        let b = ui.create(
            Some(frame),
            "",
            Box::new(FrameButton {
                st,
                icon: icon.map(str::to_owned),
                disabled_look: true,
                en_fg: Color::TRANSPARENT,
                en_bg: Color::TRANSPARENT,
                dis_fg: Color::TRANSPARENT,
                dis_bg: Color::TRANSPARENT,
                bright: None,
                depressed: None,
                disabled: None,
            }),
        );
        ui.set_size(b, 18, 18);
        b
    }

    /// `SetDisabledLook`: every colour pair follows the enabled or disabled colours.
    fn apply_look(&mut self) {
        let (fg, bg) = if self.disabled_look { (self.dis_fg, self.dis_bg) } else { (self.en_fg, self.en_bg) };
        self.st.default_fg = fg;
        self.st.default_bg = bg;
        self.st.armed_fg = fg;
        self.st.armed_bg = bg;
        self.st.depressed_fg = fg;
        self.st.depressed_bg = bg;
        let border = if self.disabled_look { self.disabled.clone() } else { self.bright.clone() };
        self.st.set_borders(border, self.depressed.clone());
    }
}

impl Control for FrameButton {
    fn class(&self) -> &'static str {
        "FrameButton"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("MousePressed") || msg.is("MouseDoublePressed") {
            // Button's press without RequestFocus.
            if ui.is_enabled(me) && msg.get_int("code") == MOUSE_LEFT && self.st.use_capture_mouse {
                self.st.set_selected(ui, me, true);
                ui.set_mouse_capture(Some(me));
            }
            return true;
        }
        self.st.on_message(ui, me, msg, s)
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.st.apply_scheme(ui, me, scheme, s);
        self.en_bg = scheme.color("TitleButtonBgColor", Color::WHITE);
        self.en_fg = scheme.color("TitleButtonFgColor", Color::WHITE);
        self.dis_bg = scheme.color("TitleButtonDisabledBgColor", Color::WHITE);
        self.dis_fg = scheme.color("TitleButtonDisabledFgColor", Color::WHITE);
        self.bright = scheme.border("TitleButtonBorder");
        self.depressed = scheme.border("TitleButtonDepressedBorder");
        self.disabled = scheme.border("TitleButtonDisabledBorder");
        self.apply_look();
        if let Some(icon) = &self.icon {
            let tex = s.texture(icon, true);
            let size = tex.map_or((0, 0), |t| (t.width, t.height));
            self.st.label.add_image(LabelImage::Texture { name: icon.clone(), tex, size, color: Color::WHITE }, 0);
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

/// The system-menu button at the title bar's left (`FrameSystemButton`, a MenuButton).
#[derive(Debug)]
pub struct FrameSystemButton {
    pub mb: MenuButton,
    frame: PanelId,
    en_col: Color,
    dis_col: Color,
    en_icon: Option<String>,
    dis_icon: Option<String>,
}

impl FrameSystemButton {
    fn create(ui: &mut Ui, frame: PanelId) -> PanelId {
        let mut st = ButtonState::new(ui, Some(frame), "");
        st.use_capture_mouse = false;
        st.activation = Activation::OnPressed;
        st.custom_click = true;
        st.set_mouse_click_enabled(MOUSE_RIGHT, true);
        let mb = MenuButton { st, menu: None, direction: MenuDirection::Down, open_offset_y: 0 };
        let b = ui.create(Some(frame), "FrameMenuButton", Box::new(FrameSystemButton { mb, frame, en_col: Color::TRANSPARENT, dis_col: Color::TRANSPARENT, en_icon: None, dis_icon: None }));
        ui.set_enabled(b, false);
        // GetSysMenu: Minimize / Maximize / Close, enabled by the matching buttons' visibility.
        let menu = Menu::create(ui, Some(frame), "SysMenu");
        Menu::add_item(ui, menu, "Minimize", "#SysMenu_Minimize", "Minimize", Some(frame), None);
        Menu::add_item(ui, menu, "Maximize", "#SysMenu_Maximize", "Maximize", Some(frame), None);
        Menu::add_item(ui, menu, "Close", "#SysMenu_Close", "Close", Some(frame), None);
        Menu::set_item_enabled(ui, menu, 0, false);
        Menu::set_item_enabled(ui, menu, 1, false);
        if let Some(c) = ui.ctrl::<FrameSystemButton>(b) {
            c.mb.menu = Some(menu);
        }
        Menu::set_visible(ui, menu, false);
        ui.add_action_signal_target(menu, b);
        ui.set_parent(menu, Some(b));
        b
    }

    /// `SetEnabled`: icon and colours follow the state.
    fn update_look(ui: &mut Ui, me: PanelId, enabled: bool) {
        let Some(c) = ui.ctrl::<FrameSystemButton>(me) else { return };
        let col = if enabled { c.en_col } else { c.dis_col };
        let st = &mut c.mb.st;
        st.default_fg = col;
        st.default_bg = col;
        st.armed_fg = col;
        st.armed_bg = col;
        st.depressed_fg = col;
        st.depressed_bg = col;
        let icon = if enabled { c.en_icon.clone() } else { c.dis_icon.clone() };
        if let (Some(LabelImage::Texture { name, tex, size, .. }), Some(icon)) = (st.label.last_image_mut(), icon) {
            *name = icon;
            *tex = None;
            *size = (0, 0);
        }
    }
}

impl Control for FrameSystemButton {
    fn class(&self) -> &'static str {
        "FrameSystemButton"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("MouseDoublePressed") && msg.get_int("code") == MOUSE_LEFT {
            ui.post(self.frame, Msg::new("CloseFrameButtonPressed"), Some(me), 0.0);
            return true;
        }
        self.mb.on_message(ui, me, msg, from, s)
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.mb.st.apply_scheme(ui, me, scheme, s);
        self.en_col = scheme.color("TitleBarBgColor", Color::WHITE);
        self.dis_col = scheme.color("TitleBarDisabledBgColor", Color::WHITE);
        let en = scheme.resource_string("TitleBarIcon").to_owned();
        let dis = scheme.resource_string("TitleBarDisabledIcon").to_owned();
        self.en_icon = Some(en.clone()).filter(|s| !s.is_empty());
        self.dis_icon = Some(dis.clone()).filter(|s| !s.is_empty());
        self.mb.st.label.text_inset = (0, 0);
        let enabled = ui.is_enabled(me);
        let icon = if enabled { en } else { dis };
        let col = if enabled { self.en_col } else { self.dis_col };
        let st = &mut self.mb.st;
        st.default_fg = col;
        st.default_bg = col;
        st.armed_fg = col;
        st.armed_bg = col;
        st.depressed_fg = col;
        st.depressed_bg = col;
        st.set_borders(None, None);
        if !icon.is_empty() {
            let tex = s.texture(&icon, true);
            let size = tex.map_or((0, 0), |t| (t.width, t.height));
            st.label.set_image_at(0, LabelImage::Texture { name: icon, tex, size, color: Color::WHITE }, 0);
        }
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        // Resolve an icon swapped by update_look.
        if let Some(LabelImage::Texture { name, tex, size, .. }) = self.mb.st.label.last_image_mut()
            && tex.is_none()
        {
            *tex = s.texture(name, true);
            *size = tex.map_or((0, 0), |t| (t.width, t.height));
        }
        self.mb.st.perform_layout(ui, me, s);
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.mb.st.label.paint(ui, me, s);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A resize grip (`GripPanel`) or the caption drag area (`CaptionGripPanel`).
#[derive(Debug)]
pub struct GripPanel {
    frame: PanelId,
    mult: (i32, i32),
    caption: bool,
    /// The bottom-right grip paints `resource/icon_resizer`.
    image: bool,
    dragging: bool,
    drag_start: (i32, i32),
    org_pos: (i32, i32),
    org_size: (i32, i32),
}

impl GripPanel {
    fn moved(&self, ui: &mut Ui, dx: i32, dy: i32) {
        let frame = self.frame;
        let Some((sizeable, moveable)) = with_frame(ui, frame, |f, _| (f.sizeable, f.moveable)) else { return };
        if self.caption {
            if !moveable {
                return;
            }
            self.caption_moved(ui, dx, dy);
            return;
        }
        if !sizeable {
            return;
        }
        let (min_w, min_h) = ui.node(frame).map_or((0, 0), |n| n.min_size());
        let (mut x, mut y) = self.org_pos;
        let (mut dx, mut dy) = (dx, dy);
        let w = self.org_size.0 + self.mult.0 * dx;
        if self.mult.0 == -1 {
            if w < min_w {
                dx = self.org_size.0 - min_w;
            }
            x += dx;
        }
        let h = self.org_size.1 + self.mult.1 * dy;
        if self.mult.1 == -1 {
            if h < min_h {
                dy = self.org_size.1 - min_h;
            }
            y += dy;
        }
        ui.set_pos(frame, x, y);
        ui.set_size(frame, w, h);
        ui.invalidate_layout(frame);
    }

    /// `CaptionGripPanel::moved`: workspace-edge snap (9 px), then docking to other popups
    /// (10 px).
    fn caption_moved(&self, ui: &mut Ui, dx: i32, dy: i32) {
        let frame = self.frame;
        let (mut nx, mut ny) = (self.org_pos.0 + dx, self.org_pos.1 + dy);
        let (ww, wt) = ui.screen;
        let (fw, fh) = ui.size(frame);
        let near = |d: i32| (d.wrapping_add(9) as u32) <= 18;
        if near(nx) {
            nx = 0;
        } else if near(nx + fw - ww) {
            nx = ww - fw;
        }
        if near(ny) {
            ny = 0;
        } else if near(ny + fh - wt) {
            ny = wt - fh;
        }
        for other in ui.children(ui.embedded()) {
            if other == frame || !ui.is_popup(other) || !ui.is_visible(other) {
                continue;
            }
            let (ox, oy) = ui.pos(other);
            let (ow, oh) = ui.size(other);
            let v_overlap = ny < oy + oh && ny + fh > oy;
            let h_overlap = nx < ox + ow && nx + fw > ox;
            if v_overlap {
                if (nx - (ox + ow)).abs() <= 10 {
                    nx = ox + ow;
                } else if (nx + fw - ox).abs() <= 10 {
                    nx = ox - fw;
                }
            }
            if h_overlap {
                if (ny - (oy + oh)).abs() <= 10 {
                    ny = oy + oh;
                } else if (ny + fh - oy).abs() <= 10 {
                    ny = oy - fh;
                }
            }
        }
        ui.set_pos(frame, nx, ny);
    }
}

impl Control for GripPanel {
    fn class(&self) -> &'static str {
        if self.caption { "CaptionGripPanel" } else { "GripPanel" }
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, _s: &mut dyn Surface) -> bool {
        if msg.is("MousePressed") && msg.get_int("code") == MOUSE_LEFT {
            self.dragging = true;
            self.drag_start = ui.cursor_pos();
            self.org_pos = ui.pos(self.frame);
            self.org_size = ui.size(self.frame);
            ui.set_mouse_capture(Some(me));
            let inside = ui.focus().is_some_and(|f| ui.has_parent(f, self.frame));
            if !inside {
                ui.request_focus(self.frame);
            }
        } else if msg.is("CursorMoved") {
            if self.dragging {
                if ui.is_mouse_down(MOUSE_LEFT) {
                    let (x, y) = ui.cursor_pos();
                    self.moved(ui, x - self.drag_start.0, y - self.drag_start.1);
                } else {
                    self.dragging = false;
                    ui.set_mouse_capture(None);
                }
            }
        } else if msg.is("MouseReleased") {
            self.dragging = false;
            ui.set_mouse_capture(None);
        } else if msg.is("MouseCaptureLost") {
            self.dragging = false;
        } else {
            return false;
        }
        true
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, _s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let fg = scheme.color("BorderBright", Color::WHITE);
        let bg = scheme.color("BorderSelection", Color::WHITE);
        if let Some(n) = ui.node_mut(me) {
            n.fg = fg;
            n.bg = bg;
        }
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.paint(ui, me, s);
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if !self.image {
            return;
        }
        if let Some(t) = s.texture("resource/icon_resizer", false) {
            let (w, h) = (ui.psv(me, 14), ui.psv(me, 14));
            s.draw_set_color(Color::WHITE);
            s.draw_textured_rect(t, 0, 0, w, h);
        }
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
