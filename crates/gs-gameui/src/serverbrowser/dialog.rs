//! `CServerBrowserDialog` (docs/re/serverbrowser.md §4): the "Servers" frame with the six game
//! pages, the status line, the saved filters (`ServerBrowser.vdf`) and the game-info dialogs.
//! Its state lives in [`Sb::dlg`] so the pages can read it while the frame is busy.

use std::any::Any;
use std::rc::Rc;

use gs_vgui::controls::frame::{FrameMsg, FrameState, with_frame};
use gs_vgui::controls::label::{Label, with_label};
use gs_vgui::controls::menu::Menu;
use gs_vgui::controls::property::{PropertySheet, with_sheet};
use gs_vgui::keyvalues::KeyValues;
use gs_vgui::scheme::Scheme;
use gs_vgui::{Control, Msg, PanelId, Surface, Ui};

use super::page::{self, PageKind};
use super::{GameServerItem, Sb, gameinfo};
use crate::options::{load_res, set_hd_scaled};

pub struct ServerBrowserDialog {
    pub fs: FrameState,
    sb: Rc<Sb>,
}

/// `new CServerBrowserDialog(NULL)` + `Initialize()` (parented to the taskbar by the loader).
pub fn create(ui: &mut Ui, sb: &Rc<Sb>, parent: PanelId, s: &mut dyn Surface) -> PanelId {
    let me = ui.create(Some(parent), "CServerBrowserDialog", Box::new(ServerBrowserDialog { fs: FrameState::default(), sb: sb.clone() }));
    FrameState::init(ui, me, true);
    set_hd_scaled(ui, me);
    sb.dlg.borrow_mut().panel = Some(me);
    load_user_data(ui, sb, s);
    let internet = page::create(ui, sb, me, PageKind::Internet, s);
    let favorites = page::create(ui, sb, me, PageKind::Favorites, s);
    let history = page::create(ui, sb, me, PageKind::History, s);
    let spectate = page::create(ui, sb, me, PageKind::Spectate, s);
    let lan = page::create(ui, sb, me, PageKind::Lan, s);
    let friends = page::create(ui, sb, me, PageKind::Friends, s);
    {
        let mut d = sb.dlg.borrow_mut();
        d.internet = Some(internet);
        d.favorites = Some(favorites);
        d.history = Some(history);
        d.spectate = Some(spectate);
        d.lan = Some(lan);
        d.friends = Some(friends);
    }
    with_frame(ui, me, |f, _| f.close_sound = Some("sound/UI/launch_dnmenu1.wav".to_owned()));
    let (mw, mh) = (ui.psv(me, 640), ui.psv(me, 384));
    ui.set_min_size(me, mw, mh);
    ui.set_size(me, mw, mh);
    sb.dlg.borrow_mut().game_list = Some(internet);
    let menu = Menu::create(ui, Some(me), "ServerContextMenu");
    sb.dlg.borrow_mut().context_menu = Some(menu);
    let tabs = PropertySheet::create(ui, Some(me), "GameTabs");
    let tw = ui.psv(me, 72);
    with_sheet(ui, tabs, |sh, ui| {
        sh.set_tab_width(ui, tabs, tw);
        sh.add_page(ui, tabs, internet, "#ServerBrowser_InternetTab");
        sh.add_page(ui, tabs, favorites, "#ServerBrowser_FavoritesTab");
        sh.add_page(ui, tabs, history, "#ServerBrowser_HistoryTab");
        sh.add_page(ui, tabs, spectate, "#ServerBrowser_SpectateTab");
        sh.add_page(ui, tabs, lan, "#ServerBrowser_LanTab");
        sh.add_page(ui, tabs, friends, "#ServerBrowser_FriendsTab");
    });
    ui.add_action_signal_target(tabs, me);
    let status = Label::create(ui, Some(me), "StatusLabel", "");
    {
        let mut d = sb.dlg.borrow_mut();
        d.tab_panel = Some(tabs);
        d.status_label = Some(status);
    }
    load_res(ui, me, sb.fs.as_ref(), "Servers/DialogServerBrowser.res", s);
    ui.load_user_config(me, "Servers/DialogServerBrowser.res", 0);
    let (sw, sh) = ui.screen;
    let (w, h) = ui.size(me);
    ui.set_size(me, w.min(sw), h.min(sh));
    with_label(ui, status, |l, ui| l.set_text(ui, ""));
    let gl = sb.dlg.borrow().saved_data.get_string("GameList", "");
    let active = match gl.to_ascii_lowercase().as_str() {
        "spectate" => spectate,
        "favorites" => favorites,
        "history" => history,
        "lan" => lan,
        "friends" => friends,
        _ => internet,
    };
    with_sheet(ui, tabs, |sh, ui| sh.set_active_page(ui, tabs, active));
    // Initialize.
    with_frame(ui, me, |f, ui| f.set_title(ui, me, "#ServerBrowser_Servers"));
    ui.set_visible(me, false);
    me
}

