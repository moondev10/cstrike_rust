use gs_vgui::surface::{Abc, Texture};
use gs_vgui::{Color, FontHandle, Surface, Ui};

use crate::GameUi;

/// Records filled rectangles in absolute coordinates; a fixed font; no textures.
#[derive(Default)]
struct Rec {
    origin: (i32, i32),
    color: Color,
    fills: Vec<(Color, [i32; 4])>,
    clipboard: String,
}

impl Surface for Rec {
    fn screen_size(&self) -> (i32, i32) {
        (1280, 720)
    }
    fn push_make_current(&mut self, origin: (i32, i32), _clip: [i32; 4]) {
        self.origin = origin;
    }
    fn pop_make_current(&mut self) {}
    fn draw_set_color(&mut self, c: Color) {
        self.color = c;
    }
    fn draw_filled_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let (ox, oy) = self.origin;
        self.fills.push((self.color, [x0 + ox, y0 + oy, x1 + ox, y1 + oy]));
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
    fn font_tall(&self, _: FontHandle) -> i32 {
        16
    }
    fn char_abc(&self, _: FontHandle, _: char) -> Abc {
        Abc { a: 0, b: 8, c: 0 }
    }
    fn clipboard_text(&mut self) -> String {
        String::new()
    }
    fn set_clipboard_text(&mut self, text: &str) {
        self.clipboard = text.to_owned();
    }
}

const MENU: &str = r##""GameMenu" {
    "1" { "label" "#GameUI_GameMenu_ResumeGame" "command" "ResumeGame" "OnlyInGame" "1" }
    "9" { "label" "#GameUI_GameMenu_NewGame" "command" "OpenCreateMultiplayerGameDialog" }
}"##;

/// No files.
struct NoFs;

impl crate::GameFs for NoFs {
    fn read(&self, _: &str) -> Option<Vec<u8>> {
        None
    }
    fn exists(&self, _: &str) -> bool {
        false
    }
    fn find(&self, _: &str, _: Option<&str>) -> Vec<(String, bool)> {
        Vec::new()
    }
    fn write_in(&self, _: &str, _: &str, _: &[u8]) -> bool {
        false
    }
    fn add_search_path(&self, _: &str, _: &str) {}
    fn remove(&self, _: &str, _: Option<&str>) {}
    fn read_in(&self, _: &str, _: &str) -> Option<Vec<u8>> {
        None
    }
    fn game_dir(&self) -> String {
        "cstrike".to_owned()
    }
}

fn start(s: &mut Rec) -> GameUi {
    let scheme = gs_vgui::scheme::Scheme::parse("resource/TrackerScheme.res", "BaseUI", "\"Scheme\" { }");
    GameUi::start(Ui::new(), scheme, Some(MENU), None, false, false, std::rc::Rc::new(NoFs), s)
}

/// In a level the background dims the whole screen with 0 0 0 128 (CBasePanel::PaintBackground).
#[test]
fn background_dims_the_level() {
    let mut s = Rec::default();
    let mut g = start(&mut s);
    g.host.borrow_mut().level_name = "maps/de_dust2.bsp".to_owned();
    let _ = g.paint(&mut s);
    let dim = Color::rgba(0, 0, 0, 128);
    assert!(s.fills.iter().any(|&(c, r)| c == dim && r == [0, 0, 1280, 720]), "fills: {:?}", s.fills);
}

/// ESC in a level hides menu and console; GameUI's HideGameUI then asks the engine for
/// `hideconsole` (which clears g_bConsoleShown), so the next ESC brings back the menu alone.
#[test]
fn escape_in_a_level_hides_the_console_for_good() {
    let mut s = Rec::default();
    let mut g = start(&mut s);
    g.host.borrow_mut().level_name = "maps/de_dust2.bsp".to_owned();
    g.toggle_console(&mut s);
    assert!(g.is_console_visible());
    let _ = g.take_requests();
    assert_eq!(g.key_event(true, 27, 96, &mut s), crate::KeyFilter::SwallowedHide(true));
    assert!(!g.is_game_ui_active() && !g.is_console_visible());
    let requests = g.take_requests();
    assert!(requests.contains(&crate::HostRequest::ClientCmd("hideconsole".to_owned())), "{requests:?}");
    // The engine runs it: CBaseUI::HideConsole.
    g.hide_console();
    assert_eq!(g.key_event(true, 27, 96, &mut s), crate::KeyFilter::SwallowedActivate);
    assert!(g.is_game_ui_active() && !g.is_console_visible());
}

