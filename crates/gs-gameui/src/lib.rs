//! GameUI.dll and the engine's BaseUI (docs/re/gameui.md, docs/re/gameui-engine.md): the
//! background, the taskbar with the game menu, the console and the dialogs, in a vgui2 tree
//! whose root is the engine's static panel (client viewport z 25, GameUI z 50).

use std::cell::RefCell;
use std::rc::Rc;

use gs_vgui::controls::menu::Menu;
use gs_vgui::scheme::Scheme;
use gs_vgui::{PanelId, Surface, Ui};

pub mod advanced;
pub mod base_panel;
pub mod console;
pub mod createserver;
pub mod cvar_controls;
pub mod demo;
pub mod loading;
pub mod modules;
pub mod options;
pub mod options_content;
pub mod options_keyboard;
pub mod options_video;
pub mod options_voice;
pub mod playerlist;
pub mod serverbrowser;
pub mod taskbar;

#[cfg(test)]
mod tests;

/// GameUI's view of the engine filesystem (`IFileSystem` as GameUI uses it).
pub trait GameFs {
    /// The whole file (`Open(path, "rb", NULL)` + Read).
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    /// `Open(path, "rb", pathID)` + Read: only the search paths with `id` ("GAMECONFIG",
    /// "GAME", "GAMEDOWNLOAD", "GAME_FALLBACK").
    fn read_in(&self, path: &str, id: &str) -> Option<Vec<u8>>;
    /// `FileExists(path)`.
    fn exists(&self, path: &str) -> bool;
    /// `FindFirst`/`FindNext` over a `dir/<wildcard>` pattern in the search paths with `id`
    /// (or all): names and directory flags in search-path order (repeats included).
    fn find(&self, pattern: &str, id: Option<&str>) -> Vec<(String, bool)>;
    /// `Open(path, "wb", pathID)` + Write + Close; `false` when it cannot be opened.
    fn write_in(&self, path: &str, id: &str, data: &[u8]) -> bool;
    /// `Open(path, "wb", "GAMECONFIG")` + Write + Close.
    fn write_config(&self, path: &str, data: &[u8]) -> bool {
        self.write_in(path, "GAMECONFIG", data)
    }
    /// `AddSearchPath(path, pathID)` (relative to the install directory, created if missing).
    fn add_search_path(&self, path: &str, id: &str);
    /// `RemoveFile(path, pathID)`: `id` is "GAMECONFIG", "GAME" or `None`.
    fn remove(&self, path: &str, id: Option<&str>);
    /// `GetGameDirectory`'s last component ("cstrike").
    fn game_dir(&self) -> String;
    /// A text file (`.res`, `.lst`, scripts).
    fn read_text(&self, path: &str) -> Option<String> {
        self.read(path).map(|b| String::from_utf8_lossy(&b).into_owned())
    }
    /// `ILocalize::AddFile`'s `%language%` expansion: the English file, then the game
    /// language's.
    fn language_files(&self, pattern: &str) -> Vec<String> {
        vec![pattern.replace("%language%", "english")]
    }
}

pub type Fs = Rc<dyn GameFs>;

/// The engine state GameUI reads and its requests to the engine.
pub type Shared = Rc<RefCell<HostState>>;

/// What GameUI asks of the engine; the host runs these after the frame.
#[derive(Debug, Clone, PartialEq)]
pub enum HostRequest {
    /// `pfnClientCmd` (a newline is appended).
    ClientCmd(String),
    /// The console key in the console dialog: `baseuifuncs->HideGameUI()` (or paint restricted
    /// to the loading dialog while one exists).
    HideGameUiFromConsole,
    /// `baseuifuncs->HideGameUI()`.
    HideGameUi,
    /// `baseuifuncs->ActivateGameUI()`.
    ActivateGameUi,
    /// `engine->Cvar_Set(name, value)` / `Cvar_SetValue` (formatted by GameUI's side).
    CvarSet { name: String, value: String },
    /// `system()->SetRegistryString(key, value)`.
    SetRegistryString { key: String, value: String },
    /// `engine->Con_Printf(text)`.
    ConPrintf(String),
    /// `IGameClientExports::MutePlayerGameVoice` / `UnmutePlayerGameVoice(index)`.
    MutePlayer { index: i32, mute: bool },
    /// `IVoiceTweak::StartVoiceTweakMode` (`true`) / `EndVoiceTweakMode` (`false`).
    VoiceTweak(bool),
    /// `IVoiceTweak::SetControlFloat(1, v)`: `voice_scale.value` written directly.
    VoiceScale(f32),
}

