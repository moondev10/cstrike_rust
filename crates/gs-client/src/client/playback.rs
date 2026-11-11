//! Demos (docs/re/demo.md, demoplayer-engine.md): the recording hooks, playback from the file
//! reader or the DemoPlayer module, `Host_EndGame`'s demo loop, and the demo console commands.

use std::net::{Ipv4Addr, SocketAddrV4};

use gs_common::bytestr::truncate_encoded;
use gs_net::Netchan;
use gs_protocol::structs::EventArgs;
use gs_protocol::{ProtocolState, ServerMessage};

use super::{Client, ClientEvent, CvarReply, State, UiEvent};
use crate::demo::{self, Ctx, Demo, DemoClientData, DemoInfo, DemoItem, Output, ReadStatus, ViewReplay};

impl Client {
    /// `CL_DemoSnapshotView`: the pass-0 refdef of every rendered frame outside a playback.
    pub fn demo_snapshot(&mut self, info: DemoInfo) {
        if !self.demo.playing() {
            self.demo.info = info;
        }
    }

    /// `CL_DemoInterpolateView` for a playback that is not an HLTV demo; with the DemoPlayer
    /// module `DemoPlayer_GetDemoViewInfo` (`None` when it left the view untouched).
    pub fn demo_view(&self) -> Option<ViewReplay> {
        if !self.demo.playing() || self.spectator() {
            return None;
        }
        if self.demo_player_is_active() {
            let p = self.demo_player.as_ref()?;
            let mut rp = [0u8; 0xE8];
            let (mut view, mut vm) = ([0.0f32; 3], 0i32);
            if !p.borrow_mut().demo_view_info(&mut rp, &mut view, &mut vm) {
                return None;
            }
            return Some(ViewReplay { rp: demo::RefParamsRaw::from_bytes(&rp), view: Some(view), viewmodel: Some(vm) });
        }
        Some(self.demo.interpolate_view(self.now))
    }

    /// `Cmd_ExecuteStringWithPrivilegeCheck`: a client-DLL command ran.
    pub fn demo_record_hud_command(&mut self, name: &str) {
        if self.demo.recording() && !self.spectator() {
            self.demo.record_hud_command(name, self.now, self.host_framecount);
        }
    }

    /// `ClientDLL_UpdateClientData`'s type-4 frame (the data before the client changed it).
    pub fn demo_record_client_data(&mut self, cd: &DemoClientData) {
        if self.state == State::Active && !self.demo.playing() && self.demo.recording() {
            self.demo.record_client_data(cd, self.now, self.host_framecount);
        }
    }

    /// `pDemoAPI->WriteBuffer`.
    pub fn demo_write_buffer(&mut self, data: &[u8]) {
        self.demo.record_buffer(data, self.now, self.host_framecount);
    }

    /// `pDemoAPI->IsRecording` / `IsPlayingback`.
    pub fn demo_recording(&self) -> bool {
        self.demo.recording()
    }

    pub fn demo_playing(&self) -> bool {
        self.demo.playing()
    }

    pub(super) fn emit(&mut self, out: Output) {
        for (dev, text) in out {
            if dev {
                self.events.push_back(ClientEvent::DevPrint(text));
            } else {
                self.print(text);
            }
        }
    }

    pub(super) fn with_ctx<R>(&mut self, f: impl FnOnce(&mut Demo, &mut Ctx) -> R) -> R {
        let mut out = Output::new();
        let r = {
            let mut ctx = Ctx { fs: &self.fs, now: self.now, frame: self.host_framecount, out: &mut out };
            f(&mut self.demo, &mut ctx)
        };
        self.emit(out);
        r
    }

    /// `CL_StopPlayback`.
    pub(super) fn stop_playback(&mut self) {
        if !self.demo.playing() {
            return;
        }
        let (gg, version) = (self.cl_gg, self.version.clone());
        // DemoPlayer_IsActive → DemoPlayer_Stop, else FS_Close(cls.demofile).
        if self.demo_player_is_active()
            && let Some(p) = self.demo_player.clone()
        {
            p.borrow_mut().stop();
        }
        let result = self.with_ctx(|demo, ctx| demo.stop_playback(ctx, gg, &version));
        if result.gg_reset {
            self.cl_gg = false;
            self.events.push_back(ClientEvent::SetCvar { name: "cl_gg".into(), value: "0".into() });
        }
        if result.quit {
            self.events.push_back(ClientEvent::Quit);
        }
        self.events.push_back(ClientEvent::Demo(DemoItem::EngineText("cd stop\n".into())));
    }