/// Without a level ESC cannot hide the menu.
#[test]
fn escape_without_a_level_keeps_the_menu() {
    let mut s = Rec::default();
    let mut g = start(&mut s);
    assert_eq!(g.key_event(true, 27, 96, &mut s), crate::KeyFilter::SwallowedActivate);
    assert!(g.is_game_ui_active());
}

/// Typing on while the completion list is up: `PositionCompletionList` moves the console, not
/// the list, to the front, so the key focus stays in the entry.
#[test]
fn console_entry_keeps_the_focus_under_completions() {
    let mut s = Rec::default();
    let mut g = start(&mut s);
    g.host.borrow_mut().cvar_names = vec!["sv_gravity".to_owned(), "sv_maxspeed".to_owned()];
    g.toggle_console(&mut s);
    let _ = g.paint(&mut s);
    for ch in "sv_".chars() {
        g.ui.key_typed(ch);
        let _ = g.paint(&mut s);
    }
    let focus = g.ui.focus().expect("key focus");
    let text = gs_vgui::controls::text_entry::with_te(&mut g.ui, focus, |te, _| te.text());
    assert_eq!(text.as_deref(), Some("sv_"));
}

/// Tab completes the partial text to the first candidate plus a space, and the next Tabs walk
/// the candidates of the same list (gameui.md §5.4, §5.6).
#[test]
fn console_tab_cycles_completions() {
    let mut s = Rec::default();
    let mut g = start(&mut s);
    g.host.borrow_mut().cvar_names = vec!["sv_gravity".to_owned(), "sv_maxspeed".to_owned()];
    g.toggle_console(&mut s);
    let _ = g.paint(&mut s);
    for ch in "sv_".chars() {
        g.ui.key_typed(ch);
        let _ = g.paint(&mut s);
    }
    let entry_text = |g: &mut GameUi| {
        let focus = g.ui.focus().expect("key focus");
        gs_vgui::controls::text_entry::with_te(&mut g.ui, focus, |te, _| te.text()).unwrap_or_default()
    };
    let mut seen = Vec::new();
    for _ in 0..3 {
        g.ui.key_code_pressed(gs_vgui::keys::KEY_TAB);
        g.ui.key_code_typed(gs_vgui::keys::KEY_TAB);
        g.ui.key_code_released(gs_vgui::keys::KEY_TAB);
        let _ = g.paint(&mut s);
        seen.push(entry_text(&mut g));
    }
    assert_eq!(seen, ["sv_gravity ", "sv_maxspeed ", "sv_gravity "]);
}

/// One key code as the window delivers it: pressed, typed, released.
fn press(g: &mut GameUi, s: &mut Rec, code: i32) {
    g.ui.key_code_pressed(code);
    g.ui.key_code_typed(code);
    g.ui.key_code_released(code);
    let _ = g.paint(s);
}

/// Ctrl+A / Ctrl+C act on the history only while the entry is empty (gameui.md §5.4); with
/// text in the entry, Ctrl+C is the entry's own copy.
#[test]
fn console_copies_the_history_only_with_an_empty_entry() {
    use gs_vgui::keys::{KEY_A, KEY_C, KEY_LCONTROL};
    let mut s = Rec::default();
    let mut g = start(&mut s);
    g.toggle_console(&mut s);
    g.clear();
    g.con_printf("first line\nsecond\n");
    let _ = g.paint(&mut s);
    g.ui.key_code_pressed(KEY_LCONTROL);
    press(&mut g, &mut s, KEY_A);
    press(&mut g, &mut s, KEY_C);
    g.ui.key_code_released(KEY_LCONTROL);
    assert_eq!(s.clipboard, "first line\r\nsecond\r\n");
    for ch in "abc".chars() {
        g.ui.key_typed(ch);
    }
    let _ = g.paint(&mut s);
    g.ui.key_code_pressed(KEY_LCONTROL);
    press(&mut g, &mut s, KEY_A);
    press(&mut g, &mut s, KEY_C);
    g.ui.key_code_released(KEY_LCONTROL);
    assert_eq!(s.clipboard, "abc");
}

