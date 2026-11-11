//! `CTaskbar`, `CBackgroundMenuButton`, the game menu and its highlight labels
//! (docs/re/gameui.md §3, §4).

use std::any::Any;

use gs_vgui::controls::button::ButtonState;
use gs_vgui::controls::label::{Align, LabelImage, LabelState};
use gs_vgui::controls::menu::{Menu, make_game_menu, make_game_menu_item, restart_reveal};
use gs_vgui::controls::message_box::MessageBox;
use gs_vgui::scheme::Scheme;
use gs_vgui::{Color, Control, Msg, PanelId, Surface, Ui};

use crate::modules::Modules;
use crate::{Fs, HostRequest, Shared};

/// `CTaskbar::AddTask` / `CenterDialog`: centre in the workspace.
pub fn center(ui: &mut Ui, id: PanelId) {
    let (w, h) = ui.screen;
    let (pw, ph) = ui.size(id);
    ui.set_pos(id, (w - pw) / 2, (h - ph) / 2);
}

/// The flags of a `GameMenu.res` item (its user data).
fn item_flags(kv: &gs_formats::keyvalues::Block) -> Msg {
    let mut m = Msg::new("GameMenuItem");
    for key in ["OnlyInGame", "notsingle", "notmulti", "notsteam", "OnlyInCareerGame", "NotInCareerGame"] {
        m = m.int(key, gs_vgui::editable::kv_int(kv, key, 0));
    }
    m
}

/// `CTaskbar`.
pub struct Taskbar {
    host: Shared,
    fs: Fs,
    game_button: PanelId,
    platform_button: PanelId,
    pub game_menu: Option<PanelId>,
    inset: i32,
    last_in_game: bool,
    last_in_game_change: f32,
    /// The dialog PHandles: `+0x170` options, `+0x174` create server, `+0x178` demo player,
    /// `+0x180` player list.
    options: Option<PanelId>,
    create_server: Option<PanelId>,
    demo_player: Option<PanelId>,
    player_list: Option<PanelId>,
    /// `g_VModuleLoader` (`OpenServerBrowser` activates the "Servers" module).
    modules: Modules,
}

impl std::fmt::Debug for Taskbar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CTaskbar").field("game_menu", &self.game_menu).finish_non_exhaustive()
    }
}

impl Taskbar {
    /// `new CTaskbar(staticPanel, "TaskBar")` with `CreateGameMenu`.
    pub fn create(ui: &mut Ui, host: Shared, fs: Fs, modules: Modules, parent: PanelId, game_menu_res: Option<&str>) -> (PanelId, Option<PanelId>) {
        let placeholder = ui.embedded();
        let me = ui.create(
            Some(parent),
            "TaskBar",
            Box::new(Taskbar {
                host: host.clone(),
                fs,
                game_button: placeholder,
                platform_button: placeholder,
                game_menu: None,
                inset: 0,
                last_in_game: false,
                last_in_game_change: -1.0,
                options: None,
                create_server: None,
                demo_player: None,
                player_list: None,
                modules,
            }),
        );
        ui.make_editable(me);
        ui.set_hd_proportional(me, true);
        ui.set_proportional(me, true);
        let inset = ui.psv(me, 24);
        let game_button = BackgroundMenuButton::create(ui, me, "GameMenuButton", "OpenGameMenu");
        ui.set_enabled(game_button, false);
        let platform_button = BackgroundMenuButton::create(ui, me, "PlatformMenuButton", "OpenPlatformMenu");
        ui.set_enabled(platform_button, false);
        ui.set_visible(platform_button, false);
        let game_menu = game_menu_res.and_then(|text| Self::create_game_menu(ui, me, text));
        if let Some(t) = ui.ctrl::<Taskbar>(me) {
            t.inset = inset;
            t.game_button = game_button;
            t.platform_button = platform_button;
            t.game_menu = game_menu;
        }
        (me, game_menu)
    }

    /// `CreateGameMenu`: one item per `GameMenu.res` block, file order.
    fn create_game_menu(ui: &mut Ui, me: PanelId, text: &str) -> Option<PanelId> {
        let root = gs_formats::keyvalues::parse(text);
        let (name, kv) = root.blocks().next()?;
        let menu = Menu::create(ui, Some(me), name);
        make_game_menu(ui, menu);
        for (_, item) in kv.blocks() {
            let label = gs_vgui::editable::kv_str(item, "label").unwrap_or("<unknown>").to_owned();
            let cmd = gs_vgui::editable::kv_str(item, "command").unwrap_or("").to_owned();
            let name = gs_vgui::editable::kv_str(item, "name").map_or_else(|| label.clone(), str::to_owned);
            let id = Menu::add_item(ui, menu, &name, &label, &cmd, Some(me), Some(item_flags(item)));
            let Some(it) = Menu::item(ui, menu, id) else { continue };
            let hl = Highlight::create(ui, menu, &name, &label);
            make_game_menu_item(ui, it, hl);
        }
        Some(menu)
    }

