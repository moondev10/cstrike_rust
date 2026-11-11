//! `DemoPlayer.dll` (`demoplayer001`, docs/re/demoplayer.md): the `viewdemo` player over the
//! DemoWorld/DemoServer pair — the playback clock, the HLTV-style stream it hands the engine
//! (`ReadDemoMessage`), the view (`GetDemoViewInfo`), the demo-file side channels and the editable
//! director command list.

use crate::Sys;
use crate::bitbuf::BitBuffer;
use crate::dict::Dict;
use crate::director::{CAMERA, CAMPATH, DirectorCmd, TIMESCALE, WAYPOINTS, tokens};
use crate::server::{Director, Server, deliver, save_as_demo};
use crate::world::{DEMO_INFO_SIZE, World};

/// The engine calls DemoPlayer makes (`IEngineWrapper`, demoplayer-engine.md §5.2).
pub trait Engine {
    fn set_cvar(&mut self, name: &str, value: &str);
    /// `DemoUpdateClientData(client_data_t*)` (32 bytes).
    fn demo_update_client_data(&mut self, cd: &[u8]);
    /// `CL_QueueEvent(flags, index, delay, event_args_t*)` (72 bytes).
    fn queue_event(&mut self, flags: i32, index: i32, delay: f32, args: &[u8]);
    fn weapon_anim(&mut self, anim: i32, body: i32);
    /// `CL_DemoPlaySound(channel, sample, attenuation, volume, flags, pitch)`.
    fn play_sound(&mut self, channel: i32, sample: &[u8], attenuation: f32, volume: f32, flags: i32, pitch: i32);
    fn read_demo_buffer(&mut self, data: &[u8]);
    fn valid_stuff_text(&mut self, cmd: &[u8]) -> bool;
    fn add_filtered_text(&mut self, text: &[u8]);
}

/// The view `GetDemoViewInfo` produces: `ref_params_t` bytes (0xE8), the view-model origin and index.
#[derive(Debug, Clone)]
pub struct DemoView {
    pub rp: [u8; 0xE8],
    pub view: [f32; 3],
    pub viewmodel: i32,
}

/// The `IDirector` part of the player (what the server and world call back into).
pub struct Core {
    sys: Sys,
    cmds: Vec<Option<DirectorCmd>>,
    commands: Dict<usize>,
    last_cmd: Option<usize>,
    pub file_name: String,
    /// 0 idle, 1 loading, 2 new game, 3 sending signon, 4 playing.
    pub player_state: i32,
    stream: BitBuffer,
    pub edit_mode: bool,
    pub master_mode: bool,
    pub force_hltv: bool,
    is_saving: bool,
    pub time_scale: f32,
    pub world_time: f64,
    pub client_time: f64,
    last_cmd_time: f64,
    pub paused: bool,
    last_system_time: f64,
    last_frame_seq: u32,
    delta_frame_seq: u32,
    frame_count: i32,
    system_time: f64,
    /// Signals for the GameUI listener (`FireSignal`): 1 = commands changed, 2 = command fired (index).
    ui_signals: Vec<(i32, i32)>,
}

impl Core {
    /// `SetTimeScale(s)`: clamped to 0.05..4, then svc_timescale into the stream.
    pub fn set_time_scale(&mut self, s: f32) {
        self.time_scale = s.clamp(0.05, 4.0);
        self.stream.write_byte(55);
        self.stream.write_float(self.time_scale);
    }

    /// `SetWorldTime(t, relative)`.
    pub fn set_world_time(&mut self, t: f64, relative: bool) {
        self.world_time = if relative { self.world_time + t } else { t };
    }

    fn cmd(&self, id: usize) -> Option<&DirectorCmd> {
        self.cmds.get(id).and_then(Option::as_ref)
    }