/// The six pages in the order `OnClose`/`SaveUserData` walk them.
fn pages(sb: &Sb) -> [Option<PanelId>; 6] {
    let d = sb.dlg.borrow();
    [d.favorites, d.history, d.internet, d.spectate, d.lan, d.friends]
}

/// `Open`: `Frame::Activate`, focus to the tabs.
pub fn open(ui: &mut Ui, sb: &Sb) {
    let Some(me) = sb.dlg.borrow().panel else { return };
    FrameState::activate(ui, me);
    if let Some(t) = sb.dlg.borrow().tab_panel {
        ui.request_focus(t);
    }
}

/// `LoadUserData`: `ServerBrowser.vdf` (`CONFIG`) with its "Filters" key split off.
pub fn load_user_data(ui: &mut Ui, sb: &Sb, s: &mut dyn Surface) {
    let mut saved = KeyValues::new("Filters");
    if let Some(data) = sb.fs.read_in("ServerBrowser.vdf", "CONFIG") {
        saved.load_from_buffer(&data);
    }
    let filters = saved.remove_sub_key("Filters").unwrap_or_else(|| KeyValues::new("Filters"));
    {
        let mut d = sb.dlg.borrow_mut();
        d.saved_data = saved;
        d.filter_data = filters;
    }
    let (panel, history, favorites) = {
        let d = sb.dlg.borrow();
        (d.panel, d.history, d.favorites)
    };
    let visible = panel.is_some_and(|p| ui.is_visible(p));
    if let Some(h) = history {
        page::with_games_page(ui, h, |p, ui| p.load_history_list(ui, h, s));
        if visible && ui.is_visible(h) {
            page::with_games_page(ui, h, |p, ui| p.start_refresh(ui, h, s));
        }
    }
    if let Some(f) = favorites {
        page::with_games_page(ui, f, |p, ui| p.load_favorites_list(ui, f, s));
        reload_filter_settings(ui, sb, s);
        if visible && ui.is_visible(f) {
            page::with_games_page(ui, f, |p, ui| p.start_refresh(ui, f, s));
        }
    }
    if let Some(p) = panel {
        ui.invalidate_layout(p);
    }
}

/// `SaveUserData`: the last tab and the filters into `ServerBrowser.vdf` (unknown keys kept),
/// then every page's user config.
pub fn save_user_data(ui: &mut Ui, sb: &Sb) {
    let mut saved = KeyValues::new("Filters");
    if let Some(data) = sb.fs.read_in("ServerBrowser.vdf", "CONFIG") {
        saved.load_from_buffer(&data);
    }
    let name = {
        let d = sb.dlg.borrow();
        let gl = d.game_list;
        if gl.is_some() && gl == d.spectate {
            "spectate"
        } else if gl.is_some() && gl == d.favorites {
            "favorites"
        } else if gl.is_some() && gl == d.lan {
            "lan"
        } else if gl.is_some() && gl == d.friends {
            "friends"
        } else if gl.is_some() && gl == d.history {
            "history"
        } else {
            "internet"
        }
    };
    saved.set_string("GameList", name);
    saved.remove_sub_key("Filters");
    saved.add_sub_key(sb.dlg.borrow().filter_data.clone());
    if !sb.fs.write_in("ServerBrowser.vdf", "CONFIG", &saved.save_to_buffer()) {
        log::debug!("ServerBrowser.vdf: not written");
    }
    sb.dlg.borrow_mut().saved_data = saved;
    for p in pages(sb).into_iter().flatten() {
        ui.save_user_config(p);
    }
}

/// `ReloadFilterSettings`: `LoadFilterSettings` on Internet, Spectate, Favorites, Lan, Friends,
/// History.
pub fn reload_filter_settings(ui: &mut Ui, sb: &Sb, s: &mut dyn Surface) {
    let order = {
        let d = sb.dlg.borrow();
        [d.internet, d.spectate, d.favorites, d.lan, d.friends, d.history]
    };
    for p in order.into_iter().flatten() {
        page::with_games_page(ui, p, |pg, ui| pg.load_filter_settings(ui, p, s));
    }
}

