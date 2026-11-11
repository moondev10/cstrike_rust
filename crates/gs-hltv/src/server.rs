//! Core.dll's `Server` module as `DemoServer` (docs/re/hltv-world.md §6) with its `DemoFile`
//! (§6.3): the demo is read one network message at a time and parsed into the `World`, building
//! one stored frame per entity-bearing message. Also `World::SaveAsDemo` (§7.10).

use std::collections::VecDeque;

use crate::Sys;
use crate::bitbuf::BitBuffer;
use crate::director::DirectorCmd;
use crate::world::{CLIENTDATA_SIZE, DEMO_INFO_SIZE, ENTITY_SIZE, InFrame, MAX_PACKET_ENTITIES, Resource, World, cut, file_base};

/// What the server and the world report to their listener (the demo player): director
/// commands at ingest and the module signals, synchronously.
pub trait Director {
    /// `IDirector::AddCommand` (svc_director at ingest).
    fn add_command(&mut self, cmd: &DirectorCmd) -> i32;
    /// A World signal (§2.2).
    fn world_signal(&mut self, world: &mut World, sig: i32, data: i32);
    /// A Server signal (§2.2).
    fn server_signal(&mut self, sig: i32);
    /// `IDirector::WriteCommands(stream, start, end)` (SaveAsDemo).
    fn write_commands(&mut self, s: &mut BitBuffer, start: f32, end: f32);
}

/// Delivers the World's pending signals.
pub fn deliver(world: &mut World, director: &mut dyn Director) {
    for (sig, data) in world.take_signals() {
        director.world_signal(world, sig, data);
    }
}

/// `DemoFile` (playback side, §6.3.1–§6.3.2).
#[derive(Default)]
pub struct DemoFile {
    data: Vec<u8>,
    pos: usize,
    /// 0 idle, 1 playing, 2 recording.
    pub state: i32,
    pub file_name: Vec<u8>,
    entries: Option<Vec<usize>>,
    num_entries: i32,
    cur_entry: i32,
    start_time: f32,
    next_read_time: f32,
    continuous: bool,
    pub map_name: Vec<u8>,
    pub game_dir: Vec<u8>,
}

impl DemoFile {
    fn read<const N: usize>(&mut self) -> Option<[u8; N]> {
        let s = self.data.get(self.pos..self.pos + N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(s);
        self.pos += N;
        Some(out)
    }

    fn read_i32(&mut self) -> Option<i32> {
        self.read::<4>().map(i32::from_le_bytes)
    }

    fn read_vec(&mut self, n: usize) -> Option<Vec<u8>> {
        let s = self.data.get(self.pos..self.pos + n)?.to_vec();
        self.pos += n;
        Some(s)
    }

    /// `CloseFile`.
    fn close(&mut self) {
        self.data.clear();
        self.pos = 0;
        self.state = 0;
    }

    /// `LoadDemo(name)`.
    fn load(&mut self, sys: &Sys, name: &str) -> bool {
        if self.state == 2 {
            sys.print("Cannot load demo, still recording.\n");
            return false;
        }
        self.close();
        let mut file = cut(name.as_bytes(), 0x100).to_ascii_lowercase();
        if !file.windows(4).any(|w| w == b".dem") {
            file.extend_from_slice(b".dem");
        }
        self.file_name = file.clone();
        let fname = String::from_utf8_lossy(&file).into_owned();
        let Some(data) = sys.files.read(&fname) else {
            sys.print(&format!("Coudn't open demo file {fname}.\n"));
            return false;
        };
        self.data = data;
        self.pos = 0;
        let Some(head) = self.read_vec(0x220) else {
            sys.print(&format!("{fname} is not a HL demo file.\n"));
            self.close();
            return false;
        };
        if cut(&head[0..6], 7) != b"HLDEMO" {
            sys.print(&format!("{fname} is not a HL demo file.\n"));
            self.close();
            return false;
        }
        let i32_at = |o: usize| i32::from_le_bytes([head[o], head[o + 1], head[o + 2], head[o + 3]]);
        if i32_at(12) != 48 || i32_at(8) != 5 {
            sys.print(&format!("WARNING! {fname} has an outdated demo format.\n"));
        }
        self.map_name = cut(&head[16..276], 260);
        self.game_dir = cut(&head[276..536], 260);
        let pos = self.pos;
        self.pos = i32_at(540).max(0) as usize;
        self.num_entries = self.read_i32().unwrap_or(0);
        self.cur_entry = 0;
        if (self.num_entries as u32).wrapping_sub(1) > 1023 {
            sys.print("WARNING! Demo had bogus number of directory entries!\n");
            self.num_entries = 2;
            self.entries = None;
            self.pos = pos;
        } else {
            let mut offs = Vec::new();
            for _ in 0..self.num_entries {
                let e = self.read_vec(0x5C).unwrap_or_else(|| vec![0; 0x5C]);
                offs.push(i32::from_le_bytes([e[0x54], e[0x55], e[0x56], e[0x57]]).max(0) as usize);
            }
            self.pos = offs.first().copied().unwrap_or(pos);
            self.entries = Some(offs);
        }
        let now = sys.time.get() as f32;
        self.start_time = now;
        self.next_read_time = now;
        self.continuous = true;
        self.state = 1;
        true
    }

    fn is_playing(&self) -> bool {
        self.state == 1
    }
}

/// One queued network message (`NetPacket`): its bytes and `incoming_sequence`.
struct Packet {
    data: Vec<u8>,
    seqnr: i32,
}

/// `Server` (`server001` "DemoServer").
pub struct Server {
    sys: Sys,
    pub system_time: f64,
    /// 2 disconnected, 6 connected (sign-on), 7 running, 8 intermission.
    pub state: i32,
    pub server_name: Vec<u8>,
    pub server_info: Vec<u8>,
    frame: InFrame,
    time: f32,
    reliable: BitBuffer,
    unreliable: BitBuffer,
    voice: BitBuffer,
    user_messages: BitBuffer,
    client_data: BitBuffer,
    demo_data: BitBuffer,
    demo_info: [u8; DEMO_INFO_SIZE],
    valid_sequence: i32,
    pub is_hltv: bool,
    pub force_hltv: bool,
    is_game_server: bool,
    pub paused: bool,
    voice_blocking: bool,
    server_crc: i32,
    server_count: i32,
    player_num: i32,
    protocol: i32,
    client_data_struct: [u8; CLIENTDATA_SIZE],
    seq_map: [u32; 256],
    pub demo: DemoFile,
    packets: VecDeque<Packet>,
    /// `m_Director` is set (DemoPlayer).
    pub has_director: bool,
    pub delay_reconnect: bool,
}

const FRAME_BUF: usize = 0xFAA;

impl Server {
    /// `Server::Init`.
    pub fn new(sys: Sys) -> Self {
        Self {
            sys,
            system_time: 0.0,
            state: 2,
            server_name: Vec::new(),
            server_info: Vec::new(),
            frame: InFrame::default(),
            time: 0.0,
            reliable: BitBuffer::new(FRAME_BUF),
            unreliable: BitBuffer::new(FRAME_BUF),
            voice: BitBuffer::new(FRAME_BUF),
            user_messages: BitBuffer::new(FRAME_BUF),
            client_data: BitBuffer::new(FRAME_BUF),
            demo_data: BitBuffer::new(FRAME_BUF),
            demo_info: [0; DEMO_INFO_SIZE],
            valid_sequence: 0,
            is_hltv: true,
            force_hltv: false,
            is_game_server: false,
            paused: false,
            voice_blocking: false,
            server_crc: 0,
            server_count: 0,
            player_num: 0,
            protocol: 48,
            client_data_struct: [0; CLIENTDATA_SIZE],
            seq_map: [0; 256],
            demo: DemoFile::default(),
            packets: VecDeque::new(),
            has_director: false,
            delay_reconnect: true,
        }
    }

