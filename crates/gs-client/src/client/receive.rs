//! The incoming side (docs/re/net-transport.md §8, net-messages.md §6.2): `fakelag` and
//! `fakeloss`, `CL_ReadPackets` with its netchan and fragment handling, `CL_ParseServerMessage`
//! and its trace lines, the messages the engine handles itself, events, the server info, and
//! `CL_ProcessPacket`.

use std::net::SocketAddrV4;

use gs_common::bytestr::truncate_encoded;
use gs_formats::bsp;
use gs_net::oob;
use gs_net::{ClientSocket, Netchan};
use gs_protocol::ServerMessage;
use gs_protocol::messages::{Hltv, ServerInfo, WireEvent};
use gs_protocol::state::{ParseTrace, SVC_NAMES, UPDATE_MASK};
use gs_protocol::structs::{EntityState, EventArgs};

use super::{Client, ClientEvent, CvarReply, State, UiEvent, check_prediction_error, string_to_adr_no_port};
use crate::demo::Output;
use crate::entities::PacketContext;
use crate::events::{FEV_RELIABLE, com_normalize_angles};
use crate::level::Level;

impl Client {
    /// `CL_GetFragmentSize`.
    pub(super) fn fragment_size(&self) -> usize {
        if self.state != State::Active {
            128
        } else if self.config.dlmax < 16 {
            256
        } else {
            self.config.dlmax.min(1024) as usize
        }
    }

    /// `NET_AdjustLag` (each `NET_GetPacket`): the applied lag moves toward `fakelag` by at most
    /// 200 ms per second of `realtime` (steps capped at 0.1 s; [INFERENCE] the cap's handling of
    /// a zero step); without `sv_cheats` a nonzero `fakelag` is refused and reset.
    fn adjust_lag(&mut self) {
        let dt = (self.now - self.fake_lag.lasttime).clamp(0.0, 0.1);
        self.fake_lag.lasttime = self.now;
        if self.config.cheats || self.config.fake_lag == 0.0 {
            let diff = f64::from(self.config.fake_lag) - self.fake_lag.lag;
            let converge = (dt * 200.0).min(diff.abs());
            self.fake_lag.lag += if diff < 0.0 { -converge } else { converge };
        } else {
            self.print("Server must enable cheats to activate fakelag\n".into());
            self.events.push_back(ClientEvent::SetCvar { name: "fakelag".into(), value: "0".into() });
            self.config.fake_lag = 0.0;
            self.fake_lag.lag = 0.0;
        }
    }

    /// `NET_LagPacket`: with a lag applied, a new datagram may be lost to `fakeloss` (with
    /// `sv_cheats`; reset without) and is queued; the oldest one at least the lag old comes out.
    /// Without lag the queue is dropped and `fresh` passes.
    fn lag_packet(&mut self, fresh: Option<(SocketAddrV4, Vec<u8>)>) -> Option<(SocketAddrV4, Vec<u8>)> {
        if self.fake_lag.lag <= 0.0 {
            self.fake_lag.queue.clear();
            return fresh;
        }
        if let Some(p) = fresh {
            let loss = self.config.fake_loss;
            if loss != 0.0 {
                if self.config.cheats {
                    self.fake_lag.losscount = self.fake_lag.losscount.wrapping_add(1);
                    let lost = if loss <= 0.0 {
                        self.fake_lag.losscount % (loss.abs() as i32).max(2) == 0
                    } else {
                        self.rng.long(0, 100) as f32 <= loss
                    };
                    if lost {
                        return None;
                    }
                } else {
                    self.events.push_back(ClientEvent::SetCvar { name: "fakeloss".into(), value: "0".into() });
                    self.config.fake_loss = 0.0;
                }
            }
            self.fake_lag.queue.push_back((self.now, p));
        }
        let due = self.fake_lag.queue.front().is_some_and(|&(received, _)| self.now - self.fake_lag.lag / 1000.0 >= received);
        due.then(|| self.fake_lag.queue.pop_front().map(|(_, p)| p)).flatten()
    }

