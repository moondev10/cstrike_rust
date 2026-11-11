//! `vgui2::TextEntry` and GameUI's `TabCatchingTextEntry` (docs/re/vgui-controls-2.md §2).

use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};

use gs_formats::keyvalues::Block;

use crate::controls::menu::Menu;
use crate::controls::scroll_bar::ScrollBar;
use crate::editable::kv_int;
use crate::keys::*;
use crate::msg::Msg;
use crate::scheme::Scheme;
use crate::surface::{Color, Cursor, FontHandle, Surface};
use crate::ui::{Control, PanelId, Ui};

/// The function-static `s_bIsShowing` toggled by Enter in every catch-enter single-line entry.
static SHOW_KEYBOARD: AtomicBool = AtomicBool::new(false);

const LB_END: i32 = 999_999;

fn iswprint(c: char) -> bool {
    !c.is_control() && c != '\0'
}

fn iswspace(c: char) -> bool {
    c.is_whitespace()
}

/// GameUI's `TabCatchingTextEntry` overrides (the console input line).
#[derive(Debug, Clone, Copy)]
pub struct TabCatching {
    pub completion_list: Option<PanelId>,
    pub ignore_next_char: bool,
}

/// `vgui2::TextEntry`.
#[derive(Debug)]
pub struct TextEntry {
    text: Vec<char>,
    undo: Vec<char>,
    lb: Vec<i32>,
    cursor: i32,
    cursor_is_at_end: bool,
    put_cursor_at_end: bool,
    undo_cursor: i32,
    cursor_blink: bool,
    hide_text: bool,
    editable: bool,
    mouse_selection: bool,
    mouse_drag_selection: bool,
    cursor_next_blink: u32,
    select: [i32; 2],
    pixels_indent: i32,
    char_count: i32,
    max_char_count: i32,
    font: FontHandle,
    data_changed: bool,
    multiline: bool,
    vertical_scrollbar: bool,
    vsb: Option<PanelId>,
    cursor_color: Color,
    disabled_fg: Color,
    disabled_bg: Color,
    selection_color: Color,
    selection_text_color: Color,
    selection_bg2: Color,
    current_start_line: i32,
    current_start_index: i32,
    horiz_scrolling: bool,
    catch_enter: bool,
    wrap: bool,
    send_new_lines: bool,
    draw_width: i32,
    scroll_bar_size: i32,
    edit_menu: Option<PanelId>,
    recalc_breaks_index: i32,
    numeric_only: bool,
    allow_non_ascii: bool,
    auto_progress: bool,
    pub tab_catching: Option<TabCatching>,
    /// A size change whose font-dependent part (`OnSizeChanged`'s scroll-back and
    /// `_drawWidth`) runs at the next layout, before any paint.
    pending_size: Option<i32>,
}

/// Runs `f` on the TextEntry of panel `id`.
pub fn with_te<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut TextEntry, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<TextEntry>().map(|te| f(te, ui))).flatten()
}

impl TextEntry {
    /// The state after the constructor (cursor at the (empty) end, editable).
    pub fn new(ui: &Ui, proportional: bool) -> Self {
        let scroll_bar_size = if proportional {
            // surface()->GetProportionalBase directly: never the HD base.
            let (sw, _) = ui.screen;
            let (bw, _) = ui.base_size(false);
            (sw as f32 / bw as f32 * 18.0) as i32
        } else {
            18
        };
        Self {
            text: Vec::new(),
            undo: Vec::new(),
            lb: vec![LB_END],
            cursor: 0,
            cursor_is_at_end: false,
            put_cursor_at_end: true,
            undo_cursor: 0,
            cursor_blink: false,
            hide_text: false,
            editable: true,
            mouse_selection: false,
            mouse_drag_selection: false,
            cursor_next_blink: ui.time_ms.wrapping_add(400),
            select: [-1, -1],
            pixels_indent: 0,
            char_count: 0,
            max_char_count: -1,
            font: FontHandle(0),
            data_changed: false,
            multiline: false,
            vertical_scrollbar: false,
            vsb: None,
            cursor_color: Color::TRANSPARENT,
            disabled_fg: Color::TRANSPARENT,
            disabled_bg: Color::TRANSPARENT,
            selection_color: Color::TRANSPARENT,
            selection_text_color: Color::TRANSPARENT,
            selection_bg2: Color::TRANSPARENT,
            current_start_line: 0,
            current_start_index: 0,
            horiz_scrolling: true,
            catch_enter: false,
            wrap: false,
            send_new_lines: false,
            draw_width: 0,
            scroll_bar_size,
            edit_menu: None,
            recalc_breaks_index: 0,
            numeric_only: false,
            allow_non_ascii: false,
            auto_progress: false,
            tab_catching: None,
            pending_size: None,
        }
    }