    /// `IsConnected`: state ≥ 6.
    pub fn is_connected(&self) -> bool {
        self.state >= 6
    }

    /// `ClearFrame(clearReliable)`.
    fn clear_frame(&mut self, clear_reliable: bool) {
        if clear_reliable {
            self.user_messages.fast_clear();
            self.reliable.fast_clear();
        }
        self.unreliable.fast_clear();
        self.voice.fast_clear();
        self.client_data.fast_clear();
        self.demo_data.fast_clear();
        self.frame = InFrame::default();
        self.demo_info = [0; DEMO_INFO_SIZE];
    }

    /// `Reset`.
    fn reset(&mut self) {
        self.packets.clear();
        self.clear_frame(true);
        self.seq_map = [0; 256];
        self.valid_sequence = 0;
        self.is_hltv = true;
        self.force_hltv = false;
        self.is_game_server = false;
        self.paused = false;
        self.time = 0.0;
        self.demo = DemoFile::default();
        self.state = 2;
    }

    /// `LoadDemo(world, name, forceHLTV, continuous)`.
    pub fn load_demo(&mut self, name: &str, force_hltv: bool, continuous: bool) -> bool {
        self.reset();
        let sys = self.sys.clone();
        if !self.demo.load(&sys, name) {
            return false;
        }
        self.demo.continuous = continuous;
        self.force_hltv = force_hltv;
        self.is_hltv = false;
        self.state = 6;
        true
    }

    /// `Disconnect`.
    pub fn disconnect(&mut self, world: &mut World, director: &mut dyn Director) {
        if self.state == 2 {
            return;
        }
        director.server_signal(6);
        self.packets.clear();
        if self.demo.is_playing() {
            self.demo.close();
            self.sys.print("Demo playback stopped.\n");
        } else {
            self.sys.print("Disconnected.\n");
        }
        world.finish_game();
        deliver(world, director);
        self.reset();
    }

    /// `DemoFile::ReadDemoPacket(demoData, info)` (§6.3.2).
    fn read_demo_packet(&mut self, world: &mut World, director: &mut dyn Director) {
        if !self.demo.is_playing() {
            return;
        }
        let now = self.sys.time.get();
        if f64::from(self.demo.next_read_time) > now && self.demo.continuous {
            return;
        }
        loop {
            let pos = self.demo.pos;
            let Some([cmd]) = self.demo.read::<1>() else { return self.eof(world, director) };
            let time = self.demo.read::<4>().map_or(0.0, f32::from_le_bytes);
            let _frame = self.demo.read_i32();
            if cmd != 0 && cmd != 5 {
                self.demo.next_read_time = time + self.demo.start_time;
            }
            if f64::from(self.demo.next_read_time) > now && self.demo.continuous {
                self.demo.pos = pos;
                return;
            }
            let len = match cmd {
                2 => {
                    self.demo.start_time = now as f32;
                    continue;
                }
                5 => {
                    self.demo.cur_entry += 1;
                    if self.demo.cur_entry >= self.demo.num_entries {
                        return self.eof(world, director);
                    }
                    if let Some(off) = self.demo.entries.as_ref().and_then(|e| e.get(self.demo.cur_entry as usize).copied()) {
                        self.demo.pos = off;
                    }
                    continue;
                }
                3 => 64,
                4 => 32,
                6 => 84,
                7 => 8,
                8 => {
                    let ch = self.demo.read_i32().unwrap_or(0);
                    let slen = self.demo.read_i32().unwrap_or(0);
                    let len = (slen + 16).max(0) as usize;
                    let Some(buf) = self.demo.read_vec(len) else { return self.eof(world, director) };
                    self.demo_data.write_byte(8);
                    self.demo_data.write_long(ch);
                    self.demo_data.write_long(slen);
                    self.demo_data.write_buf(&buf);
                    continue;
                }
                9 => {
                    let len = self.demo.read_i32().unwrap_or(0);
                    if len == 0 {
                        continue;
                    }
                    if !(0..=0x10000).contains(&len) {
                        log::warn!("demo client-DLL buffer of {len} bytes (Core reads it into 64 KiB)");
                    }
                    let Some(buf) = self.demo.read_vec(len.max(0) as usize) else { return self.eof(world, director) };
                    self.demo_data.write_byte(9);
                    self.demo_data.write_long(len);
                    self.demo_data.write_buf(&buf);
                    continue;
                }
                _ => 0,
            };
            if len > 0 {
                let Some(buf) = self.demo.read_vec(len) else { return self.eof(world, director) };
                self.demo_data.write_byte(cmd);
                self.demo_data.write_buf(&buf);
                continue;
            }
            // A network message.
            let Some(info) = self.demo.read_vec(DEMO_INFO_SIZE) else { return self.eof(world, director) };
            self.demo_info.copy_from_slice(&info);
            let seq = self.demo.read_i32().unwrap_or(0);
            for _ in 0..6 {
                self.demo.read_i32();
            }
            let Some(len) = self.demo.read_i32() else {
                self.sys.dprint("WARNING! DemoFile::ReadDemoPacket: Bad demo length.\n");
                return self.stop_playback(world, director);
            };
            if len < 0 {
                self.sys.print("WARNING! DemoFile::ReadDemoPacket: Demo message length < 0.\n");
                return self.stop_playback(world, director);
            }
            if len > 0x10000 {
                self.sys.print("WARNING! DemoFile::ReadDemoPacket: message length > MAX_POSSIBLE_MSG\n");
                return self.stop_playback(world, director);
            }
            if len == 0 {
                return;
            }
            let Some(data) = self.demo.read_vec(len as usize) else {
                self.sys.print("WARNING! DemoFile::ReadDemoPacket: Error reading demo message data.\n");
                return self.stop_playback(world, director);
            };
            self.packets.push_back(Packet { data, seqnr: seq });
            return;
        }
    }

