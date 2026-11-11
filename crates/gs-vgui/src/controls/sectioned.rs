//! `SectionedListPanel`, `CSectionHeader`, `CItemButton` (docs/re/vgui-controls-2.md §7).
//! Headers and rows are Labels whose only images are per-column TextImages; their paint is
//! `Label::Paint` for that case (images at their `SetImageBounds` x, vertically centred,
//! clipped to the slot width).

use std::any::Any;

use gs_formats::keyvalues::Block;

use crate::controls::label::TextImage;
use crate::controls::scroll_bar::ScrollBar;
use crate::editable::kv_int;
use crate::keys::*;
use crate::msg::{Msg, Value};
use crate::scheme::Scheme;
use crate::surface::{Color, FontHandle, Surface};
use crate::ui::{Control, PanelId, Ui};

pub const HEADER_IMAGE: i32 = 0x01;
pub const COLUMN_IMAGE: i32 = 0x02;
pub const COLUMN_BRIGHT: i32 = 0x04;
pub const COLUMN_CENTER: i32 = 0x08;
pub const COLUMN_RIGHT: i32 = 0x10;

#[derive(Debug, Clone)]
struct Column {
    name: String,
    text: String,
    flags: i32,
    width: i32,
}

/// `SectionSortFunc_t`: `true` when `a` goes before `b`.
pub type SortFunc = fn(&mut Ui, PanelId, i32, i32) -> bool;

#[derive(Debug, Clone)]
struct Section {
    id: i32,
    always_visible: bool,
    header: PanelId,
    columns: Vec<Column>,
    sort: Option<SortFunc>,
}

/// One column image of a header or row: the TextImage and its `SetImageBounds`.
#[derive(Debug, Clone, Default)]
struct Cell {
    img: Option<TextImage>,
    xpos: i32,
    width: i32,
}

fn paint_cells(cells: &mut [Cell], ui: &Ui, me: PanelId, s: &mut dyn Surface) {
    let (_, ph) = ui.size(me);
    let max_y = cells.iter().filter_map(|c| c.img.as_ref()).map(|t| t.size.1).max().unwrap_or(0);
    let ty0 = (ph - max_y) / 2;
    for c in cells.iter_mut() {
        let Some(t) = c.img.as_mut() else { continue };
        if c.width >= 0 && t.size.0 > c.width {
            let h = t.size.1;
            t.set_size(c.width, h);
        }
        let ih = t.size.1;
        t.pos = (c.xpos, ty0 + ((max_y - ih) >> 1));
        t.paint(s);
    }
}

/// `TextImage::ResizeImageToContentMaxWidth`.
fn resize_max(t: &mut TextImage, max_w: i32, s: &dyn Surface) {
    t.set_draw_width(max_w);
    let (cw, ch) = t.content_size(s);
    t.set_size(cw.min(max_w), ch);
}

// ---------------------------------------------------------------------------------------------

/// `vgui2::SectionedListPanel`.
#[derive(Debug)]
pub struct SectionedListPanel {
    selected: Option<PanelId>,
    sections: Vec<Section>,
    /// `m_Items`: item ID → button; `None` = free index.
    items: Vec<Option<PanelId>>,
    /// The linked-list order of live IDs (creation order).
    order: Vec<i32>,
    /// Free IDs, the head first.
    free_ids: Vec<i32>,
    /// `m_FreeItems`: buttons kept for reuse.
    free_items: Vec<PanelId>,
    sorted: Vec<PanelId>,
    edit_panel: Option<PanelId>,
    edit_item: i32,
    edit_col: i32,
    scrollbar_size: i32,
    content_height: i32,
    line_spacing: i32,
    scroll_bar: PanelId,
    sort_needed: bool,
    vscroll: bool,
}

/// Runs `f` on the sectioned list `id` (or the list part of a subclass, `Control::sectioned`).
pub fn with_sectioned<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut SectionedListPanel, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.sectioned().map(|l| f(l, ui))).flatten()
}