/// `vmode_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VideoMode {
    pub width: i32,
    pub height: i32,
    pub bpp: i32,
}

/// The engine state GameUI reads (refreshed by the host before each frame) and the outbox.
#[derive(Debug, Default)]
pub struct HostState {
    /// `pfnGetLevelName` ("" while `cls.state < 3`).
    pub level_name: String,
    /// `GetMaxClients`.
    pub max_clients: i32,
    /// `Plat_FloatTime`.
    pub time: f64,
    /// `CheckParm("-steam")`.
    pub steam: bool,
    /// `pfnGetCvarFloat("con_mono")`.
    pub con_mono: f32,
    /// `gameuifuncs->IsConnectedToVACSecureServer()`.
    pub vac_secure: bool,
    /// `GetVGUI2KeyCodeForBind("toggleconsole")`.
    pub toggle_console_key: i32,
    /// Engine command names in list order (autocomplete).
    pub command_names: Vec<String>,
    /// Cvar names in list order (autocomplete).
    pub cvar_names: Vec<String>,
    /// Every cvar: name → (string, value) (`pfnGetCvarString` / `pfnGetCvarFloat`).
    pub cvars: std::collections::HashMap<String, (String, f32)>,
    /// `Key_BindingForKey(0..=255)` ("" when unbound).
    pub bindings: Vec<String>,
    /// `GetVideoModes`.
    pub video_modes: Vec<VideoMode>,
    /// `GetCurrentVideoMode`.
    pub current_mode: VideoMode,
    /// `GetCurrentRenderer`: windowed, hdmodels, addons_folder.
    pub windowed: bool,
    /// `GetCurrentRenderer`'s name: the running backend's settings name (`dx12`, `vulkan`, `gl`).
    pub renderer: String,
    /// The backends the video options offer: settings name and label.
    pub renderers: Vec<(String, String)>,
    pub hd_models: bool,
    pub addons_folder: bool,
    /// `IVoiceTweak` has mixer controls (`g_pMixerControls`).
    pub voice_mixer: bool,
    /// `IVoiceTweak::GetSpeakingVolume`.
    pub voice_speaking_volume: i32,
    /// `voice_scale` and the registry strings GameUI reads (`GetRegistryString`).
    pub registry: std::collections::HashMap<String, String>,
    /// `LocalPlayerInfo_ValueForKey` (the local userinfo).
    pub userinfo: Vec<(String, String)>,
    /// `cl.players[i].userinfo` for players 1..=maxclients (`PlayerInfo_ValueForKey`).
    pub player_infos: Vec<String>,
    /// `IGameClientExports::IsPlayerGameVoiceMuted(i)` for players 1..=maxclients.
    pub muted: Vec<bool>,
    /// `IGameClientExports::GetServerHostName`.
    pub server_host_name: String,
    /// `SteamFriends()->GetPersonaName()` and whether the Steam context exists.
    pub persona_name: String,
    pub steam_available: bool,
    /// Steam's offline mode: `HKCU\Software\Valve\Steam\OfflineAFS != 1` and `…\Offline == 1`.
    pub steam_offline_mode: bool,
    /// `pfnGetAppID()`.
    pub app_id: i32,
    /// `IEngineWrapper::GetViewOrigin` / `GetViewAngles`: `r_refdef` of the last rendered frame.
    pub view_origin: [f32; 3],
    pub view_angles: [f32; 3],
    /// `g_pSystem->GetModule("demoplayer001")`: the engine's DemoPlayer the demo dialogs drive.
    pub demo_player: DemoModule,
    pub requests: Vec<HostRequest>,
}

/// The shared DemoPlayer module (`None` without one).
#[derive(Clone, Default)]
pub struct DemoModule(pub Option<Rc<RefCell<gs_hltv::player::DemoPlayer>>>);

impl std::fmt::Debug for DemoModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "DemoModule(loaded)" } else { "DemoModule(none)" })
    }
}

impl HostState {
    /// `PlayerInfo_ValueForKey(index, key)`: `None` outside 1..=maxclients.
    pub fn player_info_value(&self, index: i32, key: &str) -> Option<String> {
        if index < 1 || index > self.max_clients {
            return None;
        }
        let raw = self.player_infos.get(index as usize - 1).map_or("", String::as_str);
        Some(gs_common::info::InfoString::from_raw(raw, 512).get(key).unwrap_or("").to_owned())
    }