    fn eof(&mut self, world: &mut World, director: &mut dyn Director) {
        self.stop_playback(world, director);
    }

    /// `StopPlayBack`: the file closes first, so `Disconnect` takes the network branch (Q1).
    fn stop_playback(&mut self, world: &mut World, director: &mut dyn Director) {
        if self.demo.is_playing() {
            self.demo.close();
            self.disconnect(world, director);
        }
    }

    /// `Server::RunFrame` (demo part): up to 33 messages.
    pub fn run_frame(&mut self, t: f64, world: &mut World, director: &mut dyn Director) {
        self.system_time = t;
        if self.demo.is_playing() {
            self.read_demo_packet(world, director);
        }
        let mut n = 0;
        while let Some(p) = self.packets.pop_front() {
            let mut stream = BitBuffer::from_bytes(&p.data);
            self.process_message(&mut stream, p.seqnr, world, director);
            n += 1;
            if n > 32 {
                break;
            }
            if self.demo.is_playing() {
                self.read_demo_packet(world, director);
            }
        }
        if !matches!(self.state, 2 | 6 | 7 | 8) {
            self.sys.errorf("Server::RunFrame: not valid state.\n");
        }
    }

    fn set_state(&mut self, to: i32) {
        let ok = match to {
            6 => matches!(self.state, 5 | 7 | 8) || self.demo.is_playing(),
            7 => matches!(self.state, 6..=8),
            _ => true,
        };
        if ok {
            self.state = to;
        } else {
            self.sys.errorf(&format!("Server::SetState: not valid state ({} -> {}).\n", self.state, to));
        }
    }

    /// `ProcessMessage(seqNr)` (§6.5).
    fn process_message(&mut self, s: &mut BitBuffer, seq: i32, world: &mut World, director: &mut dyn Director) {
        if !self.is_connected() {
            return;
        }
        self.frame.seqnr = seq as u32;
        while self.state != 2 {
            if s.overflowed {
                self.sys.print("WARNING! Server::ProcessMessage: packet read overflow.\n");
                break;
            }
            let cmd = s.read_byte();
            if cmd == -1 {
                break;
            }
            if cmd > 58 {
                if !self.parse_user_message(cmd, s, world) {
                    break;
                }
                continue;
            }
            self.parse(cmd, s, world, director);
            deliver(world, director);
        }
        if self.frame.entitynum != 0 {
            let f = &mut self.frame;
            f.reliable = if self.reliable.overflowed { Vec::new() } else { self.reliable.bytes().to_vec() };
            f.unreliable = if self.unreliable.overflowed { Vec::new() } else { self.unreliable.bytes().to_vec() };
            f.voice = if self.voice.overflowed { Vec::new() } else { self.voice.bytes().to_vec() };
            f.client_data = if self.client_data.overflowed { Vec::new() } else { self.client_data.bytes().to_vec() };
            f.user_messages = if self.user_messages.overflowed { Vec::new() } else { self.user_messages.bytes().to_vec() };
            f.demo_data = if !self.demo_data.overflowed && !self.force_hltv { self.demo_data.bytes().to_vec() } else { Vec::new() };
            if !self.is_hltv && !self.force_hltv {
                f.demo_info = Some(Box::new(self.demo_info));
            }
            if self.force_hltv {
                let origin: Vec<u8> = self.client_data_struct.get(0..12).map(<[u8]>::to_vec).unwrap_or_default();
                for e in &mut f.entities {
                    let num = i32::from_le_bytes([e[4], e[5], e[6], e[7]]);
                    if num - 1 == self.player_num {
                        e[0x10..0x1C].copy_from_slice(&origin);
                    }
                }
            }
            let new = world.add_frame(&self.frame);
            self.seq_map[(seq & 0xFF) as usize] = new;
            deliver(world, director);
            self.valid_sequence = seq;
            self.clear_frame(true);
        } else {
            let keep = world.is_active();
            self.clear_frame(!keep);
        }
    }

    /// Appends `svc` + `payload` to the sign-on data (state 6) or to a stream (7/8).
    fn store(&mut self, world: &mut World, svc: u8, payload: &[u8], during_game: Stream, handler: &str) {
        match self.state {
            6 => {
                world.add_signon_data(svc, payload);
            }
            7 | 8 => {
                let b = self.stream(during_game);
                b.write_byte(svc);
                b.write_buf(payload);
            }
            _ => self.sys.errorf(&format!("{handler}: unexpected server state.\n")),
        }
    }

