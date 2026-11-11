//! `CDialogGameInfo` (docs/re/serverbrowser.md §10): one server's details and players, re-pinged
//! every 10 s while open, with Auto-Retry, and the connection itself (§10.7: `password "…"` and
//! `connect a.b.c.d:port` to the engine).

use std::any::Any;
use std::rc::Rc;

use gs_vgui::controls::button::{Button, with_button};
use gs_vgui::controls::frame::{FrameMsg, FrameState, with_frame};
use gs_vgui::controls::label::{Label, with_label};
use gs_vgui::controls::list_panel::{ListItem, ListPanel, ListSortFunc, with_list};
use gs_vgui::controls::text_entry::with_te;
use gs_vgui::controls::toggle::{ToggleButton, ToggleKind, with_toggle};
use gs_vgui::scheme::Scheme;
use gs_vgui::{Control, Msg, PanelId, Surface, Ui};

use super::{GameServerItem, MmEvent, Responder, Sb, ServerQuery, ip_string, small};
use crate::options::{load_res, set_hd_scaled};

pub struct GameInfoDialog {
    pub fs: FrameState,
    sb: Rc<Sb>,
    responder: Responder,
    /// `m_iRequestRetry` (system ms; 0 = none).
    request_retry: u32,
    connect: PanelId,
    info_label: PanelId,
    auto_retry: PanelId,
    auto_retry_alert: PanelId,
    auto_retry_join: PanelId,
    player_list: PanelId,
    connecting: bool,
    password: String,
    server_not_responding: bool,
    server_full: bool,
    show_auto_retry_toggle: bool,
    server: GameServerItem,
    ping_query: ServerQuery,
    players_query: ServerQuery,
    /// The next `AddPlayerToList` clears the list.
    player_list_up_to_date: bool,
}

fn ctl<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut GameInfoDialog, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<GameInfoDialog>().map(|d| f(d, ui))).flatten()
}

/// A Steam response for the dialog `panel`; `false` when it is not one.
pub fn on_event(ui: &mut Ui, panel: PanelId, ev: &MmEvent, s: &mut dyn Surface) -> bool {
    ctl(ui, panel, |d, ui| d.on_event(ui, panel, ev, s)).is_some()
}

/// `new CDialogGameInfo(NULL, ip, queryPort, connPort)` with `SetParent(browser's parent)`.
pub fn create(ui: &mut Ui, sb: &Rc<Sb>, ip: u32, query_port: u16, conn_port: u16, s: &mut dyn Surface) -> PanelId {
    let parent = sb.dlg.borrow().panel.and_then(|d| ui.parent(d));
    let ph = ui.embedded();
    let me = ui.create(
        parent,
        "DialogGameInfo",
        Box::new(GameInfoDialog {
            fs: FrameState::default(),
            sb: sb.clone(),
            responder: 0,
            request_retry: 0,
            connect: ph,
            info_label: ph,
            auto_retry: ph,
            auto_retry_alert: ph,
            auto_retry_join: ph,
            player_list: ph,
            connecting: false,
            password: String::new(),
            server_not_responding: false,
            server_full: false,
            show_auto_retry_toggle: false,
            server: GameServerItem::default(),
            ping_query: -1,
            players_query: -1,
            player_list_up_to_date: false,
        }),
    );
    FrameState::init(ui, me, true);
    set_hd_scaled(ui, me);
    let (w512, mw, mh) = (ui.psv(me, 512), ui.psv(me, 416), ui.psv(me, 340));
    ui.set_bounds(me, 0, 0, w512, w512);
    ui.set_min_size(me, mw, mh);
    with_frame(ui, me, |f, _| f.delete_self_on_close = true);
    let connect = Button::create(ui, Some(me), "Connect", "#ServerBrowser_JoinGame", None);
    let close = Button::create(ui, Some(me), "Close", "#ServerBrowser_Close", None);
    let refresh = Button::create(ui, Some(me), "Refresh", "#ServerBrowser_Refresh", None);
    let info_label = Label::create(ui, Some(me), "InfoLabel", "");
    let auto_retry = ToggleButton::create(ui, Some(me), "AutoRetry", "#ServerBrowser_AutoRetry", ToggleKind::Toggle);
    ui.add_action_signal_target(auto_retry, me);
    let auto_retry_alert = ToggleButton::create(ui, Some(me), "AutoRetryAlert", "#ServerBrowser_AlertMeWhenSlotOpens", ToggleKind::Radio);
    let auto_retry_join = ToggleButton::create(ui, Some(me), "AutoRetryJoin", "#ServerBrowser_JoinWhenSlotOpens", ToggleKind::Radio);
    let player_list = ListPanel::create(ui, Some(me), "PlayerList");
    let (c156, c64) = (ui.psv(me, 156), ui.psv(me, 64));
    with_list(ui, player_list, |l, ui| {
        l.add_column_header(ui, player_list, 0, "PlayerName", "#ServerBrowser_PlayerName", c156, 0);
        l.add_column_header(ui, player_list, 1, "Score", "#ServerBrowser_Score", c64, 0);
        l.add_column_header(ui, player_list, 2, "Time", "#ServerBrowser_Time", c64, 0);
        l.set_sort_func(2, ListSortFunc(Rc::new(|a: &ListItem<'_>, b: &ListItem<'_>| {
            let (x, y) = (a.kv.get_int("TimeSec"), b.kv.get_int("TimeSec"));
            if x > y {
                -1
            } else {
                i32::from(x < y)
            }
        })));
    });
    ui.post(player_list, Msg::new("SetSortColumn").int("column", 2), None, 0.0);
    ui.post(player_list, Msg::new("SetSortColumn").int("column", 1), None, 0.0);
    ui.post(player_list, Msg::new("SetSortColumn").int("column", 1), None, 0.0);
    with_toggle(ui, auto_retry_alert, |t, ui| t.set_selected(ui, auto_retry_alert, true, s));
    for (b, cmd) in [(connect, "Connect"), (close, "Close"), (refresh, "Refresh")] {
        with_button(ui, b, |st, _| st.command = Some(Msg::new(cmd)));
    }
    let responder = sb.new_responder(me);
    ctl(ui, me, |d, ui| {
        d.responder = responder;
        d.connect = connect;
        d.info_label = info_label;
        d.auto_retry = auto_retry;
        d.auto_retry_alert = auto_retry_alert;
        d.auto_retry_join = auto_retry_join;
        d.player_list = player_list;
        d.server.ip = ip;
        d.server.query_port = query_port;
        d.server.conn_port = conn_port;
        d.request_info(ui);
    });
    ui.add_tick_signal(me, 0);
    load_res(ui, me, sb.fs.as_ref(), "Servers/DialogGameInfo.res", s);
    FrameState::move_to_center_of_screen(ui, me);
    me
}