    pub fn is_muted(&self, index: i32) -> bool {
        usize::try_from(index - 1).ok().and_then(|i| self.muted.get(i)).copied().unwrap_or(false)
    }

    pub fn set_muted(&mut self, index: i32, mute: bool) {
        if let Some(m) = usize::try_from(index - 1).ok().and_then(|i| self.muted.get_mut(i)) {
            *m = mute;
        }
        self.requests.push(HostRequest::MutePlayer { index, mute });
    }
}

impl HostState {
    fn cvar(&self, name: &str) -> Option<&(String, f32)> {
        self.cvars.get(name).or_else(|| self.cvars.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v))
    }

    /// `pfnGetCvarFloat`: 0 for a missing cvar.
    pub fn cvar_float(&self, name: &str) -> f32 {
        self.cvar(name).map_or(0.0, |c| c.1)
    }

    /// `pfnGetCvarString`: `None` for a missing cvar.
    pub fn cvar_string(&self, name: &str) -> Option<&str> {
        self.cvar(name).map(|c| c.0.as_str())
    }

    /// `Cvar_Set`: the snapshot follows at once (GameUI re-reads it in the same frame), the
    /// engine runs the request after the frame.
    pub fn cvar_set(&mut self, name: &str, value: &str) {
        let key = self.cvars.keys().find(|k| k.eq_ignore_ascii_case(name)).cloned();
        if let Some(k) = key
            && let Some(e) = self.cvars.get_mut(&k)
        {
            e.0 = value.to_owned();
            e.1 = gs_console::cvar::q_atof(value);
        }
        self.requests.push(HostRequest::CvarSet { name: name.to_owned(), value: value.to_owned() });
    }

    /// `Cvar_SetValue` (`%d` when integral, else `%f`).
    pub fn cvar_set_value(&mut self, name: &str, v: f32) {
        self.cvar_set(name, &gs_console::cvar::format_value(v));
    }

    /// `pfnClientCmd`.
    pub fn client_cmd(&mut self, cmd: &str) {
        self.requests.push(HostRequest::ClientCmd(cmd.to_owned()));
    }
}

/// `CBaseUI` + `CGameUI` + `CGameConsole`.
pub struct GameUi {
    pub ui: Ui,
    pub host: Shared,
    pub client_panel: PanelId,
    pub game_ui_panel: PanelId,
    pub base_panel: PanelId,
    pub taskbar: PanelId,
    pub game_menu: Option<PanelId>,
    pub console: PanelId,
    /// `m_bActivatedUI`: set by the first ActivateGameUI, never cleared.
    activated: bool,
    /// `g_bConsoleShown`.
    console_shown: bool,
    /// `m_bHidingGameUIWithMouseDown`.
    hiding_with_mouse_down: bool,
    /// `g_hLoadingDialog` (a PHandle: gone once the panel is deleted).
    loading: Option<PanelId>,
    /// The game filesystem.
    pub fs: Fs,
    /// `CheckParm("-steam") || CheckParm("-showplatform")`: picks the loading layout.
    steam: bool,
    /// `g_VModuleLoader`.
    pub modules: modules::Modules,
    /// `m_bTryingToLoadTracker`: the platform modules load on the next `RunFrame`.
    trying_to_load_tracker: bool,
    /// Steam for the server browser, handed to it when the modules load.
    matchmaking: Option<Box<dyn serverbrowser::Matchmaking>>,
    /// `+0x124` / `+0x128`: the connected server's ip (network byte order) and port.
    connection: (i32, i32),
}