    /// `CTaskbar::RunFrame`: lay out (and update the menus) every frame.
    pub fn run_frame(ui: &mut Ui, me: PanelId) {
        ui.invalidate_layout(me);
    }

    /// `UpdateGameMenus`.
    fn update_game_menus(&mut self, ui: &mut Ui, me: PanelId) {
        let Some(menu) = self.game_menu else { return };
        let (_, sh) = ui.screen;
        let (in_game, multi, now, steam) = {
            let h = self.host.borrow();
            let in_game = !h.level_name.is_empty();
            (in_game, in_game && h.max_clients > 1, h.time as f32, h.steam)
        };
        if in_game != self.last_in_game {
            self.last_in_game = in_game;
            self.last_in_game_change = now;
        }
        let recent = (now - self.last_in_game_change) < 0.25;
        // CGameMenu::UpdateMenuItemState.
        if recent {
            restart_reveal(ui, menu);
        }
        for child in ui.children(menu) {
            let Some(flags) = ui.ctrl::<gs_vgui::controls::menu::MenuItem>(child).and_then(|it| it.user_data.clone()) else { continue };
            let f = |k: &str| flags.get_int(k) != 0;
            let hidden = (f("OnlyInGame") && !in_game) || (in_game && f("OnlyInCareerGame")) || (multi && f("notmulti")) || (in_game && !multi && f("notsingle")) || (steam && f("notsteam"));
            let vis = !recent && !hidden;
            ui.set_visible(child, vis);
        }
        ui.invalidate_layout(menu);
        let tall = ui.size(menu).1;
        let strip = ui.psv(me, 44);
        ui.set_pos(menu, self.inset, sh - tall - strip);
        Menu::set_visible(ui, menu, true);
    }

    /// The open-dialog pattern: a dead handle builds the dialog (centred), then `Activate()`.
    fn open_dialog(ui: &mut Ui, handle: &mut Option<PanelId>, s: &mut dyn Surface, build: impl FnOnce(&mut Ui, &mut dyn Surface) -> PanelId) {
        let d = match handle.filter(|&d| ui.exists(d)) {
            Some(d) => d,
            None => {
                let d = build(ui, s);
                center(ui, d);
                *handle = Some(d);
                d
            }
        };
        ui.send(d, &Msg::new("Activate"), None, s);
    }

    /// `CBasePanel::ActivateDemoUI`: the demo player bar, built and centred while its handle is
    /// empty, then `Activate()`.
    fn activate_demo_ui(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let (host, fs) = (self.host.clone(), self.fs.clone());
        Self::open_dialog(ui, &mut self.demo_player, s, |ui, s| crate::demo::create_demo_player_dialog(ui, me, &host, &fs, s));
    }

    /// `CTaskbar::OnCommand`.
    fn on_command(&mut self, ui: &mut Ui, me: PanelId, cmd: &str, s: &mut dyn Surface) {
        let lower = cmd.to_ascii_lowercase();
        let (host, fs) = (self.host.clone(), self.fs.clone());
        match lower.as_str() {
            "openoptionsdialog" => {
                Self::open_dialog(ui, &mut self.options, s, |ui, s| crate::options::create_options_dialog(ui, me, &host, &fs, s));
                return;
            }
            "opencreatemultiplayergamedialog" => {
                Self::open_dialog(ui, &mut self.create_server, s, |ui, s| crate::createserver::create_server_dialog(ui, me, &host, &fs, s));
                return;
            }
            "openplayerlistdialog" => {
                Self::open_dialog(ui, &mut self.player_list, s, |ui, s| crate::playerlist::create_player_list_dialog(ui, me, &host, &fs, s));
                return;
            }
            // CBasePanel::ActivateDemoUI (also from viewdemo through IGameUI::ActivateDemoUI).
            "openloaddemodialog" => {
                self.activate_demo_ui(ui, me, s);
                return;
            }
            "openserverbrowser" => {
                self.modules.borrow_mut().activate_module(ui, "Servers", s);
                return;
            }
            _ => {}
        }
        let mut host = self.host.borrow_mut();
        match lower.as_str() {
            "opengamemenu" => {
                if let Some(m) = self.game_menu {
                    ui.post(m, Msg::new("Command").str("command", "Open"), Some(me), 0.0);
                }
            }
            "quit" => {
                drop(host);
                let qb = MessageBox::create_query(ui, "#GameUI_QuitConfirmationTitle", "#GameUI_QuitConfirmationText", Some(me));
                MessageBox::set_ok_button_text(ui, qb, "#GameUI_Quit");
                MessageBox::set_commands(ui, qb, Some(Msg::new("Command").str("command", "QuitNoConfirm")), Some(Msg::new("Command").str("command", "ReleaseModalWindow")));
                ui.add_action_signal_target(qb, me);
                gs_vgui::controls::frame::with_frame(ui, qb, |f, ui| f.do_modal(ui, qb));
                ui.restrict_paint_to(Some(qb));
            }
            "quitnoconfirm" => {
                host.requests.push(HostRequest::ClientCmd("quit\n".to_owned()));
                ui.set_visible(me, false);
                ui.restrict_paint_to(Some(me));
            }
            "resumegame" => host.requests.push(HostRequest::HideGameUi),
            "disconnect" => host.requests.push(HostRequest::ClientCmd("disconnect\n".to_owned())),
            "releasemodalwindow" => ui.restrict_paint_to(None),
            _ => {
                if let Some(i) = cmd.find("engine ") {
                    let rest = &cmd[i + "engine ".len()..];
                    if !rest.is_empty() {
                        host.requests.push(HostRequest::ClientCmd(rest.to_owned()));
                    }
                    host.requests.push(HostRequest::HideGameUi);
                }
            }
        }
    }
}

