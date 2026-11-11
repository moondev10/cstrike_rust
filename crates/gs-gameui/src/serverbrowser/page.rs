//! `CBaseGamesPage` and its six pages (docs/re/serverbrowser.md §5–§7): the server list fed by a
//! Steam server-list request, the client-side filters, refresh/sort timing and the context menus.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gs_vgui::controls::button::{Button, with_button};
use gs_vgui::controls::combo_box::{ComboBox, with_combo};
use gs_vgui::controls::label::{Label, with_label};
use gs_vgui::controls::list_panel::{COLUMN_HIDDEN, ListItem, ListPanel, ListSortFunc, with_list};
use gs_vgui::controls::property::{NoHooks, PageHooks, PropertyPage, with_page};
use gs_vgui::controls::text_entry::{TextEntry, text_of, with_te};
use gs_vgui::controls::toggle::{ToggleButton, ToggleKind, with_toggle};
use gs_vgui::keys::{KEY_F5, MOUSE_LEFT, MOUSE_RIGHT};
use gs_vgui::msg::atoi;
use gs_vgui::scheme::Scheme;
use gs_vgui::{FontHandle, Msg, PanelId, Surface, Ui, Value};

use super::dialog::{self, update_status_text};
use super::{Details, GameServerItem, ListKind, ListRequest, MmEvent, Responder, Sb, ip_string, last_played_text, small};
use crate::options::load_res;

/// Which `CBaseGamesPage` subclass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    Internet,
    Favorites,
    History,
    Spectate,
    Lan,
    Friends,
}

impl PageKind {
    fn name(self) -> &'static str {
        match self {
            Self::Internet => "InternetGames",
            Self::Favorites => "FavoriteGames",
            Self::History => "HistoryGames",
            Self::Spectate => "SpectateGames",
            Self::Lan => "LanGames",
            Self::Friends => "FriendsGames",
        }
    }

    fn list_kind(self) -> ListKind {
        match self {
            Self::Internet => ListKind::Internet,
            Self::Favorites => ListKind::Favorites,
            Self::History => ListKind::History,
            Self::Spectate => ListKind::Spectator,
            Self::Lan => ListKind::Lan,
            Self::Friends => ListKind::Friends,
        }
    }

    /// `SupportsItem(GETNEWLIST)`.
    fn get_new_list(self) -> bool {
        matches!(self, Self::Internet | Self::Spectate)
    }

    /// `SupportsItem(ADDSERVER)` / `SupportsItem(ADDCURRENTSERVER)`.
    fn add_server(self) -> bool {
        self == Self::Favorites
    }

    /// The pages built on `CInternetGames`.
    fn internet_like(self) -> bool {
        matches!(self, Self::Internet | Self::Spectate)
    }
}

/// `serverdisplay_t`.
#[derive(Debug, Clone, Copy)]
struct ServerDisplay {
    list_id: i32,
    server_id: i32,
    do_not_refresh: bool,
    ip: u32,
    conn_port: u16,
    query_port: u16,
}

/// `CUtlMap<int, serverdisplay_t>`: walked in node-index order (freed nodes are reused first).
#[derive(Default)]
struct ServerMap {
    slots: Vec<Option<(i32, ServerDisplay)>>,
    free: Vec<usize>,
    index: HashMap<i32, usize>,
}

impl ServerMap {
    fn find(&self, id: i32) -> Option<usize> {
        self.index.get(&id).copied()
    }

    fn insert(&mut self, id: i32, d: ServerDisplay) -> usize {
        let slot = match self.free.pop() {
            Some(s) => {
                self.slots[s] = Some((id, d));
                s
            }
            None => {
                self.slots.push(Some((id, d)));
                self.slots.len() - 1
            }
        };
        self.index.insert(id, slot);
        slot
    }

    fn remove_at(&mut self, slot: usize) {
        if let Some((id, _)) = self.slots.get_mut(slot).and_then(Option::take) {
            self.index.remove(&id);
            self.free.push(slot);
        }
    }

    /// `RemoveAll`: the free list restarts at node 0.
    fn remove_all(&mut self) {
        let n = self.slots.len();
        self.slots.iter_mut().for_each(|s| *s = None);
        self.free = (0..n).rev().collect();
        self.index.clear();
    }

    fn get(&self, slot: usize) -> Option<&ServerDisplay> {
        self.slots.get(slot).and_then(Option::as_ref).map(|(_, d)| d)
    }

    fn get_mut(&mut self, slot: usize) -> Option<&mut ServerDisplay> {
        self.slots.get_mut(slot).and_then(Option::as_mut).map(|(_, d)| d)
    }

    fn live_slots(&self) -> Vec<usize> {
        (0..self.slots.len()).filter(|&i| self.slots[i].is_some()).collect()
    }
}

/// One game page (`CBaseGamesPage` with its subclass fields).
pub struct GamesPage {
    sb: Rc<Sb>,
    kind: PageKind,
    responder: Responder,
    game_list: PanelId,
    connect: PanelId,
    refresh_all: PanelId,
    refresh_quick: PanelId,
    add_server: PanelId,
    add_current_server: PanelId,
    filter: PanelId,
    filter_string: PanelId,
    game_filter: PanelId,
    map_filter: PanelId,
    ping_filter: PanelId,
    secure_filter: PanelId,
    full_filter: PanelId,
    empty_filter: PanelId,
    no_password_filter: PanelId,
    valid_steam_filter: PanelId,
    map_servers: ServerMap,
    /// `m_mapServerIP`: (ip, connection port) → serverID.
    map_server_ip: HashMap<(u32, u16), i32>,
    server_filters: Vec<(String, String)>,
    refresh_count: i32,
    request: ListRequest,
    /// `GetServerDetails(m_hRequest, id)` as of the last response for `id`.
    details: Details,
    combo_all_text: String,
    filters_visible: bool,
    font: FontHandle,
    game_filter_text: String,
    map_filter_text: String,
    ping: i32,
    no_full: bool,
    no_empty: bool,
    no_password: bool,
    valid_steam: bool,
    secure: i32,
    limit_app_id: u64,
    // CInternetGames
    last_sort: f32,
    dirty: bool,
    require_update: bool,
    any_retrieved: bool,
    any_responded: bool,
    offline: bool,
    // CFavoriteGames / CHistoryGames
    pub refresh_on_list_reload: bool,
    // CLanGames
    requesting: bool,
    request_time: f64,
    auto_refresh: bool,
}

/// Runs `f` on the page `id`.
pub fn with_games_page<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut GamesPage, &mut Ui) -> R) -> Option<R> {
    with_page::<GamesPage, R>(ui, id, f)
}

/// A Steam response for the page `panel`; `false` when it is not a page.
pub fn on_event(ui: &mut Ui, panel: PanelId, ev: &MmEvent, s: &mut dyn Surface) -> bool {
    with_games_page(ui, panel, |p, ui| p.on_event(ui, panel, ev, s)).is_some()
}

