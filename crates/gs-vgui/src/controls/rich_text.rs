//! `vgui2::RichText` and GameUI's `CNoKeyboardInputRichText` (the console history)
//! (docs/re/vgui-controls-2.md §3). GameUI inserts no clickable text, so ClickPanels and URL
//! handling are not modelled; format runs keep their `clickable` flag false.

use std::any::Any;

use gs_formats::keyvalues::Block;

use crate::controls::menu::Menu;
use crate::controls::scroll_bar::ScrollBar;
use crate::editable::{kv_int, kv_str};
use crate::keys::*;
use crate::msg::Msg;
use crate::scheme::Scheme;
use crate::surface::{Color, Cursor, FontHandle, Surface};
use crate::ui::{Control, PanelId, Ui};

const LB_END: i32 = 999_999;

fn iswprint(c: char) -> bool {
    !c.is_control() && c != '\0'
}

fn iswspace(c: char) -> bool {
    c.is_whitespace()
}

/// `TFormatStream`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Format {
    color: Color,
    indent: i32,
    tsi: i32,
}

/// `TRenderState`.
#[derive(Debug, Clone, Copy, Default)]
struct RenderState {
    x: i32,
    y: i32,
    color: Color,
    indent: i32,
    fsi: i32,
}

/// `vgui2::RichText`.
#[derive(Debug)]
pub struct RichText {
    text: Vec<char>,
    lb: Vec<i32>,
    fs: Vec<Format>,
    recalc_line_breaks: bool,
    recalc_breaks_index: i32,
    invalidate_vsb: bool,
    cursor: i32,
    mouse_selection: bool,
    mouse_drag_selection: bool,
    select: [i32; 2],
    pixels_indent: i32,
    max_char_count: i32,
    font: FontHandle,
    disabled_fg: Color,
    selection_color: Color,
    selection_text_color: Color,
    scroll_bar_size: i32,
    draw_offset: (i32, i32),
    pub vsb: PanelId,
    edit_menu: Option<PanelId>,
    recalc_saved_render_state: bool,
    cached: RenderState,
    /// `CNoKeyboardInputRichText::m_pFocusPanel`: focus requests go to this panel instead.
    pub focus_panel: Option<PanelId>,
    /// `CNoKeyboardInputRichText`: `con_mono > 0` picks `GameConsole_Mono` at scheme time.
    pub mono: bool,
}

/// Runs `f` on the RichText of panel `id`.
pub fn with_rt<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut RichText, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<RichText>().map(|rt| f(rt, ui))).flatten()
}

