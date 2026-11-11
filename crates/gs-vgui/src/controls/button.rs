//! `Button` (docs/re/vgui-controls.md §6).

use std::any::Any;
use std::sync::Arc;

use crate::border::Border;
use crate::controls::label::{LabelState, TextColorState};
use crate::keys::{KEY_ENTER, KEY_SPACE, MOUSE_LEFT};
use crate::msg::Msg;
use crate::scheme::Scheme;
use crate::surface::{Color, Surface};
use crate::ui::{Control, PanelId, Ui};

/// `ActivationType_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Activation {
    #[default]
    OnPressedAndReleased,
    OnPressed,
    OnReleased,
}

/// `vgui2::Button`'s state on top of the label's.
#[derive(Debug, Clone)]
pub struct ButtonState {
    pub label: LabelState,
    armed: bool,
    depressed: bool,
    selected: bool,
    force_depressed: bool,
    pub border_enabled: bool,
    pub use_capture_mouse: bool,
    key_down: bool,
    /// Bit `code + 1` per enabled mouse button.
    click_mask: u32,
    /// `_actionMessage`.
    pub command: Option<Msg>,
    pub activation: Activation,
    default_border: Option<Arc<Border>>,
    depressed_border: Option<Arc<Border>>,
    key_focus_border: Option<Arc<Border>>,
    pub default_fg: Color,
    pub default_bg: Color,
    pub armed_fg: Color,
    pub armed_bg: Color,
    pub depressed_fg: Color,
    pub depressed_bg: Color,
    keyboard_focus_color: Color,
    default_button: bool,
    pub armed_sound: Option<String>,
    pub depressed_sound: Option<String>,
    pub released_sound: Option<String>,
    /// `MenuItem::FireActionSignal`: `MenuItemSelected{panel}` goes to this menu before the
    /// command is posted.
    pub select_notify: Option<PanelId>,
    /// The owning class overrides `DoClick` (MenuButton, ComboBox's button, …).
    pub custom_click: bool,
    clicked: bool,
}

impl ButtonState {
    /// `Button::Init`: text inset PSV(6), left clicks, pressed-and-released, CS_LIGHT text.
    pub fn new(ui: &Ui, me: Option<PanelId>, text: &str) -> Self {
        let mut label = LabelState::new(ui, text);
        label.color_state = TextColorState::Light;
        let inset = me.map_or(6, |m| ui.psv(m, 6));
        label.text_inset = (inset, 0);
        Self {
            label,
            armed: false,
            depressed: false,
            selected: false,
            force_depressed: false,
            border_enabled: true,
            use_capture_mouse: true,
            key_down: false,
            click_mask: 1 << (MOUSE_LEFT + 1),
            command: None,
            activation: Activation::OnPressedAndReleased,
            default_border: None,
            depressed_border: None,
            key_focus_border: None,
            default_fg: Color::WHITE,
            default_bg: Color::WHITE,
            armed_fg: Color::WHITE,
            armed_bg: Color::WHITE,
            depressed_fg: Color::WHITE,
            depressed_bg: Color::WHITE,
            keyboard_focus_color: Color::rgba(0, 0, 0, 255),
            default_button: false,
            armed_sound: None,
            depressed_sound: None,
            released_sound: None,
            select_notify: None,
            custom_click: false,
            clicked: false,
        }
    }

    /// `SetDefaultBorder` + `SetDepressedBorder`.
    pub fn set_borders(&mut self, default: Option<Arc<Border>>, depressed: Option<Arc<Border>>) {
        self.default_border = default;
        self.depressed_border = depressed;
    }

    /// `SetCommand(cmd)`: `Command{command = cmd}`.
    pub fn set_command(&mut self, cmd: &str) {
        self.command = Some(Msg::new("Command").str("command", cmd));
    }

    pub fn set_mouse_click_enabled(&mut self, code: i32, state: bool) {
        let bit = 1 << (code + 1);
        if state {
            self.click_mask |= bit;
        } else {
            self.click_mask &= !bit;
        }
    }

