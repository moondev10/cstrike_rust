//! `CVGuiSystemModuleLoader` (docs/re/gameui.md §7): the platform modules listed in
//! `steam/games/PlatformMenu.vdf`. Only the server browser loads (Friends is skipped).

use std::cell::RefCell;
use std::rc::Rc;

use gs_vgui::keyvalues::KeyValues;
use gs_vgui::{Msg, PanelId, Surface, Ui};

use crate::serverbrowser::{Matchmaking, ServerBrowser};
use crate::{Fs, Shared};

/// The loaded modules, shared by GameUI and the taskbar (`g_VModuleLoader`).
pub type Modules = Rc<RefCell<ModuleLoader>>;

/// One loaded module: its `PlatformMenu.vdf` key and `MenuName`.
struct Module {
    name: String,
    label: String,
    module: ServerBrowser,
}

#[derive(Default)]
pub struct ModuleLoader {
    modules: Vec<Module>,
    /// `m_bModulesInitialized`.
    pub initialized: bool,
}

impl ModuleLoader {
    /// `InitializeAllModules(factories, n, useSteamModules = false)` + `InitializeModules`: the
    /// server browser gets Steam (`mm`) or fails to initialize without it.
    pub fn initialize_all(&mut self, ui: &mut Ui, host: &Shared, fs: &Fs, mm: Option<Box<dyn Matchmaking>>, taskbar: PanelId, s: &mut dyn Surface) -> bool {
        let Some(data) = fs.read_in("steam/games/PlatformMenu.vdf", "PLATFORM") else { return false };
        let mut kv = KeyValues::new("Platform");
        kv.load_from_buffer(&data);
        let mut mm = mm;
        let mut ok = true;
        let entries = kv.find_key("Menu").map(|m| m.children.clone()).unwrap_or_default();
        for it in entries {
            if it.get_int("SteamApp", 0) != 0 {
                continue;
            }
            let iface = it.get_string("interface", "");
            if iface.eq_ignore_ascii_case("VGuiModuleTracker001") {
                continue;
            }
            let dll = it.get_string("dll", "");
            if !dll.eq_ignore_ascii_case("servers/serverbrowser.dll") || !fs.exists(&dll) {
                log::debug!("Platform Error: couldn't find {dll}, not loading");
                ok = false;
                continue;
            }
            if iface != "VGuiModuleServerBrowser001" {
                log::debug!("Platform Error: module version ('{iface}, {dll}) invalid, not loading");
                ok = false;
                continue;
            }
            // IVGuiModule::Initialize: SteamAPI_Init failing is fatal for the module.
            let Some(m) = mm.take() else {
                log::warn!("Platform Error: module failed to initialize (Steam is not available)");
                ok = false;
                continue;
            };
            let module = ServerBrowser::initialize(ui, host.clone(), fs.clone(), m, taskbar, s);
            self.modules.push(Module { name: it.name.clone(), label: it.get_string("MenuName", "< unknown >"), module });
        }
        self.initialized = true;
        ok
    }

    /// `ActivateModule(name)`: by `MenuName` or key name.
    pub fn activate_module(&mut self, ui: &mut Ui, name: &str, s: &mut dyn Surface) -> bool {
        let Some(m) = self.modules.iter_mut().find(|m| m.label.eq_ignore_ascii_case(name) || m.name.eq_ignore_ascii_case(name)) else { return false };
        m.module.activate(ui, s);
        true
    }

    /// `PostMessageToAllModules(kv)`.
    pub fn post_message_to_all(&self, ui: &mut Ui, msg: &Msg) {
        for m in &self.modules {
            if let Some(p) = m.module.panel() {
                ui.post(p, msg.clone(), None, 0.0);
            }
        }
    }

    /// `RunFrame`: each module's `RunFrame`.
    pub fn run_frame(&mut self, ui: &mut Ui, s: &mut dyn Surface) {
        for m in &mut self.modules {
            m.module.run_frame(ui, s);
        }
    }

    /// `ShutdownPlatformModules`: `Deactivate`, `Command "Quit"`, `Shutdown`.
    pub fn shutdown(&mut self, ui: &mut Ui, s: &mut dyn Surface) {
        for m in &mut self.modules {
            m.module.deactivate(ui);
        }
        self.post_message_to_all(ui, &Msg::new("Command").str("command", "Quit"));
        for m in &mut self.modules {
            m.module.shutdown(ui, s);
        }
    }
}