    /// `ReindexCommands` + `FireSignal(1)`.
    fn reindex(&mut self) {
        let ids: Vec<usize> = self.commands.iter().map(|(id, _)| id).collect();
        for (i, id) in ids.into_iter().enumerate() {
            if let Some(c) = self.cmds.get_mut(id).and_then(Option::as_mut) {
                c.index = i as i32 + 1;
            }
        }
        self.ui_signals.push((1, 0));
    }

    fn clear_commands(&mut self) {
        self.cmds.clear();
        self.commands.clear();
        self.last_cmd = None;
    }

    /// `NewGame(world)` (World signal 2).
    fn new_game(&mut self, world: &mut World) {
        self.client_time = 1.0;
        self.player_state = 2;
        self.clear_commands();
        self.ui_signals.push((1, 0));
        if world.is_hltv || self.force_hltv {
            world.add_signon_data(51, &[0x01, 0x01]);
        }
    }

    /// `WriteCameraPath(cmd, stream)` (§10.7).
    fn write_camera_path(&mut self, id: usize, s: &mut BitBuffer) {
        let Some(t0) = self.cmd(id).map(|c| c.time) else { return };
        let mut list: Vec<usize> = Vec::new();
        let mut first = true;
        let mut c = self.commands.find_exact(t0);
        while let Some(cid) = c {
            let Some(cmd) = self.cmd(cid) else { break };
            if cmd.kind != CAMPATH {
                break;
            }
            let flags = cmd.camera_data().3;
            if flags & 1 != 0 {
                if !first {
                    break;
                }
                first = false;
            }
            list.push(cid);
            c = self.commands.get_next();
        }
        if list.is_empty() {
            self.sys.print("Warning! No waypoints in camera path!\n");
            return;
        }
        let n = list.len();
        let first_size = self.cmd(list[0]).map_or(0, |c| c.data.len());
        let size = (first_size + 2) * n + 2;
        if size > 250 {
            self.sys.print("Warning! Too many waypoints in a camera path!\n");
            return;
        }
        s.write_byte(51);
        s.write_byte(size as u8);
        s.write_byte(WAYPOINTS as u8);
        s.write_byte(n as u8);
        for w in list {
            if let Some(c) = self.cmd(w) {
                s.write_word(((c.time - t0) * 100.0) as i32);
                s.write_buf(&c.data.clone());
            }
        }
    }
}

impl Director for Core {
    /// `AddCommand(cmd)` → its index (0 for WAYPOINTS).
    fn add_command(&mut self, cmd: &DirectorCmd) -> i32 {
        if cmd.kind == WAYPOINTS {
            return 0;
        }
        let id = self.cmds.len();
        self.cmds.push(Some(cmd.clone()));
        self.commands.add(id, cmd.time);
        self.reindex();
        self.cmd(id).map_or(0, |c| c.index)
    }

    fn world_signal(&mut self, world: &mut World, sig: i32, _data: i32) {
        if sig == 2 {
            self.new_game(world);
        }
    }

    fn server_signal(&mut self, sig: i32) {
        if sig == 6 {
            self.sys.print("Demo file completely loaded.\n");
        }
    }

    /// `WriteCommands(stream, start, end)` (§10.6): the window `(start, end]`.
    fn write_commands(&mut self, s: &mut BitBuffer, start: f32, end: f32) {
        let mut c = self.commands.find_closest(start);
        while let Some(id) = c {
            let Some(cmd) = self.cmd(id).cloned() else { break };
            if cmd.time > end {
                break;
            }
            if cmd.time > start {
                if self.is_saving {
                    cmd.write_to_stream(s);
                } else {
                    match cmd.kind {
                        TIMESCALE => {
                            let v = cmd.time_scale_data();
                            s.write_byte(55);
                            s.write_float(v);
                            self.time_scale = v;
                        }
                        CAMPATH => {
                            if cmd.camera_data().3 & 1 != 0 {
                                // The path walk moves the shared cursor of `m_Commands`: the
                                // outer loop continues after the path.
                                self.write_camera_path(id, s);
                            }
                        }
                        _ => cmd.write_to_stream(s),
                    }
                    self.last_cmd = Some(id);
                    self.ui_signals.push((2, cmd.index));
                    self.sys.dprint(&format!("Director Cmd {}, Time {:.2}\n", cmd.name(), f64::from(cmd.time)));
                }
            }
            c = self.commands.get_next();
        }
    }
}