    pub fn click_enabled(&self, code: i32) -> bool {
        self.click_mask & (1 << (code + 1)) != 0
    }

    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// `_keyDown` (Enter/Space held).
    pub fn is_key_down(&self) -> bool {
        self.key_down
    }

    pub fn is_selected(&self) -> bool {
        self.selected
    }

    pub fn is_depressed(&self) -> bool {
        self.depressed
    }

    fn recalc_depressed(&mut self, ui: &mut Ui, me: PanelId) {
        let d = ui.is_enabled(me) && (self.force_depressed || (self.armed && self.selected));
        if d != self.depressed {
            self.depressed = d;
            if let Some(n) = ui.node_mut(me) {
                n.depressed = d;
            }
            ui.invalidate_layout(me);
        }
    }

    pub fn set_armed(&mut self, ui: &mut Ui, me: PanelId, state: bool, s: &mut dyn Surface) {
        if self.armed != state {
            self.armed = state;
            if state && let Some(snd) = &self.armed_sound {
                s.play_sound(snd);
            }
            ui.invalidate_layout(me);
        }
        self.recalc_depressed(ui, me);
    }

    pub fn set_selected(&mut self, ui: &mut Ui, me: PanelId, state: bool) {
        if self.selected != state {
            self.selected = state;
            ui.invalidate_layout(me);
        }
        self.recalc_depressed(ui, me);
    }

    pub fn force_depressed(&mut self, ui: &mut Ui, me: PanelId, state: bool) {
        if self.force_depressed != state {
            self.force_depressed = state;
            ui.invalidate_layout(me);
        }
        self.recalc_depressed(ui, me);
    }

    /// `SetAsDefaultButton` / `SetAsCurrentDefaultButton` (`current`): the parent learns of it
    /// through `DefaultButtonSet` / `CurrentDefaultButtonSet`.
    pub fn set_as_default_button(&mut self, ui: &mut Ui, me: PanelId, state: bool, current: bool) {
        if self.default_button != state {
            self.default_button = state;
            if state && let Some(p) = ui.parent(me) {
                let name = if current { "CurrentDefaultButtonSet" } else { "DefaultButtonSet" };
                ui.post(p, Msg::new(name).with("button", crate::msg::Value::Panel(me)), Some(me), 0.0);
            }
            ui.invalidate_layout(me);
        }
    }