/// `SetTitleAndActivate(name)`.
pub fn set_title_and_activate(ui: &mut Ui, me: PanelId, name: &str, _s: &mut dyn Surface) {
    with_frame(ui, me, |f, ui| {
        f.set_title(ui, me, "#ServerBrowser_GameInfoWithNameTitle");
        f.set_dialog_variable(ui, "game", name);
    });
    ctl(ui, me, |d, ui| d.request_info(ui));
    FrameState::activate(ui, me);
}

/// `Connect()` (the "Join Game" path).
pub fn connect(ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
    ctl(ui, me, |d, ui| d.on_connect(ui, me));
}

/// `EditablePanel::SetControlString(name, text)` on a label or text entry.
fn set_control_string(ui: &mut Ui, me: PanelId, name: &str, text: &str, s: &mut dyn Surface) {
    let Some(c) = ui.find_child_by_name(me, name) else { return };
    if with_label(ui, c, |l, ui| l.set_text(ui, text)).is_some() {
        ui.invalidate_layout(c);
        return;
    }
    with_te(ui, c, |te, ui| te.set_text(ui, c, text, s));
}

impl GameInfoDialog {
    fn addr(&self) -> String {
        format!("{}:{}", ip_string(self.server.ip), self.server.conn_port)
    }

    /// `RequestInfo`: one ping, and the next one 10 s later.
    fn request_info(&mut self, ui: &mut Ui) {
        if self.request_retry != 0 {
            return;
        }
        self.request_retry = ui.time_ms.wrapping_add(10_000);
        let mut mm = self.sb.mm.borrow_mut();
        if self.ping_query != -1 {
            mm.cancel_server_query(self.ping_query);
        }
        self.ping_query = mm.ping_server(self.server.ip, self.server.query_port, self.responder);
    }

    /// `RequestPlayerDetails(ip, queryPort)`.
    fn request_player_details(&mut self) {
        let mut mm = self.sb.mm.borrow_mut();
        if self.players_query != -1 {
            mm.cancel_server_query(self.players_query);
        }
        self.players_query = mm.player_details(self.server.ip, self.server.query_port, self.responder);
        self.player_list_up_to_date = true;
    }

    /// `OnConnect`.
    fn on_connect(&mut self, ui: &mut Ui, me: PanelId) {
        self.connecting = true;
        self.server_not_responding = false;
        self.server_full = false;
        ui.invalidate_layout(me);
        self.request_retry = 0;
        self.request_info(ui);
    }

