//! `CCreateMultiplayerGameDialog` with its Server and Gameplay pages
//! (docs/re/gameui-dialogs.md §12).

use std::any::Any;

use gs_vgui::controls::button::with_button;
use gs_vgui::controls::combo_box::ComboBox;
use gs_vgui::controls::property::{DialogHooks, NoHooks, PageHooks, PropertyDialog, PropertyPage, with_property_dialog};
use gs_vgui::controls::text_entry::{TextEntry, with_te};
use gs_vgui::controls::toggle::{ToggleButton, ToggleKind, with_toggle};
use gs_vgui::{Msg, PanelId, Surface, Ui};

use crate::advanced::{ControlNames, Description, MpControl, ObjType, PanelListPanel, create_controls};
use crate::cvar_controls as cv;
use crate::options::{ModInfo, c_atoi, load_res, set_hd_scaled, set_page_hooks, task_frame};
use crate::{Fs, Shared};

const GAMEPLAY_NAMES: ControlNames = ControlNames { item: "", check: "DescCheckButton", text: "DescEdit", combo: "DescEdit" };

/// `RandomInt(lo, hi)`: uniform by rejection, lazily seeded from the time.
fn random_int(lo: i32, hi: i32) -> i32 {
    use std::cell::Cell;
    thread_local!(static STATE: Cell<u64> = const { Cell::new(0) });
    STATE.with(|st| {
        if st.get() == 0 {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
            st.set(t | 1);
        }
        let span = (hi - lo + 1).max(1) as u64;
        let limit = u64::MAX - u64::MAX % span;
        loop {
            let mut x = st.get();
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            st.set(x);
            if x < limit {
                return lo + (x % span) as i32;
            }
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Server page (§12.1)

struct ServerPage {
    host: Shared,
    map_list: PanelId,
    enable_bots: PanelId,
    steam_networking: PanelId,
    tutor: PanelId,
    cz: bool,
}

fn create_server_page(ui: &mut Ui, dlg: PanelId, host: &Shared, fs: &Fs, mi: &ModInfo, s: &mut dyn Surface) -> PanelId {
    let page = PropertyPage::create(ui, Some(dlg), "ServerPage", true, Box::new(NoHooks));
    let map_list = ComboBox::create(ui, Some(page), "MapList", 12, false);
    TextEntry::create(ui, Some(page), "BotQuotaCombo");
    let tutor = cv::create_check(ui, Some(page), "CheckButtonTutor", "#CStrike_Tutor_Enabled", Some("tutor_enable"), false, host.clone(), s);
    let enable_bots = ToggleButton::create(ui, Some(page), "EnableBotsCheck", "Enable bots", ToggleKind::Check);
    let steam_networking = ToggleButton::create(ui, Some(page), "EnableSteamNetworkingCheck", "Enable Steam Networking", ToggleKind::Check);
    load_res(ui, page, fs.as_ref(), "Resource/CreateMultiplayerGameServerPage.res", s);
    load_map_list(ui, map_list, fs.as_ref(), mi, s);
    let cz = mi.game_description().eq_ignore_ascii_case("Condition Zero");
    if !cz {
        ui.set_visible(tutor, false);
    }
    set_page_hooks(ui, page, Box::new(ServerPage { host: host.clone(), map_list, enable_bots, steam_networking, tutor, cz }));
    page
}

/// `LoadMapList`.
fn load_map_list(ui: &mut Ui, combo: PanelId, fs: &dyn crate::GameFs, mi: &ModInfo, s: &mut dyn Surface) {
    ComboBox::delete_all_items(ui, combo);
    let mut tree: Vec<String> = Vec::new();
    ComboBox::add_item(ui, combo, "#GameUI_RandomMap", Some(Msg::new("data").str("mapname", "< Random Map >")));
    let game = mi.game_description().to_owned();
    let filter = mi.get_mp_filter().to_owned();
    let mut load = |id: Option<&str>| {
        for (name, _) in fs.find("maps/*.bsp", id) {
            let mut m: String = match name.find("maps") {
                Some(i) => name[i + 5..].to_owned(),
                None => name.clone(),
            }
            .chars()
            .take(255)
            .collect();
            if let Some(i) = m.find(".bsp") {
                m.truncate(i);
            }
            let b = m.as_bytes();
            if game.eq_ignore_ascii_case("Half-Life") && ((b.len() > 2 && (b[0] == b'c' || b[0] == b't') && b[2] == b'a' && (b'0'..=b'5').contains(&b[1])) || m.starts_with("hldemo")) {
                continue;
            }
            if game.eq_ignore_ascii_case("Opposing Force") && m.starts_with("of") {
                continue;
            }
            if !filter.is_empty() && m.contains(&filter) {
                continue;
            }
            let key = m.to_ascii_lowercase();
            if let Err(pos) = tree.binary_search_by(|t| t.to_ascii_lowercase().cmp(&key)) {
                tree.insert(pos, m);
            }
        }
    };
    if game.eq_ignore_ascii_case("Half-Life") {
        load(None);
    } else {
        load(Some("GAME"));
        if !mi.get_fallback_dir().is_empty() && mi.use_fallback_maps() {
            load(Some("GAME_FALLBACK"));
        }
    }
    load(Some("GAMEDOWNLOAD"));
    for m in &tree {
        ComboBox::add_item(ui, combo, m, Some(Msg::new("data").str("mapname", m)));
    }
    ComboBox::activate_item(ui, combo, 0, s);
}

impl ServerPage {
    /// `GetMapName`: `None` with only the random entry; a random pick resolved.
    fn map_name(ui: &mut Ui, combo: PanelId) -> Option<String> {
        let n = ComboBox::item_count(ui, combo);
        if n <= 1 {
            return None;
        }
        let m = ComboBox::active_item_user_data(ui, combo).map(|kv| kv.get_str("mapname")).unwrap_or_default();
        if m == "< Random Map >" {
            let i = random_int(1, n - 1);
            return ComboBox::item_user_data(ui, combo, i).map(|kv| kv.get_str("mapname"));
        }
        Some(m)
    }
}

impl PageHooks for ServerPage {
    fn on_apply_changes(&mut self, ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        // The stored host name/password/max players/map are never read by the launch.
        if self.cz {
            cv::apply_changes(ui, self.tutor, false);
        }
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _s: &mut dyn Surface) -> bool {
        if msg.is("CheckButtonChecked") {
            let enabled = with_toggle(ui, self.enable_bots, |t, _| t.is_selected()).unwrap_or(false);
            for name in ["SkillLevelExpert", "SkillLevelHard", "SkillLevelNormal", "SkillLevelEasy", "BotQuotaCombo", "BotDifficultyLabel", "BotLabel1", "BotLabel2"] {
                if let Some(c) = ui.find_child_by_name(me, name) {
                    ui.set_enabled(c, enabled);
                }
            }
            let _ = &self.host;
            return true;
        }
        false
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// Gameplay page (§12.2)

struct GameplayPage {
    host: Shared,
    fs: Fs,
    desc: Description,
    controls: Vec<PanelId>,
}

fn create_gameplay_page(ui: &mut Ui, dlg: PanelId, host: &Shared, fs: &Fs, s: &mut dyn Surface) -> PanelId {
    let page = PropertyPage::create(ui, Some(dlg), "GameplayPage", true, Box::new(NoHooks));
    let list = PanelListPanel::create(ui, Some(page), "GameOptions");
    let mut desc = Description::new("SERVER_OPTIONS");
    desc.init_from_file(fs.as_ref(), "settings.scr", host);
    load_res(ui, page, fs.as_ref(), "Resource/CreateMultiplayerGameGameplayPage.res", s);
    // LoadGameOptionsList: the persona's "<name>'s Server" replaces the hostname default.
    let persona: String = host.borrow().persona_name.chars().filter(char::is_ascii).collect();
    let persona = if host.borrow().steam_available && persona.is_empty() { "Someone".to_owned() } else { persona.trim_end_matches(' ').to_owned() };
    let def_host: String = format!("{persona}'s Server").chars().take(254).collect();
    let controls = create_controls(ui, list, &desc, &GAMEPLAY_NAMES, Some(&def_host), s);
    set_page_hooks(ui, page, Box::new(GameplayPage { host: host.clone(), fs: fs.clone(), desc, controls }));
    page
}

impl GameplayPage {
    /// `GetValue(cvar, def)`: the named control's `GetText`.
    fn value(&self, ui: &mut Ui, cvar: &str, def: &str) -> String {
        for &mp in &self.controls {
            if !ui.node(mp).is_some_and(|n| n.name.eq_ignore_ascii_case(cvar)) {
                continue;
            }
            let Some(control) = ui.ctrl::<MpControl>(mp).and_then(|m| m.control) else { continue };
            let text = with_te(ui, control, |te, _| te.text())
                .or_else(|| ui.ctrl::<ComboBox>(control).map(|cb| cb.te.text()))
                .or_else(|| with_toggle(ui, control, |t, _| t.st.label.text.text_string()));
            if let Some(t) = text {
                return t.chars().take(127).collect();
            }
        }
        def.to_owned()
    }

    /// `GatherCurrentValues` (gameplay version: list items matched by their shown text).
    fn gather(&mut self, ui: &mut Ui) {
        for &mp in &self.controls {
            let Some((obj, control)) = ui.ctrl::<MpControl>(mp).map(|m| (m.obj, m.control)) else { continue };
            let Some(o) = self.desc.objects.get(obj).cloned() else { continue };
            let value = match control {
                None => o.def_value.clone(),
                Some(c) => match o.ty {
                    ObjType::Bool => if with_toggle(ui, c, |t, _| t.is_selected()).unwrap_or(false) { "1" } else { "0" }.to_owned(),
                    ObjType::Number | ObjType::String => gs_common::bytestr::ansi_cap(&with_te(ui, c, |te, _| te.text()).unwrap_or_default(), 256),
                    ObjType::List => {
                        let shown = gs_common::bytestr::ansi_cap(&ComboBox::text(ui, c), 128);
                        o.items
                            .iter()
                            .find(|it| {
                                let w = match it.text.strip_prefix('#').and_then(|k| ui.localize.find(k)) {
                                    Some(l) => l.to_owned(),
                                    None => it.text.clone(),
                                };
                                w.eq_ignore_ascii_case(&shown)
                            })
                            .map_or_else(|| o.def_value.clone(), |it| it.value.clone())
                    }
                },
            };
            let v: String = value.chars().filter(|&c| c != '"' && c != '%').collect();
            if let Some(o) = self.desc.objects.get_mut(obj) {
                o.set_cur_value(&v);
            }
        }
    }
}

impl PageHooks for GameplayPage {
    fn on_apply_changes(&mut self, ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        self.gather(ui);
        self.desc.update_engine(&self.host);
        let text = self.desc.write_to_script();
        self.fs.write_config("settings.scr", text.as_bytes());
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// CCreateMultiplayerGameDialog

struct CreateServerHooks {
    host: Shared,
    server: PanelId,
    gameplay: PanelId,
}

impl DialogHooks for CreateServerHooks {
    /// `OnOK`: after the pages' apply, the launch command.
    fn on_ok(&mut self, ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        let map_list = ui.with_ctrl(self.server, |c, _| {
            c.as_any_mut().downcast_mut::<PropertyPage>().and_then(|p| p.hooks.as_any_mut().downcast_mut::<ServerPage>()).map(|sp| (sp.map_list, sp.steam_networking))
        });
        let Some(Some((map_list, steam_check))) = map_list else { return };
        let map: String = ServerPage::map_name(ui, map_list).unwrap_or_default().chars().take(63).collect();
        let gp = self.gameplay;
        let vals = ui.with_ctrl(gp, |c, ui| {
            c.as_any_mut().downcast_mut::<PropertyPage>().and_then(|p| p.hooks.as_any_mut().downcast_mut::<GameplayPage>()).map(|g| (g.value(ui, "hostname", "Half-Life"), g.value(ui, "sv_password", ""), c_atoi(&g.value(ui, "maxplayers", "32"))))
        });
        let Some(Some((host_name, pass, maxplayers))) = vals else { return };
        let host_name: String = host_name.chars().take(63).collect();
        let pass: String = pass.chars().take(63).collect();
        let steam = with_toggle(ui, steam_check, |t, _| t.is_selected()).unwrap_or(false);
        let cmd = format!(
            "disconnect\nsv_lan {}\nsv_use_steam_networking {}\nsetmaster enable\nmaxplayers {}\nsv_password \"{}\"\nhostname \"{}\"\ncd fadeout\nmap {}\n",
            i32::from(!steam),
            i32::from(steam),
            maxplayers,
            pass,
            host_name,
            map
        );
        let mut h = self.host.borrow_mut();
        h.client_cmd(&cmd);
        h.client_cmd("wait\nwait\n");
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub fn create_server_dialog(ui: &mut Ui, parent: PanelId, host: &Shared, fs: &Fs, s: &mut dyn Surface) -> PanelId {
    let mi = ModInfo::load(fs.as_ref());
    let dlg = PropertyDialog::create(ui, Some(parent), "CreateMultiplayerGameDialog", Box::new(gs_vgui::controls::property::NoDialogHooks));
    set_hd_scaled(ui, dlg);
    let (w, h) = (ui.psv(dlg, 448), ui.psv(dlg, 460));
    ui.set_size(dlg, w, h);
    task_frame(ui, dlg, "#GameUI_CreateServer", true);
    if let Some(parts) = with_property_dialog(ui, dlg, |d, _| d.parts) {
        with_button(ui, parts.ok, |b, ui| b.label.set_text(ui, "#GameUI_Start"));
    }
    let server = create_server_page(ui, dlg, host, fs, &mi, s);
    PropertyDialog::add_page(ui, dlg, server, "#GameUI_Server");
    let gameplay = create_gameplay_page(ui, dlg, host, fs, s);
    PropertyDialog::add_page(ui, dlg, gameplay, "#GameUI_Game");
    // CS: no bot page; CSBotConfig.vdf has no CONFIG path, so the defaults stand.
    let quota = match host.borrow().cvar_string("bot_quota") {
        Some(_) => host.borrow().cvar_float("bot_quota") as i32,
        None => 9,
    };
    if let Some(bq) = ui.find_child_by_name(server, "BotQuotaCombo") {
        with_te(ui, bq, |te, ui| te.set_text(ui, bq, &quota.to_string(), s));
    }
    if host.borrow().cvar_string("sv_use_steam_networking").is_some() {
        let on = host.borrow().cvar_float("sv_use_steam_networking") as i32 != 0;
        let sn = ui.with_ctrl(server, |c, _| c.as_any_mut().downcast_mut::<PropertyPage>().and_then(|p| p.hooks.as_any_mut().downcast_mut::<ServerPage>()).map(|sp| sp.steam_networking)).flatten();
        if let Some(sn) = sn {
            with_toggle(ui, sn, |t, ui| t.set_selected(ui, sn, on, s));
        }
    }
    if let Some(easy) = ui.find_child_by_name(server, "SkillLevelEasy") {
        with_toggle(ui, easy, |t, ui| t.set_selected(ui, easy, true, s));
    }
    with_property_dialog(ui, dlg, |d, _| d.hooks = Box::new(CreateServerHooks { host: host.clone(), server, gameplay }));
    dlg
}