impl SectionedListPanel {
    /// The state of a new list; `me` must be its panel (subclasses create the node first).
    pub fn new(ui: &mut Ui, me: PanelId) -> Self {
        let scroll_bar = ScrollBar::create(ui, Some(me), "SectionedScrollBar", true);
        ui.set_visible(scroll_bar, false);
        ui.add_action_signal_target(scroll_bar, me);
        let scrollbar_size = if ui.is_proportional(me) {
            let (sw, _) = ui.screen;
            let base = ui.proportional_base_of(me).0;
            let v = sw as f32 / base as f32 * 18.0;
            if (-2_147_483_648.0..2_147_483_648.0).contains(&v) { v as i32 } else { i32::MIN }
        } else {
            18
        };
        Self {
            selected: None,
            sections: Vec::new(),
            items: Vec::new(),
            order: Vec::new(),
            free_ids: Vec::new(),
            free_items: Vec::new(),
            sorted: Vec::new(),
            edit_panel: None,
            edit_item: 0,
            edit_col: 0,
            scrollbar_size,
            content_height: 0,
            line_spacing: 20,
            scroll_bar,
            sort_needed: false,
            vscroll: true,
        }
    }

    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str) -> PanelId {
        let me = ui.create(parent, name, Box::new(crate::controls::misc::Divider));
        let st = Self::new(ui, me);
        ui.replace_ctrl(me, |_| Box::new(st));
        me
    }

    fn find_section(&self, id: i32) -> Option<usize> {
        self.sections.iter().position(|s| s.id == id)
    }

    fn columns(&self, section: i32) -> &[Column] {
        self.find_section(section).map_or(&[], |i| &self.sections[i].columns)
    }

    /// `AddSection(id, name)` (the char overload: `name` is the header's panel name).
    pub fn add_section(&mut self, ui: &mut Ui, me: PanelId, id: i32, name: &str, sort: Option<SortFunc>) {
        let header = ui.create(Some(me), name, Box::new(SectionHeader { list: me, section: id, divider: Color::TRANSPARENT, font: FontHandle(0), cells: Vec::new(), columns: Vec::new() }));
        if let Some(n) = ui.node_mut(header) {
            n.paint_background_enabled = false;
        }
        ui.reapply_scheme(header);
        self.sections.push(Section { id, always_visible: false, header, columns: Vec::new(), sort });
    }

    /// `AddColumnToSection(id, name, text, flags, width)` (`text` localized when a `#token`).
    pub fn add_column(&mut self, ui: &Ui, id: i32, name: &str, text: &str, flags: i32, width: i32) -> bool {
        let Some(i) = self.find_section(id) else { return false };
        let t = match ui.localize.find(text.strip_prefix('#').unwrap_or(text)) {
            Some(w) if text.starts_with('#') || ui.localize.find(text).is_some() => w.to_owned(),
            _ => text.to_owned(),
        };
        let name: String = name.chars().take(31).collect();
        let text: String = t.chars().take(63).collect();
        self.sections[i].columns.push(Column { name, text, flags, width });
        true
    }

    /// `AddItem(sectionID, data)`: the new item's ID.
    pub fn add_item(&mut self, ui: &mut Ui, me: PanelId, section: i32, data: &Msg) -> i32 {
        let id = self.new_item_button(ui, me);
        self.modify_item(ui, me, id, section, data);
        if let Some(Some(b)) = self.items.get(id as usize) {
            self.sorted.push(*b);
        }
        self.sort_needed = true;
        id
    }

    fn new_item_button(&mut self, ui: &mut Ui, me: PanelId) -> i32 {
        let id = match self.free_ids.first().copied() {
            Some(i) => {
                self.free_ids.remove(0);
                i
            }
            None => {
                self.items.push(None);
                (self.items.len() - 1) as i32
            }
        };
        self.order.push(id);
        let button = if let Some(b) = (!self.free_items.is_empty()).then(|| self.free_items.remove(0)) {
            ui.with_ctrl(b, |c, _| {
                if let Some(it) = c.as_any_mut().downcast_mut::<ItemButton>() {
                    it.id = id;
                }
            });
            ui.set_visible(b, true);
            b
        } else {
            let b = ui.create(Some(me), "", Box::new(ItemButton::new(me, id)));
            if let Some(n) = ui.node_mut(b) {
                n.paint_background_enabled = false;
            }
            ui.reapply_scheme(b);
            b
        };
        self.items[id as usize] = Some(button);
        id
    }

    /// `ModifyItem(id, sectionID, data)`.
    pub fn modify_item(&mut self, ui: &mut Ui, me: PanelId, id: i32, section: i32, data: &Msg) -> bool {
        let Some(Some(b)) = self.items.get(id as usize).copied() else { return false };
        ui.invalidate_layout(me);
        ui.with_ctrl(b, |c, _| {
            if let Some(it) = c.as_any_mut().downcast_mut::<ItemButton>() {
                if it.section != section {
                    it.cells.clear();
                }
                it.section = section;
                it.data = Some(data.clone());
            }
        });
        ui.invalidate_layout(b);
        self.sort_needed = true;
        true
    }

    /// `RemoveItem(id)`: the button is destroyed.
    pub fn remove_item(&mut self, ui: &mut Ui, me: PanelId, id: i32) -> bool {
        let Some(Some(b)) = self.items.get(id as usize).copied() else { return false };
        self.sorted.retain(|&x| x != b);
        self.sort_needed = true;
        ui.mark_for_deletion(b);
        self.items[id as usize] = None;
        self.order.retain(|&x| x != id);
        self.free_ids.insert(0, id);
        if self.selected == Some(b) {
            self.selected = None;
        }
        ui.invalidate_layout(me);
        true
    }

    /// `DeleteAllItems`: buttons are kept for reuse; no `ItemSelected`.
    pub fn delete_all_items(&mut self, ui: &mut Ui, me: PanelId) {
        for &id in &self.order {
            if let Some(Some(b)) = self.items.get(id as usize).copied() {
                ui.set_visible(b, false);
                ui.with_ctrl(b, |c, _| {
                    if let Some(it) = c.as_any_mut().downcast_mut::<ItemButton>() {
                        it.selected = false;
                        it.override_colors = false;
                        it.section = -1;
                        it.cells.clear();
                    }
                });
                if let Some(n) = ui.node_mut(b) {
                    n.paint_background_enabled = false;
                }
                self.free_items.push(b);
            }
        }
        let total = self.items.len();
        self.items.iter_mut().for_each(|i| *i = None);
        self.order.clear();
        self.free_ids = (0..total as i32).collect();
        self.sorted.clear();
        self.selected = None;
        ui.invalidate_layout(me);
        self.sort_needed = true;
    }

    pub fn item_data(&self, ui: &mut Ui, id: i32) -> Option<Msg> {
        let b = self.items.get(id as usize).copied().flatten()?;
        ui.ctrl::<ItemButton>(b).and_then(|it| it.data.clone())
    }

    /// Changes one key of an item's data (the page edits its KeyValues in place).
    pub fn set_item_value(&self, ui: &mut Ui, id: i32, key: &str, value: &str) {
        let Some(b) = self.items.get(id as usize).copied().flatten() else { return };
        if let Some(it) = ui.ctrl::<ItemButton>(b)
            && let Some(d) = it.data.as_mut()
        {
            d.set(key, Value::Str(value.to_owned()));
        }
    }

    pub fn is_item_id_valid(&self, id: i32) -> bool {
        self.items.get(id as usize).is_some_and(Option::is_some)
    }

    pub fn item_count(&self) -> i32 {
        self.sorted.len() as i32
    }

    pub fn item_id_from_row(&self, ui: &mut Ui, row: i32) -> i32 {
        usize::try_from(row).ok().and_then(|r| self.sorted.get(r)).and_then(|&b| ui.ctrl::<ItemButton>(b).map(|it| it.id)).unwrap_or(-1)
    }

    pub fn selected_item(&self, ui: &mut Ui) -> i32 {
        self.selected.and_then(|b| ui.ctrl::<ItemButton>(b).map(|it| it.id)).unwrap_or(-1)
    }

    /// `SetSelectedItem(id)`.
    pub fn set_selected_item_id(&mut self, ui: &mut Ui, me: PanelId, id: i32) {
        if let Some(Some(b)) = self.items.get(id as usize).copied() {
            self.set_selected(ui, me, Some(b));
        }
    }

    /// `SetSelectedItem(CItemButton*)`: `ItemSelected` when the selection changes.
    fn set_selected(&mut self, ui: &mut Ui, me: PanelId, item: Option<PanelId>) {
        if self.selected == item {
            return;
        }
        if let Some(old) = self.selected.filter(|&o| ui.exists(o)) {
            let was = ui.ctrl::<ItemButton>(old).is_some_and(|it| std::mem::replace(&mut it.selected, false));
            if was {
                if let Some(n) = ui.node_mut(old) {
                    n.paint_background_enabled = false;
                }
                ui.invalidate_layout(old);
            }
        }
        self.selected = item;
        let mut new_id = -1;
        if let Some(it) = item {
            let was = ui.ctrl::<ItemButton>(it).map(|b| (b.selected, b.id));
            if let Some((sel, id)) = was {
                new_id = id;
                if !sel {
                    ui.request_focus(it);
                    if let Some(b) = ui.ctrl::<ItemButton>(it) {
                        b.selected = true;
                    }
                    if let Some(n) = ui.node_mut(it) {
                        n.paint_background_enabled = true;
                    }
                    ui.invalidate_layout(it);
                }
            }
        }
        ui.post_action_signal(me, Msg::new("ItemSelected").int("itemID", new_id));
    }

    /// `InvalidateItem(id)`.
    pub fn invalidate_item(&self, ui: &mut Ui, id: i32) {
        if let Some(Some(b)) = self.items.get(id as usize).copied() {
            ui.invalidate_layout(b);
        }
    }

    /// `GetCellBounds(id, col)`: `(x, y, w, h)` with x relative to the row content.
    pub fn cell_bounds(&self, ui: &mut Ui, id: i32, col: i32, s: &dyn Surface) -> Option<(i32, i32, i32, i32)> {
        let b = self.items.get(id as usize).copied().flatten()?;
        if !ui.is_visible(b) {
            return None;
        }
        let (_, y) = ui.pos(b);
        let (_, t) = ui.size(b);
        let (section, cells) = ui.ctrl::<ItemButton>(b).map(|it| (it.section, it.cells.clone()))?;
        let mut x = 0;
        for (i, c) in self.columns(section).iter().enumerate() {
            let Some(img) = cells.get(i).and_then(|c| c.img.clone()) else { continue };
            let mut img = img;
            let (cw, _) = img.content_size(s);
            let width = if c.width < 0 { cw } else { c.width };
            if i as i32 == col {
                return Some((x, y, width, t));
            }
            x += width;
        }
        Some((x, y, 0, t))
    }

    /// `EnterEditMode(id, col, panel)`.
    pub fn enter_edit_mode(&mut self, ui: &mut Ui, me: PanelId, id: i32, col: i32, panel: PanelId) {
        self.edit_panel = Some(panel);
        self.edit_item = id;
        self.edit_col = col;
        ui.set_parent(panel, Some(me));
        ui.set_visible(panel, true);
        ui.request_focus(panel);
        ui.move_to_front(panel);
        ui.invalidate_layout(me);
    }

    /// `LeaveEditMode`.
    pub fn leave_edit_mode(&mut self, ui: &mut Ui) {
        if let Some(p) = self.edit_panel.take() {
            self.invalidate_item(ui, self.edit_item);
            ui.set_visible(p, false);
            ui.set_parent(p, None);
        }
    }

    pub fn is_in_edit_mode(&self) -> bool {
        self.edit_panel.is_some()
    }

    fn section_tall(&self, ui: &mut Ui, s: &dyn Surface) -> i32 {
        let font = self.sections.first().and_then(|sec| ui.ctrl::<SectionHeader>(sec.header).map(|h| h.font)).unwrap_or(FontHandle(0));
        if self.sections.is_empty() || font == FontHandle(0) { 20 } else { s.font_tall(font) + 7 }
    }

    /// `ReSortList`.
    fn resort(&mut self, ui: &mut Ui) {
        self.sorted.clear();
        let order: Vec<(i32, PanelId, i32)> = self.order.iter().filter_map(|&id| self.items[id as usize].map(|b| (id, b))).filter_map(|(id, b)| ui.ctrl::<ItemButton>(b).map(|it| (id, b, it.section))).collect();
        let me = ui.parent(self.scroll_bar);
        for s in self.sections.clone() {
            let start = self.sorted.len();
            for &(id, b, sec) in &order {
                if sec != s.id {
                    continue;
                }
                match (s.sort, me) {
                    (Some(f), Some(me)) => {
                        let mut j = start;
                        while j < self.sorted.len() {
                            let other = ui.ctrl::<ItemButton>(self.sorted[j]).map_or(-1, |it| it.id);
                            if f(ui, me, id, other) {
                                break;
                            }
                            j += 1;
                        }
                        self.sorted.insert(j, b);
                    }
                    _ => self.sorted.push(b),
                }
            }
        }
    }

    /// `LayoutPanels`: the content height.
    fn layout_panels(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) -> i32 {
        let sec_tall = self.section_tall(ui, s);
        let mut y = 5;
        let mut wide = ui.size(me).0 - 10;
        let sb_visible = ui.is_visible(self.scroll_bar);
        if sb_visible {
            y = 5 - ScrollBar::value(ui, self.scroll_bar);
            wide -= ui.size(self.scroll_bar).0;
        }
        let row = self.line_spacing;
        for s_i in 0..self.sections.len() {
            let sec = self.sections[s_i].clone();
            let mut first = None;
            let mut last = None;
            for (i, &b) in self.sorted.iter().enumerate() {
                if ui.ctrl::<ItemButton>(b).is_some_and(|it| it.section == sec.id) {
                    last = Some(i);
                    if first.is_none() {
                        first = Some(i);
                    }
                }
            }
            if first.is_none() && !sec.always_visible {
                ui.set_visible(sec.header, false);
                continue;
            }
            ui.set_bounds(sec.header, 5, y, wide, sec_tall);
            ui.set_visible(sec.header, true);
            y += sec_tall;
            if let (Some(f), Some(l)) = (first, last) {
                for i in f..=l {
                    let it = self.sorted[i];
                    ui.set_bounds(it, 5, y, wide, row);
                    let it_id = ui.ctrl::<ItemButton>(it).map_or(-1, |b| b.id);
                    if let Some(ep) = self.edit_panel
                        && self.edit_item == it_id
                    {
                        let cells = ui.ctrl::<ItemButton>(it).map(|b| b.cells.clone()).unwrap_or_default();
                        let (mut cx, mut cw) = (0, 0);
                        for (c, col) in sec.columns.iter().enumerate() {
                            let Some(mut img) = cells.get(c).and_then(|c| c.img.clone()) else { continue };
                            let (iw, _) = img.content_size(s);
                            let w = if col.width < 0 { iw } else { col.width };
                            if c == 1 {
                                cw = w;
                                break;
                            }
                            cx += w;
                        }
                        ui.set_bounds(ep, cx, y, cw, sec_tall);
                    }
                    y += row;
                }
            }
            y += 8;
        }
        if sb_visible {
            y += ScrollBar::value(ui, self.scroll_bar);
        }
        y
    }

    /// `ScrollToItem(id)`.
    fn scroll_to_item(&self, ui: &mut Ui, me: PanelId, b: PanelId, s: &dyn Surface) {
        let sec_tall = self.section_tall(ui, s);
        let v = ScrollBar::value(ui, self.scroll_bar);
        let iy = ui.pos(b).1 + v;
        let ch = ui.size(me).1;
        if self.content_height <= ch {
            ScrollBar::set_value(ui, self.scroll_bar, 0);
        } else if iy < v {
            ScrollBar::set_value(ui, self.scroll_bar, iy);
        } else if iy > ch - sec_tall + v {
            ScrollBar::set_value(ui, self.scroll_bar, iy - ch + sec_tall);
        }
    }

    /// `OnKeyCodeTyped` (SectionedListPanel's).
    fn key_code_typed(&mut self, ui: &mut Ui, me: PanelId, code: i32, s: &mut dyn Surface) {
        if self.edit_panel.is_some() {
            ui.call_parent(me, &Msg::new("KeyCodeTyped").int("code", code), s);
            return;
        }
        let sec_tall = self.section_tall(ui, s);
        let count = self.sorted.len();
        let index_of = |l: &Self, b: Option<PanelId>| b.and_then(|b| l.sorted.iter().position(|&x| x == b));
        match code {
            KEY_DOWN | KEY_UP => {
                if count == 0 {
                    return;
                }
                let i = index_of(self, self.selected).unwrap_or(count);
                let n = if code == KEY_DOWN {
                    if i + 1 >= count {
                        if i == count {
                            // no selection: past the end (the original reads out of range)
                        }
                        return;
                    }
                    i + 1
                } else {
                    if i == 0 {
                        return;
                    }
                    i - 1
                };
                let b = self.sorted[n];
                self.set_selected(ui, me, Some(b));
                self.scroll_to_item(ui, me, b, s);
            }
            KEY_PAGEDOWN | KEY_PAGEUP => {
                let rows = ui.size(me).1 / sec_tall.max(1);
                let Some(cur) = self.selected else { return };
                let mut sec = ui.ctrl::<ItemButton>(cur).map_or(-1, |it| it.section);
                let mut r: i64 = index_of(self, Some(cur)).map_or(-1, |i| i as i64);
                let mut n = 0;
                let mut target = cur;
                let mut ran_out = false;
                while n < rows {
                    r += if code == KEY_PAGEDOWN { 1 } else { -1 };
                    if r < 0 || r >= count as i64 {
                        ran_out = true;
                        break;
                    }
                    n += 1;
                    target = self.sorted[r as usize];
                    let ts = ui.ctrl::<ItemButton>(target).map_or(-1, |it| it.section);
                    if ts != sec {
                        n += 1;
                        sec = ts;
                    }
                }
                self.set_selected(ui, me, Some(target));
                if code == KEY_PAGEUP && ran_out {
                    ScrollBar::set_value(ui, self.scroll_bar, 0);
                } else {
                    self.scroll_to_item(ui, me, target, s);
                }
            }
            KEY_LEFT | KEY_RIGHT => {}
            _ => ui.panel_on_key_code_typed(me, code, s),
        }
    }

    /// The list's own message handling (subclasses call it after their overrides).
    pub fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) -> bool {
        if msg.is("ScrollBarSliderMoved") {
            ui.invalidate_layout(me);
        } else if msg.is("KeyCodeTyped") {
            self.key_code_typed(ui, me, msg.get_int("code"), s);
        } else if msg.is("MouseWheeled") {
            let delta = msg.get_int("delta");
            if self.edit_panel.is_some() {
                ui.call_parent(me, &Msg::new("MouseWheeled").int("delta", delta), s);
            } else {
                let v = ScrollBar::value(ui, self.scroll_bar);
                ScrollBar::set_value(ui, self.scroll_bar, v - 60 * delta);
            }
        } else if msg.is("MousePressed") {
            self.set_selected(ui, me, None);
        } else if msg.is("SetFocus") {
            match self.selected.filter(|&b| ui.exists(b)) {
                Some(b) => ui.request_focus(b),
                None => return false,
            }
        } else if msg.is("ItemPressed") {
            let b = msg.get_panel("panel");
            self.set_selected(ui, me, b);
        } else {
            return false;
        }
        true
    }

    pub fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme) {
        ui.panel_apply_scheme(me, scheme);
        let bg = ui.node(me).map_or(Color::TRANSPARENT, |n| n.bg);
        let bg = scheme.color("BuddyListBgColor", bg);
        let border = scheme.border("ButtonDepressedBorder");
        if let Some(n) = ui.node_mut(me) {
            n.bg = bg;
            n.border = border;
        }
    }

    pub fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &Block, s: &mut dyn Surface) {
        ui.panel_apply_settings(me, kv, s);
        self.line_spacing = kv_int(kv, "linespacing", 0);
        if self.line_spacing == 0 {
            self.line_spacing = 20;
        }
        if ui.is_proportional(me) {
            self.line_spacing = ui.psv(me, self.line_spacing);
        }
    }

    pub fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if self.sort_needed {
            self.resort(ui);
            self.sort_needed = false;
        }
        self.content_height = self.layout_panels(ui, me, s);
        let (wide, tall) = ui.size(me);
        if self.content_height > tall && self.vscroll {
            let sb = self.scroll_bar;
            ui.set_visible(sb, true);
            ui.move_to_front(sb);
            ui.set_pos(sb, wide - self.scrollbar_size - 2, 0);
            ui.set_size(sb, self.scrollbar_size, tall - 2);
            ScrollBar::set_range_window(ui, sb, tall);
            ScrollBar::set_range(ui, sb, 0, self.content_height);
            ui.invalidate_layout(sb);
            self.content_height = self.layout_panels(ui, me, s);
        } else {
            ScrollBar::set_value(ui, self.scroll_bar, 0);
            ui.set_visible(self.scroll_bar, false);
        }
    }

    pub fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId) {
        ScrollBar::set_value(ui, self.scroll_bar, 0);
        ui.invalidate_layout(me);
    }

    fn column_info(ui: &mut Ui, list: PanelId, section: i32) -> Vec<Column> {
        with_sectioned(ui, list, |l, _| l.columns(section).to_vec()).unwrap_or_default()
    }
}