    fn stream(&mut self, which: Stream) -> &mut BitBuffer {
        match which {
            Stream::Reliable => &mut self.reliable,
            Stream::Unreliable => &mut self.unreliable,
        }
    }

    /// The svc handler table (§6.6).
    fn parse(&mut self, cmd: i32, s: &mut BitBuffer, world: &mut World, director: &mut dyn Director) {
        match cmd {
            0 => {
                self.sys.print("WARNING! Server::ParseBad: illegal server message.\n");
                s.overflowed = true;
            }
            1 | 19 | 20 | 27 | 28 | 42 => {}
            2 => {
                let reason = String::from_utf8_lossy(&s.read_string()).into_owned();
                let name = String::from_utf8_lossy(&self.server_name).into_owned();
                if reason.is_empty() {
                    self.sys.print(&format!("Dropped from {name}.\n"));
                } else {
                    self.sys.print(&format!("Dropped from {name} ({reason}).\n"));
                }
                self.disconnect(world, director);
            }
            3 => {
                s.start_bit_mode();
                let start = s.byte_pos();
                let n = s.read_bits(5);
                for _ in 0..n {
                    s.skip_bits(10);
                    if s.read_bit() {
                        s.skip_bits(11);
                    }
                    if s.read_bit() {
                        world.parse_event(s);
                    }
                    if s.read_bit() {
                        s.skip_bits(16);
                    }
                }
                s.end_bit_mode();
                self.frame.events = s.slice(start, s.byte_pos()).to_vec();
                self.frame.eventnum = n;
            }
            4 => {
                let v = s.read_long();
                if v != self.protocol {
                    self.sys.print(&format!("Server::ParseVersion: Server is protocol {v} instead of {}\n", self.protocol));
                }
            }
            5 => {
                let v = s.read_short();
                match self.state {
                    6 => world.view_entity = v,
                    7 | 8 => {
                        self.reliable.write_byte(5);
                        self.reliable.write_short(v);
                    }
                    _ => self.sys.errorf("Server::ParseSetView: unexpected server state.\n"),
                }
            }
            6 => {
                s.start_bit_mode();
                let start = s.byte_pos();
                let flags = s.read_bits(9);
                if flags & 1 != 0 {
                    s.read_bits(8);
                }
                if flags & 2 != 0 {
                    s.read_bits(8);
                }
                s.read_bits(22);
                if flags & 4 != 0 {
                    s.read_bits(8);
                }
                skip_bit_vec3_coord(s);
                if flags & 8 != 0 {
                    s.read_bits(8);
                }
                s.end_bit_mode();
                let payload = s.slice(start, s.byte_pos()).to_vec();
                if matches!(self.state, 7 | 8) {
                    self.unreliable.write_byte(6);
                    self.unreliable.write_buf(&payload);
                } else {
                    self.sys.errorf("Server::ParseSound: unexpected server state.\n");
                }
            }
            7 => {
                self.time = s.read_float();
                self.frame.time = self.time;
            }
            8 => {
                let mut t = s.read_string();
                if t.first().is_some_and(|&c| c < 0x20) {
                    t.remove(0);
                }
                self.sys.print(&format!(">{}\n", String::from_utf8_lossy(&t)));
            }
            9 => self.parse_stuff_text(s, world, director),
            10 => {
                let p = s.read_buf(6).unwrap_or_default();
                self.store(world, 10, &p, Stream::Unreliable, "Server::ParseSetAngle");
            }
            11 => self.parse_serverinfo(s, world, director),
            12 => {
                let i = s.read_byte();
                let style = s.read_string();
                match self.state {
                    6 => world.add_light_style(i, &style),
                    7 | 8 => {
                        self.reliable.write_byte(12);
                        self.reliable.write_byte(i as u8);
                        self.reliable.write_string(&style);
                    }
                    _ => self.sys.errorf("Server::ParseLightStyle: unexpected server state.\n"),
                }
            }
            13 => {
                let start = s.byte_pos();
                let slot = s.read_byte();
                let userid = s.read_long();
                let info = s.read_string();
                let mut hash = [0u8; 16];
                if let Some(h) = s.read_buf(16) {
                    hash.copy_from_slice(&h);
                }
                let payload = s.slice(start, s.byte_pos()).to_vec();
                world.update_player(slot, userid, &info, hash);
                match self.state {
                    6 => {}
                    7 | 8 => {
                        self.reliable.write_byte(13);
                        self.reliable.write_buf(&payload);
                    }
                    _ => self.sys.errorf("Server::ParseUpdateUserInfo: unexpected server state.\n"),
                }
            }
            14 => world.parse_delta_description(s),
            15 => {
                if self.is_hltv {
                    return;
                }
                s.start_bit_mode();
                let delta = if s.read_bit() { self.seq_map[(s.read_bits(8) & 0xFF) as usize] } else { 0 };
                let mut cd = [0u8; CLIENTDATA_SIZE];
                world.parse_client_data(s, delta, &mut self.client_data, &mut cd);
                self.client_data_struct = cd;
                s.end_bit_mode();
            }
            16 => {
                let v = s.read_short();
                self.unreliable.write_byte(16);
                self.unreliable.write_short(v);
            }
            17 => {
                s.start_bit_mode();
                let start = s.byte_pos();
                while s.read_bit() && !s.overflowed {
                    s.read_bits(24);
                }
                s.end_bit_mode();
                let payload = s.slice(start, s.byte_pos()).to_vec();
                self.unreliable.write_byte(17);
                self.unreliable.write_buf(&payload);
            }
            18 => {
                let p = s.read_buf(11).unwrap_or_default();
                self.unreliable.write_byte(18);
                self.unreliable.write_buf(&p);
            }
            21 => {
                s.start_bit_mode();
                let start = s.byte_pos();
                s.skip_bits(10);
                world.parse_event(s);
                if s.read_bit() {
                    s.skip_bits(16);
                }
                s.end_bit_mode();
                let payload = s.slice(start, s.byte_pos()).to_vec();
                self.reliable.write_byte(21);
                self.reliable.write_buf(&payload);
            }
            22 => world.parse_baseline(s),
            23 => self.parse_temp_entity(s, world),
            24 => {
                let b = s.read_byte();
                self.paused = b != 0;
                if b == 0 {
                    self.valid_sequence = 0;
                }
                world.set_paused(b != 0);
            }
            25 => {
                let n = s.read_byte();
                self.sys.dprint(&format!("Received signon ({n}).\n"));
                if matches!(self.state, 6 | 8) {
                    self.state = 7;
                } else if self.state != 7 {
                    self.set_state(7);
                }
            }
            26 => {
                let t = s.read_string();
                let mut p = t.clone();
                p.push(0);
                match self.state {
                    6 => {
                        world.add_signon_data(26, &p);
                    }
                    7 | 8 => {
                        self.unreliable.write_byte(26);
                        self.unreliable.write_string(&t);
                    }
                    _ => self.sys.errorf("Server::ParseStuffText: unexpected server state.\n"),
                }
                if !t.is_empty() {
                    self.sys.dprint(&format!(">>{}\n", String::from_utf8_lossy(&t)));
                }
            }
            29 => {
                let p = s.read_buf(14).unwrap_or_default();
                self.store(world, 29, &p, Stream::Unreliable, "Server::ParseSpawnStaticSound");
            }
            30 => {
                if self.state == 7 {
                    self.state = 8;
                }
            }
            31 | 34 => {
                let t = s.read_string();
                self.reliable.write_byte(cmd as u8);
                self.reliable.write_string(&t);
            }
            32 => {
                let track = s.read_byte();
                let looptrack = s.read_byte();
                world.cd_track = track;
                world.cd_looptrack = looptrack;
            }
            33 => {
                s.skip_string();
                let n = s.read_byte();
                for _ in 0..n.max(0) {
                    s.skip_string();
                }
            }
            35 => s.skip_bytes(2),
            36 => {
                let i = s.read_byte();
                let t = s.read_string();
                self.reliable.write_byte(36);
                self.reliable.write_byte(i as u8);
                self.reliable.write_string(&t);
            }
            37 => {
                let v = s.read_short();
                self.reliable.write_byte(37);
                self.reliable.write_short(v);
            }
            38 => {
                let p = s.read_buf(2).unwrap_or_default();
                self.store(world, 38, &p, Stream::Unreliable, "Server::ParseAddAngle");
            }
            39 => {
                let msg = s.read_byte();
                let size = s.read_byte();
                let name = s.read_buf(16).unwrap_or_else(|| vec![0; 16]);
                self.sys.dprint(&format!("Adding user message:{}({msg}).\n", String::from_utf8_lossy(&cut(&name, 17))));
                world.add_user_message(msg, size, &name);
            }
            40 => {
                let n = s.read_short();
                if n as usize > MAX_PACKET_ENTITIES || n < 0 {
                    self.sys.print("ERROR! Server::ParsePacketEntities: entnum>MAX_PACKET_ENTITIES.\n");
                    s.overflowed = true;
                    return;
                }
                self.frame.entitynum = n as u32;
                self.frame.entities = vec![[0u8; ENTITY_SIZE]; n as usize];
                let time = self.frame.time;
                let mut ents = std::mem::take(&mut self.frame.entities);
                world.uncompress_full(time, n as u32, &mut ents, s);
                self.frame.entities = ents;
            }
            41 => {
                let n = s.read_short();
                if n as usize > MAX_PACKET_ENTITIES {
                    self.sys.errorf("Server::ParsePacketEntities: entnum>MAX_PACKET_ENTITIES.\n");
                }
                self.frame.entitynum = n.max(0) as u32;
                let d = s.read_byte();
                let delta = self.seq_map[(d & 0xFF) as usize];
                let time = self.frame.time;
                let mut ents = Vec::new();
                if !world.uncompress_delta(time, n.max(0) as u32, &mut ents, s, delta) {
                    s.overflowed = true;
                }
                ents.resize(n.clamp(0, MAX_PACKET_ENTITIES as i32) as usize, [0; ENTITY_SIZE]);
                self.frame.entities = ents;
            }
            43 => self.parse_resource_list(s, world),
            44 => {
                let mut mv = [0u8; 0x84];
                for k in 0..16 {
                    mv[k * 4..k * 4 + 4].copy_from_slice(&s.read_float().to_le_bytes());
                }
                mv[0x40..0x44].copy_from_slice(&s.read_byte().to_le_bytes());
                for k in 0..8 {
                    mv[0x64 + k * 4..0x68 + k * 4].copy_from_slice(&s.read_float().to_le_bytes());
                }
                let sky = cut(&s.read_string(), 32);
                mv[0x44..0x44 + sky.len()].copy_from_slice(&sky);
                world.set_move_vars(&mv);
            }
            45 => s.skip_bytes(8),
            46 => {
                let idx = s.read_byte();
                if !(0..32).contains(&idx) {
                    self.sys.dprint(&format!("Bogus player index ({idx}) during customization parsing.\n"));
                    s.overflowed = true;
                    return;
                }
                s.read_byte();
                let name = s.read_string();
                s.read_short();
                s.read_long();
                let flags = s.read_byte();
                if flags & 4 != 0 {
                    s.skip_bytes(16);
                }
                self.sys.dprint(&format!("Ignoring player customization ({}).\n", String::from_utf8_lossy(&name)));
            }
            47 => {
                let p = s.read_buf(2).unwrap_or_default();
                self.reliable.write_byte(47);
                self.reliable.write_buf(&p);
            }
            48 => {
                let p = s.read_buf(4).unwrap_or_default();
                self.reliable.write_byte(48);
                self.reliable.write_buf(&p);
            }
            49 => {
                let n = s.read_string();
                if n.is_empty() {
                    self.sys.print("WARNING! Server::ParseFileTransferFailed: empty filename.\n");
                } else {
                    self.sys.print(&format!("WARNING! Downloading \"{}\" failed.\n", String::from_utf8_lossy(&n)));
                }
            }
            50 => match s.read_byte() {
                0 => {
                    self.is_hltv = true;
                    self.force_hltv = false;
                }
                1 => s.skip_bytes(10),
                2 => self.sys.print("WARNING! Server::ParseHLTV: unexpected \"listen\" command.\n"),
                _ => {
                    self.sys.print("WARNING! Server::ParseHLTV: unknown director command.\n");
                    s.overflowed = true;
                }
            },
            51 => {
                let start = s.byte_pos();
                let len = s.read_byte().max(0) as usize;
                if self.has_director {
                    let mut c = DirectorCmd::default();
                    c.read_from_stream(s);
                    c.time = self.time;
                    director.add_command(&c);
                } else {
                    s.skip_bytes(len);
                    let payload = s.slice(start, s.byte_pos()).to_vec();
                    self.store(world, 51, &payload, Stream::Reliable, "Server::ParseDirector");
                }
            }
            52 => {
                let codec = s.read_string();
                let quality = s.read_byte();
                let mut p = codec.clone();
                p.push(0);
                p.push(quality as u8);
                self.store(world, 52, &p, Stream::Reliable, "Server::ParseVoiceInit");
                world.voice_enabled = !codec.is_empty();
            }
            53 => {
                let idx = s.read_byte();
                let n = s.read_short();
                let data = s.read_buf(n.max(0) as usize).unwrap_or_default();
                if !self.voice_blocking && self.state > 6 {
                    self.voice.write_byte(53);
                    self.voice.write_byte(idx as u8);
                    self.voice.write_short(n);
                    self.voice.write_buf(&data);
                }
            }
            54 => {
                let dir = s.read_string();
                let cheats = s.read_byte();
                world.set_extra_info(&dir, cheats != 0);
            }
            55 => {
                let v = s.read_float();
                if self.has_director {
                    return;
                }
                match self.state {
                    6 => self.sys.print("Server::ParseTimeScale: invalid during signon.\n"),
                    7 | 8 => {
                        self.reliable.write_byte(55);
                        self.reliable.write_float(v);
                    }
                    _ => self.sys.errorf("Server::ParseTimeScale: unexpected server state.\n"),
                }
            }
            56 => world.download_url = cut(&s.read_string(), 0x80),
            57 => {
                s.skip_string();
            }
            58 => {
                s.read_long();
                s.skip_string();
            }
            _ => {}
        }
    }