    /// `Host_EndGame`: the demo loop's next entry, or a disconnect and `cd stop`.
    fn end_game(&mut self, message: &str) {
        self.events.push_back(ClientEvent::DevPrint(format!("Host_EndGame: {message}\n")));
        let demonum = self.demo.demonum;
        self.disconnect(message.trim_end());
        if demonum != -1 {
            self.demo.demonum = demonum;
            self.next_demo();
        } else {
            self.events.push_back(ClientEvent::Execute("cd stop\n".into()));
        }
    }

    /// `CL_NextDemo`.
    fn next_demo(&mut self) {
        let mut out = Output::new();
        let line = self.demo.next_demo(&mut out);
        self.emit(out);
        if let Some(line) = line {
            self.events.push_back(ClientEvent::InsertText(line));
        }
    }

    /// `CL_ReadPackets` in a playback: frames until one is not due, each packet parsed with
    /// the recorded netchan counters.
    pub(super) fn read_demo(&mut self, cvars: CvarReply) {
        loop {
            let mut items = Vec::new();
            let status = self.demo.read(self.now, self.host_framecount, &mut items);
            for item in items {
                self.demo_item(item);
            }
            match status {
                ReadStatus::Wait => return,
                ReadStatus::End(message) => return self.end_game(message),
                ReadStatus::Packet { data, sequences } => {
                    if let Some(chan) = self.netchan.as_mut() {
                        chan.set_demo_sequences(sequences);
                    }
                    if data.get(..4) == Some(&[0xFF; 4]) {
                        log::debug!("connectionless packet in a demo");
                        continue;
                    }
                    if self.state < State::Connected {
                        continue;
                    }
                    self.parse(&data, sequences[0] as u32, cvars, 0);
                    if self.state == State::Active {
                        self.proto.current_frame_mut().bytes.msgbytes = data.len().min(usize::from(u16::MAX)) as u16;
                    }
                    if !self.demo.playing() {
                        return;
                    }
                }
            }
        }
    }

    /// The engine part of a demo sub-frame; the rest goes to the host.
    fn demo_item(&mut self, item: DemoItem) {
        match item {
            DemoItem::Event { flags, index, delay, args } => self.event_queue.queue(flags, index as u16, delay, &args, self.clock.time),
            // cle_pfnWeaponAnim.
            DemoItem::WeaponAnim { anim, body } => {
                self.level.weapon_sequence = anim;
                self.level.weapon_starttime = 0.0;
                self.level.weapon_body = body;
            }
            DemoItem::Fade(fade) => self.level.fade.message(&fade, self.clock.time),
            other => self.events.push_back(ClientEvent::Demo(other)),
        }
    }

