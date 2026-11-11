use std::any::Any;

use crate::border::Border;
use crate::scheme::Scheme;
use crate::surface::{Abc, Color, FontHandle, Surface, Texture};
use crate::ui::{Control, PanelId, Ui};
use crate::Msg;

/// Records filled rectangles; a fixed 8-px, 16-tall font; an in-memory clipboard.
#[derive(Default)]
struct Rec {
    rects: Vec<(Color, [i32; 4])>,
    color: Color,
    clip: String,
}

impl Surface for Rec {
    fn screen_size(&self) -> (i32, i32) {
        (640, 480)
    }
    fn push_make_current(&mut self, _origin: (i32, i32), _clip: [i32; 4]) {}
    fn pop_make_current(&mut self) {}
    fn draw_set_color(&mut self, c: Color) {
        self.color = c;
    }
    fn draw_filled_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        self.rects.push((self.color, [x0, y0, x1, y1]));
    }
    fn draw_outlined_rect(&mut self, _: i32, _: i32, _: i32, _: i32) {}
    fn draw_line(&mut self, _: i32, _: i32, _: i32, _: i32) {}
    fn draw_set_text_font(&mut self, _: FontHandle) {}
    fn draw_set_text_color(&mut self, _: Color) {}
    fn draw_set_text_pos(&mut self, _: i32, _: i32) {}
    fn draw_get_text_pos(&self) -> (i32, i32) {
        (0, 0)
    }
    fn draw_unicode_char(&mut self, _: char) {}
    fn texture(&mut self, _: &str, _: bool) -> Option<Texture> {
        None
    }
    fn draw_textured_rect(&mut self, _: Texture, _: i32, _: i32, _: i32, _: i32) {}
    fn font(&mut self, _: &str, _: &str, _: bool, _: bool) -> FontHandle {
        FontHandle(1)
    }
    fn font_tall(&self, f: FontHandle) -> i32 {
        if f == FontHandle(0) { 0 } else { 16 }
    }
    fn char_abc(&self, f: FontHandle, _: char) -> Abc {
        if f == FontHandle(0) { Abc::default() } else { Abc { a: 0, b: 8, c: 0 } }
    }
    fn clipboard_text(&mut self) -> String {
        self.clip.clone()
    }
    fn set_clipboard_text(&mut self, text: &str) {
        self.clip = text.to_owned();
    }
}

const SCHEME: &str = r#"
"Scheme" {
    "BaseSettings" {
        "FgColor" "ControlText"
        "Menu" { "BgColor" "ControlBG" }
    }
    "Colors" {
        "ControlText" "216 222 211 255"
        "ControlBG" "76 88 68 255"
        "BorderBright" "136 145 128 255"
        "BorderDark" "40 46 34 255"
    }
    "Borders" {
        "RaisedBorder" {
            "inset" "0 0 1 1"
            "Left" { "1" { "color" "BorderBright" "offset" "0 1" } }
            "Right" { "1" { "color" "BorderDark" "offset" "0 0" } }
            "Top" { "1" { "color" "BorderBright" "offset" "0 1" } }
            "Bottom" { "1" { "color" "BorderDark" "offset" "0 0" } }
        }
        "MenuBorder" "RaisedBorder"
    }
}
"#;

#[test]
fn scheme_colors_resolve_names_paths_and_literals() {
    let s = Scheme::parse("resource/TrackerScheme.res", "default", SCHEME);
    assert_eq!(s.color("FgColor", Color::WHITE), Color::rgba(216, 222, 211, 255));
    assert_eq!(s.color("Menu/BgColor", Color::WHITE), Color::rgba(76, 88, 68, 255));
    assert_eq!(s.color("1 2 3 4", Color::WHITE), Color::rgba(1, 2, 3, 4));
    assert_eq!(s.color("Missing", Color::rgba(9, 9, 9, 9)), Color::rgba(9, 9, 9, 9));
}

#[test]
fn raised_border_alias_paints_the_menu_frame_rects() {
    let s = Scheme::parse("resource/TrackerScheme.res", "default", SCHEME);
    let b: std::sync::Arc<Border> = s.border("MenuBorder").unwrap_or_default();
    let mut r = Rec::default();
    b.paint_panel(&mut r, 100, 50);
    let bright = Color::rgba(136, 145, 128, 255);
    let dark = Color::rgba(40, 46, 34, 255);
    assert_eq!(r.rects, vec![(bright, [0, 0, 1, 49]), (bright, [0, 0, 99, 1]), (dark, [99, 0, 100, 50]), (dark, [0, 49, 100, 50])]);
    assert_eq!(b.inset, [0, 0, 1, 1]);
}

/// Logs deliveries; on "A" it posts "A2" to another panel.
struct Logger {
    log: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    forward: Option<PanelId>,
}