    /// svc_stufftext (§6.6.1).
    fn parse_stuff_text(&mut self, s: &mut BitBuffer, world: &mut World, director: &mut dyn Director) {
        let raw = s.read_string();
        if raw.len() >= 0x7FF {
            self.sys.print("WARNING! Server::ParseStuffText: command too long.\n");
            return;
        }
        let line = gs_common::bytestr::decode(&raw);
        let toks = crate::director::tokens(&line);
        let Some(t0) = toks.first() else {
            self.sys.print("WARNING! Server::ParseStuffText: command is NULL.\n");
            return;
        };
        let t0 = t0.to_ascii_lowercase();
        match t0.as_str() {
            "fullserverinfo" => {
                let v = toks.get(1).map(|t| t.as_bytes().to_vec()).unwrap_or_default();
                self.server_info = cut(&v, 512);
                world.set_server_info_string(&v);
                self.is_game_server = false;
                director.server_signal(4);
            }
            "reconnect" => {
                self.disconnect(world, director);
                self.paused = false;
            }
            "connect" => {
                self.sys.print("ERROR! Server::ParseStuffText: received redirect while offline.\n");
                self.disconnect(world, director);
            }
            "rate" | "cl_updaterate" | "ex_interp" | "cl_cmdrate" | "cl_cmdbackup" => {}
            _ => {
                let mut p = raw.clone();
                p.truncate(2046);
                match self.state {
                    6 => {
                        p.push(0);
                        world.add_signon_data(9, &p);
                    }
                    7 | 8 => {
                        self.unreliable.write_byte(9);
                        self.unreliable.write_string(&p);
                    }
                    _ => self.sys.errorf("Server::ParseStuffText: unexpected server state.\n"),
                }
            }
        }
    }