    /// `Netchan_Process`'s lines for a datagram past its header checks: `net_showpackets`
    /// other than 0 and 3, then `net_showdrop`'s duplicate, out-of-order and loss notes.
    fn trace_incoming(&mut self, h: gs_net::PacketHeader, remote: SocketAddrV4) {
        let show = self.config.show_packets;
        if show != 0.0 && show != 3.0 {
            let tm = self.time() as f32;
            self.print(format!(" c <-- sz={} seq={} ack={} rel={} tm={tm:.6}\n", h.size, h.sequence, h.ack, i32::from(h.reliable)));
        }
        if !self.config.show_drop {
            return;
        }
        if h.sequence == h.previous {
            self.print(format!("{remote}:duplicate packet {} at {}\n", h.sequence, h.previous));
        } else if h.sequence < h.previous {
            self.print(format!("{remote}:out of order packet {} at {}\n", h.sequence, h.previous));
        } else if h.sequence - h.previous > 1 {
            self.print(format!("{remote}:Dropped {} packets at {}\n", h.sequence - h.previous - 1, h.sequence));
        }
    }

    pub(super) fn read_packets(&mut self, cvars: CvarReply) {
        // CL_GetMessage: the demo file instead of the socket.
        if self.demo.playing() {
            // CL_GetDemoMessage: the module path when DemoPlayer_IsActive.
            if self.demo_player_is_active() {
                return self.read_module(cvars);
            }
            return self.read_demo(cvars);
        }
        let mut count = 0u32;
        let split_lines = self.config.show_packets == 4.0;
        self.socket.show_splits(split_lines);
        if let Some(sock) = self.mcast_socket.as_mut() {
            sock.show_splits(split_lines);
        }
        loop {
            self.adjust_lag();
            // NET_GetPacket(cls.passive ? NS_MULTICAST : NS_CLIENT).
            let received = match (self.passive, self.mcast_socket.as_mut()) {
                (true, Some(sock)) => sock.recv(),
                (true, None) => Ok(None),
                (false, _) => self.socket.recv(),
            };
            let fresh = match received {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("recv: {e}");
                    None
                }
            };
            let mcast_lines = self.mcast_socket.as_mut().map(ClientSocket::take_prints).unwrap_or_default();
            for line in self.socket.take_prints().into_iter().chain(mcast_lines) {
                self.print(line);
            }
            let Some((from, data)) = self.lag_packet(fresh) else { break };
            // Past 250 datagrams a frame the rest are read and dropped.
            count += 1;
            if count > 250 {
                continue;
            }
            if oob::is_oob(&data) {
                self.handle_oob(from, &data);
                continue;
            }
            if self.passive {
                // The first datagram from the group "connects"; no source check.
                if self.state == State::Connecting {
                    self.state = State::Connected;
                }
            } else if self.netchan.as_ref().is_none_or(|c| c.remote != from) {
                continue;
            }
            if matches!(self.state, State::Disconnected | State::Connecting) {
                continue;
            }
            if data.len() < 8 {
                self.print(format!("CL_ReadPackets:  undersized packet {} bytes from {from}\n", data.len()));
                continue;
            }
            let now = self.now;
            let multicast = self.netchan_on_multicast();
            let Some(chan) = self.netchan.as_mut() else { continue };
            let processed = chan.process(&data, now);
            let sequence = chan.incoming_sequence;
            let remote = chan.remote;
            if let Some(header) = processed.header {
                self.trace_incoming(header, remote);
            }
            if processed.fragment_stall {
                // Netchan_CheckForCompletion: a multicast channel re-listens silently.
                if multicast {
                    let adr: String = remote.to_string().chars().take(128).collect();
                    let adr = adr.split([';', '\n']).next().unwrap_or("");
                    self.events.push_back(ClientEvent::AddText(truncate_encoded(&format!("listen {adr}\n"), 31).into_owned()));
                } else {
                    self.print("Netchan_CheckForCompletion:  Lost/dropped fragment would cause stall, retrying connection\n".into());
                    self.events.push_back(ClientEvent::AddText("retry\n".to_owned()));
                }
                continue;
            }
            if let Some(payload) = processed.payload {
                // The netchan header (and any fragment data) precedes the message.
                self.parse(&payload, sequence, cvars, data.len() - payload.len());
                if self.state == State::Active {
                    self.proto.current_frame_mut().bytes.msgbytes = data.len().min(usize::from(u16::MAX)) as u16;
                }
            }
        }
        if count > 250 {
            self.events.push_back(ClientEvent::DevPrint(format!("Ignored {} messages\n", count - 250)));
        }
        let Some(chan) = self.netchan.as_mut() else { return };
        let sequence = chan.incoming_sequence;
        let message = chan.take_message();
        let file = chan.take_file();
        if let Some(message) = message {
            match message {
                Ok(m) => self.parse(&m, sequence, cvars, 0),
                Err(e) => self.disconnect(&format!("bad fragmented message: {e}")),
            }
        }
        if let Some(file) = file {
            match file {
                Ok(f) => {
                    // The finished file's name and a full secondary bar (100.0f, not 1.0 — the
                    // dialog clamps it), then CL_ProcessFile.
                    let text = if f.name.starts_with('!') { "#GameUI_SecurityModule".to_owned() } else { format!("File '{}'", f.name) };
                    self.ui(UiEvent::SecondaryText(text));
                    self.ui(UiEvent::SecondaryProgress(100.0));
                    self.file_received(&f.name, &f.data);
                }
                Err(e) => self.print(format!("file transfer failed: {e}\n")),
            }
        }
        // Netchan_UpdateProgress: an incoming file's progress in the loading dialog.
        if let Some((percent, name)) = self.netchan.as_ref().and_then(|c| c.download_progress())
            && percent > 0.0
            && !name.is_empty()
        {
            let text = if name.starts_with('!') { "#GameUI_DownloadingSecurityModule".to_owned() } else { truncate_encoded(&format!("Downloading file '{name}'"), 127).into_owned() };
            self.ui(UiEvent::SecondaryProgress(percent / 100.0));
            self.ui(UiEvent::SecondaryText(text));
            self.ui(UiEvent::Status("#GameUI_VerifyingAndDownloading".to_owned()));
        }
    }

    /// `CL_ParseServerMessage` of `payload`, which starts `base` bytes into `net_message`.
    pub(super) fn parse(&mut self, payload: &[u8], sequence: u32, cvars: CvarReply, base: usize) {
        if self.state < State::Connected {
            return;
        }
        if self.config.show_net == 1.0 {
            self.print(format!("{} ", base + payload.len()));
        } else if self.config.show_net == 2.0 {
            self.print("------------------\n".into());
        }
        self.proto.passive = self.passive;
        self.proto.trace = self.config.show_net == 2.0 || self.config.show_messages || self.config.need_instanced;
        let parsed = self.proto.parse(payload, sequence);
        let mut traces = self.proto.take_traces().into_iter().peekable();
        for (i, msg) in parsed.messages.into_iter().enumerate() {
            while let Some((_, t)) = traces.next_if(|(at, _)| *at <= i) {
                self.print_trace(t, base);
            }
            self.handle(msg, cvars);
            if self.state < State::Connected {
                return;
            }
        }
        for (_, t) in traces {
            self.print_trace(t, base);
        }
        // The messages before a bad one have run; the packet then ends in Host_Error.
        if let Some(gs_protocol::ProtocolError::Malformed(text)) = parsed.error {
            return self.host_error(&text);
        }
        // CL_ParseServerMessage's tail: this packet into the demo, and into the header spool
        // until the client is active.
        if !self.demo.playing() {
            let sequences = self.netchan.as_ref().map(Netchan::demo_sequences).unwrap_or_default();
            self.demo.record_packet(payload, sequences, self.now, self.host_framecount);
            if self.state != State::Active {
                self.demo.spool_packet(payload, sequences, self.now, self.host_framecount);
            }
        }
    }

    /// One `CL_ParseServerMessage` trace line (net-messages.md §2.4): `cl_shownet 2`'s opcode
    /// offsets, `cl_showmessages`' and `cl_needinstanced`'s developer lines.
    fn print_trace(&mut self, trace: ParseTrace, base: usize) {
        let dev = |c: &mut Self, text: String| c.events.push_back(ClientEvent::DevPrint(text));
        match trace {
            ParseTrace::Svc { offset, op } => {
                let name = SVC_NAMES.get(usize::from(op)).copied().unwrap_or("");
                if self.config.show_net == 2.0 && name.len() > 1 {
                    self.print(format!("{:3}:{name}\n", base + offset));
                }
                if self.config.show_messages && op > 1 {
                    dev(self, format!("Msg: {name}\n"));
                }
            }
            ParseTrace::User { name, size } => {
                if self.config.show_messages {
                    dev(self, format!("UserMsg: {name} (size {size})\n"));
                }
            }
            ParseTrace::NonInstanced { number, modelindex } => {
                if self.config.need_instanced {
                    let model = if modelindex != 0 { self.level.model_name(modelindex).unwrap_or("").to_owned() } else { "?".to_owned() };
                    dev(self, format!("Non-instanced {number} {model} without baseline\n"));
                }
            }
            ParseTrace::End { offset } => {
                if self.config.show_net == 2.0 {
                    self.print(format!("{:3}:END OF MESSAGE\n", base + offset));
                }
            }
        }
    }

    fn handle(&mut self, msg: ServerMessage, cvars: CvarReply) {
        use ServerMessage as M;
        match msg {
            M::Disconnect(reason) => {
                // Parse_svc_disconnect: the explanation GameUI's error dialog shows.
                if reason.is_empty() {
                    self.ui(UiEvent::Explain { text: "#GameUI_DisconnectedFromServer".to_owned(), extended: false });
                } else {
                    self.ui(UiEvent::Explain { text: "#GameUI_DisconnectedFromServerExtended".to_owned(), extended: false });
                    self.ui(UiEvent::Explain { text: reason.clone(), extended: true });
                }
                self.print(format!("Disconnected: {reason}\n"));
                self.disconnect(&reason);
            }
            M::Print(text) => self.print(text),
            M::CrosshairAngle { pitch, yaw } => {
                self.level.crosshair_angle[0] = pitch;
                self.level.crosshair_angle[1] = yaw;
            }
            // ClientDLL_Init hooks these two for the engine; the client DLL never does.
            M::UserMessage { name, data, .. } if name.eq_ignore_ascii_case("ScreenShake") => self.shake.message(&data, self.clock.time),
            M::UserMessage { name, data, .. } if name.eq_ignore_ascii_case("ScreenFade") => self.level.fade.message(&data, self.clock.time),
            // Parse_svc_stufftext: the whole string, validated and queued as one piece.
            M::StuffText(text) => self.events.push_back(ClientEvent::StuffText(text)),
            M::ServerInfo(info) => self.server_info_received(info),
            M::ResourceRequest { spawncount, start_index } => {
                self.ui(UiEvent::ContinueProgress(6));
                self.resource_request(spawncount, start_index);
            }
            M::ResourceList(list) => {
                self.startup_mark("begin CL_ParseResourceList()");
                self.ui(UiEvent::Status("#GameUI_VerifyingResources".to_owned()));
                self.level.set_resources(&list);
                self.resource_list_received(list);
            }
            M::Time(t) => self.clock.server_time(f64::from(t), self.proto.maxclients == 1),
            M::LightStyle { index, pattern } => self.level.lightstyles[usize::from(index)] = pattern,
            M::NewMoveVars(mv) => {
                // CL_ParseMovevars: Cvar_SetValue mirrors of the wave height and zmax.
                for (name, v) in [("gl_wateramp", mv.wave_height), ("gl_zmax", mv.zmax)] {
                    self.events.push_back(ClientEvent::SetCvar { name: name.into(), value: gs_console::cvar::format_value(v) });
                }
                if let Some(log) = self.prediction.log.as_mut() {
                    log.movevars(self.now, &mv);
                }
                self.level.movevars = mv;
            }
            M::UpdateUserInfo { slot, userid, info, hash } => self.level.set_player(slot, userid, &info, hash),
            M::DecalName { index, name } => self.level.set_decal(index, &name),
            M::Pings(list) => {
                for p in list {
                    if let Some(info) = self.level.players.get_mut(usize::from(p.slot)) {
                        info.ping = i32::from(p.ping);
                        info.packet_loss = i32::from(p.loss);
                    }
                }
            }
            M::SetView(e) => self.level.view_entity = e,
            M::Intermission | M::Finale(_) | M::Cutscene(_) => {
                self.level.intermission = match &msg {
                    M::Intermission => 1,
                    M::Finale(_) => 2,
                    _ => 3,
                };
                self.events.push_back(ClientEvent::Message(msg));
            }
            // Each writes cl.viewangles in parse order: a setangle overrides an earlier addangle.
            M::SetAngle(a) => {
                self.level.set_angles = Some(a);
                self.level.add_yaw = 0.0;
            }
            M::AddAngle(yaw) => self.level.add_yaw += yaw,
            M::Events(list) => self.parse_events(list),
            M::ReliableEvent { index, mut args, delay } => {
                // Parse_svc_event_reliable: pitch fix only, no entity fill-in.
                let ent = args.entindex();
                if (1..=i32::from(self.proto.maxclients)).contains(&ent) {
                    let mut a = args.angles();
                    a[0] *= -3.0;
                    args.set_angles(a);
                }
                self.event_queue.queue(FEV_RELIABLE, index, delay, &args, self.clock.time);
            }
            M::WeaponAnim { sequence, body } => {
                self.level.weapon_starttime = 0.0;
                self.level.weapon_sequence = i32::from(sequence);
                self.level.weapon_body = i32::from(body);
            }
            M::SignonNum(n) => {
                if n <= self.signon {
                    self.disconnect(&format!("received signon {n} when at {}", self.signon));
                    return;
                }
                self.signon = n;
                if n == 1 {
                    self.send_string_cmd("sendents");
                }
                // CL_SignonReply: signon 2 ends the level load.
                if n == 2 {
                    let name = self.map_base_name();
                    self.ui(UiEvent::LoadingFinished { kind: "level".to_owned(), name });
                }
            }
            // CL_ParseClientdata: frames skipped since the last parse are dropped, this one is
            // stamped with realtime (the packet-loss estimate counts the drops) and its latency
            // against the acknowledged command.
            M::ClientData => {
                // HUD_TxferPredictionData on the spectator state (from the second clientdata
                // on): the HUD's observer mode and target reach the camera pmove only here.
                if self.spectator() && self.parsecount.is_some() {
                    self.spec_state.client.iuser[..3].copy_from_slice(&self.hud_users);
                }
                if let Some((seq, ack)) = self.netchan.as_ref().map(|c| (c.incoming_sequence, c.incoming_acknowledged)) {
                    if let Some(last) = self.parsecount {
                        for s in last.wrapping_add(1)..seq {
                            let slot = &mut self.frame_received[(s & UPDATE_MASK) as usize];
                            if *slot >= 0.0 {
                                *slot = -1.0;
                            }
                        }
                    }
                    self.parsecount = Some(seq);
                    self.frame_received[(seq & UPDATE_MASK) as usize] = self.now;
                    let sent = self.commands[(ack & UPDATE_MASK) as usize].senttime;
                    self.frame_latency[(seq & UPDATE_MASK) as usize] = self.now - f64::from(sent);
                }
            }
            M::PacketEntities { delta_from, .. } => {
                if self.signon == 1 {
                    self.signon = 2;
                    // CL_SignonReply(2): a redirect must have led to an HLTV proxy.
                    if std::mem::take(&mut self.redirected) && !self.spectator() {
                        self.print("Redirected to invalid server\n".into());
                        self.disconnect("Redirected to invalid server");
                        return;
                    }
                }
                // A recording's first uncompressed update: ask for the full client state.
                if delta_from.is_none() && self.demo.waiting {
                    self.demo.waiting = false;
                    self.send_string_cmd("fullupdate");
                }
                self.process_packet();
            }
            // Parse_svc_sendcvarvalue(2): the answer goes into the reliable stream right here, in
            // packet order with the other writes.
            M::SendCvarValue(name) => self.reply_cvar(None, &name, &cvars(&name)),
            M::SendCvarValue2 { request_id, name } => self.reply_cvar(Some(request_id), &name, &cvars(&name)),
            // Parse_svc_filetxferfailed: CL_ProcessFile with the failure (not in a playback).
            M::FileTransferFailed(name) => {
                if !self.demo.playing() {
                    self.print(format!("Error: server failed to transmit file '{name}'\n"));
                    self.process_file(&name, None);
                }
            }
            M::Customization(c) => self.customization(c),
            M::ResourceLocation(url) => self.resource_location(&url),
            // Parse_svc_timescale: the value only; forced to 1 outside HLTV and demos (`< 0.1` is
            // false for NaN, which therefore passes).
            M::TimeScale(v) => {
                let keep = (self.proto.spectator || self.demo.playing()) && v.partial_cmp(&0.1) != Some(std::cmp::Ordering::Less);
                self.sys_timescale = if keep { v } else { 1.0 };
            }
            // CL_Parse_HLTV (the flag itself was set while parsing).
            M::Hltv(h) => match h {
                Hltv::Active => self.redirected = false,
                // HLTV_LISTEN: the game stream's group, joined on the netchan's socket.
                Hltv::Listen(addr) => {
                    self.signon = 2;
                    if let Some(adr) = string_to_adr_no_port(&addr) {
                        self.mcast_hltv_adr = adr;
                    }
                    let group = self.mcast_hltv_adr;
                    self.join_group(group);
                    self.ui(UiEvent::LoadingFinished { kind: "transition".to_owned(), name: String::new() });
                }
                Hltv::Status { .. } => {}
                Hltv::Unknown(_) => self.print(" CL_Parse_HLTV: unknown HLTV command.\n".into()),
            },
            // Parse_svc_spawnbaseline's loading step (the baselines were read while parsing).
            M::SpawnBaseline => {
                self.ui(UiEvent::ContinueProgress(11));
                self.ui(UiEvent::Status("#GameUI_ParseBaseline".to_owned()));
                self.events.push_back(ClientEvent::Message(M::SpawnBaseline));
            }
            other => self.events.push_back(ClientEvent::Message(other)),
        }
    }

    /// `Parse_svc_event`: the args and delay carry over between entries that omit them; events
    /// without a packet index are dropped; the invoker's packet state fills in the rest.
    fn parse_events(&mut self, list: Vec<WireEvent>) {
        let mut eargs = EventArgs::default();
        let mut delay = 0.0f32;
        let maxclients = i32::from(self.proto.maxclients);
        let playernum = i32::from(self.proto.playernum);
        for e in list {
            if let Some(a) = e.args {
                eargs = a;
            }
            if let Some(d) = e.delay {
                delay = d;
            }
            let Some(pkt) = e.packet_index else { continue };
            let frame = self.proto.current_frame();
            if let Some(state) = frame.entities.get(usize::from(pkt)) {
                let n = state.number();
                eargs.set_entindex(n);
                if eargs.origin() == [0.0; 3] {
                    // HUD_TxferLocalOverrides gave the local player's frame state the
                    // clientdata origin before this message was parsed.
                    eargs.set_origin(if n - 1 == playernum { frame.clientdata.origin() } else { state.origin() });
                }
                if eargs.angles() == [0.0; 3] {
                    eargs.set_angles(state.angles());
                }
                eargs.set_angles(com_normalize_angles(eargs.angles()));
                if (1..=maxclients).contains(&n) {
                    let mut a = eargs.angles();
                    a[0] *= -3.0;
                    eargs.set_angles(a);
                    let velocity = if n - 1 == playernum {
                        frame.clientdata.velocity()
                    } else {
                        // Difference of the velocity fields over the animtime step (sic).
                        self.entities.get(n as usize).map_or([0.0; 3], |ent| {
                            let dt = ent.curstate.animtime() - ent.prevstate.animtime();
                            let (cur, prev) = (ent.curstate.velocity(), ent.prevstate.velocity());
                            if dt != 0.0 { [0, 1, 2].map(|i| (cur[i] - prev[i]) * (1.0 / dt)) } else { cur }
                        })
                    };
                    eargs.set_velocity(velocity);
                    eargs.set_ducking(i32::from(state.usehull() == 1));
                }
            } else {
                let ent = eargs.entindex();
                if ent == 0 {
                    log::debug!("CL_ParseEvent:  Received non-packet entity index 0 for event");
                } else if (1..=maxclients).contains(&ent) {
                    let mut a = eargs.angles();
                    a[0] *= -3.0;
                    eargs.set_angles(a);
                }
            }
            self.event_queue.queue(0, e.index, delay, &eargs, self.clock.time);
        }
    }

    fn server_info_received(&mut self, info: ServerInfo) {
        self.startup_mark("begin CL_ParseServerInfo()");
        if !info.gamedir.is_empty() && !info.gamedir.eq_ignore_ascii_case(&self.config.game) {
            self.disconnect(&format!("Server is running game {}", info.gamedir));
            return;
        }
        // CL_ClearState's resources (the local logo registered again) and cl.serverinfo.
        self.clear_resources();
        self.serverinfo.clear();
        // Parse_svc_serverinfo ends with sys_timescale = 1.
        self.sys_timescale = 1.0;
        self.print(format!("\n{}\n{}\n", info.hostname, info.map_model));
        self.ui(UiEvent::ContinueProgress(3));
        self.ui(UiEvent::Status("#GameUI_ParseServerInfo".to_owned()));
        if info.maxclients > 1 {
            let model = info.map_model.as_str();
            let file = model.rsplit(['/', '\\']).next().unwrap_or(model);
            let name = file.rsplit_once('.').map_or(file, |(b, _)| b).to_owned();
            self.ui(UiEvent::LoadingStarted { kind: "level".to_owned(), name });
        }
        // A passive client stops listening to the sign-on group here.
        if self.passive {
            let group = self.mcast_listen_adr;
            self.leave_group(group);
        }
        // CL_CheckCRCs.
        self.startup_mark("begin CL_CheckCRCs()");
        self.ui(UiEvent::ContinueProgress(4));
        self.ui(UiEvent::Status("#GameUI_CheckCRCs".to_owned()));
        let map = info.map_model.clone();
        match self.fs.read(&map).map_err(|e| e.to_string()).and_then(|d| bsp::map_crc(&d).map_err(|e| e.to_string())) {
            Ok(crc) if crc as i32 == info.map_crc => {}
            // CL_CheckCRCs: a playback ignores the map CRC.
            Ok(_) if self.demo.playing() => {}
            Ok(local) => {
                log::warn!("map CRC mismatch: local {local:#010x}, server {:#010x}", info.map_crc as u32);
                self.ui(UiEvent::Explain { text: format!("Your map [{map}] differs from the server's."), extended: false });
                return self.host_error("Disconnected");
            }
            // CRC_MapFile failed: a file that exists but cannot be checked ends the connection;
            // a missing one is downloaded unless downloads are off.
            Err(e) => {
                log::debug!("{map}: {e}");
                if self.fs.exists(&map) {
                    self.ui(UiEvent::Explain { text: "#GameUI_ConnectionFailed".to_owned(), extended: false });
                    return self.host_error("Disconnected");
                }
                if !self.config.allow_download {
                    let text = format!("Refusing to download map {map}, (cl_allow_download is 0 ) disconnecting.\n");
                    self.ui(UiEvent::Explain { text: truncate_encoded(&text, 255).into_owned(), extended: false });
                    return self.host_error("Disconnected");
                }
                self.print(format!("Couldn't find map {map}, server will download the map\n"));
            }
        }
        self.startup_mark("end   CL_CheckCRCs()");
        self.events.push_back(ClientEvent::Connected(info.clone()));
        self.level = Level::default();
        self.clear_cl_commands();
        self.entities.clear();
        self.event_queue.clear();
        let playernum = usize::from(info.playernum);
        self.server_info = Some(info);
        self.server_info_customizations(playernum);
        self.state = State::Connected;
        // CL_BeginDemoStartup; a serverinfo also ends any full-update wait.
        let mut out = Output::new();
        self.demo.begin_startup(&self.fs, &mut out);
        self.emit(out);
        self.demo.waiting = false;
        self.send_string_cmd("sendres");
        self.startup_mark("end   CL_ParseServerInfo()");
    }

    /// `CL_ProcessPacket` for the frame just parsed.
    fn process_packet(&mut self) {
        // HUD_TxferPredictionData (part of CL_ParseClientdata): unsent weapon slots keep the
        // predicted state.
        if let Some(chan) = self.netchan.as_ref() {
            self.prediction.transfer_weapons(self.proto.current_frame_mut(), chan.incoming_acknowledged);
        }
        let level = &self.level;
        let time = self.clock.time;
        let estimate = |s: &EntityState| level.estimate_frame(s, time);
        let frame = self.proto.current_frame();
        let ctx = PacketContext {
            mtime: self.proto.mtime[0],
            time,
            maxclients: usize::from(self.proto.maxclients),
            playernum: usize::from(self.proto.playernum),
            model_names: &level.models,
            local_origin: frame.clientdata.origin(),
            estimate_frame: &estimate,
        };
        self.entities.process_packet(frame, &ctx);
        // HUD_ProcessPlayerState runs for the local player when its state came in.
        let local = i32::from(self.proto.playernum) + 1;
        if frame.entities.iter().any(|s| s.number() == local) {
            self.local_player_update = true;
            // CL_CheckPredictionError for the local player's new state.
            if let Some(chan) = self.netchan.as_ref() {
                let maxclients = usize::from(self.proto.maxclients);
                let at = (self.now, usize::from(self.proto.playernum), "packet");
                check_prediction_error(&mut self.prediction, frame, chan, maxclients, &self.config, &mut self.pred_error_counter, &mut self.events, at);
            }
        }
    }
}