    /// `new TextEntry(parent, name)`.
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str) -> PanelId {
        let prop = parent.is_some_and(|p| ui.is_proportional(p));
        let te = TextEntry::new(ui, prop);
        let me = ui.create(parent, name, Box::new(te));
        if let Some(n) = ui.node_mut(me) {
            n.cursor = Cursor::IBeam;
        }
        me
    }

    fn len(&self) -> i32 {
        self.text.len() as i32
    }

    /// `text[i]`; out-of-range reads (the source's unchecked `text[-1]`) give NUL.
    fn at(&self, i: i32) -> char {
        usize::try_from(i).ok().and_then(|i| self.text.get(i).copied()).unwrap_or('\0')
    }

    fn lb_at(&self, i: usize) -> i32 {
        self.lb.get(i).copied().unwrap_or(LB_END)
    }

    fn cw(s: &dyn Surface, font: FontHandle, ch: char) -> i32 {
        s.char_width(font, ch)
    }

    /// The text (`GetText`).
    pub fn text(&self) -> String {
        self.text.iter().collect()
    }

    pub fn font(&self) -> FontHandle {
        self.font
    }

    pub fn set_multiline(&mut self, state: bool) {
        self.multiline = state;
    }

    /// `GetNumLines` (the line-break count, sentinel included).
    pub fn num_lines(&self) -> i32 {
        self.lb.len() as i32
    }

    /// `SetToFullHeight`: tall enough for every line.
    pub fn set_to_full_height(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        <Self as Control>::perform_layout(self, ui, me, s);
        let (w, _) = ui.size(me);
        let tall = self.lb.len() as i32 * (s.font_tall(self.font) + 1) + 3;
        ui.set_size(me, w, tall);
        <Self as Control>::perform_layout(self, ui, me, s);
    }

    /// `SetToFullWidth` (single-line entries only): the raw text's width plus 6.
    pub fn set_to_full_width(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if self.multiline {
            return;
        }
        <Self as Control>::perform_layout(self, ui, me, s);
        let w = 6 + self.text.iter().map(|&c| Self::cw(s, self.font, c)).sum::<i32>();
        ui.set_size(me, w, s.font_tall(self.font) + 4);
        <Self as Control>::perform_layout(self, ui, me, s);
    }

    pub fn set_catch_enter_key(&mut self, state: bool) {
        self.catch_enter = state;
    }

    /// `SendNewLine`.
    pub fn send_new_line(&mut self, state: bool) {
        self.send_new_lines = state;
    }

    pub fn set_editable(&mut self, state: bool) {
        self.editable = state;
    }

    pub fn set_text_hidden(&mut self, state: bool) {
        self.hide_text = state;
    }

    pub fn set_maximum_char_count(&mut self, n: i32) {
        self.max_char_count = n;
    }

    pub fn maximum_char_count(&self) -> i32 {
        self.max_char_count
    }

    pub fn set_allow_non_ascii(&mut self, state: bool) {
        self.allow_non_ascii = state;
    }

    pub fn set_allow_numeric_input_only(&mut self, state: bool) {
        self.numeric_only = state;
    }

    pub fn set_horizontal_scrolling(&mut self, state: bool) {
        self.horiz_scrolling = state;
    }

    pub fn set_wrap(&mut self, state: bool) {
        self.wrap = state;
    }

    /// `SetFont`.
    pub fn set_font(&mut self, ui: &mut Ui, me: PanelId, font: FontHandle) {
        self.font = font;
        ui.invalidate_layout(me);
    }

    /// `SelectCheck`.
    fn select_check(&mut self, ui: &Ui, me: PanelId) {
        if ui.focus() == Some(me) && (ui.is_key_down(KEY_LSHIFT) || ui.is_key_down(KEY_RSHIFT)) {
            if self.select[0] == -1 {
                self.select[0] = self.cursor;
            }
        } else {
            self.select[0] = -1;
        }
    }

    /// `GetSelectedRange`.
    pub fn selected_range(&self) -> Option<(i32, i32)> {
        if self.select[0] == -1 {
            return None;
        }
        let (a, b) = (self.select[0], self.select[1]);
        Some(if b < a { (b, a) } else { (a, b) })
    }

    pub fn select_none(&mut self) {
        self.select[0] = -1;
    }

    /// `SelectNoText`: `_select = {-1, 0}`.
    pub fn select_no_text(&mut self) {
        self.select = [-1, 0];
    }

    /// `SetDrawWidth` (the ellipsis width).
    pub fn set_draw_width(&mut self, w: i32) {
        self.draw_width = w;
    }

    /// `SelectAllText(reset)`.
    pub fn select_all_text(&mut self, reset: bool) {
        if reset {
            self.cursor = 0;
        }
        self.select[0] = if self.text.is_empty() { -1 } else { 0 };
        self.select[1] = self.len();
    }

    fn reset_cursor_blink(&mut self, ui: &Ui) {
        self.cursor_blink = false;
        self.cursor_next_blink = ui.time_ms.wrapping_add(400);
    }

    fn reset_line_breaks(&mut self) {
        self.recalc_breaks_index = 0;
        self.lb.clear();
        self.lb.push(LB_END);
    }

    /// `SetCharAt`.
    fn set_char_at(&mut self, ch: char, index: i32) {
        if ch == '\n' || ch == '\0' {
            self.reset_line_breaks();
        }
        let Ok(i) = usize::try_from(index) else { return };
        if i >= self.text.len() {
            self.text.resize(i + 1, '\0');
        }
        self.text[i] = ch;
        self.data_changed = true;
    }

    pub fn is_editable(&self, ui: &Ui, me: PanelId) -> bool {
        self.editable && ui.is_enabled(me)
    }

    fn is_line_break(&self, i: i32) -> bool {
        self.lb.contains(&i)
    }

    /// `CalcBreakIndex`.
    fn calc_break_index(&mut self) {
        if self.cursor == self.len() {
            self.recalc_breaks_index = self.lb.len() as i32 - 2;
        } else {
            let mut k = 0;
            while self.cursor > self.lb_at(k) {
                k += 1;
            }
            self.recalc_breaks_index = k as i32 - 1;
        }
    }

    /// `GetCursorLine`.
    fn cursor_line(&self) -> i32 {
        let mut n = 0;
        while n < self.lb.len() && self.cursor >= self.lb[n] {
            n += 1;
        }
        let mut n = n as i32;
        if self.put_cursor_at_end && self.cursor != self.len() {
            n -= 1;
        }
        n
    }

    /// `GetCurrentLineEnd` (with the source's fall-through).
    fn current_line_end(&self) -> i32 {
        let count = self.lb.len();
        if self.is_line_break(self.cursor) {
            let mut i = 0;
            while i + 1 < count && self.lb[i] != self.cursor {
                i += 1;
            }
            if self.cursor_is_at_end {
                return self.cursor;
            }
            if i + 2 != count {
                return self.lb_at(i + 1);
            }
        }
        for i in 0..count.saturating_sub(1) {
            if self.cursor < self.lb[i] {
                return self.lb[i];
            }
        }
        self.len()
    }

    fn y_start(&self, ui: &Ui, me: PanelId, s: &dyn Surface) -> i32 {
        if self.multiline { 1 } else { ui.size(me).1 / 2 - s.font_tall(self.font) / 2 }
    }

    fn add_another_line(&mut self, x: &mut i32, y: &mut i32, s: &dyn Surface) {
        *x = self.pixels_indent + 3;
        *y += s.font_tall(self.font) + 1;
    }

    /// `ScrollLeft`.
    fn scroll_left(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        if self.multiline || !self.horiz_scrolling {
            return;
        }
        if self.cursor < self.current_start_index {
            if self.cursor < 0 {
                self.cursor = 0;
            }
            self.current_start_index = self.cursor;
        }
        self.layout_vsb(ui, me, s);
    }

    /// `ScrollRight`.
    fn scroll_right(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        if !self.horiz_scrolling {
            return;
        }
        if !self.multiline {
            let (cx, _) = self.cursor_to_pixel_space(ui, me, self.cursor, s);
            let w = ui.size(me).0;
            if w > 0 && cx >= w - 1 {
                self.current_start_index += 1;
                self.scroll_right(ui, me, s);
            }
        }
        self.layout_vsb(ui, me, s);
    }

    fn move_scroll_bar(&self, ui: &mut Ui, delta: i32) {
        if let Some(v) = self.vsb {
            let val = ScrollBar::value(ui, v) - 3 * delta;
            ScrollBar::set_value(ui, v, val);
        }
    }

    /// `FireActionSignal`: `TextChanged`.
    fn fire_action_signal(&mut self, ui: &mut Ui, me: PanelId) {
        ui.post_action_signal(me, Msg::new("TextChanged"));
        self.data_changed = false;
        ui.invalidate_layout(me);
    }

    /// The key handlers' common tail.
    fn tail(&mut self, ui: &mut Ui, me: PanelId) {
        self.select[1] = self.cursor;
        if self.data_changed {
            self.fire_action_signal(ui, me);
        }
    }

    /// `OnKeyCodeTyped`; `false` when the code goes on to `Panel::OnKeyCodeTyped`.
    pub fn key_code_typed(&mut self, ui: &mut Ui, me: PanelId, code: i32, s: &mut dyn Surface) -> bool {
        self.cursor_is_at_end = self.put_cursor_at_end;
        self.put_cursor_at_end = false;
        let shift = ui.is_key_down(KEY_LSHIFT) || ui.is_key_down(KEY_RSHIFT);
        let ctrl = ui.is_key_down(KEY_LCONTROL) || ui.is_key_down(KEY_RCONTROL);
        let alt = ui.is_key_down(KEY_LALT) || ui.is_key_down(KEY_RALT);
        let base = |te: &mut TextEntry, ui: &mut Ui| {
            te.tail(ui, me);
            te.put_cursor_at_end = te.cursor_is_at_end;
            false
        };
        if ctrl {
            match code {
                c if c == KEY_A => {
                    self.select[0] = if self.text.is_empty() { -1 } else { 0 };
                    self.select[1] = self.len();
                    if self.data_changed {
                        self.fire_action_signal(ui, me);
                    }
                    return true;
                }
                c if c == KEY_C || c == KEY_INSERT => self.copy_selected(ui, me, s),
                c if c == KEY_V => {
                    self.delete_selected(ui, me, s);
                    self.paste(ui, me, s);
                }
                c if c == KEY_X => {
                    self.copy_selected(ui, me, s);
                    self.delete_selected(ui, me, s);
                }
                c if c == KEY_Z => self.undo(ui, me),
                KEY_ENTER => {
                    if self.multiline {
                        self.delete_selected(ui, me, s);
                        self.save_undo_state();
                        self.insert_char(ui, me, '\n', s);
                    }
                    if self.send_new_lines {
                        ui.post_action_signal(me, Msg::new("TextNewLine"));
                    }
                }
                KEY_HOME => self.goto_text_start(ui, me, s),
                KEY_END => self.goto_text_end(ui, me, s),
                KEY_LEFT => self.goto_word_left(ui, me, s),
                KEY_RIGHT => self.goto_word_right(ui, me, s),
                _ => return base(self, ui),
            }
            self.tail(ui, me);
            return true;
        }
        if alt {
            return base(self, ui);
        }
        match code {
            KEY_ENTER => {
                let mut pass = false;
                if !self.catch_enter {
                    pass = true;
                } else if self.multiline {
                    self.delete_selected(ui, me, s);
                    self.save_undo_state();
                    self.insert_char(ui, me, '\n', s);
                } else {
                    let showing = !SHOW_KEYBOARD.load(Ordering::Relaxed);
                    SHOW_KEYBOARD.store(showing, Ordering::Relaxed);
                    if showing {
                        ui.post_action_signal(me, Msg::new("Command").str("command", "ShowKeyboard"));
                    }
                }
                if self.send_new_lines {
                    ui.post_action_signal(me, Msg::new("TextNewLine"));
                }
                if pass {
                    return base(self, ui);
                }
                self.tail(ui, me);
                true
            }
            KEY_BACKSPACE => {
                if self.selected_range().is_some() {
                    self.delete_selected(ui, me, s);
                } else {
                    self.backspace(ui, me, s);
                }
                self.tail(ui, me);
                true
            }
            KEY_INSERT => {
                if shift {
                    self.delete_selected(ui, me, s);
                    self.paste(ui, me, s);
                    self.tail(ui, me);
                    true
                } else {
                    base(self, ui)
                }
            }
            KEY_DELETE => {
                if shift {
                    self.copy_selected(ui, me, s);
                    self.delete_selected(ui, me, s);
                } else {
                    self.delete(ui, me, s);
                }
                self.tail(ui, me);
                true
            }
            KEY_HOME => {
                if self.multiline {
                    self.goto_first_of_line(ui, me, s);
                } else {
                    self.goto_text_start(ui, me, s);
                }
                self.tail(ui, me);
                true
            }
            KEY_END => {
                self.goto_end_of_line(ui, me, s);
                self.tail(ui, me);
                true
            }
            KEY_PAGEUP | KEY_PAGEDOWN => {
                let up = code == KEY_PAGEUP;
                let val = self.vsb.map_or(0, |v| ScrollBar::value(ui, v));
                if self.multiline {
                    let n = ui.size(me).1 / (s.font_tall(self.font) + 1);
                    for _ in 0..n {
                        if up {
                            self.goto_up(ui, me, s);
                        } else {
                            self.goto_down(ui, me, s);
                        }
                    }
                }
                if let Some(v) = self.vsb {
                    let window = ScrollBar::range_window(ui, v);
                    let newval = ScrollBar::value(ui, v);
                    let set = if up { val - (window - (val - newval)) - 1 } else { val + (window - (newval - val)) + 1 };
                    ScrollBar::set_value(ui, v, set);
                }
                self.tail(ui, me);
                true
            }
            KEY_UP | KEY_DOWN => {
                if self.multiline {
                    if code == KEY_UP {
                        self.goto_up(ui, me, s);
                    } else {
                        self.goto_down(ui, me, s);
                    }
                    self.tail(ui, me);
                    true
                } else {
                    base(self, ui)
                }
            }
            KEY_LEFT => {
                self.goto_left(ui, me, s);
                self.tail(ui, me);
                true
            }
            KEY_RIGHT => {
                self.goto_right(ui, me, s);
                self.tail(ui, me);
                true
            }
            c if c == KEY_TAB || c == KEY_ESCAPE || c == KEY_LSHIFT || c == KEY_RSHIFT || (KEY_F1..KEY_F1 + 12).contains(&c) => base(self, ui),
            // Everything else (printable keys, locks, modifiers): text comes through OnKeyTyped.
            _ => true,
        }
    }

    /// `OnKeyTyped`; `false` when it goes to the parent.
    pub fn key_typed(&mut self, ui: &mut Ui, me: PanelId, ch: char, s: &mut dyn Surface) -> bool {
        self.cursor_is_at_end = self.put_cursor_at_end;
        self.put_cursor_at_end = false;
        if !iswprint(ch) || ch == '\t' {
            return true;
        }
        if !self.is_editable(ui, me) {
            return false;
        }
        self.delete_selected(ui, me, s);
        self.save_undo_state();
        self.insert_char(ui, me, ch, s);
        self.tail(ui, me);
        true
    }

    /// `InsertChar`.
    pub fn insert_char(&mut self, ui: &mut Ui, me: PanelId, ch: char, s: &mut dyn Surface) {
        if ch == '\r' || (!self.multiline && ch == '\n') || ch == '\t' {
            return;
        }
        if self.numeric_only && !ch.is_ascii_digit() && (u32::from(ch) & 0xFF) != u32::from(b'.') {
            s.play_sound("Resource\\warning.wav");
            return;
        }
        if !self.allow_non_ascii && u32::from(ch) > 0x7F {
            return;
        }
        if self.max_char_count > -1 && self.len() >= self.max_char_count {
            if self.max_char_count > 0 && self.multiline && self.wrap {
                // Drop whole top lines (no progress when the first break is the last char: the
                // source loops forever; here the loop stops instead).
                while self.len() > self.max_char_count {
                    if self.recalc_breaks_index == 0 {
                        self.recalculate_line_breaks(ui, me, s);
                    }
                    if self.lb_at(0) > self.len() {
                        self.recalc_breaks_index = -1;
                        self.recalculate_line_breaks(ui, me, s);
                    }
                    let first = self.lb_at(0);
                    if first + 1 >= self.len() {
                        break;
                    }
                    self.text.drain(0..first as usize);
                    let len = self.len();
                    if self.cursor > len {
                        self.cursor = len;
                    } else {
                        self.cursor = (self.cursor - (first + 1)).max(0);
                    }
                    if self.select[0] > -1 {
                        self.select[0] -= first + 1;
                        if self.select[0] <= 0 {
                            self.select[0] = -1;
                        }
                        self.select[1] -= first + 1;
                        if self.select[1] <= 0 {
                            self.select[1] = -1;
                        }
                    }
                    for i in (0..len).rev() {
                        let c = self.at(i);
                        self.set_char_at(c, i + 1);
                    }
                    self.recalc_breaks_index = -1;
                    self.recalculate_line_breaks(ui, me, s);
                }
            } else {
                s.play_sound("Resource\\warning.wav");
                return;
            }
        }
        let len = self.len();
        if self.wrap {
            self.set_char_at(ch, len);
            self.cursor = len;
        } else {
            let mut i = len - 1;
            while i >= self.cursor {
                let c = self.at(i);
                self.set_char_at(c, i + 1);
                i -= 1;
            }
            self.set_char_at(ch, self.cursor);
            self.cursor += 1;
        }
        if ch == '\n' {
            self.recalculate_line_breaks(ui, me, s);
        }
        if self.auto_progress && self.len() == self.max_char_count {
            ui.panel_request_focus_next(me, None);
        }
        self.scroll_right(ui, me, s);
        self.data_changed = true;
        self.calc_break_index();
        self.layout_vsb(ui, me, s);
        self.reset_cursor_blink(ui);
    }

    /// `InsertString(const char*)` (a `#token` is localized).
    pub fn insert_string(&mut self, ui: &mut Ui, me: PanelId, text: &str, s: &mut dyn Surface) {
        let text = crate::controls::label::localized(ui, text);
        self.save_undo_state();
        for ch in text.chars() {
            if ch == '\0' {
                break;
            }
            self.insert_char(ui, me, ch, s);
        }
        if self.data_changed {
            self.fire_action_signal(ui, me);
        }
    }

    /// `SetText(const char*)`: a `#token` is localized; no filters, no message.
    pub fn set_text(&mut self, ui: &mut Ui, me: PanelId, text: &str, s: &mut dyn Surface) {
        let text = crate::controls::label::localized(ui, text);
        self.text.clear();
        for ch in text.chars().filter(|&c| c != '\r') {
            self.text.push(ch);
            if ch == '\n' || ch == '\0' {
                self.reset_line_breaks();
            }
        }
        self.goto_text_start(ui, me, s);
        self.select_none();
        self.data_changed = false;
        self.reset_line_breaks();
        ui.invalidate_layout(me);
    }

    /// `Backspace` (ignores the selection).
    fn backspace(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        let len = self.len();
        if !self.is_editable(ui, me) || self.cursor == 0 || len == 0 {
            return;
        }
        self.save_undo_state();
        for i in self.cursor..len {
            let c = self.at(i);
            self.set_char_at(c, i - 1);
        }
        self.text.pop();
        if self.cursor == self.current_start_index {
            self.current_start_index = (self.current_start_index - 6).max(0);
        }
        self.data_changed = true;
        self.cursor -= 1;
        self.calc_break_index();
        self.layout_vsb(ui, me, s);
        self.reset_cursor_blink(ui);
    }

    fn remove_range(&mut self, ui: &mut Ui, me: PanelId, x0: i32, x1: i32, select_first: bool, s: &dyn Surface) {
        self.save_undo_state();
        for _ in 0..(x1 - x0) {
            if let Ok(i) = usize::try_from(x0)
                && i < self.text.len()
            {
                self.text.remove(i);
            }
        }
        if select_first {
            self.select_none();
            self.reset_cursor_blink(ui);
        } else {
            self.reset_cursor_blink(ui);
            self.select_none();
        }
        self.cursor = x0;
        self.data_changed = true;
        self.reset_line_breaks();
        self.calc_break_index();
        self.layout_vsb(ui, me, s);
    }

    /// `Delete`.
    fn delete(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        if !self.is_editable(ui, me) || self.text.is_empty() {
            return;
        }
        let (x0, x1) = match self.selected_range() {
            Some(r) => r,
            None => {
                if self.cursor >= self.len() {
                    return;
                }
                (self.cursor, self.cursor + 1)
            }
        };
        self.remove_range(ui, me, x0, x1, false, s);
    }

    /// `DeleteSelected`.
    pub fn delete_selected(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        if !self.is_editable(ui, me) || self.text.is_empty() {
            return;
        }
        let Some((x0, x1)) = self.selected_range() else { return };
        self.remove_range(ui, me, x0, x1, true, s);
    }

    fn save_undo_state(&mut self) {
        self.undo_cursor = self.cursor;
        self.undo.clone_from(&self.text);
    }

    /// `Undo`: one level.
    fn undo(&mut self, ui: &mut Ui, me: PanelId) {
        self.cursor = self.undo_cursor;
        self.text.clone_from(&self.undo);
        ui.invalidate_layout(me);
        self.select_none();
    }

    /// `CopySelected`: the clipboard count excludes the inserted `\r`s (the tail is cut).
    fn copy_selected(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if self.hide_text {
            return;
        }
        if let Some((x0, x1)) = self.selected_range() {
            let mut buf = Vec::new();
            for i in x0..x1 {
                let c = self.at(i);
                if c == '\n' {
                    buf.push('\r');
                }
                buf.push(c);
            }
            let n = usize::try_from(x1 - x0).unwrap_or(0);
            let text: String = buf.into_iter().take(n).collect();
            s.set_clipboard_text(&text);
        }
        ui.request_focus(me);
        if self.data_changed {
            self.fire_action_signal(ui, me);
        }
    }

    fn cut_selected(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.copy_selected(ui, me, s);
        self.delete_selected(ui, me, s);
        ui.request_focus(me);
        if self.data_changed {
            self.fire_action_signal(ui, me);
        }
    }

    /// `Paste` (does not delete the selection itself).
    fn paste(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if self.hide_text || !self.is_editable(ui, me) {
            return;
        }
        let clip: Vec<char> = s.clipboard_text().chars().collect();
        let size = if self.auto_progress {
            clip.len()
        } else if self.max_char_count > 0 {
            (self.max_char_count + 1) as usize
        } else {
            clip.len()
        };
        let buf: Vec<char> = clip.iter().copied().take(size).collect();
        if buf.is_empty() {
            return;
        }
        self.save_undo_state();
        let mut moved_focus = false;
        let n = buf.len();
        for i in 0..n {
            if buf[i] == '\0' {
                break;
            }
            if self.auto_progress && self.len() == self.max_char_count {
                ui.panel_request_focus_next(me, None);
                let rest: String = buf[i..n.saturating_sub(1).max(i)].iter().collect();
                s.set_clipboard_text(&rest);
                if let Some(p) = ui.parent(me) {
                    let f = ui.node(p).and_then(|n| n.nav.current_focus);
                    if let Some(f) = f.filter(|&f| f != me) {
                        moved_focus = true;
                        ui.send(f, &Msg::new("DoPaste"), Some(me), s);
                    }
                }
                break;
            }
            self.insert_char(ui, me, buf[i], s);
        }
        if self.auto_progress {
            let all: String = clip.iter().take(size).collect();
            s.set_clipboard_text(&all);
        }
        self.data_changed = true;
        self.fire_action_signal(ui, me);
        if !moved_focus {
            ui.request_focus(me);
        }
    }

    /// `PixelToCursorSpace`.
    fn pixel_to_cursor_space(&mut self, ui: &mut Ui, me: PanelId, cx: i32, cy: i32, s: &dyn Surface) -> i32 {
        self.put_cursor_at_end = false;
        let tall = s.font_tall(self.font);
        let y_start = self.y_start(ui, me, s);
        let (mut x, mut y) = (3, y_start);
        self.pixels_indent = 0;
        let mut lbi = 0usize;
        let mut i = self.start_draw_index(ui, me, &mut lbi, s);
        let mut on_right = false;
        let wide = ui.size(me).0;
        while i < self.len() {
            let ch = if self.hide_text { '*' } else { self.at(i) };
            if self.lb_at(lbi) == i {
                self.add_another_line(&mut x, &mut y, s);
                lbi += 1;
                if on_right {
                    self.put_cursor_at_end = true;
                    return i;
                }
            }
            if cy < y_start {
                on_right = true;
                self.put_cursor_at_end = true;
            } else if cy >= y && cy < y + tall + 1 {
                on_right = true;
            }
            let w = Self::cw(s, self.font, ch);
            if on_right {
                if cx <= wide {
                    if cx < self.pixels_indent + 3 {
                        return i;
                    }
                    if cy < y_start {
                        return i;
                    }
                }
                if cx >= x && cx < x + w {
                    return if f64::from(x) + f64::from(w) * 0.5 > f64::from(cx) { i } else { i + 1 };
                }
            }
            x += w;
            i += 1;
        }
        i
    }

    /// `CursorToPixelSpace`.
    fn cursor_to_pixel_space(&mut self, ui: &mut Ui, me: PanelId, pos: i32, s: &dyn Surface) -> (i32, i32) {
        let mut cy = self.y_start(ui, me, s);
        let mut cx = 3;
        self.pixels_indent = 0;
        let mut lbi = 0usize;
        let mut i = self.start_draw_index(ui, me, &mut lbi, s);
        while i < self.len() {
            let ch = if self.hide_text { '*' } else { self.at(i) };
            if i == pos {
                break;
            }
            if lbi < self.lb.len() && self.lb[lbi] == i {
                self.add_another_line(&mut cx, &mut cy, s);
                lbi += 1;
            }
            cx += Self::cw(s, self.font, ch);
            i += 1;
        }
        (cx, cy)
    }

    /// `MoveCursor(line, pixelsAcross)`.
    fn move_cursor(&mut self, ui: &mut Ui, me: PanelId, line: i32, across: i32, s: &dyn Surface) {
        let line = line.clamp(0, self.lb.len() as i32 - 1);
        let mut y = self.y_start(ui, me, s);
        let mut x = 3;
        self.pixels_indent = 0;
        let mut lbi = 0usize;
        let mut i = 0;
        let len = self.len();
        while i < len {
            let ch = if self.hide_text { '*' } else { self.at(i) };
            if self.lb_at(lbi) == i {
                if lbi as i32 == line {
                    self.put_cursor_at_end = true;
                    self.cursor = i;
                    break;
                }
                self.add_another_line(&mut x, &mut y, s);
                lbi += 1;
            }
            let w = Self::cw(s, self.font, ch);
            if line == lbi as i32 && x + w / 2 > across {
                self.cursor = i;
                break;
            }
            x += w;
            i += 1;
        }
        if i == len {
            self.goto_text_end(ui, me, s);
        }
        self.layout_vsb(ui, me, s);
        self.reset_cursor_blink(ui);
    }

    fn goto_left(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        self.select_check(ui, me);
        if self.is_line_break(self.cursor) && !self.cursor_is_at_end {
            self.put_cursor_at_end = true;
        }
        if !self.put_cursor_at_end && self.cursor > 0 {
            self.cursor -= 1;
        }
        self.scroll_left(ui, me, s);
        self.reset_cursor_blink(ui);
    }

    fn goto_right(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        self.select_check(ui, me);
        if self.is_line_break(self.cursor) {
            if self.cursor_is_at_end {
                self.put_cursor_at_end = false;
            } else if self.cursor < self.len() {
                self.cursor += 1;
            }
        } else {
            if self.cursor < self.len() {
                self.cursor += 1;
            }
            if self.is_line_break(self.cursor) && !self.cursor_is_at_end {
                self.put_cursor_at_end = true;
            }
        }
        self.scroll_right(ui, me, s);
        self.reset_cursor_blink(ui);
    }

    fn goto_up(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        self.select_check(ui, me);
        if self.cursor_is_at_end {
            if self.cursor_line() - 1 == 0 {
                self.put_cursor_at_end = true;
                return;
            }
            self.cursor -= 1;
        }
        let (cx, _) = self.cursor_to_pixel_space(ui, me, self.cursor, s);
        let line = self.cursor_line() - 1;
        self.move_cursor(ui, me, line, cx, s);
    }

    fn goto_down(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        self.select_check(ui, me);
        if self.cursor_is_at_end {
            self.cursor = (self.cursor - 1).max(0);
        }
        let (cx, _) = self.cursor_to_pixel_space(ui, me, self.cursor, s);
        let line = self.cursor_line() + 1;
        self.move_cursor(ui, me, line, cx, s);
        if !self.put_cursor_at_end && self.cursor_is_at_end {
            self.cursor = (self.cursor + 1).min(self.len());
        }
        self.layout_vsb(ui, me, s);
    }

    /// `GotoWordLeft` (may leave the cursor at −1 without horizontal scrolling).
    fn goto_word_left(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        self.select_check(ui, me);
        if self.cursor < 1 {
            return;
        }
        self.cursor -= 1;
        while iswspace(self.at(self.cursor)) {
            self.cursor -= 1;
            if self.cursor < 0 {
                break;
            }
        }
        self.cursor -= 1;
        while self.cursor >= 0 && !iswspace(self.at(self.cursor)) {
            self.cursor -= 1;
        }
        self.cursor += 1;
        if !self.multiline && self.horiz_scrolling {
            if self.cursor < self.current_start_index {
                if self.cursor < 0 {
                    self.cursor = 0;
                }
                self.current_start_index = self.cursor;
            }
            self.layout_vsb(ui, me, s);
        }
        self.layout_vsb(ui, me, s);
        self.reset_cursor_blink(ui);
    }

    fn goto_word_right(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        self.select_check(ui, me);
        let len = self.len();
        self.cursor += 1;
        while self.cursor < len && !iswspace(self.at(self.cursor)) {
            self.cursor += 1;
        }
        self.cursor += 1;
        while self.cursor < len && iswspace(self.at(self.cursor)) {
            self.cursor += 1;
        }
        if self.cursor > len {
            self.cursor = len;
        }
        self.scroll_right(ui, me, s);
        self.layout_vsb(ui, me, s);
        self.reset_cursor_blink(ui);
    }

    fn goto_first_of_line(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        self.select_check(ui, me);
        let p = if !self.multiline {
            self.current_start_index
        } else if self.is_line_break(self.cursor) {
            let mut i = 0;
            while i < self.lb.len() && self.lb[i] != self.cursor {
                i += 1;
            }
            if self.cursor_is_at_end { if i > 0 { self.lb_at(i - 1) } else { self.lb_at(0) } } else { self.cursor }
        } else {
            let mut p = 0;
            for i in 0..self.lb.len() {
                if self.cursor < self.lb[i] {
                    p = if i == 0 { 0 } else { self.lb[i - 1] };
                    break;
                }
            }
            p
        };
        self.cursor = p;
        self.current_start_index = p;
        self.put_cursor_at_end = false;
        self.layout_vsb(ui, me, s);
        self.reset_cursor_blink(ui);
    }

    fn goto_end_of_line(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        self.select_check(ui, me);
        self.cursor = self.current_line_end();
        self.put_cursor_at_end = true;
        self.scroll_right(ui, me, s);
        self.layout_vsb(ui, me, s);
        self.reset_cursor_blink(ui);
    }

    /// `GotoTextStart`.
    pub fn goto_text_start(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        self.select_check(ui, me);
        self.cursor = 0;
        self.put_cursor_at_end = false;
        self.current_start_index = 0;
        self.layout_vsb(ui, me, s);
        self.reset_cursor_blink(ui);
    }

    /// `GotoTextEnd`.
    pub fn goto_text_end(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        self.select_check(ui, me);
        self.cursor = self.len();
        self.put_cursor_at_end = true;
        self.scroll_right(ui, me, s);
        self.layout_vsb(ui, me, s);
        self.reset_cursor_blink(ui);
    }

    /// `RecalculateLineBreaks` (word wrap; the overflowing char's width stays on the new line).
    fn recalculate_line_breaks(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        let len = self.len();
        if !self.multiline || self.hide_text || len < 1 {
            return;
        }
        let font = self.font;
        let mut wide = ui.size(me).0 - 2;
        if let Some(v) = self.vsb {
            wide -= ui.size(v).0;
        }
        let (mut x, mut y) = (3, 1);
        let mut word_start = 0;
        let mut has_word = false;
        let mut just_started = true;
        let mut word_on_new_line = true;
        let mut start = if self.recalc_breaks_index <= 0 {
            self.lb.clear();
            0
        } else {
            let k = self.recalc_breaks_index as usize;
            self.lb.truncate(k + 1);
            self.lb_at(k)
        };
        if matches!(self.at(start), '\r' | '\n') {
            start += 1;
        }
        let mut i = start;
        while i < len {
            let ch = self.at(i);
            if !iswspace(ch) {
                if !has_word {
                    word_start = i;
                    has_word = true;
                    word_on_new_line = just_started;
                }
            } else {
                has_word = false;
            }
            let w = Self::cw(s, font, ch);
            if iswprint(ch) {
                just_started = false;
            }
            if x + w >= wide || ch == '\r' || ch == '\n' {
                self.add_another_line(&mut x, &mut y, s);
                just_started = true;
                has_word = false;
                if ch == '\r' || ch == '\n' || word_on_new_line || iswspace(ch) {
                    self.lb.push(i);
                } else {
                    self.lb.push(word_start);
                    i = word_start;
                }
            }
            x += w;
            i += 1;
        }
        self.char_count = i - 1;
        self.lb.push(LB_END);
        self.layout_vsb(ui, me, s);
    }

    /// `LayoutVerticalScrollBarSlider` (with the source's top/right inset swap).
    fn layout_vsb(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        let Some(v) = self.vsb else { return };
        let (mut wide, tall) = ui.size(me);
        let [_l, t, r, b] = ui.node(me).and_then(|n| n.border.as_ref().map(|b| b.inset)).unwrap_or([0; 4]);
        // GetInset(a, b, c, d) receives (left, right, top, bottom) through swapped slots.
        wide -= t;
        ui.set_pos(v, wide - self.scroll_bar_size, 0);
        ui.set_size(v, self.scroll_bar_size, tall - b - r);
        let display = tall / (s.font_tall(self.font) + 1);
        let num = self.lb.len() as i32;
        if num <= display {
            ui.set_enabled(v, false);
            ScrollBar::set_range(ui, v, 0, num);
            ScrollBar::set_range_window(ui, v, num);
            ScrollBar::set_value(ui, v, 0);
        } else {
            ScrollBar::set_range(ui, v, 0, num);
            ScrollBar::set_range_window(ui, v, display);
            ui.set_enabled(v, true);
            ScrollBar::set_button_pressed_scroll_value(ui, v, 1);
            let mut val = ScrollBar::value(ui, v);
            let mut maxval = val + display;
            let cl = self.cursor_line();
            if cl < val {
                while cl < val {
                    val -= 1;
                }
            } else if cl >= maxval {
                while cl >= maxval {
                    maxval += 1;
                }
                val = maxval - display;
            }
            ScrollBar::set_value(ui, v, val);
            ui.invalidate_layout(v);
        }
    }

    /// `SetVerticalScrollbar`.
    pub fn set_vertical_scrollbar(&mut self, ui: &mut Ui, me: PanelId, state: bool) {
        self.vertical_scrollbar = state;
        if state {
            let v = match self.vsb {
                Some(v) => v,
                None => {
                    let v = ScrollBar::create(ui, Some(me), "ScrollBar", true);
                    ui.add_action_signal_target(v, me);
                    self.vsb = Some(v);
                    v
                }
            };
            ui.set_visible(v, true);
        } else if let Some(v) = self.vsb {
            ui.set_visible(v, false);
        }
        ui.invalidate_layout(me);
    }

    /// `GetStartDrawIndex(&lbi)`: may scroll one character per call.
    fn start_draw_index(&mut self, ui: &mut Ui, me: PanelId, lbi: &mut usize, s: &dyn Surface) -> i32 {
        let mut start_line = 0;
        let mut start_index = 0;
        let num_lines = self.lb.len() as i32;
        if let Some(v) = self.vsb.filter(|_| !self.mouse_drag_selection) {
            start_line = ScrollBar::value(ui, v);
        } else {
            let display = ui.size(me).1 / (s.font_tall(self.font) + 1);
            if num_lines > display {
                let cl = self.cursor_line();
                start_line = self.current_start_line;
                if cl < self.current_start_line {
                    start_line = cl;
                    if let Some(v) = self.vsb {
                        self.move_scroll_bar(ui, 1);
                        start_line = ScrollBar::value(ui, v);
                    }
                } else if cl > self.current_start_line + display - 1 {
                    start_line = cl - display + 1;
                    if let Some(v) = self.vsb {
                        self.move_scroll_bar(ui, -1);
                        start_line = ScrollBar::value(ui, v);
                    }
                }
            } else if !self.multiline {
                let mut x = 3;
                let mut i = self.current_start_index;
                while i < self.len() {
                    let ch = if self.hide_text { '*' } else { self.at(i) };
                    if self.cursor == i {
                        break;
                    }
                    x += Self::cw(s, self.font, ch);
                    i += 1;
                }
                if x >= ui.size(me).0 {
                    self.current_start_index += 1;
                } else if x <= 0 && self.current_start_index > 0 {
                    self.current_start_index -= 1;
                }
            }
        }
        if start_line > 0 {
            *lbi = start_line as usize;
            if start_line < num_lines {
                start_index = self.lb_at(start_line as usize - 1);
            }
        }
        if !self.horiz_scrolling {
            return 0;
        }
        self.current_start_line = start_line;
        if self.multiline { start_index } else { self.current_start_index }
    }

    /// `NeedsEllipses`.
    fn needs_ellipses(&self, font: FontHandle, s: &dyn Surface) -> Option<i32> {
        let mut sum = 3;
        for (i, &c) in self.text.iter().enumerate() {
            sum += Self::cw(s, font, c);
            if sum > self.draw_width {
                return Some(i as i32);
            }
        }
        None
    }

    /// `DrawChar`: the selection text colour is never restored.
    fn draw_char(&self, sel_col: Color, ch: char, font: FontHandle, index: i32, (x, y): (i32, i32), s: &mut dyn Surface) -> i32 {
        if !iswprint(ch) {
            return 0;
        }
        let w = s.char_abc(font, ch).advance();
        let t = s.font_tall(font);
        let (x0, x1) = self.selected_range().unwrap_or((-1, -1));
        if index >= x0 && index < x1 {
            let col = sel_col;
            s.draw_set_color(col);
            s.draw_filled_rect(x, y, x + w, y + t + 1);
            s.draw_set_text_color(self.selection_text_color);
        }
        s.draw_set_text_pos(x, y);
        s.draw_unicode_char(ch);
        w
    }

    /// `PaintBackground`: background, text, caret.
    pub fn paint_te(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let enabled = ui.is_enabled(me);
        let (fg, bg) = ui.node(me).map_or((Color::WHITE, Color::WHITE), |n| (n.fg, n.bg));
        s.draw_set_color(if enabled { bg } else { self.disabled_bg });
        let (w, h) = ui.size(me);
        s.draw_filled_rect(0, 0, w, h);
        let font = self.font;
        s.draw_set_text_font(font);
        s.draw_set_text_color(if enabled { fg } else { self.disabled_fg });
        self.pixels_indent = 0;
        let mut lbi = 0usize;
        let start = self.start_draw_index(ui, me, &mut lbi, s);
        let mut x = self.pixels_indent + 3;
        let mut y = self.y_start(ui, me, s);
        let len = self.len();
        let focus = ui.focus();
        // The selection box colour: the panel or a child of it has focus.
        let sel_col = if focus == Some(me) || focus.is_some_and(|f| ui.has_parent(f, me)) { self.selection_color } else { self.selection_bg2 };
        if self.multiline || self.horiz_scrolling {
            for i in start..len {
                let ch = if self.hide_text { '*' } else { self.at(i) };
                if self.multiline && self.lb_at(lbi) == i {
                    self.add_another_line(&mut x, &mut y, s);
                    lbi += 1;
                }
                x += self.draw_char(sel_col, ch, font, i, (x, y), s);
            }
        } else {
            let mut end = len;
            if !(ui.focus() == Some(me) && self.is_editable(ui, me)) {
                if let Some(idx) = self.needs_ellipses(font, s) {
                    let mut e = 3 * Self::cw(s, font, '.');
                    let mut i = idx;
                    while e > 0 {
                        if i < 0 {
                            break;
                        }
                        e -= Self::cw(s, font, self.at(i));
                        i -= 1;
                    }
                    end = i + 1;
                }
                if len - end == 1 || len - end == 2 {
                    end = len - 3;
                }
            }
            let mut i = start;
            while i < end {
                x += self.draw_char(sel_col, if self.hide_text { '*' } else { self.at(i) }, font, i, (x, y), s);
                i += 1;
            }
            if end < len {
                let k = i;
                for d in 0..3 {
                    x += self.draw_char(sel_col, '.', font, k + d, (x, y), s);
                }
            }
        }
        s.draw_set_color(Color::rgba(50, 50, 50, 255));
        if enabled && self.is_editable(ui, me) && ui.focus() == Some(me) {
            s.draw_set_color(Color::rgba(0, 0, 0, 255));
            if !self.cursor_blink {
                let (cx, cy) = self.cursor_to_pixel_space(ui, me, self.cursor, s);
                s.draw_set_color(self.cursor_color);
                s.draw_filled_rect(cx, cy, cx + 1, cy + s.font_tall(self.font));
            }
        }
        let _ = (y, self.char_count);
    }

    /// `CreateEditMenu` + `OpenEditMenu` (right press).
    fn open_edit_menu(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if let Some(m) = self.edit_menu.take() {
            ui.delete(m);
        }
        let m = Menu::create(ui, Some(me), "EditMenu");
        if self.editable && !self.hide_text {
            Menu::add_item_msg(ui, m, "#TextEntry_Cut", "#TextEntry_Cut", Msg::new("DoCutSelected"), Some(me), None);
        }
        if !self.hide_text {
            Menu::add_item_msg(ui, m, "#TextEntry_Copy", "#TextEntry_Copy", Msg::new("DoCopySelected"), Some(me), None);
        }
        if self.editable {
            Menu::add_item_msg(ui, m, "#TextEntry_Paste", "#TextEntry_Paste", Msg::new("DoPaste"), Some(me), None);
        }
        Menu::set_visible(ui, m, false);
        ui.set_parent(m, Some(me));
        ui.add_action_signal_target(m, me);
        self.edit_menu = Some(m);
        // SetItemEnabled("&Cut"/"C&opy") matches no item name: Cut and Copy stay enabled.
        let (cx, cy) = ui.cursor_pos();
        Menu::set_visible(ui, m, true);
        ui.request_focus(m);
        ui.invalidate_layout_ex(m, true, false, s);
        let (mw, mh) = ui.size(m);
        let (sw, sh) = ui.screen;
        let x = if sw - mw > cx { cx } else { cx - mw };
        let y = if sh - mh > cy { cy } else { cy - mh };
        ui.set_pos(m, x, y);
        ui.request_focus(m);
    }

    /// The class's message handling; `false` lets `Panel`'s default run.
    pub fn handle(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) -> bool {
        if msg.is("KeyCodeTyped") {
            let code = msg.get_int("code");
            // A tab-catching entry's parent already saw the code (`Ui::send`); then the normal
            // editing, and what TextEntry does not take goes on to Panel's.
            if self.tab_catching.is_some() {
                if !self.key_code_typed(ui, me, code, s) {
                    ui.panel_on_key_code_typed(me, code, s);
                }
                return true;
            }
            return self.key_code_typed(ui, me, code, s);
        }
        if msg.is("KeyTyped") {
            let ch = char::from_u32(msg.get_int("unichar") as u32).unwrap_or('\0');
            if let Some(tc) = &mut self.tab_catching {
                let ignore = tc.ignore_next_char;
                tc.ignore_next_char = false;
                if ignore {
                    return true;
                }
            }
            return self.key_typed(ui, me, ch, s);
        }
        if msg.is("MousePressed") {
            let code = msg.get_int("code");
            if code == MOUSE_LEFT {
                self.select_check(ui, me);
                let (cx, cy) = ui.cursor_pos();
                let (ax, ay) = ui.abs_pos(me);
                self.cursor_is_at_end = self.put_cursor_at_end;
                self.cursor = self.pixel_to_cursor_space(ui, me, cx - ax, cy - ay, s);
                if self.cursor == 0 {
                    self.put_cursor_at_end = false;
                }
                ui.set_mouse_capture(Some(me));
                self.mouse_selection = true;
                if self.select[0] < 0 {
                    self.select[0] = self.cursor;
                }
                self.select[1] = self.cursor;
                self.reset_cursor_blink(ui);
                ui.request_focus(me);
            } else if code == MOUSE_RIGHT {
                self.open_edit_menu(ui, me, s);
            }
        } else if msg.is("MouseDoublePressed") {
            if msg.get_int("code") != MOUSE_LEFT {
                return true;
            }
            self.handle(ui, me, &Msg::new("MousePressed").int("code", MOUSE_LEFT), s);
            self.goto_word_left(ui, me, s);
            let st = self.cursor;
            self.goto_word_right(ui, me, s);
            let mut e = self.cursor;
            if e > 0 {
                if iswspace(self.at(e - 1)) {
                    self.cursor -= 1;
                    e -= 1;
                }
                self.select = [st, e];
                self.mouse_selection = true;
            }
        } else if msg.is("CursorMoved") {
            if !self.mouse_selection {
                return true;
            }
            let (cx, cy) = ui.cursor_pos();
            let (ax, ay) = ui.abs_pos(me);
            self.cursor = self.pixel_to_cursor_space(ui, me, cx - ax, cy - ay, s);
            if self.cursor == 0 {
                self.put_cursor_at_end = false;
            }
            if self.cursor == self.current_start_index {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                self.scroll_left(ui, me, s);
                self.cursor = self.current_start_index;
            }
            if self.cursor != self.select[1] {
                self.select[1] = self.cursor;
            }
        } else if msg.is("MouseReleased") {
            self.mouse_selection = false;
            ui.set_mouse_capture(None);
            if self.selected_range().is_some_and(|(a, b)| a == b) {
                self.select[0] = -1;
            }
        } else if msg.is("CursorEntered") {
            self.mouse_drag_selection = false;
        } else if msg.is("CursorExited") {
            if self.mouse_selection {
                self.mouse_drag_selection = true;
            }
        } else if msg.is("MouseFocusTicked") {
            if self.mouse_drag_selection {
                return self.handle(ui, me, &Msg::new("CursorMoved").int("xpos", 0).int("ypos", 0), s);
            }
        } else if msg.is("MouseWheeled") {
            if self.vsb.is_some() {
                self.move_scroll_bar(ui, msg.get_int("delta"));
            } else {
                return false;
            }
        } else if msg.is("MouseCaptureLost") {
            self.mouse_selection = false;
        } else if msg.is("KeyFocusTicked") {
            let t = ui.time_ms;
            if t > self.cursor_next_blink {
                self.cursor_blink = !self.cursor_blink;
                self.cursor_next_blink = t.wrapping_add(400);
            }
        } else if msg.is("SetFocus") {
            if !ui.is_mouse_down(MOUSE_LEFT) && !ui.is_mouse_down(MOUSE_RIGHT) {
                self.goto_text_end(ui, me, s);
                self.select_none();
            }
        } else if msg.is("KillFocus") {
            if let Some(tc) = self.tab_catching {
                if ui.focus() != tc.completion_list
                    && let Some(p) = ui.parent(me)
                {
                    ui.post(p, Msg::new("CloseCompletionList"), Some(me), 0.0);
                }
                return true;
            }
            if self.data_changed {
                self.fire_action_signal(ui, me);
                self.data_changed = false;
            }
            // The was-pressed/released flags are cleared before KillFocus is dispatched.
            if ui.is_mouse_down(MOUSE_RIGHT) {
                let (x, y) = ui.cursor_pos();
                if ui.contains(me, x, y) {
                    return true;
                }
            }
            if self.is_editable(ui, me) {
                self.select_none();
            }
        } else if msg.is("DoCutSelected") {
            self.cut_selected(ui, me, s);
        } else if msg.is("DoCopySelected") {
            self.copy_selected(ui, me, s);
        } else if msg.is("DoPaste") {
            self.paste(ui, me, s);
        } else if msg.is("SetText") {
            let t = msg.get_str("text");
            self.set_text(ui, me, &t, s);
        } else if msg.is("SetState") {
            let t = msg.get_int("state").to_string();
            self.set_text(ui, me, &t, s);
        } else if msg.is("ScrollBarSliderMoved") {
            // Repaint only.
        } else {
            return false;
        }
        true
    }

    /// `ApplySchemeSettings`.
    pub fn apply_scheme_te(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let fg = scheme.color("WindowFgColor", Color::WHITE);
        let bg = scheme.color("WindowBgColor", Color::WHITE);
        if let Some(n) = ui.node_mut(me) {
            n.fg = fg;
            n.bg = bg;
        }
        self.cursor_color = scheme.color("TextCursorColor", Color::WHITE);
        self.disabled_fg = scheme.color("WindowDisabledFgColor", Color::WHITE);
        self.disabled_bg = scheme.color("ControlBG", Color::WHITE);
        self.selection_text_color = scheme.color("SelectionFgColor", fg);
        self.selection_color = scheme.color("SelectionBgColor", Color::WHITE);
        self.selection_bg2 = scheme.color("SelectionBG2", Color::WHITE);
        let border = scheme.border("ButtonDepressedBorder");
        if let Some(n) = ui.node_mut(me) {
            n.border = border;
        }
        let (prop, hd) = (ui.is_proportional(me), ui.is_hd_proportional(me));
        let f = s.font(&scheme.file, "Default", prop, hd);
        self.set_font(ui, me, f);
    }

    /// `OnSizeChanged`.
    fn size_changed(&mut self, ui: &mut Ui, me: PanelId, w: i32, s: &dyn Surface) {
        self.reset_line_breaks();
        if w > self.draw_width && !self.multiline && self.horiz_scrolling {
            while self.current_start_index > 0 {
                self.current_start_index -= 1;
                let n = self.current_start_index;
                let (cx, _) = self.cursor_to_pixel_space(ui, me, self.cursor, s);
                let wide = ui.size(me).0;
                if wide > 0 && cx >= wide - 1 {
                    self.current_start_index += 1;
                    break;
                }
                if n != self.current_start_index {
                    break;
                }
            }
            self.layout_vsb(ui, me, s);
        }
        self.draw_width = w;
        ui.invalidate_layout(me);
    }
}