    /// svc_serverinfo (§6.6.2).
    fn parse_serverinfo(&mut self, s: &mut BitBuffer, world: &mut World, director: &mut dyn Director) {
        if self.demo.is_playing() && self.state == 7 {
            self.disconnect(world, director);
            return;
        }
        self.sys.dprint("Serverinfo packet received.\n");
        let protocol = s.read_long();
        if protocol != self.protocol {
            self.sys.print(&format!("WARNING! Server returned protocol version {protocol}, not {}\n", self.protocol));
            self.disconnect(world, director);
            return;
        }
        self.server_count = s.read_long();
        world.new_game(self.server_count);
        deliver(world, director);
        self.server_crc = s.read_long();
        let mut md5 = [0u8; 16];
        if let Some(m) = s.read_buf(16) {
            md5.copy_from_slice(&m);
        }
        let mut maxclients = s.read_byte();
        let mut playernum = s.read_byte();
        self.player_num = playernum;
        let mut crc = self.server_crc.to_le_bytes();
        gs_net::munge::unmunge(&mut crc, i32::from((-1 - playernum) as u8), gs_net::munge::MungeTable::Three);
        self.server_crc = i32::from_le_bytes(crc);
        if self.force_hltv {
            if maxclients == 1 {
                maxclients = 2;
            }
            playernum = maxclients - 1;
        }
        let gametype = s.read_byte();
        let gamedir = cut(&s.read_string(), 0x104);
        self.server_name = cut(&s.read_string(), 0x104);
        let levelname = cut(&s.read_string(), 0x104);
        s.skip_string();
        if s.read_byte() != 0 {
            let n = s.read_byte().max(0) as usize;
            s.skip_bytes(n);
            s.skip_bytes(16);
        }
        let name = self.server_name.clone();
        world.set_server_info(protocol, self.server_crc, md5, maxclients, playernum, gametype, &gamedir, &name, &levelname);
        world.is_hltv = self.is_hltv || self.force_hltv;
        self.set_state(6);
    }