impl GameUi {
    /// `CBaseUI::Start` with `CGameUI::Initialize`/`Start` and `CGameConsole::Initialize`:
    /// `tracker` is TrackerScheme, `game_menu_res` the text of `resource/GameMenu.res`,
    /// `background_layout` that of `resource/BackgroundLayout.txt`; `console` is the
    /// `-console` command-line test.
    #[allow(clippy::too_many_arguments)]
    pub fn start(mut ui: Ui, tracker: Scheme, game_menu_res: Option<&str>, background_layout: Option<&str>, console: bool, steam: bool, fs: Fs, s: &mut dyn Surface) -> Self {
        let host: Shared = Rc::new(RefCell::new(HostState::default()));
        ui.add_scheme(tracker);
        let root = ui.embedded();
        ui.set_bounds(root, 0, 0, 40, 30);
        if let Some(n) = ui.node_mut(root) {
            n.paint_border_enabled = false;
            n.paint_background_enabled = false;
            n.paint_enabled = false;
            n.cursor = gs_vgui::Cursor::None;
        }
        let engine_panel = |ui: &mut Ui, name: &str, z: i32| {
            let p = ui.create(Some(root), name, Box::new(gs_vgui::PlainPanel));
            ui.make_editable(p);
            if let Some(n) = ui.node_mut(p) {
                n.paint_border_enabled = false;
                n.paint_background_enabled = false;
                n.paint_enabled = false;
                n.cursor = gs_vgui::Cursor::None;
            }
            ui.set_visible(p, true);
            ui.set_zpos(p, z);
            p
        };
        let client_panel = engine_panel(&mut ui, "BaseClientPanel", 25);
        let game_ui_panel = engine_panel(&mut ui, "BaseGameUIPanel", 50);
        // CGameUI::Initialize: the background panel under the GameUI panel.
        let base_panel = base_panel::BasePanel::create(&mut ui, host.clone(), game_ui_panel, background_layout);
        // CGameUI::Start: the platform config directory and the vgui user config.
        fs.add_search_path("platform/config", "CONFIG");
        let user_config = fs.read_in("InGameDialogConfig.vdf", "CONFIG").unwrap_or_default();
        ui.load_user_config_file(&user_config);
        // The taskbar (hidden until ActivateGameUI) with the game menu.
        let modules: modules::Modules = Rc::new(RefCell::new(modules::ModuleLoader::default()));
        let (taskbar, game_menu) = taskbar::Taskbar::create(&mut ui, host.clone(), fs.clone(), modules.clone(), base_panel, game_menu_res);
        ui.set_visible(taskbar, false);
        base_panel::BasePanel::set_state(&mut ui, base_panel, 2);
        host.borrow_mut().requests.push(HostRequest::ClientCmd("mp3 loop media/gamestartup.mp3\n".to_owned()));
        let (w, h) = s.screen_size();
        ui.screen = (w, h);
        ui.set_bounds(root, 0, 0, w, h);
        ui.set_bounds(game_ui_panel, 0, 0, w, h);
        ui.set_bounds(client_panel, 0, 0, w, h);
        let console_dialog = console::ConsoleDialog::create(&mut ui, host.clone(), taskbar);
        ui.set_parent(console_dialog, Some(game_ui_panel));
        let mut gameui = Self {
            ui,
            host,
            client_panel,
            game_ui_panel,
            base_panel,
            taskbar,
            game_menu,
            console: console_dialog,
            activated: false,
            console_shown: false,
            hiding_with_mouse_down: false,
            loading: None,
            fs,
            steam,
            modules,
            trying_to_load_tracker: true,
            matchmaking: None,
            connection: (0, 0),
        };
        gameui.game_ui_activate();
        if console {
            gameui.console_activate(s);
            gameui.console_shown = true;
        }
        gameui
    }

    /// GameUI's requests to the engine queued outside a frame (the start sequence).
    pub fn take_requests(&mut self) -> Vec<HostRequest> {
        std::mem::take(&mut self.host.borrow_mut().requests)
    }

    /// `IGameUI::IsGameUIActive` (= `HasExclusiveInput` = `VGuiWrap2_IsGameUIVisible`).
    pub fn is_game_ui_active(&self) -> bool {
        self.activated && self.ui.is_visible(self.base_panel)
    }

    /// `VGuiWrap2_IsConsoleVisible`.
    pub fn is_console_visible(&self) -> bool {
        self.ui.is_visible(self.console)
    }

    /// `CGameUI::ActivateGameUI`.
    fn game_ui_activate(&mut self) {
        if self.is_game_ui_active() {
            return;
        }
        self.activated = true;
        self.ui.set_visible(self.base_panel, true);
        self.ui.set_visible(self.taskbar, true);
        let mut h = self.host.borrow_mut();
        h.requests.push(HostRequest::ClientCmd("inputset_menu".to_owned()));
        h.requests.push(HostRequest::ClientCmd("setpause".to_owned()));
        drop(h);
        if let Some(m) = self.game_menu {
            gs_vgui::controls::menu::restart_reveal(&mut self.ui, m);
        }
        self.host.borrow_mut().requests.push(HostRequest::ClientCmd("richpresence_update\n".to_owned()));
    }

