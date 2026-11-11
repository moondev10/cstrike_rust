//! Connection setup and the connectionless traffic (docs/re/net-transport.md §8): the
//! challenge/connect handshake with its resends, `rcon`, the LAN server ping, the HLTV
//! multicast `listen` with its unicast fallback, and the out-of-band replies.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};

use gs_common::bytestr::{truncate_encoded, truncate_encoded_in_place};
use gs_net::Netchan;
use gs_net::oob::{self, OobReply};

use super::{CMD_SLOTS, Client, ClientEvent, DEFAULT_PORT, MAX_CONNECT_RETRIES, State, UiEvent, string_to_adr, string_to_adr_no_port, sys_float_time};
use crate::auth::cdkey_hash;

impl Client {
    /// `cmd_pingservers`' stamp and broadcasts: `T Source Engine Query` to
    /// 255.255.255.255:27015..=27024 (`send` is false with `-noip`).
    pub fn ping_lan_servers(&mut self, send: bool) {
        self.slist.time = self.now as f32;
        if !send {
            return;
        }
        let mut packet = oob::OOB_PREFIX.to_vec();
        packet.push(b'T');
        packet.extend_from_slice(b"Source Engine Query\0");
        for port in 27015..27025 {
            self.send_raw(SocketAddrV4::new(Ipv4Addr::BROADCAST, port), &packet);
        }
    }

    /// `cmd_listen` with `Cmd_Args()`: a fresh challenge, then `cmd_listen_impl`.
    pub fn listen_command(&mut self, args: &str) {
        self.challenge = 0;
        self.listen(args);
    }

    /// `cmd_rcon`: `args` is `Cmd_Args()` (`None` for a bare `rcon`). A challenge is requested
    /// from `rcon_address` (or the server), the port from `rcon_port` or the default.
    pub fn rcon(&mut self, args: Option<&str>, address: &str, port: f32) {
        let Some(args) = args else {
            self.print("Rcon:  Empty rcon string\n".into());
            return;
        };
        let resolved = |c: &mut Self| {
            let adr = string_to_adr(address);
            if adr.is_none() {
                c.print(format!("Unable to resolve rcon address {address}\n"));
            }
            adr
        };
        let connected = self.state >= State::Connected;
        let adr = if !connected {
            if address.is_empty() {
                self.print("You must either be connected,\nor set the 'rcon_address' cvar\nto issue rcon commands\n".into());
                return;
            }
            resolved(self)
        } else if !address.is_empty() {
            resolved(self)
        } else {
            self.server
        };
        let Some(mut adr) = adr else { return };
        let mut p = (port as i32) as u16;
        if p == 0 {
            p = if connected { self.server.map_or(DEFAULT_PORT, |s| s.port()) } else { DEFAULT_PORT };
        }
        adr.set_port(p);
        self.rcon_command = truncate_encoded(args, 1023).into_owned();
        self.rcon_adr = Some(adr);
        self.send_raw(adr, &oob::print("challenge rcon\n"));
    }

    pub(super) fn check_resend(&mut self) {
        if self.state != State::Connecting {
            return;
        }
        if self.passive {
            return self.check_multicast_timeout();
        }
        let resend = self.config.resend.clamp(1.5, 20.0);
        if self.now - self.connect_time < resend {
            return;
        }
        let Some(addr) = self.resolve() else {
            self.print(format!("Bad server address {}\n", self.server_name));
            self.state = State::Disconnected;
            return;
        };
        if self.connect_retry >= MAX_CONNECT_RETRIES {
            self.print(format!("Could not contact {addr}\n"));
            self.ui(UiEvent::Explain { text: "#GameUI_CouldNotContactGameServer".to_owned(), extended: false });
            self.disconnect("could not contact game server");
            return;
        }
        self.connect_time = self.now;
        if (1..=3).contains(&self.connect_retry) {
            let suffix = if self.connect_retry == 1 { String::new() } else { self.connect_retry.to_string() };
            self.ui(UiEvent::Status(format!("#GameUI_RetryingConnectionToServer{suffix}")));
        }
        self.print(if self.connect_retry == 0 { format!("Connecting to {addr}...\n") } else { format!("Retrying {addr}...\n") });
        self.connect_retry += 1;
        self.server = Some(addr);
        self.send_raw(addr, &oob::getchallenge());
    }

    /// `CL_SendConnectPacket`: Steam (3) with a ticket, else the hashed key (2).
    fn send_connect_packet(&mut self, to: SocketAddrV4, challenge: i32, auth_protocol: u8, server_steam_id: Option<u64>, secure: bool) {
        // fs_startup_timings starts over with every connect packet.
        if self.config.startup_timings {
            self.startup_timings = vec![("Startup", sys_float_time())];
        }
        self.secure = secure;
        let hash = cdkey_hash(Self::local_ip_for(to));
        let packet = if auth_protocol == 3 {
            let ticket = match self.auth.ticket(server_steam_id.unwrap_or(0), *to.ip(), to.port(), secure) {
                Ok(t) => t,
                Err(e) => {
                    self.disconnect(&format!("Steam authentication failed: {e}"));
                    return;
                }
            };
            oob::steam_protinfo(&hash).and_then(|protinfo| oob::connect(challenge, &protinfo, self.config.userinfo.as_str(), &ticket))
        } else {
            oob::hashed_protinfo(&hash).and_then(|protinfo| oob::connect(challenge, &protinfo, self.config.userinfo.as_str(), &[]))
        };
        match packet {
            Ok(p) => {
                self.connect_time = self.now;
                self.send_raw(to, &p);
                self.ui(UiEvent::ContinueProgress(1));
            }
            Err(e) => self.disconnect(&format!("connect packet: {e}")),
        }
    }

