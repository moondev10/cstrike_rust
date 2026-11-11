//! ServerBrowser.dll (docs/re/serverbrowser.md): the only platform module GameUI loads, opened
//! by "Find servers". Discovery, pings and player lists go through Steam's
//! `ISteamMatchmakingServers` ([`Matchmaking`], implemented by the host over the Steam client, or
//! by gs-serverquery without one, §8.5); the module never opens a socket itself.

pub mod dialog;
pub mod gameinfo;
pub mod page;
pub mod small;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gs_vgui::keyvalues::KeyValues;
use gs_vgui::{Msg, PanelId, Surface, Ui};

use crate::{Fs, Shared};

/// The response object a request reports to (one per page or dialog).
pub type Responder = u32;
/// `HServerListRequest` (0 = none).
pub type ListRequest = u64;
/// `HServerQuery` (−1 = none).
pub type ServerQuery = i32;

/// `EMatchMakingType` (`CBaseGamesPage+0x194`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    Internet = 0,
    Lan = 1,
    Friends = 2,
    Favorites = 3,
    History = 4,
    Spectator = 5,
}

/// `gameserveritem_t` (§3.1); addresses and ports in host order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GameServerItem {
    pub conn_port: u16,
    pub query_port: u16,
    pub ip: u32,
    pub ping: i32,
    pub had_successful_response: bool,
    pub do_not_refresh: bool,
    pub game_dir: String,
    pub map: String,
    pub game_description: String,
    pub app_id: u32,
    pub players: i32,
    pub max_players: i32,
    pub bot_players: i32,
    pub password: bool,
    pub secure: bool,
    pub time_last_played: u32,
    pub server_version: i32,
    pub server_name: String,
    pub game_tags: String,
    pub steam_id: u64,
}

impl GameServerItem {
    /// `GetName`: the server name, or `"a.b.c.d:connPort"` when it is empty.
    pub fn name(&self) -> String {
        if self.server_name.is_empty() { format!("{}:{}", ip_string(self.ip), self.conn_port) } else { self.server_name.clone() }
    }

    /// `max(players - bots, 0)`.
    pub fn humans(&self) -> i32 {
        (self.players - self.bot_players).max(0)
    }
}