    /// `CGameUI::HideGameUI`.
    fn game_ui_hide(&mut self) {
        let in_level = !self.host.borrow().level_name.is_empty();
        if in_level {
            self.ui.set_visible(self.base_panel, false);
            if let Some(m) = self.game_menu {
                Menu::set_visible(&mut self.ui, m, false);
                Menu::set_currently_selected_item(&mut self.ui, m, -1);
            }
            let mut h = self.host.borrow_mut();
            h.requests.push(HostRequest::ClientCmd("inputset_game".to_owned()));
            h.requests.push(HostRequest::ClientCmd("unpause".to_owned()));
            h.requests.push(HostRequest::ClientCmd("hideconsole".to_owned()));
        }
        self.host.borrow_mut().requests.push(HostRequest::ClientCmd("richpresence_update\n".to_owned()));
    }

    /// `CBaseUI::ActivateGameUI`: the client UI is hidden by the host (`HideClientUI`).
    pub fn activate_game_ui(&mut self, s: &mut dyn Surface) {
        self.game_ui_activate();
        self.ui.set_visible(self.game_ui_panel, true);
        self.ui.set_visible(self.client_panel, false);
        self.ui.set_mouse_input_enabled(self.client_panel, false);
        if self.console_shown {
            self.console_activate(s);
        }
    }

    /// `CGameUI::ActivateDemoUI` → `CBasePanel::ActivateDemoUI`: the demo player bar is created
    /// (centred) if it is gone, then activated.
    pub fn activate_demo_ui(&mut self, s: &mut dyn Surface) -> bool {
        let t = self.taskbar;
        self.ui.send(t, &gs_vgui::Msg::new("ActivateDemoUI"), None, s);
        true
    }

    /// The DemoPlayer's GameUI-listener signals: `ReceiveSignal` of the bar while it exists.
    pub fn demo_signals(&mut self, signals: &[(i32, i32)], s: &mut dyn Surface) {
        let t = self.taskbar;
        for &(signal, data) in signals {
            self.ui.send(t, &gs_vgui::Msg::new("DemoPlayerSignal").int("signal", signal).int("data", data), None, s);
        }
    }

    /// `CBaseUI::HideGameUI`; `true` when the client UI comes back (a level is loaded).
    pub fn hide_game_ui(&mut self) -> bool {
        self.game_ui_hide();
        self.console_hide();
        let in_level = !self.host.borrow().level_name.is_empty();
        if in_level {
            self.ui.set_visible(self.game_ui_panel, false);
            if let Some(n) = self.ui.node_mut(self.game_ui_panel) {
                n.paint_background_enabled = false;
            }
            self.ui.set_visible(self.client_panel, true);
            self.ui.set_mouse_input_enabled(self.client_panel, true);
        }
        if self.ui.is_mouse_down(gs_vgui::keys::MOUSE_LEFT) {
            self.hiding_with_mouse_down = true;
        }
        in_level
    }

    /// `CBaseUI::Key_Event`: `true` when VGUI consumed the key. `ESC`/`MOUSE1` are engine key
    /// numbers 27/241.
    pub fn key_event(&mut self, down: bool, keynum: i32, toggle_console_key: i32, s: &mut dyn Surface) -> KeyFilter {
        if toggle_console_key != 0 && keynum == toggle_console_key {
            return KeyFilter::Pass;
        }
        if down && keynum == 27 {
            let in_level = !self.host.borrow().level_name.is_empty();
            if self.is_game_ui_active() && in_level {
                return KeyFilter::SwallowedHide(self.hide_game_ui());
            }
            self.activate_game_ui(s);
            return KeyFilter::SwallowedActivate;
        }
        if self.hiding_with_mouse_down && keynum == 241 && down {
            self.hiding_with_mouse_down = false;
            return KeyFilter::Swallowed;
        }
        if self.ui.need_kb_input() { KeyFilter::Swallowed } else { KeyFilter::Pass }
    }

    /// `CGameConsole::Activate` (ignored while a loading dialog exists).
    fn console_activate(&mut self, s: &mut dyn Surface) {
        if self.loading_dialog().is_some() {
            return;
        }
        self.ui.restrict_paint_to(None);
        let c = self.console;
        console::with_console(&mut self.ui, c, |d, ui| d.activate(ui, c, s));
    }