    /// `CL_CheckForResend` while passive: 30 s for the first multicast datagram, then the
    /// unicast fallback (after a `G` challenge) or failure.
    fn check_multicast_timeout(&mut self) {
        if self.now - self.connect_time <= 30.0 {
            return;
        }
        let group = self.mcast_listen_adr;
        self.passive = false;
        if self.challenge != 0 {
            self.print("Multicast failed. Falling back to unicast...\n".into());
            self.leave_group(group);
            match self.resolve() {
                Some(addr) => {
                    self.server = Some(addr);
                    let challenge = self.challenge;
                    self.send_connect_packet(addr, challenge, 2, None, false);
                }
                None => {
                    self.print(format!("Bad server address {}\n", self.server_name));
                    self.state = State::Disconnected;
                }
            }
        } else {
            self.ui(UiEvent::Explain { text: "Joining multicast group failed after 30 seconds.".to_owned(), extended: false });
            self.connect_time = -99999.0;
            self.connect_retry = 0;
            self.state = State::Disconnected;
            self.server = None;
            self.leave_group(group);
        }
    }

    /// `listen <group>` / the `G` reply (`cmd_listen_impl`): joins a multicast group as a
    /// passive client (docs/re/hltv-client.md §4.3.3).
    pub fn listen(&mut self, adr: &str) {
        // NET_StringToAdr: the old address stays on failure; "localhost" zeroes it.
        if adr.eq_ignore_ascii_case("localhost") {
            self.mcast_listen_adr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
        } else if let Some(parsed) = string_to_adr_no_port(adr) {
            self.mcast_listen_adr = parsed;
        }
        if !self.mcast_listen_adr.ip().is_multicast() {
            self.print(format!("{adr} is not a class D IP.\n"));
            return;
        }
        self.disconnect("listen");
        self.proto.msg_stats = gs_protocol::state::MsgStats::default();
        self.connect_retry = 0;
        if self.mcast_socket.is_none() {
            match gs_net::socket::ClientSocket::bind_multicast(self.config.multicast_port, self.config.multicast_loop) {
                Ok(s) => self.mcast_socket = Some(s),
                Err(e) => {
                    log::debug!("multicast socket: {e}");
                    self.print("Warning! Couldn't allocate multicast IP port.\n".into());
                }
            }
        }
        let mut chan = Netchan::new(self.mcast_listen_adr, self.now);
        chan.fragment_size = self.fragment_size();
        self.netchan = Some(chan);
        if let Some(sock) = self.mcast_socket.as_mut() {
            sock.flush();
        }
        self.passive = true;
        if self.mcast_listen_adr.port() == 0 {
            self.mcast_listen_adr.set_port(27025);
        }
        let group = self.mcast_listen_adr;
        self.join_group(group);
        self.print(format!("Joining multicast group {group} ...\n"));
        self.connect_time = self.now;
        self.state = State::Connecting;
        self.demo.demonum = -1;
        self.signon = 0;
        self.proto.valid_sequence = 0;
        self.delta_sequence = None;
        self.last_sent_sequence = None;
        self.next_cmd_time = self.now as f32;
    }

    /// `NET_JoinGroup` on the netchan's socket (NS_MULTICAST while passive, else NS_CLIENT).
    pub(super) fn join_group(&mut self, group: SocketAddrV4) {
        let sock = if self.passive { self.mcast_socket.as_ref() } else { Some(&self.socket) };
        let result = sock.map_or(Err(gs_net::socket::WSAENOTSOCK), |s| s.join_group(*group.ip()));
        if let Err(code) = result
            && code != gs_net::socket::WSAEAFNOSUPPORT
        {
            self.print(format!("WARNING: NET_JoinGroup: IP_ADD_MEMBERSHIP: {}", gs_net::socket::error_string(code)));
        }
    }

    /// `NET_LeaveGroup` (silent).
    pub(super) fn leave_group(&mut self, group: SocketAddrV4) {
        let sock = if self.passive || self.netchan_on_multicast() { self.mcast_socket.as_ref() } else { Some(&self.socket) };
        if let Some(s) = sock {
            let _ = s.leave_group(*group.ip());
        }
    }

    /// The netchan reads NS_MULTICAST: set up by `listen` and kept through the unicast
    /// fallback until a connection is accepted.
    pub(super) fn netchan_on_multicast(&self) -> bool {
        self.netchan.as_ref().is_some_and(|c| c.remote == self.mcast_listen_adr && self.mcast_listen_adr.ip().is_multicast())
    }