/// The console key closes the console, empties the entry and swallows the character it types.
#[test]
fn console_key_closes_and_swallows_its_character() {
    let mut s = Rec::default();
    let mut g = start(&mut s);
    let key = gs_vgui::keys::KEY_BACKQUOTE;
    g.host.borrow_mut().toggle_console_key = key;
    g.toggle_console(&mut s);
    let _ = g.paint(&mut s);
    for ch in "abc".chars() {
        g.ui.key_typed(ch);
    }
    let _ = g.paint(&mut s);
    let entry = g.ui.focus().expect("key focus");
    press(&mut g, &mut s, key);
    g.ui.key_typed('`');
    let _ = g.paint(&mut s);
    let text = gs_vgui::controls::text_entry::with_te(&mut g.ui, entry, |te, _| te.text());
    assert_eq!(text.as_deref(), Some(""));
    assert!(!g.is_console_visible());
}

/// The loading dialog's layouts, sized apart.
struct LoadingFs;

impl crate::GameFs for LoadingFs {
    fn read(&self, p: &str) -> Option<Vec<u8>> {
        let frame = |file: &str, tall: i32, p2_y: i32, p2_visible: i32| {
            format!(
                "\"{file}\" {{ \"LoadingDialog\" {{ \"ControlName\" \"Frame\" \"fieldName\" \"LoadingDialog\" \"xpos\" \"0\" \"ypos\" \"0\" \"wide\" \"380\" \"tall\" \"{tall}\" }}\n\
                 \"Progress2\" {{ \"ControlName\" \"ProgressBar\" \"fieldName\" \"Progress2\" \"xpos\" \"20\" \"ypos\" \"{p2_y}\" \"wide\" \"260\" \"tall\" \"24\" \"visible\" \"{p2_visible}\" }} }}"
            )
        };
        let text = match p {
            "Resource/LoadingDialogNoBanner.res" => frame(p, 100, 114, 0),
            "Resource/LoadingDialog.res" => frame(p, 112, 114, 0),
            "Resource/LoadingDialogDualProgress.res" => frame(p, 176, 132, 1),
            _ => return None,
        };
        Some(text.into_bytes())
    }
    fn exists(&self, p: &str) -> bool {
        self.read(p).is_some()
    }
    fn find(&self, _: &str, _: Option<&str>) -> Vec<(String, bool)> {
        Vec::new()
    }
    fn write_in(&self, _: &str, _: &str, _: &[u8]) -> bool {
        false
    }
    fn add_search_path(&self, _: &str, _: &str) {}
    fn remove(&self, _: &str, _: Option<&str>) {}
    fn read_in(&self, p: &str, _: &str) -> Option<Vec<u8>> {
        self.read(p)
    }
    fn game_dir(&self) -> String {
        "cstrike".to_owned()
    }
}

/// Layouts the dialog loads from its own methods resize it too: an HTTP download switches to
/// LoadingDialog.res (SetupHTTPDownload), the first secondary progress to the dual layout,
/// whose second bar must lie inside the dialog.
#[test]
fn loading_dialog_takes_the_size_of_every_layout_it_loads() {
    let mut s = Rec::default();
    let scheme = gs_vgui::scheme::Scheme::parse("resource/TrackerScheme.res", "BaseUI", "\"Scheme\" { }");
    let mut g = GameUi::start(Ui::new(), scheme, Some(MENU), None, false, false, std::rc::Rc::new(LoadingFs), &mut s);
    g.start_progress_bar("http://127.0.0.1/", 1, &mut s);
    let d = g.loading_dialog().expect("dialog");
    assert_eq!(g.ui.size(d), (380, 112), "SetupHTTPDownload's layout");
    g.continue_progress_bar(1, &mut s);
    g.set_secondary_progress(0.0, &mut s);
    assert_eq!(g.ui.size(d), (380, 176), "the dual-progress layout");
    let bar = g.ui.children(d).into_iter().find(|&c| g.ui.node(c).is_some_and(|n| n.name == "Progress2")).expect("Progress2");
    assert!(g.ui.is_visible(bar) && g.ui.pos(bar).1 + g.ui.size(bar).1 <= g.ui.size(d).1);
}
