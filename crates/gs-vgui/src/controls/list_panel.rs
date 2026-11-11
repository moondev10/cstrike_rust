//! `ListPanel`, `ColumnButton`, `Dragger` (docs/re/vgui-controls-2.md §6).

use std::any::Any;
use std::cmp::Ordering;
use std::rc::Rc;

use crate::controls::button::ButtonState;
use crate::controls::label::{Align, LabelImage, TextImage};
use crate::keyvalues::KeyValues;
use crate::controls::scroll_bar::ScrollBar;
use crate::keys::*;
use crate::msg::{Msg, Value};
use crate::scheme::Scheme;
use crate::surface::{Color, Cursor, FontHandle, Surface, Texture};
use crate::ui::{Control, PanelId, Ui};

pub const COLUMN_FIXEDSIZE: i32 = 0x01;
pub const COLUMN_RESIZEWITHWINDOW: i32 = 0x02;
pub const COLUMN_IMAGE: i32 = 0x04;
pub const COLUMN_HIDDEN: i32 = 0x08;
pub const COLUMN_UNHIDABLE: i32 = 0x10;

/// One row as a custom sort function sees it (`ListPanelItem`: the KeyValues and `userData`).
#[derive(Debug, Clone, Copy)]
pub struct ListItem<'a> {
    pub kv: &'a Msg,
    pub user_data: u32,
}

/// The comparison behind [`ListSortFunc`].
pub type SortFn = dyn Fn(&ListItem<'_>, &ListItem<'_>) -> i32;

/// A custom sort function (`SortFunc(list, item1, item2)`): `<0`, `0`, `>0` like `DefaultSortFunc`.
#[derive(Clone)]
pub struct ListSortFunc(pub Rc<SortFn>);

impl std::fmt::Debug for ListSortFunc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ListSortFunc")
    }
}

#[derive(Debug, Clone)]
struct Column {
    header: PanelId,
    resizer: PanelId,
    min_width: i32,
    max_width: i32,
    resizes_with_window: bool,
    type_is_text: bool,
    hidden: bool,
    unhidable: bool,
    sort: Option<ListSortFunc>,
}

#[derive(Debug, Clone)]
struct Item {
    kv: Msg,
    user_data: u32,
    visible: bool,
    /// Allocation order (the heap-address tie break of `FastSortFunc`).
    serial: u64,
    primary: i32,
    secondary: i32,
}

/// `vgui2::ListPanel`.
#[derive(Debug)]
pub struct ListPanel {
    columns: Vec<Column>,
    items: Vec<Option<Item>>,
    /// Linked-list order of live IDs.
    order: Vec<i32>,
    free_ids: Vec<i32>,
    next_serial: u64,
    visible: Vec<i32>,
    needs_sort: bool,
    sort_column: i32,
    sort_column2: i32,
    ascending: bool,
    vbar: PanelId,
    header_height: i32,
    row_height: i32,
    selected: Vec<i32>,
    last_selected: i32,
    dragger_moved: i32,
    last_bar_width: i32,
    table_start: (i32, i32),
    label_fg: Color,
    selection_fg: Color,
    armed_bg: Color,
    selection_bg2: Color,
    font: FontHandle,
    empty_text: TextImage,
    allow_user_columns: bool,
    select_cells: bool,
    selected_column: i32,
    image_list: Vec<Option<Texture>>,
}

/// Runs `f` on the list panel `id`.
pub fn with_list<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut ListPanel, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.list_panel().map(|l| f(l, ui))).flatten()
}

fn cvtt(f: f32) -> i32 {
    if (-2_147_483_648.0..2_147_483_648.0).contains(&f) { f as i32 } else { i32::MIN }
}