/// `DemoPlayer` with its two Core modules.
pub struct DemoPlayer {
    pub sys: Sys,
    pub world: World,
    pub server: Server,
    pub core: Core,
}

impl DemoPlayer {
    /// `Init(system, serial, name)` (§4.1).
    pub fn new(sys: Sys) -> Self {
        let world = World::new(sys.clone());
        let mut server = Server::new(sys.clone());
        server.has_director = true;
        server.delay_reconnect = false;
        let mut core = Core {
            sys: sys.clone(),
            cmds: Vec::new(),
            commands: Dict::default(),
            last_cmd: None,
            file_name: String::new(),
            player_state: 0,
            stream: BitBuffer::new(0x10000),
            edit_mode: false,
            master_mode: true,
            force_hltv: false,
            is_saving: false,
            time_scale: 1.0,
            world_time: 0.0,
            client_time: 0.0,
            last_cmd_time: 0.0,
            paused: false,
            last_system_time: 0.0,
            last_frame_seq: 0,
            delta_frame_seq: 0,
            frame_count: 0,
            system_time: 0.0,
            ui_signals: Vec::new(),
        };
        core.set_time_scale(1.0);
        core.paused = false;
        sys.dprint("DemoPlayer module initialized.\n");
        Self { sys, world, server, core }
    }