impl Control for Taskbar {
    fn class(&self) -> &'static str {
        "CTaskbar"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("Command") {
            let cmd = msg.get_str("command");
            self.on_command(ui, me, &cmd, s);
            return true;
        }
        if msg.is("ActivateDemoUI") {
            self.activate_demo_ui(ui, me, s);
            return true;
        }
        if msg.is("DemoPlayerSignal") {
            if let Some(d) = self.demo_player.filter(|&d| ui.exists(d)) {
                ui.send(d, msg, Some(me), s);
            }
            return true;
        }
        if msg.is("ActivateGameUI") {
            self.host.borrow_mut().requests.push(HostRequest::ActivateGameUi);
            return true;
        }
        false
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        if let Some(n) = ui.node_mut(me) {
            n.border = None;
        }
        let (sw, _) = ui.screen;
        self.inset = ui.psv(me, gs_vgui::msg::atoi(scheme.resource_string("InGameDesktop/GameMenuInset")));
        if sw <= 1024 {
            self.inset = (self.inset as f32 * 0.7) as i32;
        }
        for (b, img, over) in [(self.platform_button, "resource/steam_menu", "resource/steam_menu_mouseover"), (self.game_button, "resource/game_menu", "resource/game_menu_mouseover")] {
            if let Some(bb) = ui.ctrl::<BackgroundMenuButton>(b) {
                bb.image = Some(img.to_owned());
                bb.mouse_over = Some(over.to_owned());
                bb.dirty = true;
            }
        }
        let _ = s;
        let bg = scheme.color("InGameDesktop/WidescreenBarColor", Color::WHITE);
        if let Some(n) = ui.node_mut(me) {
            n.bg = bg;
        }
        ui.invalidate_layout(me);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        let (sw, sh) = ui.screen;
        let strip = ui.psv(me, 44);
        ui.set_pos(me, 0, sh - strip);
        ui.set_size(me, sw, strip);
        let inset = self.inset;
        ui.set_pos(self.game_button, inset, (strip - inset) / 2);
        let (w240, w4) = (ui.psv(me, 240), ui.psv(me, 4));
        ui.set_size(self.game_button, w240, inset + w4);
        let pw = ui.size(self.platform_button).0;
        let y6 = ui.psv(me, 6);
        ui.set_pos(self.platform_button, sw - inset - pw, y6);
        let ph = ui.psv(me, inset + 4);
        ui.set_size(self.platform_button, w240, ph);
        if let Some(b) = ui.ctrl::<BackgroundMenuButton>(self.platform_button) {
            b.st.label.align = Align::East;
        }
        self.update_game_menus(ui, me);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `CBackgroundMenuButton`: an image button (normal / mouse-over image).
#[derive(Debug)]
pub struct BackgroundMenuButton {
    pub st: ButtonState,
    image: Option<String>,
    mouse_over: Option<String>,
    dirty: bool,
    shown_armed: bool,
}

impl BackgroundMenuButton {
    fn create(ui: &mut Ui, parent: PanelId, name: &str, command: &str) -> PanelId {
        let mut st = ButtonState::new(ui, Some(parent), "");
        st.set_command(command);
        ui.create(Some(parent), name, Box::new(BackgroundMenuButton { st, image: None, mouse_over: None, dirty: false, shown_armed: false }))
    }
}

impl Control for BackgroundMenuButton {
    fn class(&self) -> &'static str {
        "CBackgroundMenuButton"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        self.st.on_message(ui, me, msg, s)
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.st.apply_scheme(ui, me, scheme, s);
        if let Some(n) = ui.node_mut(me) {
            n.fg = Color::WHITE;
            n.bg = Color::TRANSPARENT;
            n.border = None;
        }
        self.st.default_fg = Color::WHITE;
        self.st.default_bg = Color::TRANSPARENT;
        self.st.armed_fg = Color::rgba(255, 255, 0, 255);
        self.st.armed_bg = Color::TRANSPARENT;
        self.st.depressed_fg = Color::rgba(255, 255, 0, 255);
        self.st.depressed_bg = Color::TRANSPARENT;
        self.st.label.align = Align::West;
        self.st.set_borders(None, None);
        self.st.label.text_inset = (0, 0);
        self.st.armed_sound = Some("sound/UI/buttonrollover.wav".to_owned());
        self.st.depressed_sound = Some("sound/UI/buttonclick.wav".to_owned());
        self.st.released_sound = Some("sound/UI/buttonclickrelease.wav".to_owned());
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        // SetArmed(b): image 0 = mouse-over image while armed.
        let armed = self.st.is_armed();
        if self.dirty || armed != self.shown_armed {
            self.dirty = false;
            self.shown_armed = armed;
            let name = if armed { self.mouse_over.clone() } else { self.image.clone() };
            if let Some(name) = name {
                let tex = s.texture(&name, name.contains("game_menu"));
                let size = tex.map_or((0, 0), |t| (t.width, t.height));
                self.st.label.set_image_at(0, LabelImage::Texture { name, tex, size, color: Color::WHITE }, 0);
            }
        }
        self.st.perform_layout(ui, me, s);
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.st.paint(ui, me, s);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `CGameMenuItemHighlight`: the additive glow copy of an armed item, fading out over 0.8 s.
#[derive(Debug)]
pub struct Highlight {
    label: LabelState,
    blur_color: Color,
    fade_start: f32,
    fading: bool,
}

impl Highlight {
    fn create(ui: &mut Ui, menu: PanelId, name: &str, text: &str) -> PanelId {
        let mut label = LabelState::new(ui, "GameMenuItemHighlight");
        label.set_text(ui, text);
        let me = ui.create(Some(menu), name, Box::new(Highlight { label, blur_color: Color::WHITE, fade_start: -999.0, fading: true }));
        if let Some(n) = ui.node_mut(me) {
            n.paint_enabled = true;
        }
        ui.set_mouse_input_enabled(me, false);
        ui.set_visible(me, false);
        me
    }
}

impl Control for Highlight {
    fn class(&self) -> &'static str {
        "CGameMenuItemHighlight"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, _s: &mut dyn Surface) -> bool {
        if msg.is("GameItemArmed") {
            let (x, y, w, h) = (msg.get_int("x"), msg.get_int("y"), msg.get_int("w"), msg.get_int("h"));
            let (dx, dy) = (ui.psv(me, 18), ui.psv(me, 16));
            ui.set_size(me, w + dx, h + dy);
            let d8 = ui.psv(me, 8);
            ui.set_pos(me, x, y - d8);
            ui.invalidate_layout(me);
            ui.set_visible(me, true);
            if let Some(n) = ui.node_mut(me) {
                n.fg = self.blur_color;
            }
            self.fading = false;
            return true;
        }
        if msg.is("GameItemDisarmed") {
            ui.invalidate_layout(me);
            self.fading = true;
            self.fade_start = (f64::from(ui.time_ms) / 1000.0) as f32;
            return true;
        }
        false
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ui.set_proportional(me, true);
        self.label.font_override = Some("MenuBlurLarge".to_owned());
        self.label.apply_scheme(ui, me, scheme, s);
        self.blur_color = scheme.color("InGameDesktop/BlurMenuColor", Color::WHITE);
        if let Some(n) = ui.node_mut(me) {
            n.fg = self.blur_color;
            n.bg = Color::TRANSPARENT;
            n.border = None;
        }
        self.label.align = Align::West;
        let inset = ui.psv(me, 4);
        self.label.text_inset = (inset, 0);
        self.fade_start = -999.0;
        self.fading = true;
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.label.perform_layout(ui, me, s);
        if self.fading {
            let t = (f64::from(ui.time_ms) / 1000.0) as f32 - self.fade_start;
            if t > 0.8 {
                ui.set_visible(me, false);
                return;
            }
            let k = 1.0 - t / 0.8;
            let c = self.blur_color.0;
            let faded = Color::rgba((c[0] as f32 * k) as i32 as u8, (c[1] as f32 * k) as i32 as u8, (c[2] as f32 * k) as i32 as u8, (k * 255.0) as i32 as u8);
            if let Some(n) = ui.node_mut(me) {
                n.fg = faded;
            }
            ui.invalidate_layout(me);
        }
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.label.paint(ui, me, s);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