fn sort_func(sb: &Rc<Sb>, f: fn(&GameServerItem, &GameServerItem) -> i32) -> ListSortFunc {
    let sb = sb.clone();
    ListSortFunc(Rc::new(move |a: &ListItem<'_>, b: &ListItem<'_>| {
        let cur = sb.current_details.borrow();
        let Some(d) = cur.as_ref() else { return 0 };
        let d = d.borrow();
        match (d.get(&(a.user_data as i32)), d.get(&(b.user_data as i32))) {
            (None, Some(_)) => -1,
            (Some(_), None) => 1,
            (None, None) => 0,
            (Some(x), Some(y)) => f(x, y),
        }
    }))
}

fn desc<T: Ord>(a: T, b: T) -> i32 {
    match a.cmp(&b) {
        std::cmp::Ordering::Greater => -1,
        std::cmp::Ordering::Less => 1,
        std::cmp::Ordering::Equal => 0,
    }
}

fn stricmp(a: &str, b: &str) -> i32 {
    match a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

const SORTS: [fn(&GameServerItem, &GameServerItem) -> i32; 9] = [
    |a, b| desc(a.password, b.password),
    |a, b| desc(a.bot_players, b.bot_players),
    |a, b| desc(a.secure, b.secure),
    |a, b| {
        let r = desc(a.humans(), b.humans());
        if r != 0 { r } else { desc((a.max_players - a.bot_players).max(0), (b.max_players - b.bot_players).max(0)) }
    },
    |a, b| stricmp(&a.name(), &b.name()),
    |a, b| stricmp(&a.game_description, &b.game_description),
    |a, b| stricmp(&a.map, &b.map),
    |a, b| -desc(a.ping, b.ping),
    |a, b| -desc((a.ip, a.query_port), (b.ip, b.query_port)),
];

/// The `"Server"` row of a list (§5.5).
fn server_row(sb: &Sb, s: &GameServerItem, not_responding: Option<String>) -> Msg {
    let mut kv = Msg::new("Server")
        .str("name", &s.name())
        .str("map", &s.map)
        .str("GameDir", &s.game_dir)
        .str("GameDesc", &s.game_description)
        .str("GameTags", &s.game_tags)
        .int("password", i32::from(s.password));
    kv = if s.bot_players > 0 { kv.int("bots", s.bot_players) } else { kv.str("bots", "") };
    let secure = if s.secure { if sb.is_vac_banned() { 4 } else { 3 } } else { 0 };
    kv = kv
        .int("secure", secure)
        .str("IPAddr", &format!("{}:{}", ip_string(s.ip), s.conn_port))
        .str("Players", &format!("{} / {}", s.humans(), s.max_players))
        .int("Ping", s.ping);
    if let Some(t) = last_played_text(s) {
        kv = kv.str("LastPlayed", &t);
    }
    if let Some(nr) = not_responding {
        kv.set("Ping", Value::Str(String::new()));
        kv.set("GameDesc", Value::Str(nr));
        kv.set("Players", Value::Str(String::new()));
        kv.set("map", Value::Str(String::new()));
    }
    kv
}

/// `new CInternetGames(…)` / `CFavoriteGames` / … : the `CBaseGamesPage` constructor and the
/// subclass's.
pub fn create(ui: &mut Ui, sb: &Rc<Sb>, parent: PanelId, kind: PageKind, s: &mut dyn Surface) -> PanelId {
    let me = PropertyPage::create(ui, Some(parent), kind.name(), true, Box::new(NoHooks));
    let (w624, h278) = (ui.psv(me, 624), ui.psv(me, 278));
    ui.set_size(me, w624, h278);
    let connect = Button::create(ui, Some(me), "ConnectButton", "#ServerBrowser_Connect", None);
    ui.set_enabled(connect, false);
    let refresh_all = Button::create(ui, Some(me), "RefreshButton", "#ServerBrowser_Refresh", None);
    let refresh_quick = Button::create(ui, Some(me), "RefreshQuickButton", "#ServerBrowser_RefreshQuick", None);
    let add_server = Button::create(ui, Some(me), "AddServerButton", "#ServerBrowser_AddServer", None);
    let add_current_server = Button::create(ui, Some(me), "AddCurrentServerButton", "#ServerBrowser_AddCurrentServer", None);
    let game_list = ListPanel::create(ui, Some(me), "gamelist");
    let add_to_fav = Button::create(ui, Some(me), "AddToFavoritesButton", "", None);
    ui.set_enabled(add_to_fav, false);
    ui.set_visible(add_to_fav, false);
    let cols: [(&str, &str, i32, i32); 9] = [
        ("Password", "#ServerBrowser_Password", 16, 5),
        ("Bots", "#ServerBrowser_Bots", 20, 0),
        ("Secure", "#ServerBrowser_Secure", 16, 5),
        ("Players", "#ServerBrowser_Players", 55, 0),
        ("Name", "#ServerBrowser_Servers", 50, 0x12),
        ("GameDesc", "#ServerBrowser_Game", 112, 0),
        ("Map", "#ServerBrowser_Map", 90, 0),
        ("Ping", "#ServerBrowser_Latency", 55, 0),
        ("IPAddr", "#ServerBrowser_IPAddress", 64, COLUMN_HIDDEN),
    ];
    let widths: Vec<i32> = cols.iter().map(|c| ui.psv(me, c.2)).collect();
    with_list(ui, game_list, |l, ui| {
        l.set_allow_user_modification_of_columns(true);
        for (i, ((name, text, _, flags), w)) in cols.iter().zip(&widths).enumerate() {
            l.add_column_header(ui, game_list, i as i32, name, text, *w, *flags);
        }
        l.set_column_header_tooltip(ui, 0, "#ServerBrowser_PasswordColumn_Tooltip");
        l.set_column_header_tooltip(ui, 1, "#ServerBrowser_BotColumn_Tooltip");
        l.set_column_header_tooltip(ui, 2, "#ServerBrowser_SecureColumn_Tooltip");
        for (i, f) in SORTS.iter().enumerate() {
            l.set_sort_func(i as i32, sort_func(sb, *f));
        }
        l.set_sort_column(8);
    });
    // CreateFilters.
    let filter = ToggleButton::create(ui, Some(me), "Filter", "#ServerBrowser_Filter", ToggleKind::Toggle);
    let filter_string = Label::create(ui, Some(me), "FilterString", "");
    let game_filter = ComboBox::create(ui, Some(me), "GameFilter", 6, false);
    let map_filter = TextEntry::create(ui, Some(me), "MapFilter");
    let ping_filter = ComboBox::create(ui, Some(me), "PingFilter", 6, false);
    for t in ["#ServerBrowser_All", "#ServerBrowser_LessThan50", "#ServerBrowser_LessThan100", "#ServerBrowser_LessThan150", "#ServerBrowser_LessThan250", "#ServerBrowser_LessThan350", "#ServerBrowser_LessThan600"] {
        ComboBox::add_item(ui, ping_filter, t, None);
    }
    let secure_filter = ComboBox::create(ui, Some(me), "SecureFilter", 3, false);
    for t in ["#ServerBrowser_All", "#ServerBrowser_SecureOnly", "#ServerBrowser_InsecureOnly"] {
        ComboBox::add_item(ui, secure_filter, t, None);
    }
    let empty_filter = ToggleButton::create(ui, Some(me), "ServerEmptyFilterCheck", "", ToggleKind::Check);
    let full_filter = ToggleButton::create(ui, Some(me), "ServerFullFilterCheck", "", ToggleKind::Check);
    let no_password_filter = ToggleButton::create(ui, Some(me), "NoPasswordFilterCheck", "", ToggleKind::Check);
    let valid_steam_filter = ToggleButton::create(ui, Some(me), "ValidSteamAccountFilterCheck", "", ToggleKind::Check);
    ComboBox::add_item(ui, game_filter, "#ServerBrowser_All", Some(Msg::new("mod").str("gamedir", "").str("appid", "")));
    let combo_all_text = ui.localize.find("ServerBrowser_All").unwrap_or("").to_owned();
    let responder = sb.new_responder(me);
    let page = GamesPage {
        sb: sb.clone(),
        kind,
        responder,
        game_list,
        connect,
        refresh_all,
        refresh_quick,
        add_server,
        add_current_server,
        filter,
        filter_string,
        game_filter,
        map_filter,
        ping_filter,
        secure_filter,
        full_filter,
        empty_filter,
        no_password_filter,
        valid_steam_filter,
        map_servers: ServerMap::default(),
        map_server_ip: HashMap::new(),
        server_filters: Vec::new(),
        refresh_count: 0,
        request: 0,
        details: Rc::new(RefCell::new(HashMap::new())),
        combo_all_text,
        filters_visible: false,
        font: FontHandle(0),
        game_filter_text: String::new(),
        map_filter_text: String::new(),
        ping: 0,
        no_full: false,
        no_empty: false,
        no_password: false,
        valid_steam: false,
        secure: 0,
        limit_app_id: 0,
        last_sort: 0.0,
        dirty: false,
        require_update: true,
        any_retrieved: false,
        any_responded: false,
        offline: matches!(kind, PageKind::Internet | PageKind::Spectate | PageKind::History | PageKind::Friends) && sb.host.borrow().steam_offline_mode,
        refresh_on_list_reload: false,
        requesting: false,
        request_time: 0.0,
        auto_refresh: true,
    };
    ui.with_ctrl(me, |c, _| {
        if let Some(p) = c.as_any_mut().downcast_mut::<PropertyPage>() {
            p.hooks = Box::new(page);
        }
    });
    with_games_page(ui, me, |p, ui| p.load_layout(ui, me, s));
    with_games_page(ui, me, |p, ui| p.load_filter_settings(ui, me, s));
    let config = format!("Servers/Pages/{}", kind.name());
    ui.load_user_config(me, &config, 0);
    let version = ui.user_config_file_data(&format!("{config}/Extended"), 0).map_or(1, |e| e.get_int("Version", 0));
    if version < 1 {
        with_list(ui, game_list, |l, ui| {
            l.set_column_visible(ui, game_list, 1, true);
            l.set_column_visible(ui, game_list, 8, false);
        });
        if let Some(e) = ui.user_config_file_data(&format!("{config}/Extended"), 0) {
            e.set_int("Version", 1);
        }
    }
    match kind {
        PageKind::Internet | PageKind::Spectate => {
            with_games_page(ui, me, |p, ui| p.load_filter_settings(ui, me, s));
            ui.add_tick_signal(me, 1000);
        }
        PageKind::History => {
            let w = 100;
            with_list(ui, game_list, |l, ui| {
                l.add_column_header(ui, game_list, 9, "LastPlayed", "#ServerBrowser_LastPlayed", w, 0);
                l.set_sort_func(9, sort_func(sb, |a, b| desc(a.time_last_played, b.time_last_played)));
                l.set_sort_column(9);
            });
        }
        _ => {}
    }
    sb.pages.borrow_mut().push(me);
    me
}

impl GamesPage {
    fn dlg_panel(&self) -> Option<PanelId> {
        self.sb.dlg.borrow().panel
    }

    fn dlg_visible(&self, ui: &Ui) -> bool {
        self.dlg_panel().is_some_and(|d| ui.is_visible(d))
    }

    /// `GetActiveGameName()` (empty = none).
    fn active_game(&self) -> String {
        self.sb.dlg.borrow().game_name.clone()
    }

    /// The constructor's / `OnButtonToggled`'s layout load: the normal or the filter `.res`.
    fn load_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let (w, h) = ui.size(me);
        let (w624, h278) = (ui.psv(me, 624), ui.psv(me, 278));
        ui.set_size(me, w624, h278);
        let selected = with_toggle(ui, self.filter, |t, _| t.is_selected()).unwrap_or(false);
        self.filters_visible = selected;
        let file = if selected { "servers/InternetGamesPage_Filters.res" } else { "servers/InternetGamesPage.res" };
        load_res(ui, me, self.sb.fs.as_ref(), file, s);
        let filter = self.filter;
        let vis = self.filters_visible;
        with_toggle(ui, filter, |t, ui| t.set_selected(ui, filter, vis, s));
        self.update_game_filter(ui, s);
        if self.font != FontHandle(0) {
            let (gl, font) = (self.game_list, self.font);
            with_list(ui, gl, |l, _| l.set_font(font, s));
        }
        ui.set_size(me, w, h);
        ui.invalidate_layout(me);
    }

    /// `LoadFilterSettings` (§6.1).
    pub fn load_filter_settings(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let f = self.sb.dlg.borrow_mut().filter_data.find_or_create(self.kind.name()).clone();
        let active = self.active_game();
        if active.is_empty() {
            self.game_filter_text = f.get_string("game", "").chars().take(31).collect();
            self.limit_app_id = f.get_u64("appid", 0);
        } else {
            self.game_filter_text = active.chars().take(31).collect();
            self.limit_app_id = self.sb.dlg.borrow().limit_app_id;
        }
        self.map_filter_text = f.get_string("map", "").chars().take(31).collect();
        self.ping = f.get_int("ping", 0);
        self.no_full = f.get_int("NoFull", 0) != 0;
        self.no_empty = f.get_int("NoEmpty", 0) != 0;
        self.no_password = f.get_int("NoPassword", 0) != 0;
        self.valid_steam = f.get_int("ValidSteamAccount", 0) != 0;
        let sf = self.secure_filter;
        ComboBox::activate_item(ui, sf, f.get_int("Secure", 0), s);
        self.update_game_filter(ui, s);
        let (mf, text) = (self.map_filter, self.map_filter_text.clone());
        with_te(ui, mf, |te, ui| te.set_text(ui, mf, &text, s));
        if self.ping != 0 {
            let (pf, t) = (self.ping_filter, format!("< {}", self.ping));
            with_combo(ui, pf, |cb, ui| cb.te.set_text(ui, pf, &t, s));
        }
        for (b, v) in [(self.full_filter, self.no_full), (self.empty_filter, self.no_empty), (self.no_password_filter, self.no_password), (self.valid_steam_filter, self.valid_steam)] {
            with_toggle(ui, b, |t, ui| t.set_selected(ui, b, v, s));
        }
        self.update_filter_settings(ui, me, s);
    }

    /// `UpdateFilterSettings` (§6.2).
    fn update_filter_settings(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let active = self.active_game();
        if active.is_empty() {
            let kv = ComboBox::active_item_user_data(ui, self.game_filter);
            match kv.filter(|k| !k.get_str("gamedir").is_empty()) {
                Some(k) => {
                    self.game_filter_text = k.get_str("gamedir").chars().take(31).collect();
                    self.limit_app_id = if self.game_filter_text.is_empty() { 0 } else { u64::from(atoi(&k.get_str("appid")) as u32) };
                }
                None => {
                    self.limit_app_id = 0;
                    self.game_filter_text.clear();
                }
            }
            ui.set_enabled(self.game_filter, true);
        } else {
            self.game_filter_text = active.chars().take(31).collect();
            self.limit_app_id = self.sb.dlg.borrow().limit_app_id;
            self.update_filter_description(ui);
            self.update_game_filter(ui, s);
        }
        self.game_filter_text = self.game_filter_text.to_ascii_lowercase();
        self.map_filter_text = gs_common::bytestr::ansi_cap(&text_of(ui, self.map_filter), 32).to_ascii_lowercase();
        let ping_text = ComboBox::text(ui, self.ping_filter);
        self.ping = if ping_text.is_empty() { 0 } else { atoi(ping_text.get(2..).unwrap_or("")) };
        let sel = |ui: &mut Ui, b: PanelId| with_toggle(ui, b, |t, _| t.is_selected()).unwrap_or(false);
        self.no_full = sel(ui, self.full_filter);
        self.no_empty = sel(ui, self.empty_filter);
        self.no_password = sel(ui, self.no_password_filter);
        self.valid_steam = sel(ui, self.valid_steam_filter);
        self.secure = ComboBox::active_item(ui, self.secure_filter);
        self.server_filters.clear();
        if !self.game_filter_text.is_empty() && (self.limit_app_id & 0xff_ffff) != 1002 {
            self.server_filters.push(("gamedir".to_owned(), self.game_filter_text.clone()));
        }
        if self.no_empty {
            self.server_filters.push(("empty".to_owned(), "1".to_owned()));
        }
        if self.no_full {
            self.server_filters.push(("full".to_owned(), "1".to_owned()));
        }
        if self.secure == 1 {
            self.server_filters.push(("secure".to_owned(), "1".to_owned()));
        }
        {
            let mut d = self.sb.dlg.borrow_mut();
            let f = d.filter_data.find_or_create(self.kind.name());
            if active.is_empty() {
                f.set_string("game", &self.game_filter_text);
                f.set_u64("appid", self.limit_app_id);
            }
            f.set_string("map", &self.map_filter_text);
            f.set_int("ping", self.ping);
            f.set_int("NoFull", i32::from(self.no_full));
            f.set_int("NoEmpty", i32::from(self.no_empty));
            f.set_int("NoPassword", i32::from(self.no_password));
            f.set_int("ValidSteamAccount", i32::from(self.valid_steam));
            f.set_int("Secure", self.secure);
        }
        self.update_filter_description(ui);
        let _ = me;
    }

    /// `UpdateGameFilter` (§6.4).
    fn update_game_filter(&mut self, ui: &mut Ui, s: &mut dyn Surface) {
        let gf = self.game_filter;
        let n = ComboBox::item_count(ui, gf);
        let mut chosen = None;
        for i in 0..n {
            let kv = ComboBox::item_user_data(ui, gf, i).unwrap_or_else(|| Msg::new("mod"));
            let appid = u64::from(atoi(&kv.get_str("appID")) as u32);
            let dir = kv.get_str("gamedir");
            let app_ok = appid == self.limit_app_id || (self.limit_app_id & 0xff_ffff) == 0;
            let dir_ok = self.game_filter_text.is_empty() || (!dir.is_empty() && self.game_filter_text.starts_with(&dir));
            if app_ok && dir_ok {
                chosen = Some(i);
                break;
            }
        }
        match chosen {
            Some(i) => {
                if ComboBox::active_item(ui, gf) != i {
                    ComboBox::activate_item(ui, gf, i, s);
                }
            }
            None => {
                if ComboBox::active_item(ui, gf) != 0 {
                    ComboBox::activate_item(ui, gf, 0, s);
                }
            }
        }
        if !self.active_game().is_empty() {
            ui.set_enabled(gf, false);
            let name = self.sb.dlg.borrow().mod_dir.clone();
            with_combo(ui, gf, |cb, ui| cb.te.set_text(ui, gf, &name, s));
        }
    }

    /// `UpdateFilterDescription` (§6.5).
    fn update_filter_description(&mut self, ui: &mut Ui) {
        let find = |ui: &Ui, t: &str| ui.localize.find(t).unwrap_or("").to_owned();
        let mut d = String::new();
        if !self.game_filter_text.is_empty() {
            let active = !self.active_game().is_empty();
            if active {
                d.push_str(&self.sb.dlg.borrow().mod_dir);
            }
            d.push_str("; ");
        }
        match self.secure {
            1 => d.push_str(&(find(ui, "ServerBrowser_FilterDescSecureOnly") + "; ")),
            2 => d.push_str(&(find(ui, "ServerBrowser_FilterDescInsecureOnly") + "; ")),
            _ => {}
        }
        if self.ping != 0 {
            d.push_str(&format!("{} < {}; ", find(ui, "ServerBrowser_FilterDescLatency"), self.ping));
        }
        for (on, t) in [
            (self.no_full, "ServerBrowser_FilterDescNotFull"),
            (self.no_empty, "ServerBrowser_FilterDescNotEmpty"),
            (self.no_password, "ServerBrowser_FilterDescNoPassword"),
            (self.valid_steam, "ServerBrowser_FilterDescValidSteamAccount"),
        ] {
            if on {
                d.push_str(&(find(ui, t) + "; "));
            }
        }
        d.push_str(&self.map_filter_text);
        let fs = self.filter_string;
        with_label(ui, fs, |l, _| l.text.set_text_raw(&d));
        ui.invalidate_layout(fs);
    }

    /// `CheckPrimaryFilters`.
    fn check_primary(&self, s: &GameServerItem) -> bool {
        if !self.game_filter_text.is_empty() && (!s.game_dir.is_empty() || s.ping != 0) && !self.game_filter_text.eq_ignore_ascii_case(&s.game_dir) && s.app_id != 1002 {
            return false;
        }
        s.app_id < 200
    }

    /// `CheckSecondaryFilters`.
    fn check_secondary(&self, s: &GameServerItem) -> bool {
        if self.no_empty && s.players - s.bot_players < 1 {
            return false;
        }
        if self.no_full && s.players >= s.max_players {
            return false;
        }
        if self.ping != 0 && s.ping > self.ping {
            return false;
        }
        if self.no_password && s.password {
            return false;
        }
        if self.valid_steam && (s.steam_id & 0xffff_ffff) == 0 {
            return false;
        }
        if self.secure == 1 && !s.secure {
            return false;
        }
        if self.secure == 2 && s.secure {
            return false;
        }
        if !self.map_filter_text.is_empty() {
            let n = self.map_filter_text.len();
            let head: String = s.map.chars().take(n).collect();
            if !head.eq_ignore_ascii_case(&self.map_filter_text) {
                return false;
            }
        }
        true
    }

    /// `StartRefresh` (and `CLanGames`'s request time).
    pub fn start_refresh(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        if self.request != 0 {
            self.sb.mm.borrow_mut().release_request(self.request);
            self.request = 0;
        }
        self.map_servers.remove_all();
        self.map_server_ip.clear();
        self.details.borrow_mut().clear();
        let gl = self.game_list;
        with_list(ui, gl, |l, ui| l.delete_all_items(ui, gl));
        let app = (self.limit_app_id as u32) & 0xff_ffff;
        let filters = if self.kind == PageKind::Lan { Vec::new() } else { self.server_filters.clone() };
        self.request = self.sb.mm.borrow_mut().request_server_list(self.kind.list_kind(), app, &filters, self.responder);
        self.set_refreshing(ui, me, true);
        self.refresh_count = 0;
        if self.kind == PageKind::Lan {
            self.request_time = f64::from(ui.time_ms) / 1000.0;
        }
    }

    /// `StopRefresh`.
    pub fn stop_refresh(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        self.refresh_count = 0;
        self.sb.mm.borrow_mut().cancel_query(self.request);
        if self.kind == PageKind::Lan {
            self.requesting = false;
        }
    }

    fn is_refreshing(&self) -> bool {
        self.sb.mm.borrow().is_refreshing(self.request)
    }

    /// `SetRefreshing(state)`.
    fn set_refreshing(&mut self, ui: &mut Ui, _me: PanelId, state: bool) {
        let (gl, ra, rq) = (self.game_list, self.refresh_all, self.refresh_quick);
        if state {
            update_status_text(ui, &self.sb, "#ServerBrowser_RefreshingServerList");
            with_list(ui, gl, |l, ui| l.set_empty_list_text(ui, ""));
            with_button(ui, ra, |b, ui| {
                b.label.set_text(ui, "#ServerBrowser_StopRefreshingList");
                b.set_command("stoprefresh");
            });
            ui.set_enabled(rq, false);
        } else {
            update_status_text(ui, &self.sb, "");
            let text = if self.kind.get_new_list() { "#ServerBrowser_RefreshAll" } else { "#ServerBrowser_Refresh" };
            with_button(ui, ra, |b, ui| {
                b.label.set_text(ui, text);
                b.set_command("GetNewList");
            });
            let count = with_list(ui, gl, |l, _| l.item_count()).unwrap_or(0);
            ui.set_enabled(rq, count > 0);
        }
        ui.invalidate_layout(ra);
    }

    /// `GetNewServerList` (IGameList 2).
    fn get_new_server_list(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if self.kind == PageKind::Spectate {
            self.server_filters.push(("proxy".to_owned(), "1".to_owned()));
        }
        self.start_refresh(ui, me, s);
        if self.kind.internet_like() {
            self.update_status(ui);
            self.require_update = false;
            self.any_retrieved = false;
            self.any_responded = false;
            let gl = self.game_list;
            with_list(ui, gl, |l, ui| l.delete_all_items(ui, gl));
        }
    }

    /// `UpdateStatus`: the server count in the Name column's header.
    fn update_status(&mut self, ui: &mut Ui) {
        let gl = self.game_list;
        let count = with_list(ui, gl, |l, _| l.item_count()).unwrap_or(0);
        let text = if count > 1 {
            ui.localize.find("ServerBrowser_ServersCount").unwrap_or("").replacen("%s1", &count.to_string(), 1)
        } else {
            ui.localize.find("ServerBrowser_Servers").unwrap_or("").to_owned()
        };
        with_list(ui, gl, |l, ui| l.set_column_header_text(ui, 4, &text));
    }

    /// `UpdateServer(serverID, details)` (§5.10).
    fn update_server(&mut self, ui: &mut Ui, id: i32, p: &GameServerItem) {
        let gl = self.game_list;
        let key = (p.ip, p.conn_port);
        let slot = match self.map_servers.find(id) {
            Some(slot) => slot,
            None => {
                if let Some(old_id) = self.map_server_ip.remove(&key)
                    && let Some(old) = self.map_servers.find(old_id)
                {
                    let lid = self.map_servers.get(old).map_or(-1, |d| d.list_id);
                    with_list(ui, gl, |l, ui| {
                        if l.is_valid_item_id(lid) {
                            l.remove_item(ui, gl, lid);
                        }
                    });
                    self.map_servers.remove_at(old);
                }
                let slot = self.map_servers.insert(id, ServerDisplay { list_id: -1, server_id: -1, do_not_refresh: false, ip: 0, conn_port: 0, query_port: 0 });
                self.map_server_ip.insert(key, id);
                slot
            }
        };
        let (primary, secondary) = (self.check_primary(p), self.check_secondary(p));
        let Some(d) = self.map_servers.get_mut(slot) else { return };
        d.server_id = id;
        d.ip = p.ip;
        d.conn_port = p.conn_port;
        d.query_port = p.query_port;
        let mut hide = false;
        if !primary {
            d.do_not_refresh = true;
            hide = true;
        } else if !secondary {
            hide = true;
        }
        let list_id = d.list_id;
        let not_responding = d.do_not_refresh;
        if hide {
            let valid = with_list(ui, gl, |l, ui| {
                let v = l.is_valid_item_id(list_id);
                if v {
                    l.remove_item(ui, gl, list_id);
                }
                v
            });
            if valid == Some(true)
                && let Some(d) = self.map_servers.get_mut(slot)
            {
                d.list_id = -1;
            }
            return;
        }
        let nr = not_responding.then(|| ui.localize.find("ServerBrowser_NotResponding").unwrap_or("").to_owned());
        let row = server_row(&self.sb, p, nr);
        let new_id = with_list(ui, gl, |l, ui| {
            if l.is_valid_item_id(list_id) {
                if let Some(kv) = l.item_mut(list_id) {
                    for (k, v) in row.params {
                        kv.set(&k, v);
                    }
                }
                l.apply_item_changes(ui, gl, list_id);
                l.set_item_visible(ui, gl, list_id, true);
                list_id
            } else {
                l.add_item(ui, gl, &row, id as u32, false, false)
            }
        });
        if let Some(nid) = new_id
            && let Some(d) = self.map_servers.get_mut(slot)
        {
            d.list_id = nid;
        }
        self.update_status(ui);
        self.refresh_count += 1;
    }

    /// `RemoveServer(display)`.
    fn remove_server(&mut self, ui: &mut Ui, slot: usize) {
        let gl = self.game_list;
        let lid = self.map_servers.get(slot).map_or(-1, |d| d.list_id);
        with_list(ui, gl, |l, ui| {
            if l.is_valid_item_id(lid) {
                l.set_item_visible(ui, gl, lid, false);
            }
        });
        self.update_status(ui);
    }

    /// `ApplyGameFilters` (§5.11).
    fn apply_game_filters(&mut self, ui: &mut Ui, me: PanelId) {
        let gl = self.game_list;
        for slot in self.map_servers.live_slots() {
            let Some(d) = self.map_servers.get(slot).copied() else { continue };
            let Some(p) = self.details.borrow().get(&d.server_id).cloned() else { continue };
            if !self.check_primary(&p) || !self.check_secondary(&p) {
                if let Some(dm) = self.map_servers.get_mut(slot) {
                    dm.do_not_refresh = true;
                }
                with_list(ui, gl, |l, ui| {
                    if l.is_valid_item_id(d.list_id) {
                        l.set_item_visible(ui, gl, d.list_id, false);
                    }
                });
                continue;
            }
            if !d.do_not_refresh {
                continue;
            }
            // The pending-ping description token is missing: an empty GameDesc stays empty.
            let nr = p.do_not_refresh.then(|| ui.localize.find("ServerBrowser_NotResponding").unwrap_or("").to_owned());
            let row = server_row(&self.sb, &p, nr);
            let lid = with_list(ui, gl, |l, ui| {
                let lid = if l.is_valid_item_id(d.list_id) { d.list_id } else { l.add_item(ui, gl, &row, d.server_id as u32, false, false) };
                l.set_item_visible(ui, gl, lid, true);
                lid
            });
            if let Some(dm) = self.map_servers.get_mut(slot) {
                dm.do_not_refresh = false;
                if let Some(lid) = lid {
                    dm.list_id = lid;
                }
            }
        }
        self.update_status(ui);
        with_list(ui, gl, |l, ui| l.sort_list(ui, gl));
        ui.invalidate_layout(me);
    }

    /// The list response's `ServerResponded(hReq, iServer)` (shared by `ServerFailedToRespond`).
    fn base_server_responded(&mut self, ui: &mut Ui, request: ListRequest, server: i32) {
        if request != self.request {
            return;
        }
        let Some(p) = self.sb.mm.borrow().server_details(request, server) else { return };
        self.details.borrow_mut().insert(server, p.clone());
        if self.kind == PageKind::Friends {
            self.sb.mm.borrow_mut().ping_server(p.ip, p.conn_port, self.responder);
        } else {
            self.update_server(ui, server, &p);
        }
    }

    fn server_responded(&mut self, ui: &mut Ui, request: ListRequest, server: i32) {
        if self.kind.internet_like() {
            self.dirty = true;
            self.base_server_responded(ui, request, server);
            self.any_retrieved = true;
            self.any_responded = true;
        } else {
            self.base_server_responded(ui, request, server);
        }
    }

    fn server_failed(&mut self, ui: &mut Ui, request: ListRequest, server: i32) {
        match self.kind {
            PageKind::Internet | PageKind::Spectate => {
                self.dirty = true;
                let p = self.sb.mm.borrow().server_details(request, server);
                if p.is_some_and(|p| p.had_successful_response) {
                    self.server_responded(ui, request, server);
                } else if let Some(slot) = self.map_servers.find(server) {
                    self.remove_server(ui, slot);
                }
                self.refresh_count += 1;
            }
            PageKind::Lan => {
                if let Some(slot) = self.map_servers.find(server) {
                    self.remove_server(ui, slot);
                }
            }
            _ => self.base_server_responded(ui, request, server),
        }
    }

    fn refresh_complete(&mut self, ui: &mut Ui, me: PanelId, request: ListRequest, response: i32, s: &mut dyn Surface) {
        let gl = self.game_list;
        match self.kind {
            PageKind::Internet | PageKind::Spectate => {
                if request != self.request {
                    return;
                }
                self.set_refreshing(ui, me, false);
                self.update_filter_settings(ui, me, s);
                let text = if response == 1 {
                    "#ServerBrowser_MasterServerNotResponsive"
                } else if self.any_responded {
                    "#ServerBrowser_NoInternetGames"
                } else if response == 2 {
                    "#ServerBrowser_MasterServerHasNoServersListed"
                } else {
                    "#ServerBrowser_NoInternetGamesResponded"
                };
                with_list(ui, gl, |l, ui| l.set_empty_list_text(ui, text));
                self.dirty = false;
                self.last_sort = ui.time_ms as f32 / 1000.0;
                if ui.is_visible(me) {
                    with_list(ui, gl, |l, ui| l.sort_list(ui, gl));
                }
                self.update_status(ui);
            }
            PageKind::Favorites => {
                self.set_refreshing(ui, me, false);
                self.favorites_empty_text(ui);
                with_list(ui, gl, |l, ui| l.sort_list(ui, gl));
            }
            PageKind::History => {
                self.set_refreshing(ui, me, false);
                with_list(ui, gl, |l, ui| l.set_empty_list_text(ui, "#ServerBrowser_NoServersPlayed"));
                with_list(ui, gl, |l, ui| l.sort_list(ui, gl));
            }
            PageKind::Lan => {
                self.set_refreshing(ui, me, false);
                with_list(ui, gl, |l, ui| {
                    l.sort_list(ui, gl);
                    l.set_empty_list_text(ui, "#ServerBrowser_NoLanServers");
                });
            }
            PageKind::Friends => {
                self.set_refreshing(ui, me, false);
                with_list(ui, gl, |l, ui| l.sort_list(ui, gl));
                if !self.offline {
                    with_list(ui, gl, |l, ui| l.set_empty_list_text(ui, "#ServerBrowser_NoFriendsServers"));
                }
            }
        }
    }

    fn favorites_empty_text(&mut self, ui: &mut Ui) {
        let gl = self.game_list;
        let none = self.sb.mm.borrow().favorite_game_count() == 0;
        let text = if none { "#ServerBrowser_NoFavoriteServers" } else { "#ServerBrowser_NoInternetGamesResponded" };
        with_list(ui, gl, |l, ui| l.set_empty_list_text(ui, text));
    }

    /// `LoadFavoritesList`.
    pub fn load_favorites_list(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.favorites_empty_text(ui);
        if self.refresh_on_list_reload {
            self.refresh_on_list_reload = false;
            self.start_refresh(ui, me, s);
        }
    }

    /// `LoadHistoryList`.
    pub fn load_history_list(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if !self.offline {
            let gl = self.game_list;
            with_list(ui, gl, |l, ui| l.set_empty_list_text(ui, "#ServerBrowser_NoServersPlayed"));
        }
        if self.refresh_on_list_reload {
            self.refresh_on_list_reload = false;
            self.start_refresh(ui, me, s);
        }
    }

    /// `CFavoriteGames::OnConnectToGame` / `OnDisconnectFromGame`.
    pub fn set_connected(&mut self, ui: &mut Ui, state: bool) {
        ui.set_enabled(self.add_current_server, state);
    }

    /// A Steam response for this page.
    fn on_event(&mut self, ui: &mut Ui, me: PanelId, ev: &MmEvent, s: &mut dyn Surface) {
        match ev {
            MmEvent::ServerResponded { request, server, .. } => self.server_responded(ui, *request, *server),
            MmEvent::ServerFailedToRespond { request, server, .. } => self.server_failed(ui, *request, *server),
            MmEvent::RefreshComplete { request, response, .. } => self.refresh_complete(ui, me, *request, *response, s),
            MmEvent::PingResponded { server, .. } => {
                let mut id = -1;
                while self.map_servers.find(id).is_some() {
                    id -= 1;
                }
                self.update_server(ui, id, server);
            }
            MmEvent::FavoritesChanged { ip, query_port, conn_port, add, .. } => self.on_favorites_msg(ui, me, *ip, *query_port, *conn_port, *add, s),
            _ => {}
        }
    }

    /// `OnFavoritesMsg` (§5.12).
    #[allow(clippy::too_many_arguments)]
    fn on_favorites_msg(&mut self, ui: &mut Ui, me: PanelId, ip: u32, query_port: u32, conn_port: u32, add: bool, s: &mut dyn Surface) {
        let fav = matches!(self.kind, PageKind::Favorites | PageKind::History);
        if ip == 0 {
            let active = ui.parent(me).and_then(|sheet| gs_vgui::controls::property::with_sheet(ui, sheet, |sh, _| sh.active_page()).flatten());
            let sheet_visible = ui.parent(me).is_some_and(|p| ui.is_visible(p));
            if fav && active == Some(me) && sheet_visible && self.dlg_visible(ui) {
                self.get_new_server_list(ui, me, s);
            }
            return;
        }
        if !fav {
            return;
        }
        if add {
            self.sb.mm.borrow_mut().ping_server(ip, query_port as u16, self.responder);
        } else {
            if let Some(&id) = self.map_server_ip.get(&(ip, conn_port as u16))
                && let Some(slot) = self.map_servers.find(id)
            {
                let lid = self.map_servers.get(slot).map_or(-1, |d| d.list_id);
                let gl = self.game_list;
                with_list(ui, gl, |l, ui| {
                    if l.is_valid_item_id(lid) {
                        l.set_item_visible(ui, gl, lid, false);
                    }
                });
            }
            self.update_status(ui);
        }
    }

    /// The selected rows' user data (serverIDs), in selection order.
    fn selected_ids(&self, ui: &mut Ui) -> Vec<i32> {
        let gl = self.game_list;
        with_list(ui, gl, |l, _| (0..l.selected_items_count()).map(|i| l.item_user_data(l.selected_item(i)) as i32).collect()).unwrap_or_default()
    }

    /// `GetServer(id)`: (ip, queryPort, connPort) of the display entry.
    fn server_addr(&self, id: i32) -> Option<(u32, u16, u16)> {
        let slot = self.map_servers.find(id)?;
        self.map_servers.get(slot).map(|d| (d.ip, d.query_port, d.conn_port))
    }

    /// `OnBeginConnect` / `OnViewGameInfo`.
    fn begin_connect(&mut self, ui: &mut Ui, me: PanelId, connect: bool, s: &mut dyn Surface) {
        let Some(&id) = self.selected_ids(ui).first() else { return };
        if connect {
            self.stop_refresh(ui, me, s);
        }
        let Some(addr) = self.server_addr(id) else { return };
        if connect {
            dialog::join_game_from(ui, &self.sb, addr, s);
        } else {
            dialog::open_game_info(ui, &self.sb, addr, s);
        }
    }

    /// `OnOpenContextMenu(itemID)` of each page.
    fn open_context_menu(&mut self, ui: &mut Ui, me: PanelId) {
        let gl = self.game_list;
        let selected = with_list(ui, gl, |l, _| l.selected_items_count()).unwrap_or(0) > 0;
        let id = self.selected_ids(ui).first().copied().unwrap_or(-1);
        let show = |ui: &mut Ui, sb: &Sb, a: bool, b: bool, c: bool, d: bool, id: i32| -> Option<PanelId> {
            let m = dialog::get_context_menu(ui, sb, Some(gl))?;
            dialog::show_context_menu(ui, m, me, id, a, b, c, d);
            Some(m)
        };
        match self.kind {
            PageKind::Internet | PageKind::Spectate | PageKind::Friends => {
                if selected {
                    show(ui, &self.sb, true, true, true, true, id);
                }
            }
            PageKind::Lan => {
                if selected {
                    show(ui, &self.sb, true, true, true, false, id);
                }
            }
            PageKind::Favorites => {
                let m = if selected { show(ui, &self.sb, true, true, true, false, id) } else { show(ui, &self.sb, false, false, false, false, -1) };
                if let Some(m) = m {
                    if selected {
                        gs_vgui::controls::menu::Menu::add_item_msg(ui, m, "RemoveServer", "#ServerBrowser_RemoveServerFromFavorites", Msg::new("RemoveFromFavorites"), Some(me), None);
                    }
                    gs_vgui::controls::menu::Menu::add_item_msg(ui, m, "AddServerByName", "#ServerBrowser_AddServerByIP", Msg::new("AddServerByName"), Some(me), None);
                }
            }
            PageKind::History => {
                if selected {
                    if let Some(m) = show(ui, &self.sb, true, true, true, true, id) {
                        gs_vgui::controls::menu::Menu::add_item_msg(ui, m, "RemoveServer", "#ServerBrowser_RemoveServerFromHistory", Msg::new("RemoveFromHistory"), Some(me), None);
                    }
                } else {
                    show(ui, &self.sb, false, false, false, false, -1);
                }
            }
        }
    }

    /// `OnCommand` of the page and its subclass.
    fn on_command(&mut self, ui: &mut Ui, me: PanelId, cmd: &str, s: &mut dyn Surface) -> bool {
        if self.kind == PageKind::Favorites {
            if cmd.eq_ignore_ascii_case("AddServerByName") {
                self.add_server_by_name(ui, me, s);
                return true;
            }
            if cmd.eq_ignore_ascii_case("AddCurrentServer") {
                let (connected, cur) = {
                    let d = self.sb.dlg.borrow();
                    (d.currently_connected, d.current_connection.clone())
                };
                if connected {
                    let now = chrono::Utc::now().timestamp() as u32;
                    self.sb.mm.borrow_mut().add_favorite_game(cur.app_id, cur.ip.swap_bytes(), cur.conn_port, cur.query_port, 1, now);
                    self.refresh_on_list_reload = true;
                }
                return true;
            }
        }
        if cmd.eq_ignore_ascii_case("Connect") {
            self.begin_connect(ui, me, true, s);
        } else if cmd.eq_ignore_ascii_case("stoprefresh") {
            self.stop_refresh(ui, me, s);
        } else if cmd.eq_ignore_ascii_case("refresh") {
            self.sb.mm.borrow_mut().refresh_query(self.request);
            self.set_refreshing(ui, me, true);
            self.refresh_count = 0;
        } else if cmd.eq_ignore_ascii_case("GetNewList") {
            self.get_new_server_list(ui, me, s);
        }
        true
    }

    /// `CFavoriteGames::OnAddServerByName`.
    fn add_server_by_name(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let Some(dlg) = self.dlg_panel() else { return };
        small::create_add_server(ui, &self.sb, dlg, me, s);
    }

    /// `OnTextChanged(panel, text)`.
    fn on_text_changed(&mut self, ui: &mut Ui, me: PanelId, panel: Option<PanelId>, text: &str, s: &mut dyn Surface) {
        if let Some(p) = panel
            && text.eq_ignore_ascii_case(&self.combo_all_text)
            && ui.ctrl::<ComboBox>(p).is_some()
        {
            with_combo(ui, p, |cb, ui| cb.te.set_text(ui, p, "", s));
        }
        self.update_filter_settings(ui, me, s);
        self.apply_game_filters(ui, me);
        if self.filters_visible && panel == Some(self.game_filter) && self.dlg_visible(ui) {
            self.stop_refresh(ui, me, s);
            self.get_new_server_list(ui, me, s);
        }
    }

    /// `CInternetGames::OnTick` (every second): the throttled resort.
    fn on_tick(&mut self, ui: &mut Ui) {
        if self.offline || !self.kind.internet_like() {
            return;
        }
        let now = ui.time_ms as f32 / 1000.0;
        if self.dirty && now - self.last_sort >= 1.5 {
            if !ui.is_mouse_down(MOUSE_LEFT) && !ui.is_mouse_down(MOUSE_RIGHT) {
                self.dirty = false;
                self.last_sort = now;
                let gl = self.game_list;
                with_list(ui, gl, |l, ui| l.sort_list(ui, gl));
            } else {
                self.last_sort = now - 1.5 + 1.0;
            }
        }
    }
}