/// `UpdateStatusText(text)` (`#tokens` localized by the label).
pub fn update_status_text(ui: &mut Ui, sb: &Sb, text: &str) {
    let Some(l) = sb.dlg.borrow().status_label else { return };
    with_label(ui, l, |st, ui| st.set_text(ui, text));
    ui.invalidate_layout(l);
}

/// `AddServerToFavorites(server)`.
pub fn add_server_to_favorites(sb: &Sb, s: &GameServerItem) {
    let now = chrono::Utc::now().timestamp() as u32;
    sb.mm.borrow_mut().add_favorite_game(s.app_id, s.ip, s.conn_port, s.query_port, 1, now);
}

/// `GetContextMenu(parent)`: a fresh `CServerContextMenu` under `parent` (or the dialog).
pub fn get_context_menu(ui: &mut Ui, sb: &Sb, parent: Option<PanelId>) -> Option<PanelId> {
    let me = sb.dlg.borrow().panel?;
    if let Some(old) = sb.dlg.borrow_mut().context_menu.take() {
        ui.delete(old);
    }
    let m = Menu::create(ui, Some(me), "ServerContextMenu");
    ui.set_parent(m, Some(parent.unwrap_or(me)));
    Menu::set_visible(ui, m, false);
    sb.dlg.borrow_mut().context_menu = Some(m);
    Some(m)
}

/// `CServerContextMenu::ShowMenu`: the items for `target`, then shown at the cursor.
#[allow(clippy::too_many_arguments)]
pub fn show_context_menu(ui: &mut Ui, menu: PanelId, target: PanelId, server_id: i32, connect: bool, view: bool, refresh: bool, add_to_fav: bool) {
    let items = [
        (connect, "ConnectToServer", "#ServerBrowser_ConnectToServer"),
        (view, "ViewGameInfo", "#ServerBrowser_ViewServerInfo"),
        (refresh, "RefreshServer", "#ServerBrowser_RefreshServer"),
        (add_to_fav, "AddToFavorites", "#ServerBrowser_AddServerToFavorites"),
    ];
    for (on, name, text) in items {
        if on {
            Menu::add_item_msg(ui, menu, name, text, Msg::new(name).int("serverID", server_id), Some(target), None);
        }
    }
    let (cx, cy) = ui.cursor_pos();
    let (ex, ey) = ui.pos(ui.embedded());
    ui.set_pos(menu, cx - ex, cy - ey);
    Menu::set_visible(ui, menu, true);
}

/// Adds a game-info dialog to the list (`m_GameInfoDialogs.AddToTail`), dead handles pruned.
fn track(ui: &Ui, sb: &Sb, d: PanelId) {
    let mut st = sb.dlg.borrow_mut();
    st.game_info_dialogs.retain(|&p| ui.exists(p));
    st.game_info_dialogs.push(d);
}

/// `JoinGame(ip, port)` (`IServerBrowser::JoinGame`).
pub fn join_game(ui: &mut Ui, sb: &Rc<Sb>, ip: u32, port: u16, s: &mut dyn Surface) {
    let d = gameinfo::create(ui, sb, ip, port, port, s);
    if let Some(me) = sb.dlg.borrow().panel {
        ui.add_action_signal_target(d, me);
    }
    gameinfo::set_title_and_activate(ui, d, "", s);
    track(ui, sb, d);
    gameinfo::connect(ui, d, s);
}

/// `JoinGame(gameList, serverID)`: a game-info dialog on the row's address that connects.
pub fn join_game_from(ui: &mut Ui, sb: &Rc<Sb>, addr: (u32, u16, u16), s: &mut dyn Surface) {
    let d = open_game_info(ui, sb, addr, s);
    gameinfo::connect(ui, d, s);
}

/// `OpenGameInfoDialog(gameList, serverID)`: `addr` = (ip, queryPort, connPort).
pub fn open_game_info(ui: &mut Ui, sb: &Rc<Sb>, (ip, query, conn): (u32, u16, u16), s: &mut dyn Surface) -> PanelId {
    let d = gameinfo::create(ui, sb, ip, query, conn, s);
    if let Some(me) = sb.dlg.borrow().panel {
        ui.add_action_signal_target(d, me);
    }
    gameinfo::set_title_and_activate(ui, d, "", s);
    track(ui, sb, d);
    d
}

impl ServerBrowserDialog {
    /// `OnClose`: every page stops refreshing, then `Frame::OnClose`.
    fn on_close(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let d = self.sb.dlg.borrow();
        let order = [d.internet, d.favorites, d.history, d.spectate, d.lan, d.friends];
        drop(d);
        for p in order.into_iter().flatten() {
            page::with_games_page(ui, p, |pg, ui| pg.stop_refresh(ui, p, s));
        }
        self.fs.close(ui, me);
    }