    /// `Button::ApplySchemeSettings`.
    pub fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.label.apply_scheme(ui, me, scheme, s);
        self.default_border = scheme.border("ButtonBorder");
        self.depressed_border = scheme.border("ButtonDepressedBorder");
        self.key_focus_border = scheme.border("ButtonKeyFocusBorder");
        let (fg, bg) = ui.node(me).map_or((Color::WHITE, Color::WHITE), |n| (n.fg, n.bg));
        self.default_fg = scheme.color("ButtonFgColor", fg);
        self.default_bg = scheme.color("ButtonBgColor", bg);
        self.armed_fg = scheme.color("ButtonArmedFgColor", self.default_fg);
        self.armed_bg = scheme.color("ButtonArmedBgColor", self.default_bg);
        self.depressed_fg = scheme.color("ButtonDepressedFgColor", self.default_fg);
        self.depressed_bg = scheme.color("ButtonDepressedBgColor", self.default_bg);
        self.keyboard_focus_color = scheme.color("ButtonFocusBorder", Color::rgba(0, 0, 0, 255));
        ui.invalidate_layout(me);
    }

    /// `GetBorder(depressed, armed, selected, keyfocus)`.
    fn border(&self, ui: &Ui, me: PanelId) -> Option<Arc<Border>> {
        if !self.border_enabled {
            return if self.depressed { self.depressed_border.clone() } else { self.default_border.clone() };
        }
        if self.depressed {
            self.depressed_border.clone()
        } else if ui.focus() == Some(me) || (ui.is_enabled(me) && self.default_button) {
            self.key_focus_border.clone()
        } else {
            self.default_border.clone()
        }
    }

    /// `Button::PerformLayout` with the class's border and colours.
    pub fn perform_layout_with(&mut self, ui: &mut Ui, me: PanelId, border: Option<Arc<Border>>, fg: Color, bg: Color, s: &mut dyn Surface) {
        if let Some(n) = ui.node_mut(me) {
            n.border = border;
            n.fg = fg;
            n.bg = bg;
        }
        self.label.perform_layout(ui, me, s);
    }

    pub fn state_fg(&self) -> Color {
        if self.depressed {
            self.depressed_fg
        } else if self.armed {
            self.armed_fg
        } else {
            self.default_fg
        }
    }

    pub fn state_bg(&self) -> Color {
        if self.depressed {
            self.depressed_bg
        } else if self.armed {
            self.armed_bg
        } else {
            self.default_bg
        }
    }

    pub fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let border = self.border(ui, me);
        let (fg, bg) = (self.state_fg(), self.state_bg());
        self.perform_layout_with(ui, me, border, fg, bg, s);
    }

    /// `Button::Paint`: the label, then the dashed focus rectangle.
    pub fn paint(&mut self, ui: &Ui, me: PanelId, s: &mut dyn Surface) {
        self.label.paint(ui, me, s);
        if ui.focus() == Some(me) && ui.is_enabled(me) {
            let (w, h) = ui.size(me);
            s.draw_set_color(self.keyboard_focus_color);
            let d = ui.psv(me, 1);
            dashed_line(s, 3, 3, w - 4, 3 + d, d, d);
            dashed_line(s, 3, 3, 3 + d, h - 2, d, d);
            dashed_line(s, 3, h - 2 - d, w - 4, h - 2, d, d);
            dashed_line(s, w - 4 - d, 3, w - 4, h - 2, d, d);
        }
    }

    /// `DoClick`. With `custom_click` set only the click is recorded; the owning control runs
    /// its own `DoClick` after [`ButtonState::take_click`].
    pub fn do_click(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if self.custom_click {
            self.clicked = true;
            return;
        }
        if let Some(snd) = self.depressed_sound.as_ref().or(self.released_sound.as_ref()) {
            s.play_sound(snd);
        }
        self.set_selected(ui, me, true);
        if let Some(menu) = self.select_notify {
            ui.post(menu, Msg::new("MenuItemSelected").with("panel", crate::msg::Value::Panel(me)), Some(me), 0.0);
        }
        if let Some(cmd) = self.command.clone() {
            ui.post_action_signal(me, cmd);
        }
        self.set_selected(ui, me, false);
    }

    /// A click recorded under `custom_click` since the last call.
    pub fn take_click(&mut self) -> bool {
        std::mem::take(&mut self.clicked)
    }

    /// The class-independent message handling of `Button`; `true` when handled.
    pub fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) -> bool {
        if msg.is("CursorEntered") {
            if ui.is_enabled(me) {
                self.set_armed(ui, me, true, s);
            }
        } else if msg.is("CursorExited") {
            if !self.key_down {
                self.set_armed(ui, me, false, s);
            }
        } else if msg.is("MousePressed") || msg.is("MouseDoublePressed") {
            self.mouse_pressed(ui, me, msg.get_int("code"), s);
        } else if msg.is("MouseReleased") {
            self.mouse_released(ui, me, msg.get_int("code"), s);
        } else if msg.is("KeyCodePressed") {
            let code = msg.get_int("code");
            if code == KEY_ENTER || code == KEY_SPACE {
                self.set_armed(ui, me, true, s);
                self.key_down = true;
                self.mouse_pressed(ui, me, MOUSE_LEFT, s);
                if self.use_capture_mouse {
                    ui.set_mouse_capture(None);
                }
            } else {
                self.key_down = false;
                ui.call_parent(me, msg, s);
            }
        } else if msg.is("KeyCodeReleased") {
            let code = msg.get_int("code");
            if self.key_down && (code == KEY_ENTER || code == KEY_SPACE) {
                self.set_armed(ui, me, true, s);
                self.mouse_released(ui, me, MOUSE_LEFT, s);
            } else {
                ui.call_parent(me, msg, s);
            }
            self.key_down = false;
            self.set_armed(ui, me, false, s);
        } else if msg.is("Hotkey") || msg.is("PressButton") {
            if ui.is_enabled(me) {
                self.do_click(ui, me, s);
            }
        } else if msg.is("SetAsDefaultButton") || msg.is("SetAsCurrentDefaultButton") {
            self.set_as_default_button(ui, me, msg.get_int("state") != 0, msg.is("SetAsCurrentDefaultButton"));
        } else {
            return false;
        }
        true
    }

    pub fn mouse_pressed(&mut self, ui: &mut Ui, me: PanelId, code: i32, s: &mut dyn Surface) {
        if !ui.is_enabled(me) || !self.click_enabled(code) {
            return;
        }
        match self.activation {
            Activation::OnPressed => self.do_click(ui, me, s),
            Activation::OnPressedAndReleased if self.use_capture_mouse => {
                ui.request_focus(me);
                self.set_selected(ui, me, true);
                ui.set_mouse_capture(Some(me));
            }
            _ => {}
        }
    }

    pub fn mouse_released(&mut self, ui: &mut Ui, me: PanelId, code: i32, s: &mut dyn Surface) {
        if self.use_capture_mouse {
            ui.set_mouse_capture(None);
        }
        if self.activation == Activation::OnPressed || !self.click_enabled(code) {
            return;
        }
        if self.selected || self.activation != Activation::OnPressedAndReleased {
            if ui.is_enabled(me) && (ui.mouse_over() == Some(me) || self.key_down) {
                self.do_click(ui, me, s);
            } else {
                self.set_selected(ui, me, false);
            }
        }
    }
}