    /// The dialog goes: its queries are cancelled and its response objects released.
    fn release(&mut self) {
        let mut mm = self.sb.mm.borrow_mut();
        for q in [self.ping_query, self.players_query] {
            if q != -1 {
                mm.cancel_server_query(q);
            }
        }
        drop(mm);
        self.ping_query = -1;
        self.players_query = -1;
        self.sb.drop_responder(self.responder);
    }

    fn close(&mut self, ui: &mut Ui, me: PanelId) {
        self.release();
        self.fs.close(ui, me);
    }

    /// `ConnectToServer` (§10.7).
    fn connect_to_server(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.connecting = false;
        if self.server.secure && self.sb.is_vac_banned() {
            small::create_vac_refused(ui, &self.sb, ui.parent(me), s);
            self.close(ui, me);
            return;
        }
        if self.server.password && self.password.is_empty() {
            let name = if self.server.server_name.is_empty() { self.addr() } else { self.server.server_name.clone() };
            small::create_password(ui, &self.sb, me, &name, 0, s);
            return;
        }
        if self.server.players >= self.server.max_players {
            self.server_full = true;
            self.show_auto_retry_toggle = true;
            ui.invalidate_layout(me);
            return;
        }
        let addr = self.addr();
        if !self.password.is_empty() {
            self.sb.add_text_command(&format!("password \"{}\"\n", self.password));
        }
        self.sb.add_text_command(&format!("connect {addr}\n"));
        ui.post(me, Msg::new("Close"), Some(me), 0.0);
    }

    fn on_event(&mut self, ui: &mut Ui, me: PanelId, ev: &MmEvent, s: &mut dyn Surface) {
        match ev {
            MmEvent::PingResponded { server, .. } => {
                if self.server.conn_port != 0 && self.server.conn_port != server.conn_port {
                    return;
                }
                self.ping_query = -1;
                self.server = server.clone();
                let auto = with_toggle(ui, self.auto_retry, |t, _| t.is_selected()).unwrap_or(false);
                if self.connecting {
                    self.connect_to_server(ui, me, s);
                } else if auto && server.players < server.max_players {
                    s.play_sound("Servers/game_ready.wav");
                    // FlashWindow: the OS taskbar flash has no in-game effect.
                    if with_toggle(ui, self.auto_retry_join, |t, _| t.is_selected()).unwrap_or(false) {
                        self.connect_to_server(ui, me, s);
                    }
                } else {
                    self.request_player_details();
                }
                self.server_not_responding = false;
                ui.invalidate_layout(me);
            }
            MmEvent::PingFailed { .. } => {
                if !self.server.had_successful_response {
                    self.server_not_responding = true;
                }
                ui.invalidate_layout(me);
            }
            MmEvent::AddPlayer { name, score, time, .. } => {
                let pl = self.player_list;
                if self.player_list_up_to_date {
                    with_list(ui, pl, |l, ui| l.delete_all_items(ui, pl));
                    self.player_list_up_to_date = false;
                }
                if self.server.humans() == 0 {
                    return;
                }
                let t = *time as i32;
                let (h, m, sec) = (t / 3600, (t / 60) % 60, t % 60);
                let text = if h != 0 {
                    format!("{h}h {m}m {sec}s")
                } else if m != 0 {
                    format!("{m}m {sec}s")
                } else {
                    format!("{sec}s")
                };
                let kv = Msg::new("player").str("PlayerName", name).int("Score", *score).int("TimeSec", t).str("Time", &text);
                with_list(ui, pl, |l, ui| l.add_item(ui, pl, &kv, 0, false, true));
            }
            MmEvent::PlayersComplete { .. } => self.players_query = -1,
            _ => {}
        }
    }

    /// `OnButtonToggled(panel)` / `OnRadioButtonChecked(panel)`.
    fn on_toggled(&mut self, ui: &mut Ui, me: PanelId, panel: Option<PanelId>, s: &mut dyn Surface) {
        if panel == Some(self.auto_retry) {
            let show = with_toggle(ui, self.auto_retry, |t, _| t.is_selected()).unwrap_or(false);
            let d = if show { ui.psv(me, 60) } else { -ui.psv(me, 60) };
            let (x, y) = ui.pos(me);
            let (w, h) = ui.size(me);
            let (mw, mh) = (ui.psv(me, 416), ui.psv(me, 340));
            ui.set_min_size(me, mw, mh);
            let res = if show { "Servers/DialogGameInfo_AutoRetry.res" } else { "Servers/DialogGameInfo.res" };
            load_res(ui, me, self.sb.fs.as_ref(), res, s);
            ui.set_bounds(me, x, y, w, h + d);
            self.layout(ui, me, s);
            let alert = self.auto_retry_alert;
            with_toggle(ui, alert, |t, ui| t.set_selected(ui, alert, true, s));
        }
        ui.invalidate_layout(me);
    }