impl ListPanel {
    /// The state of a new list; `me` is its panel.
    pub fn new(ui: &mut Ui, me: PanelId) -> Self {
        // m_hbar: created and hidden for good.
        let hbar = ScrollBar::create(ui, Some(me), "HorizScrollBar", false);
        ui.add_action_signal_target(hbar, me);
        ui.set_visible(hbar, false);
        let vbar = ScrollBar::create(ui, Some(me), "VertScrollBar", true);
        ui.set_visible(vbar, false);
        ui.add_action_signal_target(vbar, me);
        let s20 = ui.psv(me, 20);
        Self {
            columns: Vec::new(),
            items: Vec::new(),
            order: Vec::new(),
            free_ids: Vec::new(),
            next_serial: 0,
            visible: Vec::new(),
            needs_sort: false,
            sort_column: -1,
            sort_column2: -1,
            ascending: true,
            vbar,
            header_height: s20,
            row_height: s20,
            selected: Vec::new(),
            last_selected: -1,
            dragger_moved: -1,
            last_bar_width: 0,
            table_start: (0, 0),
            label_fg: Color::TRANSPARENT,
            selection_fg: Color::TRANSPARENT,
            armed_bg: Color::WHITE,
            selection_bg2: Color::WHITE,
            font: FontHandle(0),
            empty_text: TextImage::default(),
            allow_user_columns: false,
            select_cells: false,
            selected_column: -1,
            image_list: Vec::new(),
        }
    }

    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str) -> PanelId {
        let me = ui.create(parent, name, Box::new(crate::controls::misc::Divider));
        let st = Self::new(ui, me);
        ui.replace_ctrl(me, |_| Box::new(st));
        me
    }

    /// `AddColumnHeader(index, name, text, width, flags)`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_column_header(&mut self, ui: &mut Ui, me: PanelId, index: i32, name: &str, text: &str, width: i32, flags: i32) {
        let (min, max) = if flags & 3 == 1 { (width, width) } else { (ui.psv(me, 20), ui.psv(me, 10000)) };
        self.add_column_header_ex(ui, me, index, name, text, width, min, max, flags);
    }

    /// `AddColumnHeader(index, name, text, width, minWidth, maxWidth, flags)`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_column_header_ex(&mut self, ui: &mut Ui, me: PanelId, index: i32, name: &str, text: &str, width: i32, min: i32, max: i32, flags: i32) {
        let header = ColumnButton::create(ui, me, name, text);
        let h24 = ui.psv(me, 24);
        ui.set_size(header, width, h24);
        ui.add_action_signal_target(header, me);
        let inset = ui.psv(me, 5);
        ui.with_ctrl(header, |c, ui| {
            if let Some(b) = c.as_any_mut().downcast_mut::<ColumnButton>() {
                b.st.label.align = Align::West;
                b.st.label.set_text_inset(ui, header, inset, 0);
            }
        });
        let resizer = ui.create(Some(me), "", Box::new(Dragger { column: index, dragging: false, drag_pos: 0, movable: true }));
        if let Some(n) = ui.node_mut(resizer) {
            n.paint_background_enabled = false;
            n.paint_border_enabled = false;
            n.cursor = Cursor::SizeWe;
        }
        ui.add_action_signal_target(resizer, me);
        ui.move_to_front(resizer);
        if min == max || flags & COLUMN_FIXEDSIZE != 0 {
            Dragger::set_movable(ui, resizer, false);
        }
        let col = Column {
            header,
            resizer,
            min_width: min,
            max_width: max,
            resizes_with_window: flags & COLUMN_RESIZEWITHWINDOW != 0,
            type_is_text: flags & COLUMN_IMAGE == 0,
            hidden: false,
            unhidable: flags & COLUMN_UNHIDABLE != 0,
            sort: None,
        };
        let at = (index.max(0) as usize).min(self.columns.len());
        self.columns.insert(at, col);
        self.reset_column_header_commands(ui);
        ui.move_to_front(self.vbar);
        self.set_column_visible(ui, me, index, flags & COLUMN_HIDDEN == 0);
        ui.invalidate_layout(me);
    }

    /// `ResetColumnHeaderCommands`: every header sorts its column.
    fn reset_column_header_commands(&self, ui: &mut Ui) {
        for (i, c) in self.columns.iter().enumerate() {
            let h = c.header;
            ui.with_ctrl(h, |ctl, _| {
                if let Some(b) = ctl.as_any_mut().downcast_mut::<ColumnButton>() {
                    b.st.command = Some(Msg::new("SetSortColumn").int("column", i as i32));
                }
            });
        }
    }

    /// `SetColumnVisible(col, visible)`.
    pub fn set_column_visible(&mut self, ui: &mut Ui, me: PanelId, col: i32, visible: bool) {
        let Some(c) = usize::try_from(col).ok().and_then(|i| self.columns.get_mut(i)) else { return };
        if c.hidden == visible && !c.unhidable {
            c.hidden = !visible;
            ui.set_visible(c.header, visible);
            ui.set_visible(c.resizer, visible);
            ui.invalidate_layout(me);
        }
    }

    /// `SetSortFunc(col, func)`.
    pub fn set_sort_func(&mut self, col: i32, f: ListSortFunc) {
        if let Some(c) = usize::try_from(col).ok().and_then(|i| self.columns.get_mut(i)) {
            c.sort = Some(f);
        }
    }

    /// `SetSortColumn(column)`.
    pub fn set_sort_column(&mut self, column: i32) {
        self.sort_column = column;
    }

    /// `SetColumnSortable(col, sortable)`.
    pub fn set_column_sortable(&self, ui: &mut Ui, col: i32, sortable: bool) {
        let Some(c) = usize::try_from(col).ok().and_then(|i| self.columns.get(i)) else { return };
        let h = c.header;
        ui.with_ctrl(h, |ctl, _| {
            if let Some(b) = ctl.as_any_mut().downcast_mut::<ColumnButton>() {
                b.st.command = sortable.then(|| Msg::new("SetSortColumn").int("column", col));
            }
        });
    }

    pub fn set_allow_user_modification_of_columns(&mut self, b: bool) {
        self.allow_user_columns = b;
    }

    pub fn set_select_individual_cells(&mut self, b: bool) {
        self.select_cells = b;
    }

    /// `SetImageList`: images by index (0 = none).
    pub fn set_image_list(&mut self, images: Vec<Option<Texture>>) {
        self.image_list = images;
    }

    /// `SetFont(font)`: the cell text font; rows become `fontTall + 2` high.
    pub fn set_font(&mut self, font: FontHandle, s: &dyn Surface) {
        if font == FontHandle(0) {
            return;
        }
        self.font = font;
        self.row_height = s.font_tall(font) + 2;
    }

    /// `SetColumnHeaderText(col, text)`.
    pub fn set_column_header_text(&self, ui: &mut Ui, col: i32, text: &str) {
        let Some(h) = usize::try_from(col).ok().and_then(|i| self.columns.get(i)).map(|c| c.header) else { return };
        ui.with_ctrl(h, |ctl, ui| {
            if let Some(b) = ctl.as_any_mut().downcast_mut::<ColumnButton>() {
                b.st.label.set_text(ui, text);
            }
        });
        ui.invalidate_layout(h);
    }

    /// `SetColumnHeaderImage(col, imageListIndex)`: the header shows the image instead of its
    /// text (`SetTextImageIndex(-1)`, `SetImageAtIndex(0, image, 0)`).
    pub fn set_column_header_image(&self, ui: &mut Ui, col: i32, index: usize) {
        let Some(h) = usize::try_from(col).ok().and_then(|i| self.columns.get(i)).map(|c| c.header) else { return };
        let tex = self.image_list.get(index).copied().flatten();
        ui.with_ctrl(h, |ctl, _| {
            if let Some(b) = ctl.as_any_mut().downcast_mut::<ColumnButton>() {
                b.st.label.clear_text_image();
                let size = tex.map_or((0, 0), |t| (t.width, t.height));
                b.st.label.set_image_at(0, LabelImage::Texture { name: String::new(), tex, size, color: Color::WHITE }, 0);
            }
        });
        ui.invalidate_layout(h);
    }

    /// `SetColumnHeaderTooltip(col, text)`: the header's tooltip, shown at once on one line.
    pub fn set_column_header_tooltip(&self, ui: &mut Ui, col: i32, text: &str) {
        let Some(h) = usize::try_from(col).ok().and_then(|i| self.columns.get(i)).map(|c| c.header) else { return };
        let text = crate::controls::label::localized(ui, text);
        ui.set_tooltip_text(h, &text);
        ui.set_tooltip_one_line(h);
        ui.set_tooltip_delay(h, 0);
    }

    /// `GetUserConfigSettings`: `"%d_hidden"` per current column.
    pub fn get_user_config_settings(&self, data: &mut KeyValues) {
        for (i, c) in self.columns.iter().enumerate() {
            data.set_int(&format!("{i}_hidden"), i32::from(c.hidden));
        }
    }

    /// `ApplyUserConfigSettings`: `"%d_hidden"` 0 shows, 1 hides, anything else leaves it.
    pub fn apply_user_config_settings(&mut self, ui: &mut Ui, me: PanelId, data: &KeyValues) {
        for i in 0..self.columns.len() as i32 {
            match data.get_int(&format!("{i}_hidden"), -1) {
                0 => self.set_column_visible(ui, me, i, true),
                1 => self.set_column_visible(ui, me, i, false),
                _ => {}
            }
        }
    }

    /// `GetItemCount`'s columns: `GetNumColumnHeaders`.
    pub fn column_count(&self) -> i32 {
        self.columns.len() as i32
    }

    /// `SetEmptyListText(text)` (`#tokens` localized).
    pub fn set_empty_list_text(&mut self, ui: &Ui, text: &str) {
        self.empty_text.set_text(ui, text);
    }

    /// `AddItem(data, userData, scrollToItem, sortOnAdd)`.
    pub fn add_item(&mut self, ui: &mut Ui, me: PanelId, data: &Msg, user_data: u32, scroll_to: bool, sort_on_add: bool) -> i32 {
        self.next_serial += 1;
        let item = Item { kv: data.clone(), user_data, visible: true, serial: self.next_serial, primary: 0, secondary: 0 };
        let id = match self.free_ids.first().copied() {
            Some(i) => {
                self.free_ids.remove(0);
                self.items[i as usize] = Some(item);
                i
            }
            None => {
                self.items.push(Some(item));
                (self.items.len() - 1) as i32
            }
        };
        self.order.push(id);
        let row = self.visible.len() as i32;
        self.visible.push(id);
        if sort_on_add {
            self.needs_sort = true;
        }
        ui.invalidate_layout(me);
        if scroll_to {
            ScrollBar::set_value(ui, self.vbar, row);
        }
        id
    }

    pub fn item_count(&self) -> i32 {
        self.visible.len() as i32
    }

    pub fn item(&self, id: i32) -> Option<&Msg> {
        self.items.get(usize::try_from(id).ok()?)?.as_ref().map(|i| &i.kv)
    }

    pub fn item_mut(&mut self, id: i32) -> Option<&mut Msg> {
        self.items.get_mut(usize::try_from(id).ok()?)?.as_mut().map(|i| &mut i.kv)
    }

    pub fn item_user_data(&self, id: i32) -> u32 {
        usize::try_from(id).ok().and_then(|i| self.items.get(i)).and_then(Option::as_ref).map_or(0, |i| i.user_data)
    }

    /// `GetItemIDFromUserData`.
    pub fn item_id_from_user_data(&self, ud: u32) -> i32 {
        self.order.iter().copied().find(|&id| self.item_user_data(id) == ud).unwrap_or(-1)
    }

    pub fn item_id_from_row(&self, row: i32) -> i32 {
        usize::try_from(row).ok().and_then(|r| self.visible.get(r)).copied().unwrap_or(-1)
    }

    pub fn item_current_row(&self, id: i32) -> i32 {
        self.visible.iter().position(|&v| v == id).map_or(-1, |p| p as i32)
    }

    pub fn is_valid_item_id(&self, id: i32) -> bool {
        usize::try_from(id).ok().and_then(|i| self.items.get(i)).is_some_and(Option::is_some)
    }

    /// `ApplyItemChanges(id)` (no re-sort flag).
    pub fn apply_item_changes(&mut self, ui: &mut Ui, me: PanelId, _id: i32) {
        ui.invalidate_layout(me);
    }

    /// `RemoveItem(id)`.
    pub fn remove_item(&mut self, ui: &mut Ui, me: PanelId, id: i32) {
        if !self.is_valid_item_id(id) {
            return;
        }
        self.selected.retain(|&s| s != id);
        self.visible.retain(|&v| v != id);
        self.order.retain(|&o| o != id);
        self.items[id as usize] = None;
        self.free_ids.insert(0, id);
        ui.invalidate_layout(me);
    }

    /// `DeleteAllItems`: IDs restart at 0.
    pub fn delete_all_items(&mut self, ui: &mut Ui, me: PanelId) {
        let total = self.items.len();
        self.items.iter_mut().for_each(|i| *i = None);
        self.free_ids = (0..total as i32).collect();
        self.order.clear();
        self.visible.clear();
        self.selected.clear();
        ui.invalidate_layout(me);
    }

    /// `SetItemVisible(id, state)`.
    pub fn set_item_visible(&mut self, ui: &mut Ui, me: PanelId, id: i32, state: bool) {
        let Some(it) = usize::try_from(id).ok().and_then(|i| self.items.get_mut(i)).and_then(Option::as_mut) else { return };
        if it.visible == state {
            return;
        }
        self.needs_sort = true;
        it.visible = state;
        if state {
            self.visible.push(id);
            return;
        }
        self.selected.retain(|&s| s != id);
        self.visible.retain(|&v| v != id);
        ui.invalidate_layout(me);
    }

    pub fn selected_items_count(&self) -> i32 {
        self.selected.len() as i32
    }

    pub fn selected_item(&self, index: i32) -> i32 {
        usize::try_from(index).ok().and_then(|i| self.selected.get(i)).copied().unwrap_or(-1)
    }

    /// `ClearSelectedItems` (no message).
    pub fn clear_selected_items(&mut self) {
        self.selected.clear();
    }

    /// `AddSelectedItem(id)`: `ItemSelected` every time.
    pub fn add_selected_item(&mut self, ui: &mut Ui, me: PanelId, id: i32) {
        if self.is_valid_item_id(id) {
            self.selected.push(id);
            ui.post_action_signal(me, Msg::new("ItemSelected"));
        }
    }

    /// `SetSingleSelectedItem(id)`.
    pub fn set_single_selected_item(&mut self, ui: &mut Ui, me: PanelId, id: i32) {
        self.clear_selected_items();
        self.add_selected_item(ui, me, id);
    }

    /// `SetSelectedCell(id, column)`.
    fn set_selected_cell(&mut self, ui: &mut Ui, me: PanelId, id: i32, column: i32) {
        if self.is_valid_item_id(id) && (0..self.columns.len() as i32).contains(&column) {
            self.selected_column = column;
            self.set_single_selected_item(ui, me, id);
            ui.post_action_signal(me, Msg::new("ItemSelected"));
        }
    }

    fn column_name(&self, ui: &Ui, col: usize) -> String {
        self.columns.get(col).and_then(|c| ui.node(c.header)).map(|n| n.name.clone()).unwrap_or_default()
    }

    /// `GetRowsPerPage`.
    fn rows_per_page(&self, ui: &Ui, me: PanelId) -> f32 {
        (ui.size(me).1 - self.header_height) as f32 / self.row_height as f32
    }

    /// `GetStartItem`.
    fn start_item(&self, ui: &mut Ui, me: PanelId) -> i32 {
        if self.visible.len() as f32 > self.rows_per_page(ui, me) { ScrollBar::value(ui, self.vbar) } else { 0 }
    }

    /// `DefaultSortFunc` (or the column's own) on the column `name`.
    fn compare(col: &Column, a: &ListItem<'_>, b: &ListItem<'_>, name: &str) -> i32 {
        if let Some(f) = &col.sort {
            return (f.0)(a, b);
        }
        let (a, b) = (a.kv, b.kv);
        if !col.type_is_text {
            return 0;
        }
        match (a.get(name), b.get(name)) {
            (Some(Value::Int(x)), _) => match x.cmp(&b.get_int(name)) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            },
            _ => {
                let (x, y) = (a.get_str(name).to_ascii_lowercase(), b.get_str(name).to_ascii_lowercase());
                match x.cmp(&y) {
                    Ordering::Less => -1,
                    Ordering::Equal => 0,
                    Ordering::Greater => 1,
                }
            }
        }
    }

    /// The SortList ranks of one column: in-order of the RB tree (descending by the sort
    /// function), equal keys sharing a rank, ranks from 2.
    fn assign_ranks(&mut self, ui: &Ui, col: i32, secondary: bool) {
        let Some(c) = usize::try_from(col).ok().and_then(|i| self.columns.get(i)).cloned() else { return };
        let name = self.column_name(ui, col as usize);
        let mut keyed: Vec<(i32, Msg, u32)> = self.order.iter().filter_map(|&id| self.items[id as usize].as_ref().filter(|i| i.visible).map(|i| (id, i.kv.clone(), i.user_data))).collect();
        fn item(k: &(i32, Msg, u32)) -> ListItem<'_> {
            ListItem { kv: &k.1, user_data: k.2 }
        }
        keyed.sort_by(|x, y| Self::compare(&c, &item(y), &item(x), &name).cmp(&0));
        let mut idx = 1;
        let mut prev: Option<&(i32, Msg, u32)> = None;
        let mut ranks = Vec::with_capacity(keyed.len());
        for k in &keyed {
            if prev.is_none_or(|p| Self::compare(&c, &item(p), &item(k), &name) != 0) {
                idx += 1;
            }
            ranks.push((k.0, idx));
            prev = Some(k);
        }
        for (id, r) in ranks {
            if let Some(it) = self.items[id as usize].as_mut() {
                if secondary {
                    it.secondary = r;
                } else {
                    it.primary = r;
                }
            }
        }
    }

    /// `SortList`.
    pub fn sort_list(&mut self, ui: &mut Ui, me: PanelId) {
        self.needs_sort = false;
        if self.visible.len() <= 1 {
            return;
        }
        let start = self.start_item(ui, me);
        let rows = cvtt(self.rows_per_page(ui, me));
        let mut keep = -1;
        if self.last_selected != -1 && !self.selected.is_empty() {
            let r = self.item_current_row(self.last_selected);
            if r >= start && r <= start + rows {
                keep = r - start;
            }
        }
        let n = self.columns.len() as i32;
        if (0..n).contains(&self.sort_column) {
            self.assign_ranks(ui, self.sort_column, false);
        }
        if (0..n).contains(&self.sort_column2) {
            self.assign_ranks(ui, self.sort_column2, true);
        }
        // FastSortFunc never returns 0 (primary rank, secondary rank, then the item's address:
        // later allocations first), so AscendingSortFunc is `ascending ? r : -r` and the
        // secondary direction flag is never consulted.
        let asc = self.ascending;
        let rank = |x: i32, y: i32| match x.cmp(&y) {
            Ordering::Less => 1,
            Ordering::Greater => -1,
            Ordering::Equal => 0,
        };
        let items = &self.items;
        self.visible.sort_by(|&ia, &ib| {
            let (Some(a), Some(b)) = (items[ia as usize].as_ref(), items[ib as usize].as_ref()) else { return Ordering::Equal };
            let mut r = rank(a.primary, b.primary);
            if r == 0 {
                r = rank(a.secondary, b.secondary);
            }
            if r == 0 {
                r = if a.serial < b.serial { 1 } else { -1 };
            }
            if !asc {
                r = -r;
            }
            r.cmp(&0)
        });
        if keep != -1 {
            let r = self.item_current_row(self.last_selected);
            ScrollBar::set_value(ui, self.vbar, if r > keep { r - keep } else { 0 });
        }
        ui.invalidate_layout(me);
    }

    /// `OnSetSortColumn(column)` (a header click).
    fn on_set_sort_column(&mut self, ui: &mut Ui, me: PanelId, column: i32) {
        if self.sort_column == column {
            self.ascending = !self.ascending;
        } else {
            self.sort_column2 = self.sort_column;
        }
        self.set_sort_column(column);
        self.sort_list(ui, me);
    }

    /// `GetCellAtPos(x, y)` (screen coordinates).
    fn cell_at_pos(&self, ui: &mut Ui, me: PanelId, x: i32, y: i32) -> Option<(i32, i32)> {
        let (ax, ay) = ui.abs_pos(me);
        let (x, y) = (x - ax - self.table_start.0, y - ay - self.table_start.1);
        let start = self.start_item(ui, me);
        if x < 0 || y < 0 {
            return None;
        }
        let n = self.visible.len() as i32;
        let mut row = start;
        while row < n && y >= (row - start + 1) * self.row_height {
            row += 1;
        }
        let mut column = 0;
        let mut cx = 0;
        for c in &self.columns {
            cx += ui.size(c.header).0;
            if x < cx {
                break;
            }
            column += 1;
        }
        if row == n || column == self.columns.len() as i32 { None } else { Some((row, column)) }
    }

    /// `OnMousePressed(code)`.
    fn mouse_pressed(&mut self, ui: &mut Ui, me: PanelId, code: i32) {
        if code != MOUSE_LEFT && code != MOUSE_RIGHT {
            return;
        }
        let (x, y) = ui.cursor_pos();
        let (row, _) = self.cell_at_pos(ui, me, x, y).unwrap_or((-1, -1));
        let n = self.visible.len() as i32;
        let mut skip = false;
        if self.last_selected < 0 && n > 0 && row < n {
            self.last_selected = 0;
        } else if self.last_selected == -1 {
            skip = true;
        }
        if !skip {
            let id = if (0..n).contains(&row) { self.visible[row as usize] } else { -1 };
            let keep = code == MOUSE_RIGHT && self.selected.contains(&id);
            if !keep && (0..n).contains(&row) {
                let shift = ui.is_key_down(KEY_LSHIFT) || ui.is_key_down(KEY_RSHIFT);
                let ctrl = ui.is_key_down(KEY_LCONTROL) || ui.is_key_down(KEY_RCONTROL);
                if !shift && !ctrl {
                    self.last_selected = id;
                    self.set_single_selected_item(ui, me, id);
                } else if !shift {
                    if self.selected.contains(&id) {
                        self.selected.retain(|&s| s != id);
                    } else {
                        self.add_selected_item(ui, me, id);
                    }
                    self.last_selected = id;
                } else {
                    let last = self.item_current_row(self.last_selected).max(0);
                    let (lo, hi) = (row.min(last), row.max(last));
                    if !ctrl {
                        self.clear_selected_items();
                    }
                    for r in lo..=hi {
                        let v = self.visible[r as usize];
                        if !self.selected.contains(&v) {
                            self.add_selected_item(ui, me, v);
                        }
                    }
                }
                if self.selected.len() == 1 && self.select_cells {
                    let id0 = self.selected[0];
                    self.set_selected_cell(ui, me, id0, self.selected_column);
                }
            }
        }
        ui.request_focus(me);
        if code == MOUSE_RIGHT {
            let first = self.selected.first().copied().unwrap_or(-1);
            ui.post_action_signal(me, Msg::new("OpenContextMenu").int("itemID", first));
        }
    }

    /// `OnKeyCodeTyped(code)`.
    fn key_code_typed(&mut self, ui: &mut Ui, me: PanelId, code: i32, s: &mut dyn Surface) {
        let n = self.visible.len() as i32;
        if n == 0 {
            ui.panel_on_key_code_typed(me, code, s);
            return;
        }
        let start = self.start_item(ui, me);
        let rows = cvtt(self.rows_per_page(ui, me));
        let mut row = if self.is_valid_item_id(self.last_selected) { self.item_current_row(self.last_selected) } else { 0 };
        match code {
            KEY_HOME => row = 0,
            KEY_END => row = n - 1,
            KEY_PAGEUP => row = if row > start { start } else { row + 1 - rows },
            KEY_PAGEDOWN => row = if row < start - 1 + rows { start - 1 + rows } else { row - 1 + rows },
            KEY_UP => row -= 1,
            KEY_DOWN => row += 1,
            _ => {
                ui.panel_on_key_code_typed(me, code, s);
                return;
            }
        }
        let row = row.clamp(0, n - 1);
        self.last_selected = self.visible[row as usize];
        let id = self.last_selected;
        self.set_single_selected_item(ui, me, id);
        if n as f32 > rows as f32 {
            let v = ScrollBar::value(ui, self.vbar);
            if row < v {
                ScrollBar::set_value(ui, self.vbar, row);
            } else if row >= v + rows {
                ScrollBar::set_value(ui, self.vbar, row - rows + 1);
            }
        }
        ui.invalidate_layout(me);
    }

    /// `OnColumnResized(column, delta)`.
    fn column_resized(&mut self, ui: &mut Ui, me: PanelId, column: i32, delta: i32) {
        self.dragger_moved = column;
        let Some(c) = usize::try_from(column).ok().and_then(|i| self.columns.get(i)).cloned() else { return };
        let (w, t) = ui.size(c.header);
        let mut w = (w + delta).max(c.min_width).min(c.max_width);
        let (pw, _) = ui.size(me);
        let (hx, _) = ui.pos(c.header);
        let rest: i32 = self.columns.iter().skip(column as usize + 1).map(|c| c.min_width).sum();
        w = w.min(pw - (ui.psv(me, 20) + rest + hx));
        ui.set_size(c.header, w, t);
        ui.invalidate_layout(me);
    }

    pub fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let border = scheme.border("ButtonDepressedBorder");
        let bg = scheme.color("ListBgColor", scheme.color("WindowDisabledBgColor", Color::WHITE));
        if let Some(n) = ui.node_mut(me) {
            n.border = border;
            n.bg = bg;
        }
        self.armed_bg = scheme.color("Menu/ArmedBgColor", Color::WHITE);
        self.selection_bg2 = scheme.color("SelectionBG2", Color::WHITE);
        self.label_fg = scheme.color("WindowFgColor", Color::WHITE);
        self.selection_fg = scheme.color("ListSelectionFgColor", self.label_fg);
        self.empty_text.color = scheme.color("LabelDimText", Color::WHITE);
        let (prop, hd) = (ui.is_proportional(me), ui.is_hd_proportional(me));
        self.font = s.font(&scheme.file, "Default", prop, hd);
        if self.font != FontHandle(0) {
            self.row_height = s.font_tall(self.font) + 2;
        }
        self.empty_text.font = self.font;
    }

    /// `PerformLayout`.
    pub fn perform_layout(&mut self, ui: &mut Ui, me: PanelId) {
        if self.needs_sort {
            self.sort_list(ui, me);
        }
        let rows = cvtt(self.rows_per_page(ui, me));
        let n = self.visible.len() as i32;
        let vb = self.vbar;
        ui.set_visible(vb, true);
        ui.set_enabled(vb, false);
        ScrollBar::set_range_window(ui, vb, rows);
        ScrollBar::set_range(ui, vb, 0, n);
        ScrollBar::set_button_pressed_scroll_value(ui, vb, 1);
        let (wide, tall) = ui.size(me);
        let (s20, s18, s2, s4, s8) = (ui.psv(me, 20), ui.psv(me, 18), ui.psv(me, 2), ui.psv(me, 4), ui.psv(me, 8));
        ui.set_pos(vb, wide - s20, 0);
        ui.set_size(vb, s18, tall - s2);
        ui.invalidate_layout(vb);
        let max_x = wide - s20;
        let nc = self.columns.len();
        let num_to_resize = if self.dragger_moved != -1 { 1 } else { self.columns.iter().filter(|c| c.resizes_with_window && !c.hidden).count() as i32 };
        let mut last_vis = nc as i32 - 1;
        let mut last_x = 0;
        for i in (0..nc).rev() {
            if !self.columns[i].hidden {
                last_x = ui.pos(self.columns[i].header).0;
                last_vis = i as i32;
                break;
            }
        }
        let mut force = false;
        let change = if num_to_resize == 0 {
            let total_min: i32 = self.columns.iter().filter(|c| !c.hidden).map(|c| c.min_width).sum();
            if total_min > max_x {
                force = true;
                cvtt((max_x - total_min) as f32 / nc.max(1) as f32)
            } else {
                0
            }
        } else if last_x != 0 {
            cvtt((max_x - self.last_bar_width) as f32 / num_to_resize as f32)
        } else {
            let sum: i32 = self.columns.iter().filter(|c| !c.hidden).map(|c| ui.size(c.header).0).sum();
            cvtt((max_x - sum) as f32 / num_to_resize as f32)
        };
        self.last_bar_width = max_x;
        for _pass in 0..=nc {
            let mut x = -1;
            let mut i = 0;
            while i < nc {
                let c = self.columns[i].clone();
                let h = c.header;
                if c.hidden {
                    ui.set_visible(h, false);
                    i += 1;
                    continue;
                }
                ui.set_pos(h, x, 0);
                ui.set_visible(h, true);
                if c.min_width + x >= max_x && !force {
                    break;
                }
                let mut w = ui.size(h).0;
                if i as i32 == last_vis {
                    w = max_x - x;
                } else if i as i32 == self.dragger_moved || (self.dragger_moved == -1 && (c.resizes_with_window || force)) {
                    w += change;
                }
                w = if w >= c.min_width || force { w.min(c.max_width) } else { c.min_width };
                ui.set_size(h, w, s18);
                x += w;
                ui.set_visible(c.resizer, i as i32 != last_vis);
                ui.move_to_front(c.resizer);
                ui.set_pos(c.resizer, x - s4, 0);
                ui.set_size(c.resizer, s8, s18);
                i += 1;
            }
            if i == nc {
                break;
            }
            let mut total: i32 = self.columns.iter().filter(|c| !c.hidden).map(|c| ui.size(c.header).0).sum();
            let mut j = nc as i32 - 1;
            while j >= 0 {
                let c = &self.columns[j as usize];
                if !c.hidden {
                    total -= ui.size(c.header).0;
                    if c.min_width + total > max_x {
                        total += c.min_width;
                        ui.set_size(c.header, c.min_width, s18);
                    } else {
                        ui.set_size(c.header, max_x - total, s18);
                        break;
                    }
                }
                j -= 1;
            }
            if j == -1 {
                break;
            }
        }
        self.dragger_moved = -1;
    }

    /// `Paint`: the shared cell renderer per visible cell, then the empty-list text.
    pub fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if self.needs_sort {
            self.sort_list(ui, me);
        }
        let (wide, tall) = ui.size(me);
        self.table_start = (0, self.header_height + ui.psv(me, 1));
        let n = self.visible.len() as i32;
        let rows = cvtt(self.rows_per_page(ui, me));
        let start = if n as f32 >= rows as f32 { ScrollBar::value(ui, self.vbar) } else { 0 };
        let focus = ui.focus() == Some(me) || ui.focus().is_some_and(|f| ui.parent(me).is_some_and(|p| ui.has_parent(f, p)));
        let (s2, s3, s5) = (ui.psv(me, 2), ui.psv(me, 3), ui.psv(me, 5));
        let mut drawn = 0;
        let mut i = start;
        'rows: while i < n {
            if i < 0 {
                i += 1;
                continue;
            }
            let id = self.visible[i as usize];
            let mut xoff = 0;
            for j in 0..self.columns.len() {
                let c = self.columns[j].clone();
                if !ui.is_visible(c.header) {
                    continue;
                }
                let hw = ui.size(c.header).0;
                let (cx, cy) = (xoff + self.table_start.0 + s2, self.table_start.1 + drawn * self.row_height);
                let ch = self.row_height - 1;
                // GetClipRect against the list: a row cut short stops the paint.
                let visible_h = (cy + ch).min(tall) - cy.max(0);
                if visible_h < self.row_height - s3 {
                    break 'rows;
                }
                let selected = self.selected.contains(&id) && (!self.select_cells || j as i32 == self.selected_column);
                if selected {
                    s.draw_set_color(if focus { self.armed_bg } else { self.selection_bg2 });
                    s.draw_filled_rect(cx, cy, cx + hw, cy + ch);
                }
                let name = self.column_name(ui, j);
                if c.type_is_text {
                    let text: String = self.item(id).map(|kv| match kv.get(&name) {
                        Some(Value::Int(v)) => v.to_string(),
                        Some(Value::Float(v)) => format!("{:.6}", f64::from(*v)),
                        _ => kv.get_str(&name),
                    }).unwrap_or_default().chars().take(255).collect();
                    let mut t = TextImage::with_font(self.font, if selected { self.selection_fg } else { self.label_fg });
                    t.set_text_raw(&text);
                    let (_, th) = t.content_size(s);
                    t.set_size(hw - s5, th);
                    t.pos = (cx + 3, cy + (ch - th) / 2);
                    t.paint(s);
                } else {
                    let idx = self.item(id).map_or(0, |kv| kv.get_int(&name));
                    if idx > 0
                        && let Some(Some(tex)) = self.image_list.get(idx as usize)
                    {
                        let (iw, ih) = (ui.psv(me, tex.width), ui.psv(me, tex.height));
                        s.draw_set_color(Color::WHITE);
                        let y = cy + (ch - ih) / 2;
                        s.draw_textured_rect(*tex, cx, y, cx + iw, y + ih);
                    }
                }
                xoff += hw;
            }
            drawn += 1;
            i += 1;
        }
        if n < 1 {
            let (s8, s4) = (ui.psv(me, 8), ui.psv(me, 4));
            self.empty_text.pos = (self.table_start.0 + s8, self.table_start.1 + s4);
            let rh = self.row_height;
            self.empty_text.set_size(wide - s8, rh);
            self.empty_text.paint(s);
        }
    }

    /// The list's messages.
    pub fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) -> bool {
        if msg.is("ScrollBarSliderMoved") {
            ui.invalidate_layout(me);
        } else if msg.is("ColumnResized") {
            self.column_resized(ui, me, msg.get_int("column"), msg.get_int("delta"));
        } else if msg.is("SetSortColumn") {
            self.on_set_sort_column(ui, me, msg.get_int("column"));
        } else if msg.is("ResizeColumnToContents") {
            let column = msg.get_int("column");
            let Some(c) = usize::try_from(column).ok().and_then(|i| self.columns.get(i)).cloned() else { return true };
            if !c.type_is_text {
                return true;
            }
            let name = self.column_name(ui, column as usize);
            let mut min_w = ui.with_ctrl(c.header, |ctl, ui| ctl.as_any_mut().downcast_mut::<ColumnButton>().map(|b| b.st.label.content_size(ui, c.header, s).0)).flatten().unwrap_or(0);
            for &id in &self.visible {
                let text = self.item(id).map(|kv| kv.get_str(&name)).unwrap_or_default();
                let mut t = TextImage::with_font(self.font, Color::WHITE);
                t.set_text_raw(&text.chars().take(255).collect::<String>());
                let (cw, _) = t.content_size(s);
                if cw > min_w {
                    min_w = cw + 2;
                }
            }
            let cur = ui.size(c.header).0;
            self.column_resized(ui, me, column, min_w - cur);
        } else if msg.is("OpenColumnChoiceMenu") {
            // GameUI never allows user column changes; the menu needs m_bAllowUserAddDeleteColumns.
        } else if msg.is("ToggleColumnVisible") {
            let col = msg.get_int("col");
            if let Some(c) = usize::try_from(col).ok().and_then(|i| self.columns.get(i)) {
                let vis = c.hidden;
                self.set_column_visible(ui, me, col, vis);
            }
        } else if msg.is("MousePressed") {
            self.mouse_pressed(ui, me, msg.get_int("code"));
        } else if msg.is("MouseDoublePressed") {
            if msg.get_int("code") != MOUSE_LEFT {
                return true;
            }
            self.mouse_pressed(ui, me, MOUSE_LEFT);
            if !self.selected.is_empty() {
                ui.panel_on_key_code_typed(me, KEY_ENTER, s);
                ui.post_action_signal(me, Msg::new("ItemDoubleClicked").int("itemID", self.selected[0]));
            }
        } else if msg.is("MouseWheeled") {
            let v = ScrollBar::value(ui, self.vbar);
            ScrollBar::set_value(ui, self.vbar, v - msg.get_int("delta") * 3);
        } else if msg.is("KeyCodeTyped") {
            self.key_code_typed(ui, me, msg.get_int("code"), s);
        } else {
            return false;
        }
        true
    }
}