impl PageHooks for GamesPage {
    fn on_page_show(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        *self.sb.current_details.borrow_mut() = Some(self.details.clone());
        match self.kind {
            PageKind::Internet | PageKind::Spectate => {
                let empty = with_list(ui, self.game_list, |l, _| l.item_count()).unwrap_or(0) == 0;
                if empty && self.dlg_visible(ui) {
                    self.start_refresh(ui, me, s);
                }
            }
            PageKind::Lan => {
                if self.auto_refresh {
                    self.start_refresh(ui, me, s);
                }
            }
            _ => self.start_refresh(ui, me, s),
        }
    }

    fn on_page_hide(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.stop_refresh(ui, me, s);
    }

    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) -> bool {
        if msg.is("Command") {
            return self.on_command(ui, me, &msg.get_str("command"), s);
        }
        if msg.is("Tick") {
            self.on_tick(ui);
            return true;
        }
        if msg.is("KeyCodePressed") && msg.get_int("code") == KEY_F5 {
            self.start_refresh(ui, me, s);
            return true;
        }
        if msg.is("ItemSelected") {
            let n = with_list(ui, self.game_list, |l, _| l.selected_items_count()).unwrap_or(0);
            ui.set_enabled(self.connect, n >= 1);
            return true;
        }
        if msg.is("ConnectToServer") {
            self.begin_connect(ui, me, true, s);
            return true;
        }
        if msg.is("ViewGameInfo") {
            self.begin_connect(ui, me, false, s);
            return true;
        }
        if msg.is("RefreshServer") {
            for id in self.selected_ids(ui) {
                self.sb.mm.borrow_mut().refresh_server(self.request, id);
            }
            let r = self.is_refreshing();
            self.set_refreshing(ui, me, r);
            if self.kind.internet_like() {
                update_status_text(ui, &self.sb, "#ServerBrowser_GettingNewServerList");
            }
            return true;
        }
        if msg.is("AddToFavorites") {
            for id in self.selected_ids(ui) {
                let p = self.details.borrow().get(&id).cloned();
                if let Some(p) = p {
                    dialog::add_server_to_favorites(&self.sb, &p);
                }
            }
            return true;
        }
        if msg.is("GetNewServerList") && self.kind.internet_like() {
            self.get_new_server_list(ui, me, s);
            return true;
        }
        if msg.is("OpenContextMenu") {
            self.open_context_menu(ui, me);
            return true;
        }
        if msg.is("RemoveFromFavorites") && self.kind == PageKind::Favorites {
            for id in self.selected_ids(ui) {
                let p = self.details.borrow().get(&id).cloned();
                if let Some(p) = p {
                    self.sb.mm.borrow_mut().remove_favorite_game(p.app_id, p.ip, p.conn_port, p.query_port, 1);
                }
            }
            self.update_status(ui);
            ui.invalidate_layout(me);
            return true;
        }
        if msg.is("RemoveFromHistory") && self.kind == PageKind::History {
            for id in self.selected_ids(ui).into_iter().rev() {
                let p = self.details.borrow().get(&id).cloned();
                if let Some(p) = p {
                    self.sb.mm.borrow_mut().remove_favorite_game(p.app_id, p.ip, p.conn_port, p.query_port, 2);
                }
            }
            self.update_status(ui);
            ui.invalidate_layout(me);
            return true;
        }
        if msg.is("AddServerByName") && self.kind == PageKind::Favorites {
            self.add_server_by_name(ui, me, s);
            return true;
        }
        if msg.is("ButtonToggled") {
            let panel = msg.get_panel("panel");
            if panel == Some(self.filter) {
                self.load_layout(ui, me, s);
            } else if [self.full_filter, self.empty_filter, self.no_password_filter, self.valid_steam_filter].iter().any(|&b| Some(b) == panel) {
                self.on_text_changed(ui, me, panel, "", s);
            }
            return true;
        }
        if msg.is("TextChanged") {
            let panel = msg.get_panel("panel");
            self.on_text_changed(ui, me, panel, &msg.get_str("text"), s);
            return true;
        }
        false
    }

    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        let mut images = vec![None];
        for name in [
            "servers/icon_password",
            "servers/icon_bots",
            "servers/icon_robotron",
            "servers/icon_secure_deny",
            "servers/icon_password_column",
            "servers/icon_bots_column",
            "servers/icon_robotron_column",
        ] {
            images.push(s.texture(name, false));
        }
        let (prop, hd) = (ui.is_proportional(me), ui.is_hd_proportional(me));
        let mut font = s.font(&scheme.file, "ListSmall", prop, hd);
        if font == FontHandle(0) {
            font = s.font(&scheme.file, "DefaultSmall", prop, hd);
        }
        self.font = font;
        let gl = self.game_list;
        with_list(ui, gl, |l, ui| {
            l.set_image_list(images);
            l.set_font(font, s);
            l.set_column_header_image(ui, 0, 5);
            l.set_column_header_image(ui, 1, 6);
            l.set_column_header_image(ui, 2, 7);
        });
    }

    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if self.kind.internet_like() && !self.offline && self.require_update && self.dlg_visible(ui) {
            ui.post(me, Msg::new("GetNewServerList"), Some(me), 0.1);
            self.require_update = false;
        }
        if self.offline {
            let gl = self.game_list;
            with_list(ui, gl, |l, ui| l.set_empty_list_text(ui, "#ServerBrowser_OfflineMode"));
            for b in [self.connect, self.refresh_all, self.refresh_quick, self.add_server, self.filter] {
                ui.set_enabled(b, false);
            }
        }
        // CBaseGamesPage::PerformLayout.
        let gl = self.game_list;
        let (selected, count) = with_list(ui, gl, |l, _| (l.selected_items_count(), l.item_count())).unwrap_or((0, 0));
        ui.set_enabled(self.connect, selected >= 1);
        let (ra, rq) = (self.refresh_all, self.refresh_quick);
        if self.kind.get_new_list() {
            ui.set_visible(rq, true);
            with_button(ui, ra, |b, ui| b.label.set_text(ui, "#ServerBrowser_RefreshAll"));
        } else {
            ui.set_visible(rq, false);
            with_button(ui, ra, |b, ui| b.label.set_text(ui, "#ServerBrowser_Refresh"));
        }
        ui.set_visible(self.add_server, self.kind.add_server());
        ui.set_visible(self.add_current_server, self.kind.add_server());
        if self.is_refreshing() {
            with_button(ui, ra, |b, ui| b.label.set_text(ui, "#ServerBrowser_StopRefreshingList"));
        }
        ui.set_enabled(rq, count > 0);
        ui.invalidate_layout(ra);
        let _ = s;
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