    /// `PerformLayout` (§10.5).
    fn layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.fs.perform_layout(ui, me);
        let sv = self.server.clone();
        let name = if sv.server_name.is_empty() { self.addr() } else { sv.server_name.clone() };
        set_control_string(ui, me, "ServerText", &name, s);
        set_control_string(ui, me, "GameText", &sv.game_description, s);
        set_control_string(ui, me, "MapText", &sv.map, s);
        set_control_string(ui, me, "GameTags", &sv.game_tags, s);
        let secure = if !sv.had_successful_response {
            ""
        } else if sv.secure {
            "#ServerBrowser_Secure"
        } else {
            "#ServerBrowser_NotSecure"
        };
        set_control_string(ui, me, "SecureText", secure, s);
        let players = if sv.max_players > 0 { format!("{} / {}", sv.humans(), sv.max_players) } else { String::new() };
        set_control_string(ui, me, "PlayersText", &players, s);
        let auto = with_toggle(ui, self.auto_retry, |t, _| t.is_selected()).unwrap_or(false);
        if sv.ip != 0 && sv.query_port != 0 {
            set_control_string(ui, me, "ServerIPText", &self.addr(), s);
            ui.set_enabled(self.connect, true);
            ui.set_visible(self.auto_retry_alert, auto);
            ui.set_visible(self.auto_retry_join, auto);
        } else {
            set_control_string(ui, me, "ServerIPText", "", s);
            ui.set_enabled(self.connect, false);
        }
        let ping = if sv.had_successful_response { sv.ping.to_string() } else { String::new() };
        set_control_string(ui, me, "PingText", &ping, s);
        let info = if auto {
            if sv.players < sv.max_players {
                "#ServerBrowser_PressJoinToConnect"
            } else if with_toggle(ui, self.auto_retry_join, |t, _| t.is_selected()).unwrap_or(false) {
                "#ServerBrowser_JoinWhenSlotIsFree"
            } else {
                "#ServerBrowser_AlertWhenSlotIsFree"
            }
        } else if self.server_full {
            "#ServerBrowser_CouldNotConnectServerFull"
        } else if self.server_not_responding {
            "#ServerBrowser_ServerNotResponding"
        } else {
            ""
        };
        let il = self.info_label;
        with_label(ui, il, |l, ui| l.set_text(ui, info));
        ui.invalidate_layout(il);
        let empty = if sv.had_successful_response && sv.humans() == 0 { "#ServerBrowser_ServerHasNoPlayers" } else { "#ServerBrowser_ServerNotResponding" };
        let pl = self.player_list;
        with_list(ui, pl, |l, ui| l.set_empty_list_text(ui, empty));
        ui.set_visible(self.auto_retry, self.show_auto_retry_toggle);
    }
}

impl Control for GameInfoDialog {
    fn class(&self) -> &'static str {
        "CDialogGameInfo"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("Tick") {
            if self.request_retry != 0 && self.request_retry < ui.time_ms {
                self.request_retry = 0;
                self.request_info(ui);
            }
            return true;
        }
        if msg.is("Connect") {
            self.on_connect(ui, me);
            return true;
        }
        if msg.is("Refresh") {
            self.request_retry = 0;
            self.request_info(ui);
            return true;
        }
        if msg.is("ButtonToggled") || msg.is("RadioButtonChecked") {
            self.on_toggled(ui, me, msg.get_panel("panel"), s);
            return true;
        }
        if msg.is("JoinServerWithPassword") {
            self.password = msg.get_str("password").chars().take(63).collect();
            self.on_connect(ui, me);
            return true;
        }
        if msg.is("ConnectedToGame") {
            if self.server.ip == msg.get_int("ip") as u32 && i32::from(self.server.conn_port) == msg.get_int("port") {
                self.close(ui, me);
            }
            return true;
        }
        if msg.is("Close") {
            self.close(ui, me);
            return true;
        }
        match self.fs.on_message(ui, me, msg, s) {
            FrameMsg::Handled => true,
            FrameMsg::Unhandled => false,
            FrameMsg::Command(c) => {
                if c.eq_ignore_ascii_case("Close") {
                    self.close(ui, me);
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
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.layout(ui, me, s);
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.fs.paint_background(ui, me, s);
    }
    fn frame(&mut self) -> Option<&mut FrameState> {
        Some(&mut self.fs)
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