impl RichText {
    /// `new RichText(parent, name, trusted)`: the vertical scrollbar exists and is visible.
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str) -> PanelId {
        let prop = parent.is_some_and(|p| ui.is_proportional(p));
        // Panel::GetProportionalBase: the new panel inherits its parent's (HD) mode.
        let base_w = parent.map_or(ui.base_size(false).0, |p| ui.proportional_base_of(p).0);
        let s = if prop { ui.screen.0 as f32 / base_w as f32 } else { 1.0 };
        let (sb, ox, oy) = if prop { ((18.0 * s) as i32, (3.0 * s) as i32, s as i32) } else { (18, 3, 1) };
        let placeholder = ui.embedded();
        let me = ui.create(
            parent,
            name,
            Box::new(RichText {
                text: Vec::new(),
                lb: vec![LB_END],
                fs: vec![Format { color: Color::TRANSPARENT, indent: 0, tsi: 0 }],
                recalc_line_breaks: true,
                recalc_breaks_index: 0,
                invalidate_vsb: true,
                cursor: 0,
                mouse_selection: false,
                mouse_drag_selection: false,
                select: [-1, -1],
                pixels_indent: 0,
                max_char_count: 65536,
                font: FontHandle(0),
                disabled_fg: Color::TRANSPARENT,
                selection_color: Color::TRANSPARENT,
                selection_text_color: Color::TRANSPARENT,
                scroll_bar_size: sb,
                draw_offset: (ox, oy),
                vsb: placeholder,
                edit_menu: None,
                recalc_saved_render_state: true,
                cached: RenderState::default(),
                focus_panel: None,
                mono: false,
            }),
        );
        let vsb = ScrollBar::create(ui, Some(me), "ScrollBar", true);
        ui.add_action_signal_target(vsb, me);
        ui.add_action_signal_target(me, me);
        if let Some(n) = ui.node_mut(me) {
            n.cursor = Cursor::IBeam;
        }
        if let Some(rt) = ui.ctrl::<RichText>(me) {
            rt.vsb = vsb;
        }
        me
    }

    fn len(&self) -> i32 {
        self.text.len() as i32
    }

    fn at(&self, i: i32) -> char {
        usize::try_from(i).ok().and_then(|i| self.text.get(i).copied()).unwrap_or('\0')
    }

    fn lb_at(&self, i: usize) -> i32 {
        self.lb.get(i).copied().unwrap_or(LB_END)
    }

    /// The whole text.
    pub fn text(&self) -> String {
        self.text.iter().collect()
    }

    fn invalidate_line_breaks(&mut self, ui: &mut Ui, me: PanelId) {
        self.lb.clear();
        self.lb.push(LB_END);
        self.recalc_breaks_index = 0;
        self.recalc_line_breaks = true;
        ui.invalidate_layout(me);
    }

    /// `SetText(const char*)` (`#token` localized; a missing token shows literally).
    pub fn set_text(&mut self, ui: &mut Ui, me: PanelId, text: &str) {
        let text = crate::controls::label::localized(ui, text);
        let fg = ui.node(me).map_or(Color::WHITE, |n| n.fg);
        self.fs.clear();
        self.fs.push(Format { color: fg, indent: 0, tsi: 0 });
        self.text = text.chars().collect();
        self.goto_text_start(ui);
        self.select[0] = -1;
        self.invalidate_line_breaks(ui, me);
    }

    /// `InsertString(const char*)`: `\r` (by low byte) dropped; scroll-back trimmed.
    pub fn insert_string(&mut self, ui: &mut Ui, me: PanelId, text: &str) {
        let text = crate::controls::label::localized(ui, text);
        for ch in text.chars() {
            if ch == '\0' {
                break;
            }
            if (u32::from(ch) & 0xFF) == 0x0D {
                continue;
            }
            if self.max_char_count > 0 && self.len() > self.max_char_count {
                self.truncate_text_stream(ui, me);
            }
            self.text.push(ch);
            self.recalc_breaks_index = self.lb.len() as i32 - 2;
        }
        ui.invalidate_layout(me);
        self.recalc_line_breaks = true;
    }

    /// `InsertColorChange`.
    pub fn insert_color_change(&mut self, col: Color) {
        let count = self.len();
        let Some(last) = self.fs.last_mut() else { return };
        if last.color == col {
            return;
        }
        if last.tsi == count {
            last.color = col;
        } else {
            let indent = last.indent;
            self.fs.push(Format { color: col, indent, tsi: count });
        }
    }

    /// `TruncateTextStream`: drop the oldest half of the limit.
    fn truncate_text_stream(&mut self, ui: &mut Ui, me: PanelId) {
        if self.max_char_count < 1 {
            return;
        }
        let cull = self.max_char_count / 2;
        let n = (cull as usize).min(self.text.len());
        self.text.drain(0..n);
        let fi = self.find_format(cull);
        if fi > 0 {
            self.fs[0] = self.fs[fi];
            self.fs[0].tsi = 0;
            self.fs.drain(1..=fi);
        }
        for f in self.fs.iter_mut().skip(1) {
            f.tsi -= cull;
        }
        self.invalidate_line_breaks(ui, me);
        self.invalidate_vsb = true;
    }

    /// `FindFormatStreamIndexForTextStreamPos`.
    fn find_format(&self, pos: i32) -> usize {
        self.fs.iter().rposition(|f| f.tsi <= pos).unwrap_or(0)
    }

    fn generate_render_state(&self, pos: i32, rs: &mut RenderState) {
        let fi = self.find_format(pos);
        rs.fsi = fi as i32;
        if let Some(f) = self.fs.get(fi) {
            rs.color = f.color;
            rs.indent = f.indent;
        }
    }

    /// `UpdateRenderState`: one format entry per character.
    fn update_render_state(&mut self, pos: i32, rs: &mut RenderState) -> bool {
        let Some(f) = usize::try_from(rs.fsi).ok().and_then(|i| self.fs.get(i)).copied() else { return false };
        if f.tsi != pos {
            return false;
        }
        rs.color = f.color;
        if f.indent != rs.indent {
            rs.x = self.draw_offset.0 + f.indent;
        }
        rs.indent = f.indent;
        self.pixels_indent = f.indent;
        rs.fsi += 1;
        true
    }

    fn recalculate_default_state(&mut self, ui: &Ui, me: PanelId, start: i32) {
        if self.text.is_empty() {
            return;
        }
        self.cached.color = ui.node(me).map_or(Color::WHITE, |n| n.fg);
        self.pixels_indent = 0;
        let mut rs = self.cached;
        self.generate_render_state(start, &mut rs);
        self.cached = rs;
        self.recalc_saved_render_state = false;
    }

    /// `SelectAllText`.
    pub fn select_all_text(&mut self) {
        self.select = [0, self.len()];
    }

    /// `GotoTextStart`.
    pub fn goto_text_start(&mut self, ui: &mut Ui) {
        self.cursor = 0;
        self.invalidate_vsb = true;
        ScrollBar::set_value(ui, self.vsb, 0);
    }

    /// `GotoTextEnd`.
    pub fn goto_text_end(&mut self, ui: &mut Ui) {
        self.cursor = self.len();
        self.invalidate_vsb = true;
        let (_, max) = ScrollBar::range(ui, self.vsb);
        ScrollBar::set_value(ui, self.vsb, max);
    }

    /// `SetFont`.
    pub fn set_font(&mut self, ui: &mut Ui, me: PanelId, font: FontHandle) {
        self.font = font;
        ui.invalidate_layout(me);
        self.recalc_line_breaks = true;
    }

    /// `SetVerticalScrollbar`.
    pub fn set_vertical_scrollbar(&mut self, ui: &mut Ui, me: PanelId, state: bool) {
        if ui.is_visible(self.vsb) != state {
            ui.set_visible(self.vsb, state);
            ui.invalidate_layout(me);
        }
    }

    /// `RecalculateLineBreaks`.
    fn recalculate_line_breaks(&mut self, ui: &Ui, me: PanelId, s: &dyn Surface) {
        self.recalc_line_breaks = false;
        self.recalc_saved_render_state = true;
        let count = self.len();
        if count == 0 {
            return;
        }
        let font = self.font;
        let mut wide = ui.size(me).0;
        if ui.is_visible(self.vsb) {
            wide -= ui.size(self.vsb).0;
        }
        let mut x = self.draw_offset.0;
        let mut word_start = 0;
        let mut has_word = false;
        let mut just_started = true;
        let mut word_on_new_line = true;
        let r = self.recalc_breaks_index;
        let mut start = if r <= 0 {
            self.lb.clear();
            0
        } else {
            self.lb.truncate(r as usize + 1);
            self.lb_at(r as usize)
        };
        if matches!(self.at(start), '\r' | '\n') {
            start += 1;
        }
        let mut rs = RenderState::default();
        self.generate_render_state(start, &mut rs);
        let mut i = start;
        while i < count {
            let ch = self.at(i);
            rs.x = x;
            if self.update_render_state(i, &mut rs) {
                x = rs.x;
            }
            if !iswspace(ch) {
                if !has_word {
                    word_start = i;
                    has_word = true;
                    word_on_new_line = just_started;
                }
            } else {
                has_word = false;
            }
            let cw = s.char_width(font, ch);
            if iswprint(ch) {
                just_started = false;
            }
            if x + cw >= wide || ch == '\r' || ch == '\n' {
                x = self.pixels_indent + self.draw_offset.0;
                just_started = true;
                has_word = false;
                if ch == '\r' || ch == '\n' {
                    self.lb.push(i);
                } else if !word_on_new_line && !iswspace(ch) {
                    self.lb.push(word_start);
                    i = word_start;
                } else {
                    self.lb.push(i);
                }
            }
            x += cw;
            i += 1;
        }
        self.lb.push(LB_END);
        self.invalidate_vsb = true;
    }

    /// `LayoutVerticalScrollBarSlider` (top/right inset swap kept; follows new text only
    /// when the view was at the end).
    fn layout_vsb(&mut self, ui: &mut Ui, me: PanelId, s: &dyn Surface) {
        self.invalidate_vsb = false;
        let sb = self.vsb;
        if !ui.is_visible(sb) {
            return;
        }
        let val = ScrollBar::value(ui, sb);
        let (min, max) = ScrollBar::range(ui, sb);
        let at_end = max != 0 && min + val + ScrollBar::range_window(ui, sb) == max;
        let (mut wide, tall) = ui.size(me);
        let [_l, t, r, b] = ui.node(me).and_then(|n| n.border.as_ref().map(|b| b.inset)).unwrap_or([0; 4]);
        wide -= t;
        ui.set_pos(sb, wide - self.scroll_bar_size - 1, 0);
        ui.set_size(sb, self.scroll_bar_size, tall - r - b);
        let vis = tall / (s.font_tall(self.font) + self.draw_offset.1);
        let n = self.lb.len() as i32;
        if n > vis && vis != 0 {
            ScrollBar::set_range(ui, sb, 0, n);
            ScrollBar::set_range_window(ui, sb, vis);
            ui.set_enabled(sb, true);
            ScrollBar::set_button_pressed_scroll_value(ui, sb, 1);
            if at_end {
                ScrollBar::set_value(ui, sb, n - vis);
            }
            ui.invalidate_layout(sb);
        } else {
            ui.set_enabled(sb, false);
            ScrollBar::set_range(ui, sb, 0, n);
            ScrollBar::set_range_window(ui, sb, n);
            ScrollBar::set_value(ui, sb, 0);
        }
    }

    /// The first visible line and its first text index.
    fn start_of_view(&self, ui: &mut Ui) -> (usize, i32) {
        let mut lbi = ScrollBar::value(ui, self.vsb).max(0) as usize;
        if lbi >= self.lb.len() {
            lbi = self.lb.len().saturating_sub(1);
        }
        let start = if lbi != 0 && lbi < self.lb.len() { self.lb[lbi - 1] } else { 0 };
        (lbi, start)
    }

    /// `PaintBackground`.
    fn paint_rt(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let bg = ui.node(me).map_or(Color::WHITE, |n| n.bg);
        s.draw_set_color(bg);
        let (wide, tall) = ui.size(me);
        s.draw_filled_rect(0, 0, wide, tall);
        s.draw_set_text_font(self.font);
        if self.text.is_empty() {
            return;
        }
        let (mut lbi, start) = self.start_of_view(ui);
        if self.recalc_saved_render_state {
            self.recalculate_default_state(ui, me, start);
        }
        let mut rs = self.cached;
        self.pixels_indent = self.cached.indent;
        if let Some(f) = usize::try_from(rs.fsi).ok().and_then(|i| self.fs.get(i)) {
            rs.color = f.color;
        }
        rs.fsi += 1;
        rs.x = self.draw_offset.0 + self.pixels_indent;
        rs.y = self.draw_offset.1;
        let th = s.font_tall(self.font);
        let (lo, hi) = if self.select[0] == -1 { (-1, -1) } else { (self.select[0].min(self.select[1]), self.select[0].max(self.select[1])) };
        let count = self.len();
        let mut i = start;
        while i < count && rs.y < tall {
            let ch = self.at(i);
            self.update_render_state(i, &mut rs);
            if self.lb_at(lbi) == i {
                rs.x = self.draw_offset.0 + self.pixels_indent;
                rs.y += th + self.draw_offset.1;
                lbi += 1;
            }
            let mut adv = 0;
            if iswprint(ch) {
                let cw = s.char_width(self.font, ch);
                if i >= lo && i < hi {
                    s.draw_set_color(self.selection_color);
                    s.draw_filled_rect(rs.x, rs.y, rs.x + cw, rs.y + th + 1);
                    s.draw_set_text_color(self.selection_text_color);
                } else {
                    s.draw_set_text_color(rs.color);
                }
                s.draw_set_text_pos(rs.x, rs.y);
                s.draw_unicode_char(ch);
                adv = cw;
            }
            rs.x += adv;
            i += 1;
        }
    }

    /// `CursorToPixelSpace` (used by OnKillFocus only).
    fn cursor_to_pixel_space(&mut self, ui: &mut Ui, pos: i32, s: &dyn Surface) -> (i32, i32) {
        let (mut x, mut y) = self.draw_offset;
        self.pixels_indent = 0;
        let (mut lbi, start) = self.start_of_view(ui);
        let th = s.font_tall(self.font);
        let mut i = start;
        while i < self.len() {
            if i == pos {
                if self.lb_at(lbi) == i {
                    x = self.draw_offset.0 + self.pixels_indent;
                    y += th + self.draw_offset.1;
                }
                break;
            }
            if self.lb_at(lbi) == i {
                x = self.draw_offset.0 + self.pixels_indent;
                y += th + self.draw_offset.1;
                lbi += 1;
            }
            x += s.char_width(self.font, self.at(i));
            i += 1;
        }
        (x, y)
    }

    /// `PixelToCursorSpace`.
    fn pixel_to_cursor_space(&mut self, ui: &mut Ui, me: PanelId, cx: i32, cy: i32, s: &dyn Surface) -> i32 {
        let tall = s.font_tall(self.font);
        let mut x = self.draw_offset.0;
        let y_top = self.draw_offset.1;
        let mut y = y_top;
        self.pixels_indent = 0;
        let (mut lbi, start) = self.start_of_view(ui);
        if self.recalc_saved_render_state {
            self.recalculate_default_state(ui, me, start);
        }
        self.pixels_indent = self.cached.indent;
        let mut on_right = false;
        let wide = ui.size(me).0;
        let mut i = start;
        while i < self.len() {
            let ch = self.at(i);
            if self.lb_at(lbi) == i {
                x = self.draw_offset.0 + self.pixels_indent;
                y += tall + self.draw_offset.1;
                lbi += 1;
                if on_right {
                    return i;
                }
            }
            if cy < y_top || (cy >= y && cy < y + tall + self.draw_offset.1) {
                on_right = true;
            }
            let w = s.char_width(self.font, ch);
            if on_right {
                if cx <= wide && (cx < self.draw_offset.0 + self.pixels_indent || cy < y_top) {
                    return i;
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

    /// `RequestFocus(0)`, routed to the focus panel for the console history.
    fn request_focus(&self, ui: &mut Ui, me: PanelId) {
        ui.request_focus(self.focus_panel.unwrap_or(me));
    }

    /// `CopySelected`.
    fn copy_selected(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if self.select[0] != -1 {
            let (x0, x1) = (self.select[0].min(self.select[1]), self.select[0].max(self.select[1]));
            let mut buf = String::new();
            for i in x0..x1 {
                let c = self.at(i);
                if c == '\n' {
                    buf.push('\r');
                }
                buf.push(c);
            }
            s.set_clipboard_text(&buf);
        }
        self.request_focus(ui, me);
    }

    fn mouse_pressed(&mut self, ui: &mut Ui, me: PanelId, code: i32, s: &mut dyn Surface) {
        if code == MOUSE_LEFT {
            self.select[0] = -1;
            let (cx, cy) = ui.cursor_pos();
            let (ax, ay) = ui.abs_pos(me);
            self.cursor = self.pixel_to_cursor_space(ui, me, cx - ax, cy - ay, s);
            ui.set_mouse_capture(Some(me));
            self.mouse_selection = true;
            if self.select[0] < 0 {
                self.select[0] = self.cursor;
            }
            self.select[1] = self.cursor;
            self.request_focus(ui, me);
        } else if code == MOUSE_RIGHT {
            if let Some(m) = self.edit_menu.take() {
                ui.delete(m);
            }
            let m = Menu::create(ui, Some(me), "EditMenu");
            Menu::add_item_msg(ui, m, "C&opy", "C&opy", Msg::new("DoCopySelected"), Some(me), None);
            Menu::set_visible(ui, m, false);
            ui.set_parent(m, Some(me));
            ui.add_action_signal_target(m, me);
            self.edit_menu = Some(m);
            let (cx, cy) = ui.cursor_pos();
            // SetItemEnabled("C&opy", select[0] != -1): the item's name is "C&opy".
            Menu::set_item_enabled(ui, m, 0, self.select[0] != -1);
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
    }

    /// `OnKeyCodeTyped`; `false` passes the code to `Panel::OnKeyCodeTyped`.
    fn key_code_typed(&mut self, ui: &mut Ui, me: PanelId, code: i32, s: &mut dyn Surface) -> bool {
        let shift = ui.is_key_down(KEY_LSHIFT) || ui.is_key_down(KEY_RSHIFT);
        let ctrl = ui.is_key_down(KEY_LCONTROL) || ui.is_key_down(KEY_RCONTROL);
        let alt = ui.is_key_down(KEY_LALT) || ui.is_key_down(KEY_RALT);
        if ctrl {
            match code {
                c if c == KEY_C || c == KEY_X || c == KEY_INSERT => self.copy_selected(ui, me, s),
                KEY_HOME | KEY_PAGEUP => self.goto_text_start(ui),
                KEY_END | KEY_PAGEDOWN => self.goto_text_end(ui),
                _ => {
                    self.select[1] = self.cursor;
                    return false;
                }
            }
            self.select[1] = self.cursor;
            return true;
        }
        if alt {
            self.select[1] = self.cursor;
            return false;
        }
        match code {
            KEY_ENTER | KEY_TAB | KEY_ESCAPE | KEY_LSHIFT | KEY_RSHIFT => {
                self.select[1] = self.cursor;
                return false;
            }
            KEY_DELETE => {
                if shift {
                    self.copy_selected(ui, me, s);
                }
            }
            KEY_HOME => self.goto_text_start(ui),
            KEY_END => self.goto_text_end(ui),
            KEY_PAGEUP | KEY_PAGEDOWN => {
                if ui.is_visible(self.vsb) {
                    let v = ScrollBar::value(ui, self.vsb);
                    let w = ScrollBar::range_window(ui, self.vsb);
                    let nv = if code == KEY_PAGEUP { v - w - 1 } else { v + w + 1 };
                    ScrollBar::set_value(ui, self.vsb, nv);
                }
            }
            _ => return true,
        }
        self.select[1] = self.cursor;
        true
    }

    fn handle(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) -> bool {
        if msg.is("KeyCodeTyped") {
            return self.key_code_typed(ui, me, msg.get_int("code"), s);
        }
        if msg.is("MousePressed") {
            self.mouse_pressed(ui, me, msg.get_int("code"), s);
        } else if msg.is("MouseDoublePressed") {
            if msg.get_int("code") != MOUSE_LEFT {
                return true;
            }
            self.mouse_pressed(ui, me, MOUSE_LEFT, s);
            if self.cursor > 0 {
                let mut p = self.cursor - 1;
                while p >= 0 && iswspace(self.at(p)) {
                    p -= 1;
                }
                if p >= 0 {
                    p -= 1;
                    while p >= 0 && !iswspace(self.at(p)) {
                        p -= 1;
                    }
                } else {
                    p -= 1;
                }
                self.cursor = p + 1;
            }
            let sel_start = self.cursor;
            let count = self.len();
            let mut p = self.cursor + 1;
            while p < count && !iswspace(self.at(p)) {
                p += 1;
            }
            p += 1;
            while p < count && iswspace(self.at(p)) {
                p += 1;
            }
            if p > count {
                p = count;
            }
            self.cursor = p;
            if p > 0 && p - 1 < count && iswspace(self.at(p - 1)) {
                p -= 1;
                self.cursor = p;
            }
            self.select = [sel_start, self.cursor];
            self.mouse_selection = true;
        } else if msg.is("MouseReleased") {
            self.mouse_selection = false;
            ui.set_mouse_capture(None);
            if self.select[0] != -1 && self.select[0] == self.select[1] {
                self.select[0] = -1;
            }
        } else if msg.is("CursorMoved") {
            if self.mouse_selection {
                let (cx, cy) = ui.cursor_pos();
                let (ax, ay) = ui.abs_pos(me);
                self.cursor = self.pixel_to_cursor_space(ui, me, cx - ax, cy - ay, s);
                if self.cursor != self.select[1] {
                    self.select[1] = self.cursor;
                }
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
        } else if msg.is("MouseCaptureLost") {
            self.mouse_selection = false;
        } else if msg.is("MouseWheeled") {
            if ui.is_visible(self.vsb) {
                let v = ScrollBar::value(ui, self.vsb) - 3 * msg.get_int("delta");
                ScrollBar::set_value(ui, self.vsb, v);
                self.recalc_saved_render_state = true;
            }
        } else if msg.is("KillFocus") {
            // The was-pressed/released flags are cleared before KillFocus is dispatched.
            if ui.is_mouse_down(MOUSE_RIGHT) && self.select[0] != -1 {
                let (s0, e0) = (self.select[0].min(self.select[1]), self.select[0].max(self.select[1]));
                let (_, sy) = self.cursor_to_pixel_space(ui, s0, s);
                let (_, ey) = self.cursor_to_pixel_space(ui, e0, s);
                let (_, cy) = ui.cursor_pos();
                let cy = cy - ui.abs_pos(me).1;
                if sy < cy && cy < ey + s.font_tall(self.font) {
                    return true;
                }
            }
            self.select[0] = -1;
        } else if msg.is("DoCopySelected") {
            self.copy_selected(ui, me, s);
        } else if msg.is("DoCutSelected") {
            self.copy_selected(ui, me, s);
            self.request_focus(ui, me);
        } else if msg.is("SetText") {
            let t = msg.get_str("text");
            self.set_text(ui, me, &t);
        } else if msg.is("ScrollBarSliderMoved") {
            self.recalc_saved_render_state = true;
        } else {
            return false;
        }
        true
    }
}

impl Control for RichText {
    fn class(&self) -> &'static str {
        if self.focus_panel.is_some() { "CNoKeyboardInputRichText" } else { "RichText" }
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        self.handle(ui, me, msg, s)
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let (prop, hd) = (ui.is_proportional(me), ui.is_hd_proportional(me));
        self.font = s.font(&scheme.file, "Default", prop, hd);
        let fg = scheme.color("WindowFgColor", Color::WHITE);
        let bg = scheme.color("WindowBgColor", Color::WHITE);
        if let Some(n) = ui.node_mut(me) {
            n.fg = fg;
            n.bg = bg;
        }
        // RichText::SetFgColor: the untouched default run takes the colour.
        if self.fs.len() == 1 && self.fs[0].color == Color::TRANSPARENT {
            self.fs[0].color = fg;
        }
        self.disabled_fg = scheme.color("WindowDisabledFgColor", Color::WHITE);
        self.selection_text_color = scheme.color("SelectionFgColor", fg);
        self.selection_color = scheme.color("SelectionBgColor", Color::WHITE);
        let border = scheme.border("ButtonDepressedBorder");
        if let Some(n) = ui.node_mut(me) {
            n.border = border;
        }
        if self.mono {
            let f = s.font(&scheme.file, "GameConsole_Mono", prop, hd);
            self.set_font(ui, me, f);
        }
    }
    fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &Block, s: &mut dyn Surface) {
        ui.panel_apply_settings(me, kv, s);
        self.max_char_count = kv_int(kv, "maxchars", -1);
        if let Some(t) = kv_str(kv, "text").filter(|t| !t.is_empty()) {
            let t = t.to_owned();
            self.set_text(ui, me, &t);
        }
    }
    fn think(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if self.recalc_line_breaks {
            self.recalc_saved_render_state = true;
            self.recalculate_line_breaks(ui, me, s);
            if self.invalidate_vsb {
                self.layout_vsb(ui, me, s);
            }
        }
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.paint_rt(ui, me, s);
    }
    fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId, _w: i32, _h: i32) {
        self.invalidate_vsb = true;
        self.invalidate_line_breaks(ui, me);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