/// `Label::DrawDashedLine`.
pub fn dashed_line(s: &mut dyn Surface, x0: i32, y0: i32, x1: i32, y1: i32, dash: i32, gap: i32) {
    if dash + gap <= 0 {
        return;
    }
    if x1 - x0 <= y1 - y0 {
        let mut y = y0;
        loop {
            s.draw_filled_rect(x0, y, x1, (y + dash).min(y1));
            y += dash + gap;
            if y > y1 {
                break;
            }
        }
    } else {
        let mut x = x0;
        loop {
            s.draw_filled_rect(x, y0, (x + dash).min(x1), y1);
            x += dash + gap;
            if x > x1 {
                break;
            }
        }
    }
}

/// Runs `f` on the `ButtonState` of the Button `id`.
pub fn with_button<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut ButtonState, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<Button>().map(|b| f(&mut b.st, ui))).flatten()
}

/// A plain Button panel.
#[derive(Debug)]
pub struct Button {
    pub st: ButtonState,
}

impl Button {
    /// `new Button(parent, name, text)` (+ `SetCommand` when `command` is given).
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str, text: &str, command: Option<&str>) -> PanelId {
        let id = ui.create(parent, name, Box::new(Button { st: ButtonState::new(ui, None, text) }));
        let inset = ui.psv(id, 6);
        if let Some(b) = ui.ctrl::<Button>(id) {
            b.st.label.text_inset = (inset, 0);
            if let Some(c) = command {
                b.st.set_command(c);
            }
        }
        id
    }
}

impl Control for Button {
    fn class(&self) -> &'static str {
        "Button"
    }
    fn can_be_default_button(&self) -> bool {
        true
    }
    fn has_hotkey(&self, key: char) -> bool {
        self.st.label.hotkey == Some(key)
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        self.st.on_message(ui, me, msg, s)
    }
    /// `Button::ApplySettings`: Label's, the light colour state, `command`, `default`.
    fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &gs_formats::keyvalues::Block, s: &mut dyn Surface) {
        self.st.label.apply_settings(ui, me, kv, s);
        self.st.label.color_state = TextColorState::Light;
        if let Some(cmd) = crate::editable::kv_str(kv, "command").filter(|c| !c.is_empty()) {
            self.st.set_command(cmd);
        }
        if crate::editable::kv_int(kv, "default", 0) != 0 {
            self.st.set_as_default_button(ui, me, true, false);
        }
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.st.apply_scheme(ui, me, scheme, s);
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