    /// `cmd_playdemo` (`argv` as typed).
    fn playdemo(&mut self, argv: &[&str]) {
        if !(2..=3).contains(&argv.len()) {
            demo::PLAYDEMO_USAGE.iter().for_each(|l| self.print((*l).to_owned()));
            return;
        }
        let speed = demo::parse_speed(argv.get(2).copied());
        self.print(demo::speed_line(speed));
        self.disconnect("playdemo");
        self.demo_player_active = false;
        let mut corrupt = false;
        let name = argv[1].to_owned();
        let ok = self.with_ctx(|d, ctx| d.open_playback(ctx, &name, speed, &mut corrupt));
        if corrupt {
            self.disconnect("Error: Corrupt demo file.");
        }
        if !ok {
            return;
        }
        // cls.state = connected; Netchan_Setup with no remote; the HUD's InitHUD.
        self.state = State::Connected;
        self.signon = 0;
        let msg_stats = self.proto.msg_stats;
        self.proto = ProtocolState::default();
        self.proto.msg_stats = msg_stats;
        self.passive = false;
        self.netchan = Some(Netchan::new(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0), self.now));
        self.last_sent_sequence = None;
        self.next_cmd_time = self.now as f32;
        self.events.push_back(ClientEvent::Message(ServerMessage::UserMessage { id: 0, name: "InitHUD".into(), data: Vec::new() }));
    }

    /// `DemoPlayer_IsActive`: the module is loaded and `viewdemo` set the flag.
    pub fn demo_player_is_active(&self) -> bool {
        self.demo_player.is_some() && self.demo_player_active
    }

    /// `CL_GetDemoMessage`'s module path (demoplayer-engine.md §3.4): `ReadDemoMessage` until it
    /// returns nothing; the engine calls the module made meanwhile run first, then the packet is
    /// parsed with the module's netchan counters.
    pub(super) fn read_module(&mut self, cvars: CvarReply) {
        let Some(p) = self.demo_player.clone() else { return };
        loop {
            let mut items = Vec::new();
            let mut set_cvars = Vec::new();
            let (data, sequences) = {
                let mut eng = ModuleEngine { items: &mut items, cvars: &mut set_cvars, filter: self.filter_stuff_cmd };
                let mut dp = p.borrow_mut();
                let data = dp.read_demo_message(0x10000, &mut eng);
                (data, dp.read_netchan_state())
            };
            for (name, value) in set_cvars {
                self.events.push_back(ClientEvent::SetCvar { name, value });
            }
            for item in items {
                self.demo_item(item);
            }
            if data.is_empty() {
                return;
            }
            if let Some(chan) = self.netchan.as_mut() {
                chan.set_demo_sequences(sequences);
            }
            if data.get(..4) == Some(&[0xFF; 4]) {
                log::debug!("connectionless packet from the demo player");
                continue;
            }
            if self.state < State::Connected {
                continue;
            }
            self.parse(&data, sequences[0] as u32, cvars, 0);
            if self.state == State::Active {
                self.proto.current_frame_mut().bytes.msgbytes = data.len().min(usize::from(u16::MAX)) as u16;
            }
            if !self.demo.playing() {
                return;
            }
        }
    }

    /// `cmd_viewdemo` (demoplayer-engine.md §3.2).
    fn viewdemo(&mut self, argv: &[&str]) {
        self.demo_player_active = true;
        if !self.demo_player_is_active() {
            self.print("viewdemo not available.\n".into());
            return;
        }
        self.ui(UiEvent::ShowDemoUi);
        if argv.len() != 2 {
            self.print("viewdemo <demoname>: shows a demo\n".into());
            return;
        }
        self.disconnect("viewdemo");
        let mut name = truncate_encoded(argv[1], 255).into_owned();
        let base = name.rsplit(['/', '\\']).next().unwrap_or("");
        if !base.contains('.') {
            name.push_str(".dem");
        }
        self.print(format!("Playing demo from {name}.\n"));
        let ok = self.demo_player.clone().is_some_and(|p| p.borrow_mut().load_game(&name));
        if !ok {
            self.print(format!("Viewdemo: couldn't load demo {name}.\n"));
            self.demo.demonum = -1;
            return;
        }
        self.demo.module = true;
        self.state = State::Connected;
        self.signon = 0;
        let msg_stats = self.proto.msg_stats;
        self.proto = ProtocolState::default();
        self.proto.msg_stats = msg_stats;
        self.passive = false;
        self.netchan = Some(Netchan::new(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0), self.now));
        self.last_sent_sequence = None;
        self.next_cmd_time = self.now as f32;
        self.events.push_back(ClientEvent::Message(ServerMessage::UserMessage { id: 0, name: "InitHUD".into(), data: Vec::new() }));
    }

    /// The demo console commands; `false` when `argv[0]` is not one.
    pub fn demo_command(&mut self, argv: &[&str]) -> bool {
        let Some(name) = argv.first().map(|a| a.to_ascii_lowercase()) else { return false };
        let active = self.state == State::Active;
        let (map, crc) = self.server_info.as_ref().map_or((String::new(), 0), |i| (i.map_model.clone(), i.map_crc));
        match name.as_str() {
            "record" => self.with_ctx(|d, ctx| d.cmd_record(ctx, argv, active, &map, crc)),
            "stop" => self.with_ctx(|d, ctx| d.cmd_stop(ctx)),
            "appenddemo" => self.with_ctx(|d, ctx| d.cmd_appenddemo(ctx, argv, active, &map)),
            "listdemo" => self.with_ctx(|d, ctx| d.cmd_listdemo(ctx, argv)),
            "swapdemo" => self.with_ctx(|d, ctx| d.cmd_swapdemo(ctx, argv)),
            "setdemoinfo" => self.with_ctx(|d, ctx| d.cmd_setdemoinfo(ctx, argv)),
            "removedemo" => self.with_ctx(|d, ctx| d.cmd_removedemo(ctx, argv)),
            "playdemo" => self.playdemo(argv),
            "timedemo" => {
                if argv.len() != 2 {
                    self.print("timedemo <demoname>: gets demo speeds\n".into());
                } else {
                    self.playdemo(argv);
                    self.demo.start_timedemo(self.host_framecount, self.now, false);
                }
            }
            "gg" => {
                if argv.len() != 2 {
                    self.print("gg <demoname> : Game Gauge 99\n".into());
                } else {
                    self.playdemo(argv);
                    self.cl_gg = true;
                    self.events.push_back(ClientEvent::SetCvar { name: "cl_gg".into(), value: "1".into() });
                    self.demo.start_timedemo(self.host_framecount, self.now, true);
                }
            }
            "viewdemo" => self.viewdemo(argv),
            "upload" => self.cmd_upload(argv),
            "httpstop" => self.cmd_httpstop(),
            "startdemos" => {
                let mut n = argv.len().saturating_sub(1);
                if n > demo::MAX_DEMOS {
                    self.print(format!("Max {} demos in demoloop\n", demo::MAX_DEMOS));
                    n = demo::MAX_DEMOS;
                }
                self.print(format!("{n} demo(s) in loop\n"));
                for (slot, arg) in self.demo.demos.iter_mut().zip(&argv[1..=n]) {
                    *slot = truncate_encoded(arg.split([';', '\n']).next().unwrap_or(""), 15).into_owned();
                }
                if self.demo.demonum == -1 || self.demo.playing() {
                    self.demo.demonum = -1;
                } else {
                    self.demo.demonum = 0;
                    self.next_demo();
                }
            }
            "demos" => {
                if self.demo.demonum == -1 {
                    self.demo.demonum = 0;
                }
                self.disconnect("demos");
                self.next_demo();
            }
            "stopdemo" => {
                if self.demo.playing() {
                    self.stop_playback();
                    self.disconnect("stopdemo");
                }
            }
            _ => return false,
        }
        true
    }
}