    /// `OnConnectToGame` (§12.2).
    fn on_connect_to_game(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) {
        let ip = msg.get_int("ip");
        let conn = msg.get_int("connectionport");
        let query = msg.get_int("queryport");
        if ip == 0 || query == 0 {
            return;
        }
        {
            let mut d = self.sb.dlg.borrow_mut();
            let cur = &mut d.current_connection;
            *cur = GameServerItem { conn_port: cur.conn_port, query_port: cur.query_port, ip: cur.ip, ..GameServerItem::default() };
            cur.query_port = query as u16;
            cur.ip = ip as u32;
            cur.conn_port = conn as u16;
        }
        if self.sb.dlg.borrow().history.is_some() {
            let now = chrono::Utc::now().timestamp() as u32;
            self.sb.mm.borrow_mut().add_favorite_game(0, (ip as u32).swap_bytes(), conn as u16, query as u16, 2, now);
            let history = self.sb.dlg.borrow().history;
            if let Some(h) = history {
                page::with_games_page(ui, h, |p, _| p.refresh_on_list_reload = true);
            }
        }
        let dialogs: Vec<PanelId> = self.sb.dlg.borrow().game_info_dialogs.iter().copied().filter(|&d| ui.exists(d)).collect();
        for d in dialogs {
            ui.post(d, Msg::new("ConnectedToGame").int("ip", ip).int("connectionport", conn).int("queryport", query), Some(me), 0.0);
        }
        let favorites = self.sb.dlg.borrow().favorites;
        if let Some(f) = favorites {
            page::with_games_page(ui, f, |p, ui| p.set_connected(ui, true));
        }
        self.sb.dlg.borrow_mut().currently_connected = true;
        self.on_close(ui, me, s);
    }
}

impl Control for ServerBrowserDialog {
    fn class(&self) -> &'static str {
        "CServerBrowserDialog"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("PageChanged") {
            let tabs = self.sb.dlg.borrow().tab_panel;
            let active = tabs.and_then(|t| with_sheet(ui, t, |sh, _| sh.active_page()).flatten());
            self.sb.dlg.borrow_mut().game_list = active;
            update_status_text(ui, &self.sb, "");
            ui.invalidate_layout(me);
            return true;
        }
        if msg.is("ActiveGameName") {
            {
                let mut d = self.sb.dlg.borrow_mut();
                d.game_name = msg.get_str("name").chars().take(127).collect();
                d.mod_dir = msg.get_str("game").chars().take(127).collect();
                d.limit_app_id = msg.get_int("appid") as u32 as u64;
            }
            reload_filter_settings(ui, &self.sb, s);
            return true;
        }
        if msg.is("ConnectedToGame") {
            self.on_connect_to_game(ui, me, msg, s);
            return true;
        }
        if msg.is("DisconnectedFromGame") {
            {
                let mut d = self.sb.dlg.borrow_mut();
                d.currently_connected = false;
                d.current_connection = GameServerItem::default();
            }
            let favorites = self.sb.dlg.borrow().favorites;
            if let Some(f) = favorites {
                page::with_games_page(ui, f, |p, ui| p.set_connected(ui, false));
            }
            return true;
        }
        if msg.is("Close") {
            self.on_close(ui, me, s);
            return true;
        }
        match self.fs.on_message(ui, me, msg, s) {
            FrameMsg::Handled => true,
            FrameMsg::Unhandled => false,
            FrameMsg::Command(c) => {
                if c.eq_ignore_ascii_case("Close") {
                    self.on_close(ui, me, s);
                } else {
                    self.fs.on_command(ui, me, &c);
                }
                true
            }
        }
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.fs.apply_scheme(ui, me, scheme, s);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        self.fs.perform_layout(ui, me);
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.fs.paint_background(ui, me, s);
    }
    fn frame(&mut self) -> Option<&mut FrameState> {
        Some(&mut self.fs)
    }
    /// `GetDefaultScreenPosition`: 5 % from the left, 40 % down, half the width, 55 % of the
    /// height of the workspace.
    fn default_screen_position(&self, ui: &Ui) -> Option<(i32, i32, i32, i32)> {
        let (ww, wt) = ui.screen;
        let (ww, wt) = (f64::from(ww), f64::from(wt));
        Some((-((ww * -0.05) as i32), -((wt * -0.4) as i32), (ww * 0.5) as i32, (wt * 0.55) as i32))
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