    /// The registered console commands (`RegisterCommand`, ids 1..6 in this order).
    pub const COMMANDS: [&'static str; 6] = ["dem_jump", "dem_forcehltv", "dem_pause", "dem_speed", "dem_start", "dem_save"];

    /// GameUI-listener signals since the last call.
    pub fn take_ui_signals(&mut self) -> Vec<(i32, i32)> {
        std::mem::take(&mut self.core.ui_signals)
    }

    /// `IsActive`.
    pub fn is_active(&self) -> bool {
        self.core.player_state != 0
    }

    /// `IsLoading`.
    pub fn is_loading(&self) -> bool {
        self.server.is_connected()
    }

    pub fn is_paused(&self) -> bool {
        self.core.paused
    }

    pub fn set_paused(&mut self, p: bool) {
        self.core.paused = p;
    }

    pub fn set_time_scale(&mut self, s: f32) {
        self.core.set_time_scale(s);
    }

    pub fn set_world_time(&mut self, t: f64, relative: bool) {
        self.core.set_world_time(t, relative);
    }

    pub fn world_time(&self) -> f64 {
        self.core.world_time
    }

    pub fn time_scale(&self) -> f32 {
        self.core.time_scale
    }

    /// `GetStartTime`.
    pub fn start_time(&mut self) -> f64 {
        self.world.first_frame().and_then(|f| self.world.frame(f)).map_or(0.0, |f| f64::from(f.time))
    }

    /// `GetEndTime`.
    pub fn end_time(&self) -> f64 {
        self.world.last_frame().and_then(|f| self.world.frame(f)).map_or(0.0, |f| f64::from(f.time))
    }

    /// The commands in time order (`GetCommands`).
    pub fn commands(&self) -> Vec<DirectorCmd> {
        self.core.commands.iter().filter_map(|(id, _)| self.core.cmd(id).cloned()).collect()
    }

    /// `GetLastCommand`'s index (0 = none).
    pub fn last_command_index(&self) -> i32 {
        self.core.last_cmd.and_then(|id| self.core.cmd(id)).map_or(0, |c| c.index)
    }

    pub fn add_command(&mut self, cmd: &DirectorCmd) -> i32 {
        self.core.add_command(cmd)
    }

    /// `RemoveCommand(index)`.
    pub fn remove_command(&mut self, index: i32) -> bool {
        let found = self.core.commands.iter().find(|&(id, _)| self.core.cmd(id).is_some_and(|c| c.index == index)).map(|(id, _)| id);
        let Some(id) = found else { return false };
        self.core.commands.remove(id);
        if self.core.last_cmd == Some(id) {
            self.core.last_cmd = None;
        }
        self.core.cmds[id] = None;
        self.core.reindex();
        true
    }

    /// `ExecuteDirectorCmd(cmd)` (§10.5).
    pub fn execute_director_cmd(&mut self, cmd: &DirectorCmd) {
        match cmd.kind {
            CAMERA => {
                let (pos, ang, fov, _) = cmd.camera_data();
                let mut c = cmd.clone();
                c.set_camera_data(pos, ang, fov, 0);
                c.write_to_stream(&mut self.core.stream);
            }
            TIMESCALE => self.core.set_time_scale(cmd.time_scale_data()),
            _ => cmd.write_to_stream(&mut self.core.stream),
        }
    }

    /// `LoadGame(file)` (§4.3).
    pub fn load_game(&mut self, file: &str) -> bool {
        if !self.server.load_demo(file, self.core.force_hltv, false) {
            return false;
        }
        self.core.file_name = gs_common::bytestr::truncate_encoded(file, 0x103).into_owned();
        self.world.set_buffer_size(-1.0);
        self.core.player_state = 1;
        self.core.frame_count = 0;
        self.core.last_system_time = 0.0;
        self.core.last_cmd_time = 0.0;
        self.core.master_mode = true;
        true
    }

    /// `Stop()`.
    pub fn stop(&mut self) {
        self.server.disconnect(&mut self.world, &mut self.core);
        self.world.reset();
        self.core.player_state = 0;
    }

    /// `SaveGame(file)` (§10.8).
    pub fn save_game(&mut self, file: Option<&str>) -> bool {
        if self.is_loading() {
            return false;
        }
        self.core.paused = true;
        self.core.is_saving = true;
        let name = file.map_or_else(|| self.core.file_name.clone(), str::to_owned);
        let sys = self.sys.clone();
        let ok = save_as_demo(&mut self.world, &sys, &name, &mut self.core);
        self.core.is_saving = false;
        ok
    }

    /// The `SystemWrapper` frame for the three modules in list order: DemoPlayer, DemoWorld,
    /// DemoServer (each `RunFrame(accumulated time)`).
    pub fn run_modules(&mut self, time: f64) {
        self.sys.time.set(time);
        if self.sys.stopped.get() {
            return;
        }
        self.run_frame(time);
        if self.sys.stopped.get() {
            return;
        }
        self.world.system_time = time;
        if self.sys.stopped.get() {
            return;
        }
        self.server.run_frame(time, &mut self.world, &mut self.core);
    }

    /// `RunFrame(time)` (§6.2).
    fn run_frame(&mut self, time: f64) {
        let c = &mut self.core;
        c.system_time = time;
        if c.player_state == 0 {
            return;
        }
        let dt = f64::from((c.system_time - c.last_system_time) as f32);
        c.last_system_time = c.system_time;
        c.client_time += dt;
        if !self.world.is_active() || c.paused {
            return;
        }
        c.world_time += dt;
        let cur = self.world.frame_by_seq(c.last_frame_seq).and_then(|f| self.world.frame(f)).map(|f| f.time);
        let next = self.world.frame_by_seq(c.last_frame_seq + 1).and_then(|f| self.world.frame(f)).map(|f| f.time);
        if let (Some(a), Some(b)) = (cur, next)
            && b - a > 2.0
        {
            c.world_time = f64::from(b - 0.01);
        }
        let last = self.world.last_frame().and_then(|f| self.world.frame(f)).map(|f| f.time);
        let first = self.world.first_frame().and_then(|f| self.world.frame(f)).map(|f| f.time);
        if let (Some(l), Some(f)) = (last, first) {
            if c.world_time > f64::from(l) {
                c.world_time = f64::from(l);
            } else if f64::from(f) > c.world_time {
                c.world_time = f64::from(f - 0.01);
            }
        }
    }

    /// `ReadDemoMessage(buf, size)` (§7): the next engine message (empty = none this time).
    pub fn read_demo_message(&mut self, size: usize, engine: &mut dyn Engine) -> Vec<u8> {
        let old = self.core.last_frame_seq;
        match self.core.player_state {
            2 => {
                self.world.write_new_data(&mut self.core.stream);
                self.core.player_state = 3;
            }
            3 => {
                self.core.last_frame_seq = 0;
                self.core.delta_frame_seq = 0;
                self.world.write_signon_data(&mut self.core.stream);
                self.core.stream.write_byte(7);
                self.core.stream.write_float(1.0);
                let mut i = 0;
                while i < self.world.max_clients {
                    self.world.write_client_update(&mut self.core.stream, i);
                    i += 1;
                }
                self.world.write_light_styles(&mut self.core.stream);
                self.core.stream.write_byte(25);
                self.core.stream.write_byte(1);
                engine.set_cvar("spec_pip", "0");
                self.core.world_time = 0.0;
                self.core.set_time_scale(1.0);
                self.core.paused = false;
                self.core.player_state = 4;
            }
            4 => {
                self.play_frame();
                let new = self.core.last_frame_seq;
                if new > old {
                    for s in old + 1..=new {
                        let data = self.world.frame_by_seq(s).and_then(|f| self.world.frame(f)).map(|f| f.demo_data.clone());
                        if let Some(d) = data.filter(|d| !d.is_empty()) {
                            self.execute_demo_file_commands(&d, engine);
                        }
                    }
                }
            }
            _ => {}
        }
        // Flush.
        let n = self.core.stream.current_size();
        if n == 0 {
            return Vec::new();
        }
        if n > size {
            self.sys.print(&format!("ERROR! DemoPlayer::ReadDemoMessage: data overflow ({n} bytes).\n"));
            self.core.stream.clear();
            return Vec::new();
        }
        let out = self.core.stream.bytes().to_vec();
        self.core.stream.fast_clear();
        out
    }

    /// State 4: at most one world frame per message (§7.2).
    fn play_frame(&mut self) {
        let Some(fid) = self.world.frame_by_time(self.core.world_time) else { return };
        let Some((ftime, fseq)) = self.world.frame(fid).map(|f| (f.time, f.seqnr)) else { return };
        let client_time = self.core.client_time - (self.core.world_time - f64::from(ftime));
        let mut cur = self.core.last_frame_seq;
        if cur == 0 || cur > fseq {
            cur = fseq - 1;
            self.core.last_frame_seq = cur;
        }
        if cur < fseq {
            self.core.stream.write_byte(7);
            self.core.stream.write_float(client_time as f32);
            let (last, delta, count) = (self.core.last_frame_seq, self.core.delta_frame_seq, self.core.frame_count);
            self.world.write_frame(fid, last, &mut self.core.stream, delta, count as u32);
            if self.core.master_mode {
                let (start, end) = (self.core.last_cmd_time as f32, self.core.world_time as f32);
                let mut s = std::mem::take(&mut self.core.stream);
                self.core.write_commands(&mut s, start, end);
                self.core.stream = s;
            }
            self.core.last_cmd_time = self.core.world_time;
            if self.core.stream.overflowed {
                self.sys.print("Demo data stream overflow.\n");
                self.core.stream.clear();
                self.core.delta_frame_seq = 0;
                self.core.last_frame_seq = 0;
            } else {
                self.core.frame_count += 1;
                self.core.delta_frame_seq = fseq;
                self.core.last_frame_seq = fseq;
            }
        }
    }

    /// `ReadNetchanState`: `(seq, seq, 0, 0, seq, 0, 0)` with seq = frames sent.
    pub fn read_netchan_state(&self) -> [i32; 7] {
        let n = self.core.frame_count;
        [n, n, 0, 0, n, 0, 0]
    }

    /// `ExecuteDemoFileCommands(b)` (§8).
    fn execute_demo_file_commands(&mut self, data: &[u8], engine: &mut dyn Engine) {
        let mut b = BitBuffer::from_bytes(data);
        loop {
            let t = b.read_byte();
            if t == -1 {
                return;
            }
            match t {
                3 => {
                    let cmd = b.read_buf(64).unwrap_or_else(|| vec![0; 64]);
                    let cmd = crate::world::cut(&cmd, 65);
                    if engine.valid_stuff_text(&cmd) {
                        engine.add_filtered_text(&cmd);
                        engine.add_filtered_text(b"\n");
                    } else {
                        self.sys.print(&format!("Demo tried to send invalid command:\"{}\"\n", String::from_utf8_lossy(&cmd)));
                    }
                }
                4 => {
                    let cd = b.read_buf(32).unwrap_or_else(|| vec![0; 32]);
                    engine.demo_update_client_data(&cd);
                }
                6 => {
                    let flags = b.read_long();
                    let index = b.read_long();
                    let delay = b.read_float();
                    let args = b.read_buf(0x48).unwrap_or_else(|| vec![0; 0x48]);
                    engine.queue_event(flags, index, delay, &args);
                }
                7 => {
                    let anim = b.read_long();
                    let body = b.read_long();
                    engine.weapon_anim(anim, body);
                }
                8 => {
                    let channel = b.read_long();
                    let len = b.read_long();
                    let n = len.clamp(0, 256) as usize;
                    let sample = b.read_buf(n).unwrap_or_default();
                    let attn = b.read_float();
                    let vol = b.read_float();
                    let flags = b.read_long();
                    let pitch = b.read_long();
                    engine.play_sound(channel, &crate::world::cut(&sample, 257), attn, vol, flags, pitch);
                }
                9 => {
                    let size = b.read_long();
                    let n = size.clamp(0, 0x8000) as usize;
                    let buf = b.read_buf(n).unwrap_or_default();
                    engine.read_demo_buffer(&buf);
                }
                other => {
                    self.sys.print(&format!("WARNING! DemoPlayer::ExecuteDemoFileCommands: unexpected demo file command {other}\n"));
                    return;
                }
            }
        }
    }

    /// `GetDemoViewInfo(rp, view, viewmodel)` (§9): `None` leaves the outputs untouched.
    /// `rp` keeps its `cmd`, `movevars` and `viewport` (bytes 0xC8..0xE0).
    pub fn demo_view_info(&mut self, rp: &mut [u8; 0xE8], view: &mut [f32; 3], viewmodel: &mut i32) -> bool {
        let seq = self.core.last_frame_seq;
        let cur = self.world.frame_by_seq(seq).and_then(|f| self.world.frame(f)).and_then(|f| f.demo_info.as_ref().map(|d| (f.time, **d)));
        let Some((ctime, c)) = cur else { return false };
        let prev = self.world.frame_by_seq(seq.wrapping_sub(1)).and_then(|f| self.world.frame(f)).and_then(|f| f.demo_info.as_ref().map(|d| (f.time, **d)));
        let saved: Vec<u8> = rp[0xC8..0xE0].to_vec();
        rp.copy_from_slice(&c[4..4 + 0xE8]);
        rp[0xC8..0xE0].copy_from_slice(&saved);
        let f = |b: &[u8; DEMO_INFO_SIZE], o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        for (i, v) in view.iter_mut().enumerate() {
            *v = f(&c, 0x1A4 + i * 4);
        }
        *viewmodel = i32::from_le_bytes([c[0x1B0], c[0x1B1], c[0x1B2], c[0x1B3]]);
        let Some((ptime, p)) = prev else { return true };
        if ptime >= ctime {
            return true;
        }
        let frac = ((self.core.world_time - f64::from(ptime)) / f64::from(ctime - ptime)) as f32;
        let put = |rp: &mut [u8; 0xE8], o: usize, v: f32| rp[o..o + 4].copy_from_slice(&v.to_le_bytes());
        // rp starts at demo_info + 4.
        let lerp = |o: usize| (f(&c, 4 + o) - f(&p, 4 + o)) * frac + f(&p, 4 + o);
        for i in 0..3 {
            put(rp, i * 4, lerp(i * 4));
        }
        for i in 0..3 {
            let o = 0x0C + i * 4;
            let mut d = f(&c, 4 + o) - f(&p, 4 + o);
            if d > 180.0 {
                d -= 360.0;
            } else if d < -180.0 {
                d += 360.0;
            }
            let mut a = d * frac + f(&p, 4 + o);
            if a > 180.0 {
                a = (f64::from(a) - 360.0) as f32;
            } else if a < -180.0 {
                a = (f64::from(a) + 360.0) as f32;
            }
            put(rp, o, a);
        }
        for base in [0x58usize, 0x64, 0x70] {
            for i in 0..3 {
                let o = base + i * 4;
                put(rp, o, lerp(o));
            }
        }
        for (i, v) in view.iter_mut().enumerate() {
            let o = 0x1A4 + i * 4;
            *v = (f(&c, o) - f(&p, o)) * frac + f(&p, o);
        }
        true
    }

    /// `ExecuteCommand(id, line)` (§5): `line` is the whole command line.
    pub fn execute_command(&mut self, id: i32, line: &str) {
        let t = tokens(line);
        let need_active = |s: &mut Self| {
            if !s.is_active() {
                s.sys.print("Not viewing a demo.\n");
                false
            } else {
                true
            }
        };
        match id {
            1 => {
                if !need_active(self) {
                    return;
                }
                if t.len() != 2 {
                    self.sys.print("dem_jump <seconds>\n");
                    return;
                }
                self.set_world_time(atof(&t[1]), true);
                self.set_paused(true);
            }
            2 => {
                if t.len() != 2 {
                    self.sys.print("dem_forcehltv <0|1>\n");
                    return;
                }
                self.core.force_hltv = crate::world::atoi(t[1].as_bytes()) == 1;
            }
            3 => {
                if !need_active(self) {
                    return;
                }
                if t.len() != 2 {
                    self.sys.print("dem_pause <0|1>\n");
                    return;
                }
                self.set_paused(crate::world::atoi(t[1].as_bytes()) == 1);
            }
            4 => {
                if !need_active(self) {
                    return;
                }
                if t.len() != 2 {
                    self.sys.print("dem_speed <replayspeed>\n");
                    return;
                }
                self.set_time_scale(atof(&t[1]) as f32);
            }
            5 => {
                if !need_active(self) {
                    return;
                }
                let st = self.start_time();
                self.set_world_time(st, false);
            }
            6 => {
                if t.len() != 2 {
                    self.sys.print("dem_save <filename>\n");
                    return;
                }
                if !self.save_game(Some(&t[1])) {
                    self.sys.print("Warning! Could not save game as demo file.\n");
                }
            }
            _ => self.sys.print(&format!("ERROR! DemoPlayer::ExecuteCommand: unknown command ID {id}.\n")),
        }
    }

    /// Runs the world's pending signals (after GameUI-side changes).
    pub fn deliver_signals(&mut self) {
        deliver(&mut self.world, &mut self.core);
    }
}

/// C `atof`.
fn atof(s: &str) -> f64 {
    let t = s.trim_start();
    let end = t
        .char_indices()
        .take_while(|&(i, c)| c.is_ascii_digit() || c == '.' || ((c == '-' || c == '+') && i == 0) || c == 'e' || c == 'E')
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    let mut cand = &t[..end];
    while !cand.is_empty() {
        if let Ok(v) = cand.parse::<f64>() {
            return v;
        }
        cand = &cand[..cand.len() - 1];
    }
    0.0
}
