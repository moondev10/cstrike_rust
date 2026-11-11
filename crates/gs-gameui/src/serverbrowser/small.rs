//! The small dialogs of ServerBrowser.dll (docs/re/serverbrowser.md §11): the server password
//! prompt, "Add server" (by address, with a test ping of the common ports) and the VAC refusal.

use std::any::Any;
use std::rc::Rc;

use gs_vgui::controls::button::{Button, with_button};
use gs_vgui::controls::frame::{FrameMsg, FrameState, with_frame};
use gs_vgui::controls::label::{Label, with_label};
use gs_vgui::controls::list_panel::{COLUMN_HIDDEN, ListPanel, with_list};
use gs_vgui::controls::message_box::MessageBox;
use gs_vgui::controls::property::{PropertySheet, with_sheet};
use gs_vgui::controls::text_entry::{TextEntry, text_of, with_te};
use gs_vgui::scheme::Scheme;
use gs_vgui::{Control, FontHandle, Msg, PanelId, Surface, Ui};

use super::dialog::add_server_to_favorites;
use super::{GameServerItem, MmEvent, Responder, Sb, ServerQuery, ip_string};
use crate::options::load_res;

/// A dialog's Steam responses (only "Add server" has any); `false` when `panel` is not one.
pub fn on_event(ui: &mut Ui, panel: PanelId, ev: &MmEvent, s: &mut dyn Surface) -> bool {
    ui.with_ctrl(panel, |c, ui| c.as_any_mut().downcast_mut::<AddServerDialog>().map(|d| d.on_event(ui, panel, ev, s))).flatten().is_some()
}

// ---------------------------------------------------------------------------------------------
// CDialogServerPassword

pub struct PasswordDialog {
    pub fs: FrameState,
    game_label: PanelId,
    password_entry: PanelId,
    connect_button: PanelId,
    server_id: i32,
}

/// `new CDialogServerPassword(parent)` + `Activate(gameName, serverID)`; the parent gets
/// `JoinServerWithPassword`.
pub fn create_password(ui: &mut Ui, sb: &Rc<Sb>, parent: PanelId, game_name: &str, server_id: i32, s: &mut dyn Surface) -> PanelId {
    let ph = ui.embedded();
    let me = ui.create(Some(parent), "DialogServerPassword", Box::new(PasswordDialog { fs: FrameState::default(), game_label: ph, password_entry: ph, connect_button: ph, server_id: 0 }));
    FrameState::init(ui, me, false);
    ui.set_size(me, 320, 240);
    with_frame(ui, me, |f, _| f.delete_self_on_close = true);
    FrameState::set_sizeable(ui, me, false);
    Label::create(ui, Some(me), "InfoLabel", "#ServerBrowser_ServerRequiresPassword");
    let game_label = Label::create(ui, Some(me), "GameLabel", "<game label>");
    let password_entry = TextEntry::create(ui, Some(me), "PasswordEntry");
    let connect_button = Button::create(ui, Some(me), "ConnectButton", "#ServerBrowser_Connect", None);
    with_te(ui, password_entry, |te, _| te.set_text_hidden(true));
    load_res(ui, me, sb.fs.as_ref(), "Servers/DialogServerPassword.res", s);
    with_frame(ui, me, |f, ui| f.set_title(ui, me, "#ServerBrowser_ServerRequiresPasswordTitle"));
    FrameState::move_to_center_of_screen(ui, me);
    ui.add_action_signal_target(me, parent);
    ui.with_ctrl(me, |c, _| {
        if let Some(d) = c.as_any_mut().downcast_mut::<PasswordDialog>() {
            d.game_label = game_label;
            d.password_entry = password_entry;
            d.connect_button = connect_button;
            d.server_id = server_id;
        }
    });
    with_label(ui, game_label, |l, ui| l.set_text(ui, game_name));
    with_button(ui, connect_button, |b, ui| b.set_as_default_button(ui, connect_button, true, true));
    ui.request_focus(password_entry);
    FrameState::activate(ui, me);
    me
}