    /// `LoadingDialog()`.
    pub fn loading_dialog(&self) -> Option<PanelId> {
        self.loading.filter(|&l| self.ui.exists(l))
    }

    fn read(&self, name: &str) -> Option<String> {
        self.fs.read_text(name)
    }

    /// `IGameUI::LoadingStarted`.
    pub fn loading_started(&mut self, kind: &str) {
        let state = if kind.eq_ignore_ascii_case("transition") { 4 } else { 3 };
        base_panel::BasePanel::set_state(&mut self.ui, self.base_panel, state);
    }

    /// `IGameUI::LoadingFinished` (then `baseuifuncs->HideGameUI`).
    pub fn loading_finished(&mut self) -> bool {
        base_panel::BasePanel::set_state(&mut self.ui, self.base_panel, 2);
        self.hide_game_ui()
    }

    /// `new CLoadingDialog(staticPanel)` (centred by the background's `OnChildAdded`): the
    /// NoBanner layout without `-steam`/`-showplatform`, else the VAC one on a secure server.
    fn create_loading_dialog(&mut self, s: &mut dyn Surface) -> PanelId {
        let vac = self.steam && self.host.borrow().vac_secure;
        let res = if !self.steam {
            "Resource/LoadingDialogNoBanner.res"
        } else if vac {
            "Resource/LoadingDialogVAC.res"
        } else {
            "Resource/LoadingDialog.res"
        };
        let text = self.read(res);
        let d = loading::LoadingDialog::create(&mut self.ui, self.host.clone(), self.base_panel, text.as_deref(), s);
        loading::with_loading(&mut self.ui, d, |dlg, _| dlg.showing_vac = vac);
        taskbar::center(&mut self.ui, d);
        self.loading = Some(d);
        d
    }