    pub(super) fn send_raw(&mut self, to: SocketAddrV4, data: &[u8]) {
        if let Err(e) = self.socket.send(to, data) {
            log::warn!("send to {to}: {e}");
        }
    }

    fn local_ip_for(server: SocketAddrV4) -> Ipv4Addr {
        UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
            .and_then(|s| {
                s.connect(server)?;
                s.local_addr()
            })
            .ok()
            .and_then(|a| match a {
                SocketAddr::V4(v4) => Some(*v4.ip()),
                SocketAddr::V6(_) => None,
            })
            .unwrap_or(Ipv4Addr::LOCALHOST)
    }

    pub(super) fn handle_oob(&mut self, from: SocketAddrV4, data: &[u8]) {
        let Some(reply) = oob::parse_reply(data) else { return };
        // CL_IsAllowedOOBSource (the connect address or the rcon target) guards every case
        // except `I`, `G` and unknown commands.
        let allowed = Some(from) == self.server || Some(from) == self.rcon_adr;
        match reply {
            OobReply::Info(info) => {
                let entry = crate::slist::Entry::from_reply(from, info);
                if let Some(text) = self.slist.add(entry, self.now, self.config.slist) {
                    self.print(text);
                }
            }
            OobReply::Multicast { challenge, auth_protocol, group } if self.state == State::Connecting => {
                self.challenge = challenge;
                match auth_protocol {
                    2 | 0xFF => self.listen(&group),
                    other => {
                        self.print(format!("Invalid multicast authentication type ({other}).\n"));
                        self.disconnect("disconnect");
                    }
                }
            }
            OobReply::Unknown(c) => {
                // `%c` of NUL ends the C string.
                let text = if c == 0 { "Unknown command:\n".to_owned() } else { format!("Unknown command:\n{}\n", char::from(c)) };
                self.print(text);
            }
            _ if !allowed => {}
            OobReply::Challenge { challenge, auth_protocol, server_steam_id, secure } if self.state == State::Connecting => {
                self.challenge = challenge;
                self.send_connect_packet(from, challenge, auth_protocol, server_steam_id, secure);
            }
            OobReply::Accept { build, true_address, .. } if self.state != State::Connected => {
                self.demo.demonum = -1;
                self.true_address = true_address.chars().take(31).collect();
                self.build_num = build;
                let mut chan = Netchan::new(from, self.now);
                chan.fragment_size = self.fragment_size();
                self.netchan = Some(chan);
                self.state = State::Connected;
                self.signon = 0;
                self.last_sent_sequence = None;
                self.next_cmd_time = self.now as f32;
                self.reset_protocol();
                self.delta_sequence = None;
                self.frame_received = [0.0; CMD_SLOTS];
                self.parsecount = None;
                self.packet_loss = 0.0;
                self.startup_mark("connection accepted by server");
                self.print(format!("Connection accepted by {from}\n"));
                self.ui(UiEvent::ContinueProgress(2));
                self.send_string_cmd("new");
            }
            // `9`: COM_ExplainDisconnection(1, text) + CL_Disconnect in any state (hl.exe always
            // runs with -steam).
            OobReply::Reject(reason) => {
                self.ui(UiEvent::Explain { text: truncate_encoded(&reason, 255).into_owned(), extended: false });
                self.disconnect(reason.trim_end());
            }
            OobReply::BadPassword(text) if self.state == State::Connecting => {
                self.print(text);
                self.ui(UiEvent::Explain { text: "#GameUI_ServerConnectionFailedBadPassword".to_owned(), extended: false });
                self.print("Invalid server password.\n".to_owned());
                self.disconnect("bad server password");
            }
            // A2A_PRINT: Con_Printf("%s\n") (rcon output gains a newline per packet).
            OobReply::Print(text) => self.print(format!("{text}\n")),
            OobReply::RconChallenge(challenge) => {
                if self.rcon_command.is_empty() {
                    self.print("Rcon:  Empty rcon string\n".into());
                } else if Some(from) != self.rcon_adr {
                    self.print("Rcon:  Challenge from spoofed/invalid IP address\n".into());
                } else {
                    // Q_snprintf into 1152 bytes; the password is not escaped.
                    let mut text = format!("rcon {challenge} \"{}\" {}", self.config.rcon_password, self.rcon_command);
                    truncate_encoded_in_place(&mut text, 1151);
                    self.send_raw(from, &oob::print(&text));
                }
            }
            // An `L` redirect: `connect` through the command buffer; it must end at an HLTV
            // sign-on.
            OobReply::Redirect(addr) if self.state == State::Connecting => {
                if addr.contains(['\n', ';']) {
                    let text = "Invalid command separator in redirect server name, disconnecting.\n";
                    self.ui(UiEvent::Explain { text: text.to_owned(), extended: false });
                    self.disconnect("Invalid command separator in redirect server name");
                } else {
                    let command = format!("connect {addr}\n");
                    self.events.push_back(ClientEvent::AddText(truncate_encoded(&command, 259).into_owned()));
                    self.print(format!("Redirecting connection to {addr}.\n"));
                    self.redirected = true;
                }
            }
            other => log::debug!("ignoring connectionless {other:?}"),
        }
    }
}