impl Control for TextEntry {
    fn class(&self) -> &'static str {
        if self.tab_catching.is_some() { "TabCatchingTextEntry" } else { "TextEntry" }
    }
    fn key_codes_to_parent_first(&self) -> bool {
        self.tab_catching.is_some()
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        self.handle(ui, me, msg, s)
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.apply_scheme_te(ui, me, scheme, s);
    }
    fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &Block, s: &mut dyn Surface) {
        ui.panel_apply_settings(me, kv, s);
        self.hide_text = kv_int(kv, "textHidden", 0) != 0;
        self.editable = kv_int(kv, "editable", 1) != 0;
        self.max_char_count = kv_int(kv, "maxchars", -1);
        self.numeric_only = kv_int(kv, "NumericInputOnly", 0) != 0;
        self.allow_non_ascii = kv_int(kv, "unicode", 0) != 0;
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if let Some(w) = self.pending_size.take() {
            self.size_changed(ui, me, w, s);
        }
        self.recalculate_line_breaks(ui, me, s);
        if self.vertical_scrollbar {
            self.layout_vsb(ui, me, s);
        }
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.paint_te(ui, me, s);
    }
    fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId, w: i32, _h: i32) {
        // Line breaks reset; the width-dependent scroll-back needs font metrics and runs at
        // the next layout through `size_changed_pending`.
        self.reset_line_breaks();
        self.pending_size = Some(w);
        ui.invalidate_layout(me);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Request-info style accessor: the entry's text.
pub fn text_of(ui: &mut Ui, id: PanelId) -> String {
    with_te(ui, id, |te, _| te.text()).unwrap_or_default()
}

/// `SetText` from outside.
pub fn set_text_of(ui: &mut Ui, id: PanelId, text: &str, s: &mut dyn Surface) {
    with_te(ui, id, |te, ui| te.set_text(ui, id, text, s));
}