impl Control for PasswordDialog {
    fn class(&self) -> &'static str {
        "CDialogServerPassword"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        match self.fs.on_message(ui, me, msg, s) {
            FrameMsg::Handled => true,
            FrameMsg::Unhandled => false,
            FrameMsg::Command(c) => {
                if c.eq_ignore_ascii_case("Connect") {
                    let pw = gs_common::bytestr::ansi_cap(&text_of(ui, self.password_entry), 63);
                    ui.post_action_signal(me, Msg::new("JoinServerWithPassword").str("password", &pw).int("serverID", self.server_id));
                    ui.post(me, Msg::new("Close"), Some(me), 0.0);
                } else if c.eq_ignore_ascii_case("Close") {
                    ui.post(me, Msg::new("Close"), Some(me), 0.0);
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
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// CDialogAddServer

pub struct AddServerDialog {
    pub fs: FrameState,
    sb: Rc<Sb>,
    responder: Responder,
    test_button: PanelId,
    ok_button: PanelId,
    selected_ok: PanelId,
    tabs: PanelId,
    text_entry: PanelId,
    servers_list: PanelId,
    original_height: i32,
    servers: Vec<GameServerItem>,
    queries: Vec<ServerQuery>,
}

/// `netadr_t::SetFromString(s, bUseDNS = true)`: `(ip, port)` in host order (0 = invalid).
fn parse_address(text: &str) -> (u32, u16) {
    if text.is_empty() {
        return (0, 0);
    }
    if text.as_bytes()[0].is_ascii_digit() && text.contains('.') {
        let mut nums = [0i32; 5];
        let mut n = 0;
        for (i, part) in text.splitn(5, ['.', ':']).enumerate() {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
            let Ok(v) = digits.parse::<i32>() else { break };
            nums[i] = v;
            n = i + 1;
        }
        let ip = if n >= 4 { nums[..4].iter().fold(0u32, |a, &b| (a << 8) | (b as u32 & 0xff)) } else { 0 };
        let port = if n == 5 { nums[4] as u16 } else { 0 };
        return (ip, port);
    }
    let host = text.split(':').next().unwrap_or("");
    let port = text.split_once(':').map_or(0, |(_, p)| gs_vgui::msg::atoi(p) as u16);
    use std::net::ToSocketAddrs;
    let ip = (host, 0).to_socket_addrs().ok().and_then(|mut a| a.find_map(|a| match a {
        std::net::SocketAddr::V4(v4) => Some(u32::from(*v4.ip())),
        std::net::SocketAddr::V6(_) => None,
    }));
    match ip {
        Some(ip) => (ip, port),
        None => (0, 0),
    }
}

/// `GetMostCommonQueryPorts`: 27015, 26900, 27016, 26901 … 27020, 26905, 4242, 27215.
fn common_query_ports() -> Vec<u16> {
    let mut v = Vec::with_capacity(14);
    for i in 0..6 {
        v.push(27015 + i);
        v.push(26900 + i);
    }
    v.push(4242);
    v.push(27215);
    v
}

/// `new CDialogAddServer(dlg, gameList)`, `MoveToCenterOfScreen`, `DoModal`.
pub fn create_add_server(ui: &mut Ui, sb: &Rc<Sb>, parent: PanelId, _game_list: PanelId, s: &mut dyn Surface) -> PanelId {
    let ph = ui.embedded();
    let me = ui.create(
        Some(parent),
        "DialogAddServer",
        Box::new(AddServerDialog {
            fs: FrameState::default(),
            sb: sb.clone(),
            responder: 0,
            test_button: ph,
            ok_button: ph,
            selected_ok: ph,
            tabs: ph,
            text_entry: ph,
            servers_list: ph,
            original_height: 0,
            servers: Vec::new(),
            queries: Vec::new(),
        }),
    );
    FrameState::init(ui, me, false);
    with_frame(ui, me, |f, ui| {
        f.delete_self_on_close = true;
        f.set_title(ui, me, "#ServerBrowser_AddServersTitle");
    });
    FrameState::set_sizeable(ui, me, false);
    let tabs = PropertySheet::create(ui, Some(me), "GameTabs");
    with_sheet(ui, tabs, |sh, ui| sh.set_tab_width(ui, tabs, 72));
    let list = ListPanel::create(ui, Some(me), "Servers");
    let cols: [(&str, &str, i32, i32); 9] = [
        ("Password", "#ServerBrowser_Password", 16, 5),
        ("Bots", "#ServerBrowser_Bots", 16, 5),
        ("Secure", "#ServerBrowser_Secure", 16, 5),
        ("Players", "#ServerBrowser_Players", 60, 0),
        ("Name", "#ServerBrowser_Servers", 20, 0x12),
        ("IPAddr", "#ServerBrowser_IPAddress", 60, COLUMN_HIDDEN),
        ("GameDesc", "#ServerBrowser_Game", 150, 0),
        ("Map", "#ServerBrowser_Map", 80, 0),
        ("Ping", "#ServerBrowser_Latency", 60, 0),
    ];
    let widths: Vec<i32> = cols.iter().map(|c| ui.psv(me, c.2)).collect();
    with_list(ui, list, |l, ui| {
        for (i, ((name, text, _, flags), w)) in cols.iter().zip(&widths).enumerate() {
            l.add_column_header(ui, list, i as i32, name, text, *w, *flags);
        }
        l.set_column_header_tooltip(ui, 0, "#ServerBrowser_PasswordColumn_Tooltip");
        l.set_column_header_tooltip(ui, 1, "#ServerBrowser_BotColumn_Tooltip");
        l.set_column_header_tooltip(ui, 2, "#ServerBrowser_SecureColumn_Tooltip");
        l.set_sort_column(8);
    });
    let text_entry = TextEntry::create(ui, Some(me), "ServerNameText");
    ui.add_action_signal_target(text_entry, me);
    let test_button = Button::create(ui, Some(me), "TestServersButton", "", None);
    let ok_button = Button::create(ui, Some(me), "OKButton", "", None);
    let selected_ok = Button::create(ui, Some(me), "SelectedOKButton", "", None);
    with_button(ui, selected_ok, |b, _| b.set_command("addselected"));
    ui.add_action_signal_target(selected_ok, me);
    with_sheet(ui, tabs, |sh, ui| sh.add_page(ui, tabs, list, "#ServerBrowser_Servers"));
    load_res(ui, me, sb.fs.as_ref(), "Servers/DialogAddServer.res", s);
    for b in [ok_button, test_button, selected_ok] {
        ui.set_enabled(b, false);
    }
    ui.set_visible(selected_ok, false);
    ui.set_visible(tabs, false);
    ui.request_focus(text_entry);
    let (_, ty) = ui.pos(tabs);
    let original_height = ty + ui.psv(me, 50) + ui.size(tabs).1;
    let w = ui.size(me).0;
    ui.set_size(me, w, ty);
    let responder = sb.new_responder(me);
    ui.with_ctrl(me, |c, _| {
        if let Some(d) = c.as_any_mut().downcast_mut::<AddServerDialog>() {
            d.responder = responder;
            d.test_button = test_button;
            d.ok_button = ok_button;
            d.selected_ok = selected_ok;
            d.tabs = tabs;
            d.text_entry = text_entry;
            d.servers_list = list;
            d.original_height = original_height;
        }
    });
    FrameState::move_to_center_of_screen(ui, me);
    with_frame(ui, me, |f, ui| f.do_modal(ui, me));
    me
}

impl AddServerDialog {
    /// The dialog goes: every query cancelled (the destructor).
    fn release(&mut self) {
        let mut mm = self.sb.mm.borrow_mut();
        for &q in &self.queries {
            mm.cancel_server_query(q);
        }
        drop(mm);
        self.queries.clear();
        self.sb.drop_responder(self.responder);
    }

    fn close(&mut self, ui: &mut Ui, me: PanelId) {
        self.release();
        self.fs.close(ui, me);
    }

    /// `TestServers`.
    fn test_servers(&mut self, ui: &mut Ui, s: &mut dyn Surface) {
        let list = self.servers_list;
        with_list(ui, list, |l, ui| l.set_empty_list_text(ui, ""));
        let text = text_of(ui, self.text_entry);
        let (ip, port) = parse_address(&text);
        self.servers.clear();
        let ports = if port == 0 { common_query_ports() } else { vec![port] };
        let tabs = self.tabs;
        let title = if text.is_empty() {
            ui.localize.find("ServerBrowser_ServersRespondingLocal").unwrap_or("").to_owned()
        } else {
            ui.localize.find("ServerBrowser_ServersResponding").unwrap_or("").replacen("%s1", &text, 1)
        };
        with_sheet(ui, tabs, |sh, ui| {
            sh.remove_all_pages(ui, tabs, s);
            sh.add_page(ui, tabs, list, &title);
        });
        ui.invalidate_layout(tabs);
        for p in ports {
            let q = self.sb.mm.borrow_mut().ping_server(ip, p, self.responder);
            self.queries.push(q);
        }
    }

    fn on_event(&mut self, ui: &mut Ui, me: PanelId, ev: &MmEvent, _s: &mut dyn Surface) {
        let list = self.servers_list;
        match ev {
            MmEvent::PingResponded { server, .. } => {
                let secure = if server.secure { if self.sb.is_vac_banned() { 4 } else { 3 } } else { 0 };
                let kv = Msg::new("Server")
                    .str("name", &server.name())
                    .str("map", &server.map)
                    .str("GameDir", &server.game_dir)
                    .str("GameDesc", &server.game_description)
                    .int("password", i32::from(server.password))
                    .int("bots", if server.bot_players != 0 { 2 } else { 0 })
                    .int("secure", secure)
                    .str("IPAddr", &format!("{}:{}", ip_string(server.ip), server.conn_port))
                    .str("Players", &format!("{} / {}", server.players, server.max_players))
                    .int("Ping", server.ping);
                self.servers.push(server.clone());
                let index = self.servers.len() as u32 - 1;
                with_list(ui, list, |l, ui| {
                    let id = l.add_item(ui, list, &kv, index, false, false);
                    if l.item_count() == 1 {
                        l.add_selected_item(ui, list, id);
                    }
                });
                ui.invalidate_layout(me);
            }
            MmEvent::PingFailed { .. } => {
                with_list(ui, list, |l, ui| l.set_empty_list_text(ui, "#ServerBrowser_ServerNotResponding"));
            }
            _ => {}
        }
    }

    fn on_command(&mut self, ui: &mut Ui, me: PanelId, cmd: &str, s: &mut dyn Surface) {
        if cmd.eq_ignore_ascii_case("OK") {
            let text: String = text_of(ui, self.text_entry);
            let (ip, mut port) = parse_address(&text);
            if port == 0 {
                port = 27015;
            }
            if ip != 0 {
                let item = GameServerItem { server_name: text.chars().take(63).collect(), ip, conn_port: port, query_port: port, ..GameServerItem::default() };
                add_server_to_favorites(&self.sb, &item);
            } else {
                let mb = MessageBox::create(ui, "#ServerBrowser_AddServerErrorTitle", "#ServerBrowser_AddServerError", None);
                MessageBox::do_modal(ui, mb, None);
            }
            ui.post(me, Msg::new("Close"), Some(me), 0.0);
        } else if cmd.eq_ignore_ascii_case("TestServers") {
            let h = self.original_height;
            let w = ui.size(me).0;
            ui.set_size(me, w, h);
            ui.set_visible(self.tabs, true);
            ui.set_visible(self.selected_ok, true);
            self.test_servers(ui, s);
        } else if cmd.eq_ignore_ascii_case("addselected") {
            let list = self.servers_list;
            let sel = with_list(ui, list, |l, _| (l.selected_items_count() > 0).then(|| (l.selected_item(0), l.item_user_data(l.selected_item(0))))).flatten();
            if let Some((id, index)) = sel {
                if let Some(srv) = self.servers.get(index as usize) {
                    add_server_to_favorites(&self.sb, srv);
                }
                with_list(ui, list, |l, ui| {
                    l.remove_item(ui, list, id);
                    l.set_empty_list_text(ui, "");
                });
            }
        } else if cmd.eq_ignore_ascii_case("Close") {
            self.close(ui, me);
        } else {
            self.fs.on_command(ui, me, cmd);
        }
    }
}

impl Control for AddServerDialog {
    fn class(&self) -> &'static str {
        "CDialogAddServer"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("TextChanged") {
            let has = !text_of(ui, self.text_entry).is_empty();
            ui.set_enabled(self.ok_button, has);
            ui.set_enabled(self.test_button, has);
            return true;
        }
        if msg.is("ItemSelected") {
            let list = self.servers_list;
            let sel = with_list(ui, list, |l, _| l.selected_item(0)).unwrap_or(-1);
            ui.set_enabled(self.selected_ok, sel != -1);
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
                self.on_command(ui, me, &c, s);
                true
            }
        }
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.fs.apply_scheme(ui, me, scheme, s);
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
        let list = self.servers_list;
        with_list(ui, list, |l, ui| {
            l.set_image_list(images);
            l.set_font(font, s);
            l.set_column_header_image(ui, 0, 5);
            l.set_column_header_image(ui, 1, 6);
            l.set_column_header_image(ui, 2, 7);
        });
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
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// CVACBannedConnRefusedDialog

pub struct VacRefusedDialog {
    pub fs: FrameState,
}

/// `new CVACBannedConnRefusedDialog(vparent, "VACBannedConnRefusedDialog")` + `Activate`.
pub fn create_vac_refused(ui: &mut Ui, sb: &Rc<Sb>, parent: Option<PanelId>, s: &mut dyn Surface) -> PanelId {
    let me = ui.create(parent, "VACBannedConnRefusedDialog", Box::new(VacRefusedDialog { fs: FrameState::default() }));
    FrameState::init(ui, me, false);
    ui.set_size(me, 480, 220);
    FrameState::set_sizeable(ui, me, false);
    load_res(ui, me, sb.fs.as_ref(), "servers/VACBannedConnRefusedDialog.res", s);
    FrameState::move_to_center_of_screen(ui, me);
    FrameState::activate(ui, me);
    me
}

impl Control for VacRefusedDialog {
    fn class(&self) -> &'static str {
        "CVACBannedConnRefusedDialog"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        match self.fs.on_message(ui, me, msg, s) {
            FrameMsg::Handled => true,
            FrameMsg::Unhandled => false,
            FrameMsg::Command(c) => {
                self.fs.on_command(ui, me, &c);
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
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