/// `"%u.%u.%u.%u"` of a host-order address.
pub fn ip_string(ip: u32) -> String {
    let b = ip.to_be_bytes();
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

/// What Steam reports from `SteamAPI_RunCallbacks`, in delivery order.
#[derive(Debug, Clone, PartialEq)]
pub enum MmEvent {
    /// `ISteamMatchmakingServerListResponse::ServerResponded(hRequest, iServer)`.
    ServerResponded { responder: Responder, request: ListRequest, server: i32 },
    /// `…::ServerFailedToRespond(hRequest, iServer)`.
    ServerFailedToRespond { responder: Responder, request: ListRequest, server: i32 },
    /// `…::RefreshComplete(hRequest, response)` (`EMatchMakingServerResponse`).
    RefreshComplete { responder: Responder, request: ListRequest, response: i32 },
    /// `ISteamMatchmakingPingResponse::ServerResponded(server)`.
    PingResponded { responder: Responder, server: GameServerItem },
    /// `…::ServerFailedToRespond()`.
    PingFailed { responder: Responder },
    /// `ISteamMatchmakingPlayersResponse::AddPlayerToList(name, score, timePlayed)`.
    AddPlayer { responder: Responder, name: String, score: i32, time: f32 },
    /// `…::PlayersFailedToRespond()`.
    PlayersFailed { responder: Responder },
    /// `…::PlayersRefreshComplete()`.
    PlayersComplete { responder: Responder },
    /// `FavoritesListChanged_t` (callback 502).
    FavoritesChanged { ip: u32, query_port: u32, conn_port: u32, app_id: u32, flags: u32, add: bool },
}

/// The Steam calls ServerBrowser.dll makes (`SteamMatchMakingServers002`, `SteamMatchMaking009`,
/// `SteamApps`), §8.
pub trait Matchmaking {
    /// `Request{Internet,LAN,Friends,Favorites,History,Spectator}ServerList(app, filters, n, resp)`
    /// (the LAN request takes no filters).
    fn request_server_list(&mut self, kind: ListKind, app: u32, filters: &[(String, String)], responder: Responder) -> ListRequest;
    fn release_request(&mut self, request: ListRequest);
    /// `GetServerDetails(hRequest, iServer)` (`None` = NULL).
    fn server_details(&self, request: ListRequest, server: i32) -> Option<GameServerItem>;
    fn cancel_query(&mut self, request: ListRequest);
    fn refresh_query(&mut self, request: ListRequest);
    fn is_refreshing(&self, request: ListRequest) -> bool;
    fn refresh_server(&mut self, request: ListRequest, server: i32);
    /// `PingServer(ip, port, resp)` (host-order ip).
    fn ping_server(&mut self, ip: u32, port: u16, responder: Responder) -> ServerQuery;
    /// `PlayerDetails(ip, port, resp)`.
    fn player_details(&mut self, ip: u32, port: u16, responder: Responder) -> ServerQuery;
    fn cancel_server_query(&mut self, query: ServerQuery);
    /// The response objects of `responder` are no longer referenced by any query.
    fn release_responder(&mut self, responder: Responder);
    fn favorite_game_count(&self) -> i32;
    fn add_favorite_game(&mut self, app: u32, ip: u32, conn_port: u16, query_port: u16, flags: u32, time: u32) -> i32;
    fn remove_favorite_game(&mut self, app: u32, ip: u32, conn_port: u16, query_port: u16, flags: u32) -> bool;
    /// `SteamApps()->BIsVACBanned()`.
    fn is_vac_banned(&self) -> bool;
    /// `SteamAPI_RunCallbacks`: the responses delivered meanwhile.
    fn run_callbacks(&mut self) -> Vec<MmEvent>;
}

/// The server details a page's sort functions read (`GetServerDetails(s_pCurrentPage->m_hRequest, id)`).
pub type Details = Rc<RefCell<HashMap<i32, GameServerItem>>>;

/// `CServerBrowserDialog`'s state that its pages and dialogs read back.
#[derive(Default)]
pub struct DlgState {
    pub panel: Option<PanelId>,
    /// `m_GameInfoDialogs` (PHandles).
    pub game_info_dialogs: Vec<PanelId>,
    /// `m_pGameList`: the active page.
    pub game_list: Option<PanelId>,
    pub status_label: Option<PanelId>,
    pub tab_panel: Option<PanelId>,
    pub favorites: Option<PanelId>,
    pub history: Option<PanelId>,
    pub internet: Option<PanelId>,
    pub spectate: Option<PanelId>,
    pub lan: Option<PanelId>,
    pub friends: Option<PanelId>,
    pub saved_data: KeyValues,
    pub filter_data: KeyValues,
    pub context_menu: Option<PanelId>,
    /// `m_szModDir`: GameUI's "game" ("Counter-Strike").
    pub mod_dir: String,
    /// `m_szGameName`: GameUI's "name" ("cstrike").
    pub game_name: String,
    pub limit_app_id: u64,
    pub currently_connected: bool,
    pub current_connection: GameServerItem,
}

/// The module's shared state: Steam, the engine, the dialog and the response routing.
pub struct Sb {
    pub mm: RefCell<Box<dyn Matchmaking>>,
    pub host: Shared,
    pub fs: Fs,
    pub dlg: RefCell<DlgState>,
    /// `s_pCurrentPage`'s server details.
    pub current_details: RefCell<Option<Details>>,
    routes: RefCell<Vec<(Responder, PanelId)>>,
    next_responder: Cell<Responder>,
    /// The pages in `FavoritesListChanged_t` registration order.
    pub pages: RefCell<Vec<PanelId>>,
}

impl Sb {
    /// A response object for `panel`.
    pub fn new_responder(&self, panel: PanelId) -> Responder {
        let r = self.next_responder.get() + 1;
        self.next_responder.set(r);
        self.routes.borrow_mut().push((r, panel));
        r
    }

    /// The panel's response objects go (its queries are cancelled).
    pub fn drop_responder(&self, r: Responder) {
        self.routes.borrow_mut().retain(|&(x, _)| x != r);
        self.mm.borrow_mut().release_responder(r);
    }

    fn route(&self, r: Responder) -> Option<PanelId> {
        self.routes.borrow().iter().find(|&&(x, _)| x == r).map(|&(_, p)| p)
    }

    /// `IRunGameEngine::AddTextCommand` = `ClientCmd`.
    pub fn add_text_command(&self, cmd: &str) {
        self.host.borrow_mut().client_cmd(cmd);
    }

    /// `IsVACBannedFromGame(appid)`: `SteamApps() && BIsVACBanned()` (the app id is ignored).
    pub fn is_vac_banned(&self) -> bool {
        self.mm.borrow().is_vac_banned()
    }
}

/// `CServerBrowser` + its `IVGuiModule`.
pub struct ServerBrowser {
    pub sb: Rc<Sb>,
    /// `s_bFirstActivate`.
    first_activate: bool,
}

impl ServerBrowser {
    /// `Initialize` (the localization file) and `PostInitialize` (`CreateDialog`, hidden),
    /// then `SetParent(taskbar)`.
    pub fn initialize(ui: &mut Ui, host: Shared, fs: Fs, mm: Box<dyn Matchmaking>, parent: PanelId, s: &mut dyn Surface) -> Self {
        for file in fs.language_files("servers/serverbrowser_%language%.txt") {
            match fs.read(&file) {
                Some(data) => ui.localize.add_file(&data),
                None => log::debug!("{file}: missing"),
            }
        }
        let sb = Rc::new(Sb {
            mm: RefCell::new(mm),
            host,
            fs,
            dlg: RefCell::new(DlgState::default()),
            current_details: RefCell::new(None),
            routes: RefCell::new(Vec::new()),
            next_responder: Cell::new(0),
            pages: RefCell::new(Vec::new()),
        });
        let d = dialog::create(ui, &sb, parent, s);
        ui.set_visible(d, false);
        Self { sb, first_activate: true }
    }

    /// `IVGuiModule::GetPanel`.
    pub fn panel(&self) -> Option<PanelId> {
        self.sb.dlg.borrow().panel
    }

    /// `IServerBrowser::Activate`: the first time `LoadUserData`, then `Open`.
    pub fn activate(&mut self, ui: &mut Ui, s: &mut dyn Surface) {
        if self.first_activate {
            dialog::load_user_data(ui, &self.sb, s);
        }
        self.first_activate = false;
        dialog::open(ui, &self.sb);
    }

    /// `IVGuiModule::Deactivate`: `SaveUserData`.
    pub fn deactivate(&mut self, ui: &mut Ui) {
        if self.panel().is_some() {
            dialog::save_user_data(ui, &self.sb);
        }
    }

    /// `IVGuiModule::Shutdown`: `Close`, then `MarkForDeletion`.
    pub fn shutdown(&mut self, ui: &mut Ui, s: &mut dyn Surface) {
        if let Some(d) = self.panel() {
            ui.send(d, &Msg::new("Close"), None, s);
            ui.mark_for_deletion(d);
        }
    }

    /// `IVGuiModule::RunFrame` = `SteamAPI_RunCallbacks`: every response goes to its page or dialog.
    pub fn run_frame(&mut self, ui: &mut Ui, s: &mut dyn Surface) {
        let events = self.sb.mm.borrow_mut().run_callbacks();
        for ev in events {
            match &ev {
                MmEvent::FavoritesChanged { .. } => {
                    let pages = self.sb.pages.borrow().clone();
                    for p in pages {
                        page::on_event(ui, p, &ev, s);
                    }
                }
                MmEvent::ServerResponded { responder, .. }
                | MmEvent::ServerFailedToRespond { responder, .. }
                | MmEvent::RefreshComplete { responder, .. }
                | MmEvent::PingResponded { responder, .. }
                | MmEvent::PingFailed { responder }
                | MmEvent::AddPlayer { responder, .. }
                | MmEvent::PlayersFailed { responder }
                | MmEvent::PlayersComplete { responder } => {
                    let Some(p) = self.sb.route(*responder) else { continue };
                    if !ui.exists(p) {
                        continue;
                    }
                    if !page::on_event(ui, p, &ev, s) && !gameinfo::on_event(ui, p, &ev, s) {
                        small::on_event(ui, p, &ev, s);
                    }
                }
            }
        }
    }
}

/// `_localtime64` + `strftime("%a %d %b %I:%M%p")` + `_strlwr` of the last 4 characters; the
/// 64-bit time is `m_ulTimeLastPlayed` with `m_nServerVersion` as its high half (§5.5 quirk).
pub fn last_played_text(s: &GameServerItem) -> Option<String> {
    use chrono::TimeZone;
    if s.time_last_played == 0 {
        return None;
    }
    let t = i64::from(s.time_last_played) | (i64::from(s.server_version) << 32);
    if !(0..=32_535_215_999).contains(&t) {
        return None;
    }
    let local = chrono::Local.timestamp_opt(t, 0).single()?;
    let mut text = local.format("%a %d %b %I:%M%p").to_string();
    let n = text.len();
    if n >= 4 {
        let tail = text[n - 4..].to_ascii_lowercase();
        text.replace_range(n - 4.., &tail);
    }
    Some(text)
}