    /// svc_temp_entity (§6.6.3).
    fn parse_temp_entity(&mut self, s: &mut BitBuffer, world: &mut World) {
        let start = s.byte_pos();
        let ty = s.read_byte();
        if !(0..128).contains(&ty) {
            self.sys.errorf("Server::ParseTempEntity:type >= TE_MAX.\n");
            return;
        }
        let len = match ty {
            13 => {
                s.skip_bytes(8);
                if s.read_short() != 0 {
                    s.skip_bytes(2);
                    12
                } else {
                    10
                }
            }
            29 => {
                s.skip_bytes(5);
                let effect = s.read_byte();
                let base = if effect == 2 {
                    s.skip_bytes(2);
                    8
                } else {
                    6
                };
                s.skip_bytes(14);
                base + 14 + s.skip_string()
            }
            _ => {
                let l = te_len(ty);
                if l < 0 {
                    self.sys.errorf(&format!("Server::ParseTempEntity:invalid type {ty}.\n"));
                    return;
                }
                s.skip_bytes(l as usize);
                l as usize
            }
        };
        let payload = s.slice(start, start + len + 1).to_vec();
        self.store(world, 23, &payload, Stream::Unreliable, "Server::ParseTempEntity");
    }

    /// svc_resourcelist (§6.6.4).
    fn parse_resource_list(&mut self, s: &mut BitBuffer, world: &mut World) {
        s.start_bit_mode();
        let n = s.read_bits(12);
        let mut added = 0;
        for _ in 0..n {
            if s.overflowed {
                break;
            }
            let kind = s.read_bits(4) as i32;
            let name = cut(&s.read_bit_string(), 0x104);
            let index = s.read_bits(12) as i32;
            let download_size = s.read_bits(24) as i32;
            let flags = s.read_bits(3) as u8;
            let mut md5 = [0u8; 16];
            if flags & 4 != 0 {
                for b in &mut md5 {
                    *b = s.read_bits(8) as u8;
                }
            }
            let mut reserved = [0u8; 32];
            if s.read_bit() {
                for b in &mut reserved {
                    *b = s.read_bits(8) as u8;
                }
            }
            if !name.starts_with(b"gfx/temp/") {
                world.add_resource(Resource { name, kind, index, download_size, flags, md5, reserved });
                added += 1;
            }
        }
        let _ = added;
        self.sys.print(&format!("Added {n} resources.\n"));
        if s.read_bit() {
            while s.read_bit() && !s.overflowed {
                let b = if s.read_bit() { 5 } else { 10 };
                s.read_bits(b);
            }
        }
        s.end_bit_mode();
    }