/// DemoPlayer's `IEngineWrapper` calls during `ReadDemoMessage` (demoplayer-engine.md §5.2): the
/// client-side ones become the file reader's sub-frame items, `Cvar_Set` goes to the host.
struct ModuleEngine<'a> {
    items: &'a mut Vec<DemoItem>,
    cvars: &'a mut Vec<(String, String)>,
    /// `cl_filterstuffcmd > 0`.
    filter: bool,
}

impl gs_hltv::player::Engine for ModuleEngine<'_> {
    fn set_cvar(&mut self, name: &str, value: &str) {
        self.cvars.push((name.to_owned(), value.to_owned()));
    }

    fn demo_update_client_data(&mut self, cd: &[u8]) {
        self.items.push(DemoItem::ClientData(DemoClientData::read(cd)));
    }

    fn queue_event(&mut self, flags: i32, index: i32, delay: f32, args: &[u8]) {
        let mut a = EventArgs::default();
        let dst = a.bytes_mut();
        let n = args.len().min(dst.len());
        dst[..n].copy_from_slice(&args[..n]);
        self.items.push(DemoItem::Event { flags, index, delay, args: a });
    }

    fn weapon_anim(&mut self, anim: i32, body: i32) {
        self.items.push(DemoItem::WeaponAnim { anim, body });
    }

    /// `CL_DemoPlaySound` gets `(attenuation, volume)` as declared; [`DemoItem::Sound`] carries
    /// the file reader's order, which the host swaps, so the pair goes in reversed.
    fn play_sound(&mut self, channel: i32, sample: &[u8], attenuation: f32, volume: f32, flags: i32, pitch: i32) {
        let sample = demo::c_string(sample);
        self.items.push(DemoItem::Sound { channel, sample, volume: attenuation, attenuation: volume, flags, pitch });
    }

    fn read_demo_buffer(&mut self, data: &[u8]) {
        self.items.push(DemoItem::Buffer(data.to_vec()));
    }

    /// `CL_FilteredCmdAllowed` (no single-player bypass in a playback).
    fn valid_stuff_text(&mut self, cmd: &[u8]) -> bool {
        let policy = gs_console::stufftext::StuffPolicy { filter: self.filter, ..Default::default() };
        gs_console::stufftext::valid_stuff_text(&String::from_utf8_lossy(cmd), policy)
    }

    /// `Cbuf_AddFilteredText(cmd)` then `"\n"`: [`DemoItem::Command`] is the pair.
    fn add_filtered_text(&mut self, text: &[u8]) {
        if text != b"\n" {
            self.items.push(DemoItem::Command(demo::c_string(text)));
        }
    }
}