impl Control for ListPanel {
    fn class(&self) -> &'static str {
        "ListPanel"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        ListPanel::on_message(self, ui, me, msg, s)
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ListPanel::apply_scheme(self, ui, me, scheme, s);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        ListPanel::perform_layout(self, ui, me);
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        ListPanel::paint(self, ui, me, s);
    }
    fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId, _w: i32, _h: i32) {
        ui.invalidate_layout(me);
    }
    fn list_panel(&mut self) -> Option<&mut ListPanel> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------

/// `ColumnButton`: a header (DefaultSmall, a_west, no focus on press).
#[derive(Debug)]
pub struct ColumnButton {
    pub st: ButtonState,
}

impl ColumnButton {
    fn create(ui: &mut Ui, list: PanelId, name: &str, text: &str) -> PanelId {
        let st = ButtonState::new(ui, Some(list), text);
        let me = ui.create(Some(list), name, Box::new(ColumnButton { st }));
        ui.reapply_scheme(me);
        me
    }
}

impl Control for ColumnButton {
    fn class(&self) -> &'static str {
        "ColumnButton"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("MousePressed") {
            if !ui.is_enabled(me) {
                return true;
            }
            let code = msg.get_int("code");
            if code == MOUSE_RIGHT {
                ui.call_parent(me, &Msg::new("OpenColumnChoiceMenu"), s);
                return true;
            }
            if !self.st.click_enabled(code) {
                return true;
            }
            self.st.set_selected(ui, me, true);
            ui.set_mouse_capture(Some(me));
            return true;
        }
        self.st.on_message(ui, me, msg, s)
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.st.apply_scheme(ui, me, scheme, s);
        self.st.label.align = Align::West;
        self.st.label.font_override = Some("DefaultSmall".to_owned());
        let (prop, hd) = (ui.is_proportional(me), ui.is_hd_proportional(me));
        self.st.label.text.font = s.font(&scheme.file, "DefaultSmall", prop, hd);
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

// ---------------------------------------------------------------------------------------------

/// `Dragger`: a column edge (`ColumnResized` per cursor move while dragged).
#[derive(Debug)]
struct Dragger {
    column: i32,
    dragging: bool,
    drag_pos: i32,
    movable: bool,
}

impl Dragger {
    fn set_movable(ui: &mut Ui, me: PanelId, state: bool) {
        if let Some(d) = ui.ctrl::<Dragger>(me) {
            d.movable = state;
        }
        if ui.is_visible(me)
            && let Some(n) = ui.node_mut(me)
        {
            n.cursor = if state { Cursor::SizeWe } else { Cursor::Arrow };
        }
    }
}

impl Control for Dragger {
    fn class(&self) -> &'static str {
        "Dragger"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, _s: &mut dyn Surface) -> bool {
        if msg.is("MousePressed") {
            if self.movable {
                ui.set_mouse_capture(Some(me));
                self.drag_pos = ui.cursor_pos().0;
                self.dragging = true;
            }
        } else if msg.is("MouseReleased") {
            if self.movable {
                ui.set_mouse_capture(None);
                self.dragging = false;
            }
        } else if msg.is("CursorMoved") {
            if self.dragging {
                let x = ui.cursor_pos().0;
                let kv = Msg::new("ColumnResized").int("column", self.column).int("delta", x - self.drag_pos);
                self.drag_pos = x;
                if let Some(p) = ui.parent(me) {
                    ui.post(p, kv, Some(me), 0.0);
                }
            }
        } else if msg.is("MouseDoublePressed") {
            if self.movable
                && let Some(p) = ui.parent(me)
            {
                ui.post(p, Msg::new("ResizeColumnToContents").int("column", self.column), None, 0.0);
            }
        } else {
            return false;
        }
        true
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