impl Control for SectionedListPanel {
    fn class(&self) -> &'static str {
        "SectionedListPanel"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        SectionedListPanel::on_message(self, ui, me, msg, s)
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, _s: &mut dyn Surface) {
        SectionedListPanel::apply_scheme(self, ui, me, scheme);
    }
    fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &Block, s: &mut dyn Surface) {
        SectionedListPanel::apply_settings(self, ui, me, kv, s);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        SectionedListPanel::perform_layout(self, ui, me, s);
    }
    fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId, _w: i32, _h: i32) {
        SectionedListPanel::on_size_changed(self, ui, me);
    }
    fn sectioned(&mut self) -> Option<&mut SectionedListPanel> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------

/// `CSectionHeader`: the column titles in DefaultVerySmall and a divider line.
#[derive(Debug)]
struct SectionHeader {
    list: PanelId,
    section: i32,
    divider: Color,
    font: FontHandle,
    cells: Vec<Cell>,
    columns: Vec<Column>,
}

impl Control for SectionHeader {
    fn class(&self) -> &'static str {
        "CSectionHeader"
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let fg = scheme.color("SectionTextColor", Color::WHITE);
        self.divider = scheme.color("SectionDividerColor", Color::WHITE);
        let bg = ui.node(me).map_or(Color::TRANSPARENT, |n| n.bg);
        let bg = scheme.color("BuddyListBgColor", bg);
        if let Some(n) = ui.node_mut(me) {
            n.fg = fg;
            n.bg = bg;
        }
        let (prop, hd) = (ui.is_proportional(me), ui.is_hd_proportional(me));
        self.font = s.font(&scheme.file, "DefaultVerySmall", prop, hd);
        // Label::ApplySchemeSettings clears the non-text images: rebuilt at the next layout.
        self.cells.clear();
        ui.invalidate_layout(me);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.columns = SectionedListPanel::column_info(ui, self.list, self.section);
        let n = self.columns.len();
        let fg = ui.node(me).map_or(Color::WHITE, |nd| nd.fg);
        if n != self.cells.len() {
            self.cells = self
                .columns
                .iter()
                .map(|c| {
                    let img = (c.flags & HEADER_IMAGE == 0).then(|| TextImage::with_font(self.font, fg));
                    Cell { img, xpos: -1, width: -1 }
                })
                .collect();
        }
        for _ in 0..2 {
            let mut xpos = 0;
            for i in 0..n {
                let (flags, max_w) = (self.columns[i].flags, self.columns[i].width);
                let Some(mut img) = self.cells[i].img.clone() else {
                    xpos += max_w;
                    continue;
                };
                let (cw, _) = img.content_size(s);
                let mut ext_w = max_w;
                if flags & COLUMN_RIGHT == 0 {
                    for j in i + 1..n {
                        let iw = self.cells[j].img.as_mut().map_or(0, |t| t.content_size(s).0);
                        if iw == 0 {
                            ext_w += self.columns[j].width;
                        }
                    }
                }
                let w = if ext_w >= 0 { ext_w } else { cw };
                let (x, width) = if flags & COLUMN_CENTER != 0 {
                    (xpos + w / 2 - cw / 2, w - cw - 2)
                } else if flags & COLUMN_RIGHT != 0 {
                    (xpos + w - cw, cw - 2)
                } else {
                    (xpos, w - 2)
                };
                self.cells[i].xpos = x;
                self.cells[i].width = width;
                xpos += max_w;
                if flags & HEADER_IMAGE == 0 {
                    img.set_text_raw(&self.columns[i].text);
                    resize_max(&mut img, ext_w, s);
                    self.cells[i].img = Some(img);
                }
            }
        }
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        paint_cells(&mut self.cells, ui, me, s);
        let (w, t) = ui.size(me);
        s.draw_set_color(self.divider);
        s.draw_filled_rect(1, t - 2, w - 2, t - 1);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------

/// `CItemButton`: one row.
#[derive(Debug)]
pub struct ItemButton {
    list: PanelId,
    pub id: i32,
    pub section: i32,
    pub data: Option<Msg>,
    selected: bool,
    override_colors: bool,
    cells: Vec<Cell>,
    font: FontHandle,
    fg2: Color,
    armed_fg1: Color,
    armed_fg2: Color,
    armed_bg: Color,
    selection_bg2: Color,
}

impl ItemButton {
    fn new(list: PanelId, id: i32) -> Self {
        Self {
            list,
            id,
            section: -1,
            data: None,
            selected: false,
            override_colors: false,
            cells: Vec::new(),
            font: FontHandle(0),
            fg2: Color::TRANSPARENT,
            armed_fg1: Color::TRANSPARENT,
            armed_fg2: Color::TRANSPARENT,
            armed_bg: Color::TRANSPARENT,
            selection_bg2: Color::TRANSPARENT,
        }
    }