    /// `IGameUI::StartProgressBar`.
    pub fn start_progress_bar(&mut self, kind: &str, steps: i32, s: &mut dyn Surface) {
        let dialog = match self.loading_dialog() {
            Some(d) => d,
            None => self.create_loading_dialog(s),
        };
        base_panel::BasePanel::set_state(&mut self.ui, self.base_panel, 3);
        let http = kind.len() >= 7 && (kind[..7].eq_ignore_ascii_case("http://") || kind.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("https://")));
        let http_res = http.then(|| self.read("Resource/LoadingDialog.res")).flatten();
        loading::with_loading(&mut self.ui, dialog, |d, ui| {
            d.set_progress_range(0, steps);
            d.set_progress_point(ui, 0);
            d.open(ui, dialog);
            if http {
                d.http_download = true;
                if let Some(t) = &http_res {
                    ui.load_control_settings(dialog, t, &mut gs_vgui::editable::create_control_by_name, s);
                }
            }
        });
    }

    /// `IGameUI::ContinueProgressBar` (the fraction argument is ignored).
    pub fn continue_progress_bar(&mut self, point: i32, s: &mut dyn Surface) -> bool {
        let Some(d) = self.loading_dialog() else { return false };
        // SetProgressPoint: the VAC layout once connected to a secure server.
        let vac = self.host.borrow().vac_secure && loading::with_loading(&mut self.ui, d, |dlg, _| !dlg.showing_vac).unwrap_or(false);
        let vac_res = vac.then(|| self.read("Resource/LoadingDialogVAC.res")).flatten();
        loading::with_loading(&mut self.ui, d, |dlg, ui| {
            if vac {
                if let Some(t) = &vac_res {
                    ui.load_control_settings(d, t, &mut gs_vgui::editable::create_control_by_name, s);
                }
                dlg.showing_vac = true;
            }
            dlg.set_progress_point(ui, point);
        });
        true
    }

    /// `IGameUI::StopProgressBar`.
    pub fn stop_progress_bar(&mut self, error: bool, reason: &str, ext: &str, s: &mut dyn Surface) {
        if self.loading_dialog().is_none() && error {
            self.create_loading_dialog(s);
        }
        let Some(d) = self.loading_dialog() else { return };
        if error {
            let err_res = self.read("Resource/LoadingDialogError.res");
            loading::with_loading(&mut self.ui, d, |dlg, ui| dlg.display_generic_error(ui, d, reason, ext, err_res.as_deref(), s));
        } else {
            self.ui.restrict_paint_to(None);
            gs_vgui::controls::frame::with_frame(&mut self.ui, d, |f, ui| f.close(ui, d));
            self.ui.mark_for_deletion(d);
            self.loading = None;
        }
        base_panel::BasePanel::set_state(&mut self.ui, self.base_panel, 2);
    }

    /// `IGameUI::SetProgressBarStatusText`.
    pub fn set_progress_status(&mut self, text: &str) -> bool {
        let Some(d) = self.loading_dialog() else { return false };
        loading::with_loading(&mut self.ui, d, |dlg, ui| dlg.set_status_text(ui, text));
        true
    }

    /// `IGameUI::SetSecondaryProgressBar`.
    pub fn set_secondary_progress(&mut self, f: f32, s: &mut dyn Surface) {
        let Some(d) = self.loading_dialog() else { return };
        let vac = self.host.borrow().vac_secure || loading::with_loading(&mut self.ui, d, |dlg, _| dlg.showing_vac).unwrap_or(false);
        let dual = self.read(if vac { "Resource/LoadingDialogDualProgressVAC.res" } else { "Resource/LoadingDialogDualProgress.res" });
        loading::with_loading(&mut self.ui, d, |dlg, ui| {
            if vac {
                dlg.showing_vac = true;
            }
            dlg.set_secondary_progress(ui, d, f, dual.as_deref(), s);
        });
    }

    /// `IGameUI::SetSecondaryProgressBarText`.
    pub fn set_secondary_text(&mut self, text: &str) {
        if let Some(d) = self.loading_dialog() {
            loading::LoadingDialog::set_secondary_text(&mut self.ui, d, text);
        }
    }

    /// `VGuiWrap2_ConnectToServer` (the moment the client goes active): the menu closes, the
    /// progress bar stops, `CGameUI::ConnectToServer(game, ip, port)` stops the menu music
    /// outside `valve`, runs `ui_cmd_on_connect` (then clears it), hides the UI again and tells
    /// the platform modules (`ip` with the address bytes in network order, `port` in host
    /// order). `true` when the client UI came back.
    pub fn connect_to_server(&mut self, game: &str, ip: i32, port: i32, s: &mut dyn Surface) -> bool {
        let shown = self.hide_game_ui();
        self.stop_progress_bar(false, "", "", s);
        {
            let mut h = self.host.borrow_mut();
            if !game.eq_ignore_ascii_case("valve") {
                h.client_cmd("mp3 stop\n");
            }
            let cmd = gs_common::bytestr::truncate_encoded(h.cvar_string("ui_cmd_on_connect").unwrap_or(""), 0x7f).into_owned();
            if gs_common::bytestr::encoded_len(&cmd) > 2 {
                h.client_cmd(&cmd);
                h.client_cmd("ui_cmd_on_connect 0\n");
            }
        }
        let shown = shown | self.hide_game_ui();
        self.connection = (ip, port);
        let msg = gs_vgui::Msg::new("ConnectedToGame").int("ip", ip).int("connectionport", port).int("queryport", port);
        self.modules.borrow().post_message_to_all(&mut self.ui, &msg);
        shown
    }

    /// `VGuiWrap2_NotifyOfServerDisconnect` after `CL_Disconnect` (`cls.state` is 1): the
    /// menu comes back (`CGameUI::DisconnectFromServer`: the modules hear it) and the progress
    /// bar stops with the explanation, if any.
    pub fn notify_disconnect(&mut self, explained: bool, reason: &str, ext: &str, s: &mut dyn Surface) {
        self.activate_game_ui(s);
        self.connection = (0, 0);
        self.modules.borrow().post_message_to_all(&mut self.ui, &gs_vgui::Msg::new("DisconnectedFromGame"));
        self.stop_progress_bar(explained, reason, ext, s);
    }

    /// Steam's server queries for the server browser (before the first frame; without them the
    /// module fails to initialize like ServerBrowser.dll without `SteamAPI_Init`).
    pub fn set_matchmaking(&mut self, mm: Box<dyn serverbrowser::Matchmaking>) {
        self.matchmaking = Some(mm);
    }

    /// `CGameUI::RunFrame`'s platform part: on the first frame the modules load, then hear the
    /// active game and the current connection.
    fn load_platform_modules(&mut self, s: &mut dyn Surface) {
        self.trying_to_load_tracker = false;
        let mm = self.matchmaking.take();
        let taskbar = self.taskbar;
        let (host, fs) = (self.host.clone(), self.fs.clone());
        self.modules.borrow_mut().initialize_all(&mut self.ui, &host, &fs, mm, taskbar, s);
        let game = options::ModInfo::load(self.fs.as_ref()).game_description().to_owned();
        let app_id = self.host.borrow().app_id;
        let msg = gs_vgui::Msg::new("ActiveGameName").str("name", &self.fs.game_dir()).int("type", 1).int("appid", app_id).str("game", &game);
        self.modules.borrow().post_message_to_all(&mut self.ui, &msg);
        let (ip, port) = self.connection;
        if ip != 0 {
            let msg = gs_vgui::Msg::new("ConnectedToGame").int("ip", ip).int("connectionport", port).int("queryport", port);
            self.modules.borrow().post_message_to_all(&mut self.ui, &msg);
        }
    }

    /// GameUI shutdown: `ShutdownPlatformModules` and `SaveUserConfigFile`.
    pub fn shutdown(&mut self, s: &mut dyn Surface) {
        self.modules.borrow_mut().shutdown(&mut self.ui, s);
        if let Some(text) = self.ui.user_config_file_text()
            && !self.fs.write_in("InGameDialogConfig.vdf", "CONFIG", &text)
        {
            log::debug!("InGameDialogConfig.vdf: not written");
        }
    }

    fn console_hide(&mut self) {
        let c = self.console;
        console::with_console(&mut self.ui, c, |d, ui| d.hide(ui, c));
    }

    /// `CBaseUI::ShowConsole`.
    pub fn show_console(&mut self, s: &mut dyn Surface) {
        self.console_shown = true;
        self.console_activate(s);
    }

    /// `CBaseUI::HideConsole`.
    pub fn hide_console(&mut self) {
        self.console_shown = false;
        self.console_hide();
    }

    /// `toggleconsole`: hides a visible console (the menu stays), else menu + console.
    pub fn toggle_console(&mut self, s: &mut dyn Surface) {
        if self.is_console_visible() {
            self.hide_console();
        } else {
            self.activate_game_ui(s);
            self.show_console(s);
        }
    }

    /// `CGameConsole::Printf`.
    pub fn con_printf(&mut self, text: &str) {
        let c = self.console;
        console::with_console(&mut self.ui, c, |d, ui| d.print(ui, text));
    }

    /// `CGameConsole::DPrintf`.
    pub fn con_dprintf(&mut self, text: &str) {
        let c = self.console;
        console::with_console(&mut self.ui, c, |d, ui| d.dprint(ui, text));
    }

    /// `CGameConsole::Clear`.
    pub fn clear(&mut self) {
        let c = self.console;
        console::with_console(&mut self.ui, c, |d, ui| {
            let h = d.history;
            gs_vgui::controls::rich_text::with_rt(ui, h, |rt, ui| rt.set_text(ui, h, ""));
        });
    }

    /// The console history text (`condump`).
    pub fn console_text(&mut self) -> String {
        let c = self.console;
        console::with_console(&mut self.ui, c, |d, ui| d.history_text(ui)).unwrap_or_default()
    }

    /// `CBaseUI::Paint`: `IGameUI::RunFrame`, `IVGui::RunFrame`, bounds, `PaintTraverse`.
    /// Returns GameUI's requests to the engine.
    pub fn paint(&mut self, s: &mut dyn Surface) -> Vec<HostRequest> {
        let (w, h) = s.screen_size();
        self.ui.screen = (w, h);
        self.ui.set_size(self.base_panel, w, h);
        self.modules.borrow_mut().run_frame(&mut self.ui, s);
        taskbar::Taskbar::run_frame(&mut self.ui, self.taskbar);
        if self.trying_to_load_tracker {
            self.load_platform_modules(s);
        }
        self.ui.run_frame(s);
        let root = self.ui.embedded();
        self.ui.set_bounds(root, 0, 0, w, h);
        self.ui.set_bounds(self.game_ui_panel, 0, 0, w, h);
        self.ui.set_bounds(self.client_panel, 0, 0, w, h);
        self.ui.paint(s);
        std::mem::take(&mut self.host.borrow_mut().requests)
    }
}

/// `CBaseUI::Key_Event`'s answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFilter {
    /// The key goes on to the engine's binds.
    Pass,
    /// Consumed by VGUI.
    Swallowed,
    /// ESC opened the GameUI.
    SwallowedActivate,
    /// ESC hid the GameUI; `true` when the client UI came back.
    SwallowedHide(bool),
}