    /// User messages ≥ 59 (§6.6.5).
    fn parse_user_message(&mut self, cmd: i32, s: &mut BitBuffer, world: &mut World) -> bool {
        let Some(um) = world.user_msg(cmd).cloned() else {
            self.sys.dprint(&format!("WARNING! Server::ProcessMessage: unknown user message ({cmd}).\n"));
            return false;
        };
        let start = s.byte_pos();
        let (size, total) = if um.size == -1 {
            let n = s.read_byte();
            (n, n + 1)
        } else {
            (um.size, um.size)
        };
        if size > 192 {
            self.sys.dprint(&format!("WARNING! Server::ParseUserMessage:  User Msg {cmd} sent too much data ({size} bytes)\n"));
            return false;
        }
        let begin = s.byte_pos();
        s.skip_bytes(size.max(0) as usize);
        let payload = s.slice(start, start + total.max(0) as usize).to_vec();
        match self.state {
            6 => {
                world.add_signon_data(cmd as u8, &payload);
            }
            7 | 8 => {
                self.user_messages.write_byte(cmd as u8);
                self.user_messages.write_buf(&payload);
            }
            _ => self.sys.print("WARNING! Server::ParseUserMessage: unexpected server state.\n"),
        }
        let name = crate::world::cut(&um.name, 17);
        let text = s.slice(begin + 1, begin + size.max(0) as usize).to_vec();
        if name == b"SayText" {
            self.sys.print(&format!("{}\n", String::from_utf8_lossy(&cut(&text, text.len() + 1))));
        } else if name == b"TextMsg" {
            self.sys.dprint(&format!("{}\n", String::from_utf8_lossy(&cut(&text, text.len() + 1))));
        }
        true
    }
}

#[derive(Debug, Clone, Copy)]
enum Stream {
    Reliable,
    Unreliable,
}

/// `ReadBitVec3Coord` consumed.
fn skip_bit_vec3_coord(s: &mut BitBuffer) {
    let flags = [s.read_bit(), s.read_bit(), s.read_bit()];
    for f in flags {
        if f {
            let int = s.read_bit();
            let fract = s.read_bit();
            if int || fract {
                s.read_bit();
            }
            if int {
                s.read_bits(12);
            }
            if fract {
                s.read_bits(3);
            }
        }
    }
}

/// The svc_temp_entity length table (`core@0x10024BD0`): bytes after the type, −1 invalid.
fn te_len(ty: i32) -> i32 {
    match ty {
        0 | 19..=21 | 108 => 24,
        1 => 20,
        2 | 4 | 9..=11 | 121 => 6,
        3 | 23 => 11,
        5 | 17 | 22 | 100 | 112 | 122 => 10,
        6 | 27 | 102 | 115 => 12,
        7 | 31 | 106 | 110 => 17,
        8 | 18 | 24 | 28 | 119 => 16,
        12 => 8,
        14 | 104 | 109 | 118 => 9,
        15 | 25 | 30 | 113 | 114 => 19,
        99 => 2,
        101 | 103 => 14,
        105 => 5,
        107 | 123 => 13,
        111 | 116 | 117 | 124 => 7,
        120 | 126 => 18,
        125 => 1,
        127 => 15,
        _ => -1,
    }
}

/// `World::SaveAsDemo(name, director)` (§7.10): every stored frame through `WriteFrame` into a new
/// demo file, with the director's commands baked in.
pub fn save_as_demo(world: &mut World, sys: &Sys, name: &str, director: &mut dyn Director) -> bool {
    let Some(first) = world.first_frame() else { return false };
    let first_time = world.frame(first).map_or(0.0, |f| f.time);
    world.world_time = f64::from(first_time);
    let mut name = name.to_owned();
    if !name.contains('.') {
        name.push_str(".dem");
    }
    if name.starts_with('/') || name.contains('\\') || name.contains("..") || name.contains(':') {
        sys.print(&format!("WARNING! DemoFile::StartRecording: unable to open {name} (contains illegal characters).\n"));
        return false;
    }
    if !name.to_ascii_lowercase().ends_with(".dem") {
        sys.print(&format!("WARNING! DemoFile::StartRecording: unable to open {name} (wrong file extension, must be .dem).\n"));
        return false;
    }
    let start_time = sys.time.get();
    let mut out: Vec<u8> = Vec::new();
    let mut header = vec![0u8; 0x220];
    header[0..6].copy_from_slice(b"HLDEMO");
    header[8..12].copy_from_slice(&5i32.to_le_bytes());
    header[12..16].copy_from_slice(&48i32.to_le_bytes());
    let map = world.level_base();
    header[16..16 + map.len().min(259)].copy_from_slice(&map[..map.len().min(259)]);
    let dir = world.game_dir_base();
    header[276..276 + dir.len().min(259)].copy_from_slice(&dir[..dir.len().min(259)]);
    out.extend_from_slice(&header);
    let entry0 = out.len();
    sys.print(&format!("Start recording to {name}.\n"));
    // Sign-on: one type-0 frame, then a type-5 frame.
    let mut signon = BitBuffer::new(0x20000);
    world.write_new_data(&mut signon);
    world.write_signon_data(&mut signon);
    signon.write_byte(7);
    signon.write_float((sys.time.get() - start_time) as f32);
    for i in 0..world.max_clients {
        world.write_client_update(&mut signon, i);
    }
    world.write_light_styles(&mut signon);
    signon.write_byte(25);
    signon.write_byte(1);
    let mut frames = 0i32;
    let mut seqs = [0i32; 7];
    let write_frame = |out: &mut Vec<u8>, kind: u8, time: f32, frames: &mut i32, seqs: &[i32; 7], msg: Option<&[u8]>| {
        out.push(kind);
        out.extend_from_slice(&time.to_le_bytes());
        *frames += 1;
        out.extend_from_slice(&frames.to_le_bytes());
        if let Some(m) = msg {
            out.extend_from_slice(&[0u8; DEMO_INFO_SIZE]);
            for v in seqs {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&(m.len() as i32).to_le_bytes());
            out.extend_from_slice(m);
        }
    };
    write_frame(&mut out, 0, 0.0, &mut frames, &seqs, Some(signon.bytes()));
    write_frame(&mut out, 5, 0.0, &mut frames, &seqs, None);
    let entry0_len = out.len() - entry0;
    let entry1 = out.len();
    write_frame(&mut out, 2, 0.0, &mut frames, &seqs, None);
    sys.print(&format!("Saving game to \"{name}\". This can take some time...\n"));
    let mut prev_time = 0.0f32;
    let mut seq: u32 = 0;
    let mut client_delta: u32 = 0;
    let mut outgoing: i32 = 0;
    let mut cur = Some(first);
    while let Some(id) = cur {
        let Some((ftime, fseq)) = world.frame(id).map(|f| (f.time, f.seqnr)) else { break };
        world.world_time = f64::from(ftime);
        if seq == 0 || seq > fseq {
            seq = fseq.wrapping_sub(1);
        }
        let mut unrel = BitBuffer::new(0x10000);
        let mut rel = BitBuffer::new(0x10000);
        unrel.write_byte(7);
        unrel.write_float(ftime);
        world.write_frame(id, seq, &mut unrel, seq, client_delta);
        director.write_commands(&mut rel, prev_time, ftime);
        client_delta = (outgoing & 0xFF) as u32;
        seq = fseq;
        if rel.overflowed {
            sys.print("WARNING! World::SaveAsDemo: reliable data overflow.\n");
            return false;
        }
        if unrel.overflowed {
            unrel.clear();
        }
        // WriteSequenceInfo order: +0x84, +0x74, +0x8C, +0x88, +0x78, +0x80, +0x7C (Q14).
        seqs = [outgoing, 0, 0, 0, 0, 0, 0];
        let mut msg = unrel.bytes().to_vec();
        msg.extend_from_slice(rel.bytes());
        let t = (sys.time.get() - start_time) as f32;
        write_frame(&mut out, 1, t, &mut frames, &seqs, Some(&msg));
        outgoing += 1;
        let zero = [0u8; 32];
        out.push(4);
        out.extend_from_slice(&t.to_le_bytes());
        frames += 1;
        out.extend_from_slice(&frames.to_le_bytes());
        out.extend_from_slice(&zero);
        prev_time = ftime;
        cur = world.frame_by_seq(fseq + 1);
    }
    // CloseFile: type 5, the directory, the header's directory offset.
    let t = (sys.time.get() - start_time) as f32;
    write_frame(&mut out, 5, t, &mut frames, &seqs, None);
    let entry1_len = out.len() - entry1;
    let dir_off = out.len() as i32;
    out.extend_from_slice(&2i32.to_le_bytes());
    for (k, (kind, desc, off, len, time)) in [(0i32, &b"LOADING"[..], entry0, entry0_len, 0.0f32), (1, b"Playback", entry1, entry1_len, t)].into_iter().enumerate() {
        let mut e = vec![0u8; 0x5C];
        e[0..4].copy_from_slice(&kind.to_le_bytes());
        e[4..4 + desc.len()].copy_from_slice(desc);
        e[0x4C..0x50].copy_from_slice(&time.to_le_bytes());
        e[0x50..0x54].copy_from_slice(&(if k == 1 { frames } else { 0 }).to_le_bytes());
        e[0x54..0x58].copy_from_slice(&(off as i32).to_le_bytes());
        e[0x58..0x5C].copy_from_slice(&(len as i32).to_le_bytes());
        out.extend_from_slice(&e);
    }
    out[540..544].copy_from_slice(&dir_off.to_le_bytes());
    let ok = sys.files.write(&name, &out);
    if ok {
        sys.print(&format!("Completed demo {name}.\n"));
    }
    let _ = file_base;
    ok
}