impl Control for Logger {
    fn class(&self) -> &'static str {
        "Logger"
    }
    fn on_message(&mut self, ui: &mut Ui, _me: PanelId, msg: &Msg, _from: Option<PanelId>, _s: &mut dyn Surface) -> bool {
        self.log.borrow_mut().push(msg.name.clone());
        if msg.is("A")
            && let Some(f) = self.forward
        {
            ui.post(f, Msg::new("A2"), None, 0.0);
        }
        true
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[test]
fn messages_posted_during_dispatch_run_before_the_rest_of_the_queue() {
    let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut ui = Ui::new();
    let root = ui.embedded();
    let b = ui.create(Some(root), "b", Box::new(Logger { log: log.clone(), forward: None }));
    let a = ui.create(Some(root), "a", Box::new(Logger { log: log.clone(), forward: Some(b) }));
    ui.post(a, Msg::new("A"), None, 0.0);
    ui.post(b, Msg::new("B"), None, 0.0);
    ui.run_frame(&mut Rec::default());
    let names: Vec<String> = log.borrow().iter().filter(|n| matches!(n.as_str(), "A" | "A2" | "B")).cloned().collect();
    assert_eq!(names, ["A", "A2", "B"]);
}

fn count(log: &std::rc::Rc<std::cell::RefCell<Vec<String>>>, name: &str) -> usize {
    log.borrow().iter().filter(|n| *n == name).count()
}

#[test]
fn combo_box_posts_text_changed_only_on_change_and_typeahead_wraps() {
    use crate::controls::combo_box::ComboBox;
    let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut ui = Ui::new();
    let root = ui.embedded();
    let owner = ui.create(Some(root), "owner", Box::new(Logger { log: log.clone(), forward: None }));
    let cb = ComboBox::create(&mut ui, Some(owner), "cb", 5, false);
    ui.add_action_signal_target(cb, owner);
    for t in ["alpha", "beta", "gamma"] {
        ComboBox::add_item(&mut ui, cb, t, None);
    }
    let mut s = Rec::default();
    ComboBox::activate_item(&mut ui, cb, 1, &mut s);
    ui.run_frame(&mut s);
    assert_eq!(ComboBox::text(&mut ui, cb), "beta");
    ComboBox::activate_item(&mut ui, cb, 1, &mut s);
    ui.run_frame(&mut s);
    assert_eq!(count(&log, "TextChanged"), 1);
    // Typeahead from the active row (1) wraps past the end to "alpha".
    ui.post(cb, Msg::new("KeyTyped").int("unichar", 'a' as i32), None, 0.0);
    ui.run_frame(&mut s);
    assert_eq!(ComboBox::text(&mut ui, cb), "alpha");
    assert_eq!(count(&log, "TextChanged"), 2);
}

#[test]
fn checking_a_radio_button_unchecks_its_group_siblings() {
    use crate::controls::toggle::{ToggleButton, ToggleKind, with_toggle};
    let mut ui = Ui::new();
    let root = ui.embedded();
    let r1 = ToggleButton::create(&mut ui, Some(root), "r1", "One", ToggleKind::Radio);
    let r2 = ToggleButton::create(&mut ui, Some(root), "r2", "Two", ToggleKind::Radio);
    let mut s = Rec::default();
    with_toggle(&mut ui, r1, |t, ui| t.set_selected(ui, r1, true, &mut s));
    ui.run_frame(&mut s);
    with_toggle(&mut ui, r2, |t, ui| t.set_selected(ui, r2, true, &mut s));
    ui.run_frame(&mut s);
    let sel = |ui: &mut Ui, id| with_toggle(ui, id, |t, _| t.is_selected()).unwrap_or(false);
    assert!(!sel(&mut ui, r1));
    assert!(sel(&mut ui, r2));
}

/// Counts the PropertyPage notifications.
struct PageLog(std::rc::Rc<std::cell::RefCell<Vec<String>>>, &'static str);

impl crate::controls::property::PageHooks for PageLog {
    fn on_reset_data(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        self.0.borrow_mut().push(format!("{}:reset", self.1));
    }
    fn on_apply_changes(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        self.0.borrow_mut().push(format!("{}:apply", self.1));
    }
    fn on_page_show(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        self.0.borrow_mut().push(format!("{}:show", self.1));
    }
    fn on_page_hide(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        self.0.borrow_mut().push(format!("{}:hide", self.1));
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

struct NoDialogHooks;

impl crate::controls::property::DialogHooks for NoDialogHooks {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[test]
fn property_dialog_resets_shows_first_page_and_applies_all_on_ok() {
    use crate::controls::property::{PropertyDialog, PropertyPage, with_property_dialog, with_sheet};
    let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut ui = Ui::new();
    let root = ui.embedded();
    let dlg = PropertyDialog::create(&mut ui, Some(root), "dlg", Box::new(NoDialogHooks));
    ui.set_size(dlg, 400, 300);
    let a = PropertyPage::create(&mut ui, None, "a", true, Box::new(PageLog(log.clone(), "a")));
    let b = PropertyPage::create(&mut ui, None, "b", true, Box::new(PageLog(log.clone(), "b")));
    PropertyDialog::add_page(&mut ui, dlg, a, "A");
    PropertyDialog::add_page(&mut ui, dlg, b, "B");
    let mut s = Rec::default();
    with_property_dialog(&mut ui, dlg, |d, ui| d.activate(ui, dlg));
    ui.run_frame(&mut s);
    ui.run_frame(&mut s);
    assert_eq!(*log.borrow(), ["a:reset", "b:reset", "a:show"]);
    assert!(ui.is_visible(a) && !ui.is_visible(b));
    log.borrow_mut().clear();
    let sheet = with_property_dialog(&mut ui, dlg, |d, _| d.parts.sheet).unwrap_or(root);
    with_sheet(&mut ui, sheet, |sh, ui| sh.change_active_tab(ui, sheet, 1));
    ui.run_frame(&mut s);
    assert_eq!(*log.borrow(), ["a:hide", "b:show"]);
    log.borrow_mut().clear();
    ui.post(dlg, Msg::new("Command").str("command", "OK"), None, 0.0);
    ui.run_frame(&mut s);
    assert_eq!(*log.borrow(), ["a:apply", "b:apply"]);
    assert!(!ui.is_visible(dlg));
}

#[test]
fn list_panel_sorts_by_header_and_toggles_direction_with_newest_first_ties() {
    use crate::controls::list_panel::{ListPanel, with_list};
    let mut ui = Ui::new();
    let root = ui.embedded();
    let list = ListPanel::create(&mut ui, Some(root), "list");
    ui.set_size(list, 300, 200);
    let names = ["b", "a", "c", "a"];
    let ids: Vec<i32> = with_list(&mut ui, list, |l, ui| {
        l.add_column_header(ui, list, 0, "Name", "Name", 100, 0);
        names.iter().map(|n| l.add_item(ui, list, &Msg::new("item").str("Name", n), 0, false, false)).collect()
    })
    .unwrap_or_default();
    let rows = |ui: &mut Ui| with_list(ui, list, |l, _| (0..l.item_count()).map(|r| l.item_id_from_row(r)).collect::<Vec<_>>()).unwrap_or_default();
    let mut s = Rec::default();
    ui.post(list, Msg::new("SetSortColumn").int("column", 0), None, 0.0);
    ui.run_frame(&mut s);
    // Ascending; the two "a" rows newest first.
    assert_eq!(rows(&mut ui), [ids[3], ids[1], ids[0], ids[2]]);
    ui.post(list, Msg::new("SetSortColumn").int("column", 0), None, 0.0);
    ui.run_frame(&mut s);
    assert_eq!(rows(&mut ui), [ids[2], ids[0], ids[1], ids[3]]);
}

/// A click on the title bar's close button closes the frame: the button stays above the caption
/// grip and posts `CloseFrameButtonPressed`, which Frame turns into `OnCommand("Close")`.
#[test]
fn close_button_click_closes_the_frame() {
    use crate::controls::frame::{Frame, FrameState};
    use crate::keys::MOUSE_LEFT;
    let mut ui = Ui::new();
    let root = ui.embedded();
    ui.set_bounds(root, 0, 0, 640, 480);
    let f = Frame::create(&mut ui, Some(root), "f");
    ui.set_bounds(f, 100, 100, 300, 200);
    FrameState::activate(&mut ui, f);
    let mut s = Rec::default();
    ui.run_frame(&mut s);
    // Close button at (w - 5 - 20, 8), 18×18: its centre in screen space.
    ui.cursor_moved(100 + 300 - 25 + 9, 100 + 8 + 9);
    ui.run_frame(&mut s);
    ui.mouse_pressed(MOUSE_LEFT, false);
    ui.mouse_released(MOUSE_LEFT);
    ui.run_frame(&mut s);
    assert!(!ui.is_visible(f));
}

/// Fills its panel with one colour.
struct Fill(Color);

impl Control for Fill {
    fn class(&self) -> &'static str {
        "Fill"
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let (w, h) = ui.size(me);
        s.draw_set_color(self.0);
        s.draw_filled_rect(0, 0, w, h);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A popup listed before its popup parent (the console's completion list after
/// `PositionCompletionList`) is still painted: the root pass walks the parent's subtree
/// without painting it and flags the nested popup.
#[test]
fn nested_popup_listed_before_its_parent_is_painted() {
    let mut ui = Ui::new();
    let root = ui.embedded();
    ui.set_bounds(root, 0, 0, 640, 480);
    let red = Color::rgba(255, 0, 0, 255);
    let parent = ui.create(Some(root), "parent", Box::new(Fill(Color::rgba(0, 0, 255, 255))));
    let list = ui.create(Some(parent), "list", Box::new(Fill(red)));
    ui.make_popup(list);
    ui.make_popup(parent);
    ui.set_bounds(parent, 10, 10, 100, 100);
    ui.set_bounds(list, 10, 120, 50, 50);
    let mut s = Rec::default();
    ui.run_frame(&mut s);
    ui.paint(&mut s);
    assert!(s.rects.iter().any(|&(c, _)| c == red), "rects: {:?}", s.rects);
}