    /// Selected with the focus anywhere inside the list.
    fn active(&self, ui: &Ui, me: PanelId) -> bool {
        self.selected && (ui.focus() == Some(me) || ui.focus().is_some_and(|f| ui.parent(me).is_some_and(|p| ui.has_parent(f, p))))
    }
}

impl Control for ItemButton {
    fn class(&self) -> &'static str {
        "CItemButton"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        let list = self.list;
        if msg.is("MousePressed") {
            let code = msg.get_int("code");
            if code == MOUSE_LEFT {
                ui.post_action_signal(list, Msg::new("ItemLeftClick").int("itemID", self.id));
            } else if code == MOUSE_RIGHT {
                ui.post_action_signal(list, Msg::new("ItemContextMenu").int("itemID", self.id).with("SubPanel", Value::Panel(me)));
            }
            with_sectioned(ui, list, |l, ui| l.set_selected(ui, list, Some(me)));
            return true;
        }
        if msg.is("MouseDoublePressed") {
            let code = msg.get_int("code");
            if code == MOUSE_LEFT {
                ui.post_action_signal(list, Msg::new("ItemDoubleLeftClick").int("itemID", self.id));
                ui.send(list, &Msg::new("KeyCodeTyped").int("code", KEY_ENTER), Some(me), s);
            } else {
                return self.on_message(ui, me, &Msg::new("MousePressed").int("code", code), None, s);
            }
            with_sectioned(ui, list, |l, ui| l.set_selected(ui, list, Some(me)));
            return true;
        }
        if msg.is("SetFocus") || msg.is("KillFocus") {
            ui.invalidate_layout(me);
            return false;
        }
        false
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let (prop, hd) = (ui.is_proportional(me), ui.is_hd_proportional(me));
        self.font = s.font(&scheme.file, "Default", prop, hd);
        self.armed_fg1 = scheme.color("BuddyButton/ArmedFgColor1", Color::WHITE);
        self.armed_fg2 = scheme.color("BuddyButton/ArmedFgColor2", Color::WHITE);
        self.armed_bg = scheme.color("BuddyButton/ArmedBgColor", Color::WHITE);
        self.fg2 = scheme.color("BuddyButton/FgColor2", Color::WHITE);
        self.selection_bg2 = scheme.color("SelectionBG2", Color::WHITE);
        self.cells.clear();
        ui.invalidate_layout(me);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let columns = SectionedListPanel::column_info(ui, self.list, self.section);
        let n = columns.len();
        let Some(data) = self.data.clone().filter(|_| n >= 1) else {
            self.cells = vec![Cell { img: Some(TextImage::with_font(self.font, ui.node(me).map_or(Color::WHITE, |nd| nd.fg))), xpos: -1, width: -1 }];
            if let Some(t) = self.cells[0].img.as_mut() {
                t.set_text_raw("< unset >");
                let (cw, ch) = t.content_size(s);
                t.set_size(cw, ch);
            }
            return;
        };
        if n != self.cells.len() {
            self.cells = columns
                .iter()
                .map(|c| Cell { img: (c.flags & COLUMN_IMAGE == 0).then(|| TextImage::with_font(self.font, Color::TRANSPARENT)), xpos: -1, width: -1 })
                .collect();
        }
        let active = self.active(ui, me);
        let fg = ui.node(me).map_or(Color::WHITE, |nd| nd.fg);
        let mut xpos = 0;
        for (i, c) in columns.iter().enumerate() {
            let (mut cw, mut ch) = (0, 0);
            if c.flags & COLUMN_IMAGE == 0
                && let Some(img) = self.cells[i].img.as_mut()
            {
                img.set_text(ui, &data.get_str(&c.name));
                resize_max(img, c.width, s);
                img.color = if self.override_colors {
                    if active { self.armed_fg2 } else { fg }
                } else if active {
                    self.armed_fg2
                } else if c.flags & COLUMN_BRIGHT != 0 {
                    self.armed_fg1
                } else {
                    self.fg2
                };
                (cw, ch) = img.content_size(s);
            }
            let _ = ch;
            let w = if c.width >= 0 { c.width } else { cw };
            let (x, width) = if i == 0 && c.flags & COLUMN_IMAGE == 0 {
                (xpos + 6, w - 8)
            } else if c.flags & COLUMN_CENTER != 0 {
                let off = w / 2 - cw / 2;
                (xpos + off, w - off - 2)
            } else if c.flags & COLUMN_RIGHT != 0 {
                (xpos + w - cw, w - 2)
            } else {
                (xpos, w - 2)
            };
            self.cells[i].xpos = x;
            self.cells[i].width = width;
            xpos += w;
        }
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let in_edit = with_sectioned(ui, self.list, |l, _| l.is_in_edit_mode()).unwrap_or(false);
        let c = if !self.selected || in_edit {
            ui.node(me).map_or(Color::TRANSPARENT, |n| n.bg)
        } else if self.active(ui, me) {
            self.armed_bg
        } else {
            self.selection_bg2
        };
        let (w, h) = ui.size(me);
        s.draw_set_color(c);
        s.draw_filled_rect(0, 0, w, h);
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        paint_cells(&mut self.cells, ui, me, s);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
