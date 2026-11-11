//! Core.dll's `World` module (`world001` "DemoWorld", docs/re/hltv-world.md §3, §5, §6.7, §7):
//! the game state and the stored frames of an ingested demo, and every stream writer the demo
//! player's virtual client receives.

use crate::bitbuf::BitBuffer;
use crate::delta::{Delta, DeltaTime, Header, Registry, parse_description, write_description};
use crate::dict::Dict;
use crate::Sys;

pub const ENTITY_SIZE: usize = 0x154;
pub const CLIENTDATA_SIZE: usize = 0x1DC;
pub const DEMO_INFO_SIZE: usize = 0x1B4;
pub const MOVEVARS_SIZE: usize = 0x84;
pub const MAX_ENTITIES: usize = 1380;
pub const MAX_PACKET_ENTITIES: usize = 1024;
/// `entityType` of an unused baseline slot.
const UNUSED: i32 = 0x4000_0000;

pub type Ent = [u8; ENTITY_SIZE];

fn i32_at(b: &[u8], o: usize) -> i32 {
    b.get(o..o + 4).map_or(0, |s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(i32_at(b, o) as u32)
}

fn put_i32(b: &mut [u8], o: usize, v: i32) {
    if let Some(d) = b.get_mut(o..o + 4) {
        d.copy_from_slice(&v.to_le_bytes());
    }
}

/// `strncpy(dst, s, n)` + terminator at `n - 1`: at most `n - 1` bytes.
pub fn cut(s: &[u8], n: usize) -> Vec<u8> {
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len()).min(n.saturating_sub(1));
    s[..end].to_vec()
}

/// A stored frame (`frame_t`, delta −1): entities in the Core-compressed form.
#[derive(Debug, Clone, Default)]
pub struct Frame {
    pub time: f32,
    pub seqnr: u32,
    pub entities: Vec<u8>,
    pub entitynum: u32,
    pub client_data: Vec<u8>,
    pub events: Vec<u8>,
    pub eventnum: u32,
    pub reliable: Vec<u8>,
    pub unreliable: Vec<u8>,
    pub voice: Vec<u8>,
    pub user_messages: Vec<u8>,
    pub demo_data: Vec<u8>,
    pub demo_info: Option<Box<[u8; DEMO_INFO_SIZE]>>,
}

/// The frame the server assembles (`Server::m_Frame`, delta 0: entities uncompressed).
#[derive(Debug, Clone, Default)]
pub struct InFrame {
    pub time: f32,
    pub seqnr: u32,
    pub entities: Vec<Ent>,
    pub entitynum: u32,
    pub client_data: Vec<u8>,
    pub events: Vec<u8>,
    pub eventnum: u32,
    pub reliable: Vec<u8>,
    pub unreliable: Vec<u8>,
    pub voice: Vec<u8>,
    pub user_messages: Vec<u8>,
    pub demo_data: Vec<u8>,
    pub demo_info: Option<Box<[u8; DEMO_INFO_SIZE]>>,
}

/// An uncompressed copy of a stored frame (`GetUncompressedFrame`).
#[derive(Debug, Clone, Default)]
pub struct Uncompressed {
    pub time: f32,
    pub seqnr: u32,
    pub entitynum: u32,
    pub ents: Vec<Ent>,
}

/// `player_t`.
#[derive(Debug, Clone, Default)]
pub struct Player {
    pub userid: i32,
    pub userinfo: Vec<u8>,
    pub name: Vec<u8>,
    pub hltv: i32,
    pub model: Vec<u8>,
    pub topcolor: i32,
    pub bottomcolor: i32,
    pub active: bool,
    pub hashedcdkey: [u8; 16],
    pub fid: i32,
}

/// A `svc_newusermsg` registration (`UserMsg`, name kept as 16 raw bytes).
#[derive(Debug, Clone)]
pub struct UserMsg {
    pub msg: i32,
    pub size: i32,
    pub name: [u8; 16],
}

/// A `resource_t` as Core keeps it.
#[derive(Debug, Clone)]
pub struct Resource {
    pub name: Vec<u8>,
    pub kind: i32,
    pub index: i32,
    pub download_size: i32,
    pub flags: u8,
    pub md5: [u8; 16],
    pub reserved: [u8; 32],
}

/// `World::Init` … `ShutDown`.
pub struct World {
    sys: Sys,
    /// `m_SystemTime` (every `RunFrame(t)`).
    pub system_time: f64,
    /// Signals fired since the last drain (`FireSignal`), with their data.
    signals: Vec<(i32, i32)>,
    pub paused: bool,
    /// `m_WorldState`: 1 initializing, 2 disconnected, 3 connecting, 4 running, 5 complete.
    pub state: i32,
    pub protocol: i32,
    pub server_count: i32,
    pub server_crc: i32,
    pub client_dll_md5: [u8; 16],
    pub max_clients: i32,
    pub player_num: i32,
    pub game_type: i32,
    pub game_dir: Vec<u8>,
    pub level_name: Vec<u8>,
    pub server_name: Vec<u8>,
    pub view_entity: i32,
    pub players: Vec<Player>,
    baselines: Vec<Ent>,
    pub max_baseline_entity: i32,
    instanced: Vec<Ent>,
    pub max_instanced: i32,
    lightstyles: Vec<Vec<u8>>,
    movevars: [u8; MOVEVARS_SIZE],
    pub server_info: Vec<u8>,
    user_msgs: Vec<UserMsg>,
    resources: Vec<Resource>,
    signon: BitBuffer,
    pub cd_track: i32,
    pub cd_looptrack: i32,
    frames: Vec<Option<Frame>>,
    by_seq: Dict<usize>,
    by_time: Dict<usize>,
    pub sequence_nr: u32,
    max_buffer_length: f32,
    frame_cache: Vec<(u32, Vec<Ent>)>,
    frame_cache_index: usize,
    delta_cache: Vec<(u32, u32, BitBuffer)>,
    delta_cache_index: usize,
    pub cache_hits: i32,
    pub cache_faults: i32,
    pub world_time: f64,
    pub start_time: f64,
    pub voice_enabled: bool,
    pub fallback_dir: Vec<u8>,
    pub allow_cheats: bool,
    pub is_hltv: bool,
    pub host_name: Vec<u8>,
    pub registry: Registry,
    dtime: DeltaTime,
    /// `g_DownloadURL` (a Core global).
    pub download_url: Vec<u8>,
}

impl World {
    /// `World::Init`.
    pub fn new(sys: Sys) -> Self {
        let cache = 32;
        let mut w = Self {
            sys,
            system_time: 0.0,
            signals: Vec::new(),
            paused: false,
            state: 1,
            protocol: 48,
            server_count: 0,
            server_crc: 0,
            client_dll_md5: [0; 16],
            max_clients: 0,
            player_num: 0,
            game_type: 0,
            game_dir: Vec::new(),
            level_name: Vec::new(),
            server_name: b"Unnamed HLTV".to_vec(),
            view_entity: 0,
            players: vec![Player::default(); 32],
            baselines: vec![[0; ENTITY_SIZE]; MAX_ENTITIES],
            max_baseline_entity: 0,
            instanced: vec![[0; ENTITY_SIZE]; 64],
            max_instanced: 0,
            lightstyles: vec![Vec::new(); 64],
            movevars: [0; MOVEVARS_SIZE],
            server_info: Vec::new(),
            user_msgs: Vec::new(),
            resources: Vec::new(),
            signon: BitBuffer::new(0x8000),
            cd_track: 0,
            cd_looptrack: 0,
            frames: Vec::new(),
            by_seq: Dict::default(),
            by_time: Dict::default(),
            sequence_nr: 0,
            max_buffer_length: -1.0,
            frame_cache: vec![(0, Vec::new()); cache],
            frame_cache_index: 0,
            delta_cache: (0..cache).map(|_| (0, 0, BitBuffer::new(0))).collect(),
            delta_cache_index: 0,
            cache_hits: 0,
            cache_faults: 0,
            world_time: 0.0,
            start_time: 0.0,
            voice_enabled: false,
            fallback_dir: Vec::new(),
            allow_cheats: false,
            is_hltv: false,
            host_name: Vec::new(),
            registry: Registry::default(),
            dtime: DeltaTime::default(),
            download_url: Vec::new(),
        };
        w.reset();
        w.state = 2;
        w.sys.print("World module initialized.\n");
        w
    }

    /// Signals fired since the last call, in order (`FireSignal`).
    pub fn take_signals(&mut self) -> Vec<(i32, i32)> {
        std::mem::take(&mut self.signals)
    }

    fn fire(&mut self, sig: i32, data: i32) {
        self.signals.push((sig, data));
    }

    fn errorf(&self, s: &str) {
        self.sys.errorf(s);
    }

    /// `ClearBaseline`.
    fn clear_baseline(&mut self) {
        for b in &mut self.baselines {
            *b = [0; ENTITY_SIZE];
            put_i32(b, 0, UNUSED);
        }
    }

    /// `ClearFrameCaches`.
    fn clear_frame_caches(&mut self) {
        for c in &mut self.frame_cache {
            *c = (0, Vec::new());
        }
        for c in &mut self.delta_cache {
            *c = (0, 0, BitBuffer::new(0));
        }
        self.cache_hits = 1;
        self.cache_faults = 1;
    }

    /// `Reset` (§5.8; Q16: identity strings, movevars, CD info and the instanced count stay).
    pub fn reset(&mut self) {
        self.user_msgs.clear();
        self.resources.clear();
        self.clear_baseline();
        for b in &mut self.instanced {
            *b = [0; ENTITY_SIZE];
        }
        for l in &mut self.lightstyles {
            l.clear();
        }
        for p in &mut self.players {
            *p = Player::default();
        }
        self.clear_frame_caches();
        self.server_info.clear();
        self.frames.clear();
        self.by_seq.clear();
        self.by_time.clear();
        self.signon.clear();
        self.sequence_nr = 0;
        self.view_entity = 0;
        self.world_time = 0.0;
        self.start_time = 0.0;
        self.voice_enabled = false;
        self.fallback_dir.clear();
        self.allow_cheats = false;
        self.is_hltv = false;
        self.paused = false;
        self.download_url.clear();
    }

    fn set_state(&mut self, to: i32) {
        let ok = match to {
            3 => matches!(self.state, 2..=5),
            4 => matches!(self.state, 3 | 4),
            _ => true,
        };
        if ok {
            self.state = to;
        } else {
            self.errorf(&format!("World::SetState: not valid m_State ({} -> {}).\n", self.state, to));
        }
    }

    /// `NewGame(serverCount)`.
    pub fn new_game(&mut self, server_count: i32) {
        self.fire(1, 0);
        self.reset();
        self.server_count = server_count;
        self.set_state(3);
    }

    /// `ConnectionComplete` (from the first `AddFrame` of a game).
    fn connection_complete(&mut self) {
        let (p, e, c) = (self.delta("entity_state_player_t"), self.delta("entity_state_t"), self.delta("custom_entity_state_t"));
        match (p, e, c) {
            (Some(p), Some(e), Some(c)) => self.write_baseline(&p, &e, &c),
            _ => self.errorf("World::WriteBaseline: delta encoder not found.\n"),
        }
        self.start_time = self.system_time;
        self.set_state(4);
        self.fire(2, 0);
    }

    /// `FinishGame`.
    pub fn finish_game(&mut self) {
        if self.state == 4 {
            self.state = 5;
            self.fire(7, 0);
        }
    }

    /// `StopGame`.
    pub fn stop_game(&mut self) {
        self.fire(1, 0);
        self.reset();
        self.state = 2;
    }

    /// `ShutDown`.
    pub fn shut_down(&mut self) {
        self.reset();
        self.state = 2;
        self.fire(8, 0);
        self.sys.print("World module shutdown.\n");
    }

    pub fn is_active(&self) -> bool {
        self.state >= 4
    }

    pub fn is_complete(&self) -> bool {
        self.state == 5
    }

    pub fn is_player_index(&self, i: i32) -> bool {
        i >= 1 && i <= self.max_clients
    }

    /// `GetHostName`: the proxy-set name, else `m_ServerName`.
    pub fn host_name(&self) -> &[u8] {
        if self.host_name.is_empty() { &self.server_name } else { &self.host_name }
    }

    fn delta(&self, name: &str) -> Option<Delta> {
        self.registry.get(name).cloned()
    }

    // --- setters (§3, §5.4) --------------------------------------------------------------------

    /// `SetServerInfo(protocol, crc, md5, maxclients, playernum, gametype, gamedir, hostname, levelname)`.
    #[allow(clippy::too_many_arguments)]
    pub fn set_server_info(&mut self, protocol: i32, crc: i32, md5: [u8; 16], maxclients: i32, playernum: i32, gametype: i32, gamedir: &[u8], hostname: &[u8], levelname: &[u8]) {
        self.protocol = protocol;
        self.server_crc = crc;
        self.client_dll_md5 = md5;
        self.max_clients = maxclients;
        self.player_num = playernum;
        self.game_type = gametype;
        self.game_dir = cut(gamedir, 0x104);
        self.level_name = cut(levelname, 0x28);
        let mut name = hostname.to_vec();
        name.extend_from_slice(format!(":{playernum}").as_bytes());
        name.truncate(0xFE);
        self.server_name = name;
    }

    /// `SetServerInfoString`.
    pub fn set_server_info_string(&mut self, s: &[u8]) {
        self.server_info = cut(s, 512);
    }

    /// `SetPaused`: signals 5/6 only on change.
    pub fn set_paused(&mut self, p: bool) {
        if self.paused != p {
            self.paused = p;
            self.fire(if p { 5 } else { 6 }, 0);
        }
    }

    /// `SetBufferSize(s)`.
    pub fn set_buffer_size(&mut self, s: f32) {
        self.max_buffer_length = if s <= 0.0 { -1.0 } else { s + 10.0 };
    }

    pub fn set_move_vars(&mut self, mv: &[u8]) {
        let n = mv.len().min(MOVEVARS_SIZE);
        self.movevars[..n].copy_from_slice(&mv[..n]);
    }

    /// `SetExtraInfo(fallbackDir, allowCheats)`.
    pub fn set_extra_info(&mut self, dir: &[u8], cheats: bool) {
        self.fallback_dir = cut(dir, 0x104);
        self.allow_cheats = cheats;
    }

    /// `AddResource` (prepended).
    pub fn add_resource(&mut self, r: Resource) {
        self.resources.insert(0, r);
    }

    /// `AddLightStyle(i, s)`.
    pub fn add_light_style(&mut self, i: i32, s: &[u8]) {
        if !(0..64).contains(&i) {
            self.sys.print(&format!("WARNING! World::SetLightStyle: index ({i}) out of bounds.\n"));
            return;
        }
        if s.len() > 64 {
            self.sys.print(&format!("WARNING! World::SetLightStyle: style too long ({}).\n", s.len()));
        }
        self.lightstyles[i as usize] = cut(s, 64);
    }

    /// `AddSignonData(svc, data)`: the overflow flag.
    pub fn add_signon_data(&mut self, svc: u8, data: &[u8]) -> bool {
        self.signon.write_byte(svc);
        self.signon.write_buf(data);
        self.signon.overflowed
    }

    /// `AddUserMessage(msg, size, name)`: an existing name (`_stricmp`) is updated.
    pub fn add_user_message(&mut self, msg: i32, size: i32, name: &[u8]) -> bool {
        let mut raw = [0u8; 16];
        let n = name.iter().position(|&c| c == 0).unwrap_or(name.len()).min(16);
        raw[..n].copy_from_slice(&name[..n]);
        let size = if size == 255 { -1 } else { size };
        let key = cut(&raw, 17);
        if let Some(u) = self.user_msgs.iter_mut().find(|u| cut(&u.name, 17).eq_ignore_ascii_case(&key)) {
            u.msg = msg;
            u.size = size;
            return false;
        }
        self.user_msgs.insert(0, UserMsg { msg, size, name: raw });
        true
    }

    /// `GetUserMsg(msgNumber)`.
    pub fn user_msg(&self, msg: i32) -> Option<&UserMsg> {
        self.user_msgs.iter().find(|u| u.msg == msg)
    }

    /// `AddBaselineEntity(i, s)`.
    pub fn add_baseline_entity(&mut self, i: i32, s: &Ent) {
        if !(0..MAX_ENTITIES as i32).contains(&i) {
            self.sys.print(&format!("WARNING! World::SetBaselineEntity: index ({i}) out of bounds.\n"));
            return;
        }
        self.baselines[i as usize] = *s;
    }

    /// `AddInstancedBaselineEntity(i, s)`.
    pub fn add_instanced_baseline_entity(&mut self, i: i32, s: &Ent) {
        if !(0..64).contains(&i) {
            self.sys.print(&format!("WARNING! World::SetInstancedBaselineEntity: index ({i}) out of bounds.\n"));
            return;
        }
        self.instanced[i as usize] = *s;
    }

    /// `UpdatePlayer(slot, userId, info, hashedcdkey)` (Q12, Q18).
    pub fn update_player(&mut self, slot: i32, userid: i32, info: &[u8], hash: [u8; 16]) {
        if !(0..32).contains(&slot) {
            self.errorf(&format!("World::UpdatePlayer: player number {slot} > MAX_CLIENTS\n"));
            return;
        }
        if info.is_empty() {
            return;
        }
        let mut s = info.to_vec();
        let p = &mut self.players[slot as usize];
        p.active = true;
        self.sys.dprint(&format!("Player update({}:{})\n", slot, String::from_utf8_lossy(&p.name)));
        remove_prefixed_keys(&mut s, b'_');
        p.userid = userid;
        p.userinfo = cut(&s, 256);
        p.hashedcdkey = hash;
        p.name = cut(&value_for_key(&s, b"name"), 0x20);
        p.model = cut(&value_for_key(&s, b"model"), 0x104);
        p.topcolor = atoi(&value_for_key(&s, b"topcolor"));
        p.bottomcolor = atoi(&value_for_key(&s, b"bottomcolor"));
        p.hltv = atoi(&value_for_key(&s, b"*hltv"));
        p.fid = atoi(&value_for_key(&s, b"*fid"));
        self.fire(4, slot);
    }

    // --- frames (§5.9–§5.11) -------------------------------------------------------------------

    pub fn frame(&self, id: usize) -> Option<&Frame> {
        self.frames.get(id).and_then(Option::as_ref)
    }

    /// `GetFrameBySeqNr(n)`.
    pub fn frame_by_seq(&mut self, seq: u32) -> Option<usize> {
        self.by_seq.find_exact(f64::from(seq) as f32)
    }

    /// `GetFirstFrame`.
    pub fn first_frame(&mut self) -> Option<usize> {
        self.by_seq.first()
    }

    /// `GetLastFrame`.
    pub fn last_frame(&self) -> Option<usize> {
        self.by_seq.last()
    }

    /// `GetFrameByTime(t)` (§5.11): compared with the (possibly rewritten) frame time.
    pub fn frame_by_time(&mut self, t: f64) -> Option<usize> {
        let f = self.by_time.find_closest(t as f32)?;
        if t > f64::from(self.frame(f).map_or(0.0, |fr| fr.time))
            && let Some(g) = self.by_time.get_next()
        {
            return Some(g);
        }
        Some(f)
    }

    /// `AddFrame(newFrame)` → the new sequence number (0 on failure).
    pub fn add_frame(&mut self, nf: &InFrame) -> u32 {
        self.sequence_nr += 1;
        let mut f = Frame { seqnr: self.sequence_nr, time: nf.time, ..Frame::default() };
        if self.world_time > f64::from(nf.time) {
            self.sys.dprint(&format!("Fixing frame time, delta {:.3}\n", self.world_time - f64::from(nf.time)));
            let mut t = f.time - 0.05;
            if let Some(mut last) = self.by_seq.last() {
                let mut seq = self.frames[last].as_ref().map_or(0, |fr| fr.seqnr).wrapping_sub(1);
                let mut prev = self.by_seq.find_exact(seq as i32 as f32);
                while let Some(p) = prev {
                    let (lt, pt) = (self.frames[last].as_ref().map_or(0.0, |x| x.time), self.frames[p].as_ref().map_or(0.0, |x| x.time));
                    let d = lt - pt;
                    if let Some(x) = self.frames[last].as_mut() {
                        x.time = t;
                    }
                    t -= d;
                    last = p;
                    seq = seq.wrapping_sub(1);
                    prev = self.by_seq.find_exact(seq as i32 as f32);
                }
                if let Some(x) = self.frames[last].as_mut() {
                    x.time = t;
                }
            }
        }
        self.world_time = f64::from(f.time);
        if self.state == 3 {
            self.connection_complete();
        }
        if !nf.entities.is_empty() || nf.entitynum != 0 {
            self.dtime.large = true;
            let mut stream = BitBuffer::new(0x55000);
            let u = Uncompressed { time: nf.time, seqnr: 0, entitynum: nf.entitynum, ents: nf.entities.clone() };
            let n = self.compress_frame(&u, &mut stream);
            self.dtime.large = false;
            if stream.overflowed {
                self.sys.print("WARNING! World::AddFrame: couldn't compress frame entities.\n");
                return 0;
            }
            f.entities = stream.slice(0, n).to_vec();
            f.entitynum = nf.entitynum;
        }
        f.client_data = nf.client_data.clone();
        if !nf.events.is_empty() {
            f.events = nf.events.clone();
            f.eventnum = nf.eventnum;
        }
        f.reliable = nf.reliable.clone();
        f.unreliable = nf.unreliable.clone();
        f.voice = nf.voice.clone();
        f.user_messages = nf.user_messages.clone();
        f.demo_data = nf.demo_data.clone();
        f.demo_info = nf.demo_info.clone();
        let seq = f.seqnr;
        let id = self.frames.len();
        self.frames.push(Some(f));
        self.by_seq.add(id, f64::from(seq) as f32);
        self.by_time.add(id, self.world_time as f32);
        self.fire(3, seq as i32);
        seq
    }

    /// `GetUncompressedFrame(frame)` (§5.7): the frame cache keyed by seqnr.
    pub fn uncompressed(&mut self, id: usize) -> Option<Uncompressed> {
        let f = self.frame(id)?;
        let (time, seqnr, n) = (f.time, f.seqnr, f.entitynum);
        if n as usize > MAX_PACKET_ENTITIES {
            self.errorf("World::GetUncompressedFrame: entnum>=MAX_PACKET_ENTITIES.\n");
        }
        if n == 0 {
            return Some(Uncompressed { time, seqnr, entitynum: 0, ents: Vec::new() });
        }
        if let Some(c) = self.frame_cache.iter().find(|c| c.0 == seqnr) {
            let ents = c.1.clone();
            self.cache_hits += 1;
            return Some(Uncompressed { time, seqnr, entitynum: n, ents });
        }
        let slot = self.frame_cache_index;
        self.frame_cache_index = (self.frame_cache_index + 1) % self.frame_cache.len();
        self.cache_faults += 1;
        self.dtime.large = true;
        let data = self.frame(id).map(|f| f.entities.clone()).unwrap_or_default();
        let mut stream = BitBuffer::from_bytes(&data);
        let mut ents = vec![[0u8; ENTITY_SIZE]; n.min(MAX_PACKET_ENTITIES as u32) as usize];
        self.uncompress_full(time, n, &mut ents, &mut stream);
        self.dtime.large = false;
        self.frame_cache[slot] = (seqnr, ents.clone());
        Some(Uncompressed { time, seqnr, entitynum: n, ents })
    }

    /// `GetUncompressedFrame(seq)`.
    pub fn uncompressed_by_seq(&mut self, seq: u32) -> Option<Uncompressed> {
        let id = self.frame_by_seq(seq)?;
        if self.frame(id).map(|f| f.seqnr) != Some(seq) {
            self.sys.dprint(&format!("WARNING! World::GetUncompressedFrame: frame not found {seq}.\n"));
            return None;
        }
        self.uncompressed(id)
    }

    // --- ingest parsers (§6.7) -------------------------------------------------------------------

    /// `ParseBaseline` (svc_spawnbaseline).
    pub fn parse_baseline(&mut self, s: &mut BitBuffer) {
        let (Some(de), Some(dp), Some(dc)) = (self.delta("entity_state_t"), self.delta("entity_state_player_t"), self.delta("custom_entity_state_t")) else {
            self.errorf("Server::ParseBaseline: needed delta encoder missing.\n");
            return;
        };
        s.start_bit_mode();
        self.max_baseline_entity = 0;
        self.clear_baseline();
        let zero = [0u8; ENTITY_SIZE];
        let mut count = 0;
        while s.peek_bits(16) != 0xFFFF && !s.overflowed {
            let num = s.read_bits(11) as i32;
            self.max_baseline_entity = self.max_baseline_entity.max(num);
            let mut e = [0u8; ENTITY_SIZE];
            let ty = s.read_bits(2) as i32;
            let d = if ty & 2 != 0 { &dc } else if self.is_player_index(num) { &dp } else { &de };
            d.parse(s, &zero, &mut e, self.dtime);
            put_i32(&mut e, 0, ty);
            self.add_baseline_entity(num, &e);
            count += 1;
        }
        self.sys.print(&format!("Received baseline with {count} entities.\n"));
        s.read_bits(16);
        for b in &mut self.instanced {
            *b = [0; ENTITY_SIZE];
        }
        self.max_instanced = s.read_bits(6) as i32;
        for i in 0..self.max_instanced {
            let mut e = [0u8; ENTITY_SIZE];
            de.parse(s, &zero, &mut e, self.dtime);
            self.add_instanced_baseline_entity(i, &e);
        }
        if self.max_instanced != 0 {
            self.sys.print(&format!("Received instanced baseline with {} entities.\n", self.max_instanced));
        }
        s.end_bit_mode();
    }

    /// `ParseDeltaDescription` (svc_deltadescription); an unregistered name is skipped (Q7).
    pub fn parse_delta_description(&mut self, s: &mut BitBuffer) {
        let name = s.read_string();
        if name.is_empty() {
            self.errorf("ParseDeltaDescription:  Illegible description name\n");
            return;
        }
        let name = String::from_utf8_lossy(&name).into_owned();
        self.sys.dprint(&format!("Reading delta description for: {name}.\n"));
        s.start_bit_mode();
        let n = s.read_bits(16) as usize;
        let d = parse_description(s, n);
        s.end_bit_mode();
        if !self.registry.set(&name, d) {
            log::warn!("svc_deltadescription for unregistered {name} (Core dereferences NULL)");
        }
    }

    /// `GetClientData(frame, cd)`: the stored client data delta from zero.
    pub fn client_data(&self, id: usize, cd: &mut [u8; CLIENTDATA_SIZE]) -> bool {
        let Some(f) = self.frame(id) else { return false };
        if f.client_data.is_empty() {
            return false;
        }
        let Some(d) = self.registry.get("clientdata_t") else { return false };
        let mut s = BitBuffer::from_bytes(&f.client_data);
        s.start_bit_mode();
        *cd = [0; CLIENTDATA_SIZE];
        let zero = [0u8; CLIENTDATA_SIZE];
        d.parse(&mut s, &zero, cd, self.dtime);
        s.end_bit_mode();
        true
    }

    /// `ParseClientData(in, deltaSeq, to, cd)`: the stored form (delta from zero) and the weapons skipped.
    pub fn parse_client_data(&mut self, s: &mut BitBuffer, delta_seq: u32, to: &mut BitBuffer, cd: &mut [u8; CLIENTDATA_SIZE]) {
        let zero = [0u8; CLIENTDATA_SIZE];
        let mut base = [0u8; CLIENTDATA_SIZE];
        put_i32(cd, 0, 0);
        if delta_seq != 0 {
            let id = self.frame_by_seq(delta_seq);
            if !id.is_some_and(|id| self.client_data(id, &mut base)) {
                self.sys.print(&format!("WARNING! World::ParseClientData: couldn't uncompress delta frame {delta_seq}\n"));
            }
        }
        let (Some(dc), Some(dw)) = (self.delta("clientdata_t"), self.delta("weapon_data_t")) else { return };
        dc.parse(s, &base, cd, self.dtime);
        to.start_bit_mode();
        dc.write(to, &zero, cd, true, None, self.dtime);
        to.end_bit_mode();
        while s.read_bit() && !s.overflowed {
            s.read_bits(6);
            let mut w = [0u8; 88];
            let from = w;
            dw.parse(s, &from, &mut w, self.dtime);
        }
    }

    /// `ParseEvent`: one event_args delta, discarded.
    pub fn parse_event(&mut self, s: &mut BitBuffer) {
        if let Some(d) = self.delta("event_t") {
            let mut a = [0u8; 72];
            let from = a;
            d.parse(s, &from, &mut a, self.dtime);
        }
    }

    fn entity_delta(&self, custom: bool, num: i32) -> Option<Delta> {
        if custom {
            self.delta("custom_entity_state_t")
        } else if self.is_player_index(num) {
            self.delta("entity_state_player_t")
        } else {
            self.delta("entity_state_t")
        }
    }

    /// `UncompressEntitiesFromStream(frame, in)` — the full list (§6.7.5).
    pub fn uncompress_full(&mut self, time: f32, entitynum: u32, dst: &mut [Ent], s: &mut BitBuffer) -> bool {
        self.dtime.time = f64::from(time);
        s.start_bit_mode();
        let mut num = 0i32;
        let mut count = 0usize;
        while !s.overflowed {
            if s.peek_bits(16) == 0 {
                break;
            }
            if s.read_bit() {
                num += 1;
            } else {
                num = if s.read_bit() { s.read_bits(11) as i32 } else { num + s.read_bits(6) as i32 };
            }
            let custom = s.read_bit();
            let mut inst = None;
            if self.max_instanced != 0 && s.read_bit() {
                inst = Some(s.read_bits(6) as usize);
            }
            let mut off = 0usize;
            if inst.is_none() && s.read_bit() {
                off = s.read_bits(6) as usize;
            }
            if num as usize >= MAX_ENTITIES {
                self.errorf(&format!("World::GetUncompressedFrame: entity number {num} > MAX_ENTITIES\n"));
                return false;
            }
            if count >= dst.len() {
                break;
            }
            let from = match inst {
                Some(i) => self.instanced.get(i).copied().unwrap_or([0; ENTITY_SIZE]),
                None if off != 0 => count.checked_sub(off).map_or([0; ENTITY_SIZE], |k| dst[k]),
                None => self.baselines[num as usize],
            };
            let Some(d) = self.entity_delta(custom, num) else { return false };
            let mut e = [0u8; ENTITY_SIZE];
            d.parse(s, &from, &mut e, self.dtime);
            put_i32(&mut e, 0, if custom { 2 } else { 1 });
            put_i32(&mut e, 4, num);
            dst[count] = e;
            count += 1;
        }
        if s.overflowed {
            self.sys.print("WARNING! World::UncompressEntitiesFromStream: incoming entities are corrupt.\n");
        }
        if s.read_short() != 0 {
            self.sys.print("WARNING! World::UncompressEntitiesFromStream: missing end tag.\n");
            return false;
        }
        s.end_bit_mode();
        if count != entitynum as usize {
            self.sys.dprint("WARNING! World::UncompressEntitiesFromStream: newindex != entnum.\n");
            return false;
        }
        true
    }

    /// `UncompressEntitiesFromStream(frame, in, deltaSeq)` — against a stored frame (§6.7.6).
    pub fn uncompress_delta(&mut self, time: f32, entitynum: u32, dst: &mut Vec<Ent>, s: &mut BitBuffer, delta_seq: u32) -> bool {
        let Some(old) = self.uncompressed_by_seq(delta_seq) else {
            self.sys.dprint("WARNING! World::UncompressEntitiesFromStream: delta frame too old.\n");
            s.overflowed = true;
            return false;
        };
        self.dtime.time = f64::from(time);
        s.start_bit_mode();
        dst.clear();
        let mut oldi = 0usize;
        let mut num = 0i32;
        let oldn = old.entitynum as usize;
        let push = |dst: &mut Vec<Ent>, e: Ent, sys: &Sys, s: &mut BitBuffer, msg: &str| {
            if dst.len() >= MAX_PACKET_ENTITIES {
                sys.dprint(msg);
                s.overflowed = true;
            } else {
                dst.push(e);
            }
        };
        while s.peek_bits(16) != 0 && !s.overflowed {
            let remove = s.read_bit();
            num = if s.read_bit() { s.read_bits(11) as i32 } else { num + s.read_bits(6) as i32 };
            let mut custom = false;
            let mut inst = None;
            if !remove {
                custom = s.read_bit();
                if self.max_instanced != 0 && s.read_bit() {
                    inst = Some(s.read_bits(6) as usize);
                }
            }
            let mut oldnum = if oldi < oldn { i32_at(&old.ents[oldi], 4) } else { 9999 };
            while num > oldnum {
                push(dst, old.ents[oldi], &self.sys, s, "WARNING!World::UncompressEntitiesFromStream: newindex >= MAX_PACKET_ENTITIES.\n");
                oldi += 1;
                oldnum = if oldi < oldn { i32_at(&old.ents[oldi], 4) } else { 9999 };
            }
            if num < oldnum {
                if remove {
                    continue;
                }
                let from = match inst {
                    Some(i) => self.instanced.get(i).copied().unwrap_or([0; ENTITY_SIZE]),
                    None => self.baselines.get(num as usize).copied().unwrap_or([0; ENTITY_SIZE]),
                };
                let Some(d) = self.entity_delta(custom, num) else { return false };
                let mut e = [0u8; ENTITY_SIZE];
                d.parse(s, &from, &mut e, self.dtime);
                put_i32(&mut e, 0, if custom { 2 } else { 1 });
                put_i32(&mut e, 4, num);
                push(dst, e, &self.sys, s, "World::UncompressEntitiesFromStream: newindex >= MAX_PACKET_ENTITIES.\n");
            } else if num == oldnum {
                if remove {
                    oldi += 1;
                } else {
                    let Some(d) = self.entity_delta(custom, num) else { return false };
                    let mut e = [0u8; ENTITY_SIZE];
                    d.parse(s, &old.ents[oldi], &mut e, self.dtime);
                    put_i32(&mut e, 0, if custom { 2 } else { 1 });
                    put_i32(&mut e, 4, num);
                    push(dst, e, &self.sys, s, "World::UncompressEntitiesFromStream: newindex >= MAX_PACKET_ENTITIES.\n");
                    oldi += 1;
                }
            }
        }
        if s.read_short() != 0 {
            self.sys.dprint("WARNING! World::UncompressEntitiesFromStream: missing end tag.\n");
            return false;
        }
        s.end_bit_mode();
        while oldi < oldn {
            push(dst, old.ents[oldi], &self.sys, s, "WARNING! World::UncompressEntitiesFromStream: newindex >= MAX_PACKET_ENTITIES.\n");
            oldi += 1;
        }
        if dst.len() != entitynum as usize {
            self.sys.dprint("WARNING! World::UncompressEntitiesFromStream: newindex != frame.entitynum.\n");
        }
        true
    }

    // --- writers (§7) ----------------------------------------------------------------------------

    /// svc_spawnbaseline into the sign-on buffer (`ConnectionComplete`, Q4).
    fn write_baseline(&mut self, dp: &Delta, de: &Delta, dc: &Delta) {
        let zero = [0u8; ENTITY_SIZE];
        let mut s = std::mem::take(&mut self.signon);
        s.write_byte(22);
        s.start_bit_mode();
        for i in 0..MAX_ENTITIES {
            let b = self.baselines[i];
            let ty = i32_at(&b, 0);
            if ty == UNUSED {
                continue;
            }
            s.write_bits(i as u32, 11);
            s.write_bits(ty as u32, 2);
            let d = if ty & 1 == 0 { dc } else if self.is_player_index(i as i32) { dp } else { de };
            d.write(&mut s, &zero, &b, true, None, self.dtime);
        }
        s.write_bits(0xFFFF, 16);
        s.write_bits(self.max_instanced as u32, 6);
        for i in 0..self.max_instanced.clamp(0, 64) as usize {
            de.write(&mut s, &zero, &self.instanced[i], true, None, self.dtime);
        }
        s.end_bit_mode();
        self.signon = s;
    }

    /// `WriteServerinfo` + user messages + fullserverinfo + resources (`WriteNewData`, §7.3).
    pub fn write_new_data(&mut self, s: &mut BitBuffer) {
        s.write_byte(8);
        let text = format!("Protocol Version {}, Spawn count {} {}\n", self.protocol, self.server_count, if self.is_hltv { "(HLTV)" } else { "" });
        s.write_string(&text.as_bytes()[..text.len().min(0x800)]);
        if self.is_hltv {
            s.write_byte(50);
            s.write_byte(0);
        }
        s.write_byte(11);
        s.write_long(self.protocol);
        s.write_long(self.server_count);
        let mut crc = self.server_crc.to_le_bytes();
        gs_net::munge::munge(&mut crc, i32::from((-1 - self.player_num) as u8), gs_net::munge::MungeTable::Three);
        s.write_long(i32::from_le_bytes(crc));
        s.write_buf(&self.client_dll_md5);
        s.write_byte(self.max_clients as u8);
        s.write_byte(self.player_num as u8);
        s.write_byte(self.game_type as u8);
        s.write_string(&self.game_dir.clone());
        s.write_string(self.host_name());
        s.write_string(&self.level_name.clone());
        s.write_string(b"mapcycle failure");
        s.write_byte(0);
        s.write_byte(54);
        s.write_string(&self.fallback_dir.clone());
        s.write_byte(u8::from(self.allow_cheats));
        for (name, d) in self.registry.entries.clone() {
            s.write_byte(14);
            s.write_string(name.as_bytes());
            let Some(d) = d else {
                self.errorf(&format!("World::WriteDeltaDescriptions : missing delta for {name}\n"));
                break;
            };
            s.start_bit_mode();
            write_description(s, &d);
            s.end_bit_mode();
        }
        self.write_movevars(s);
        s.write_byte(32);
        s.write_byte(self.cd_track as u8);
        s.write_byte(self.cd_track as u8);
        s.write_byte(5);
        s.write_short(self.view_entity);
        for u in &self.user_msgs {
            s.write_byte(39);
            s.write_byte(u.msg as u8);
            s.write_byte(if u.size == -1 { 255 } else { u.size as u8 });
            s.write_buf(&u.name);
        }
        s.write_byte(9);
        let mut cmd = b"fullserverinfo \"".to_vec();
        cmd.extend_from_slice(&self.server_info);
        cmd.extend_from_slice(b"\"\n");
        cmd.truncate(1023);
        s.write_string(&cmd);
        if !self.download_url.is_empty() {
            s.write_byte(56);
            s.write_string(&self.download_url.clone());
        }
        s.write_byte(43);
        s.start_bit_mode();
        s.write_bits(self.resources.len() as u32, 12);
        for r in &self.resources {
            s.write_bits(r.kind as u32, 4);
            s.write_bit_string(&r.name);
            s.write_bits(r.index as u32, 12);
            s.write_bits(r.download_size as u32, 24);
            s.write_bits(u32::from(r.flags & 3), 3);
            if r.flags & 4 != 0 {
                s.write_bit_data(&r.md5);
            }
            if r.reserved.iter().any(|&b| b != 0) {
                s.write_bit(true);
                s.write_bit_data(&r.reserved);
            } else {
                s.write_bit(false);
            }
        }
        s.write_bit(false);
        s.end_bit_mode();
    }

    /// `WriteSigonData`: the raw sign-on buffer.
    pub fn write_signon_data(&self, s: &mut BitBuffer) {
        s.write_buf(self.signon.bytes());
    }

    /// `WriteClientUpdate(s, slot)`.
    pub fn write_client_update(&self, s: &mut BitBuffer, i: i32) {
        let Some(p) = usize::try_from(i).ok().and_then(|i| self.players.get(i)) else { return };
        if !p.active {
            return;
        }
        s.write_byte(13);
        s.write_byte(i as u8);
        s.write_long(p.userid);
        s.write_string(&p.userinfo);
        s.write_buf(&p.hashedcdkey);
    }

    /// `WriteLightStyles`.
    pub fn write_light_styles(&self, s: &mut BitBuffer) {
        for (i, l) in self.lightstyles.iter().enumerate() {
            s.write_byte(12);
            s.write_byte(i as u8);
            s.write_string(l);
        }
    }

    /// `WriteMovevars`.
    pub fn write_movevars(&self, s: &mut BitBuffer) {
        let mv = &self.movevars;
        s.write_byte(44);
        for k in 0..16 {
            s.write_float(f32_at(mv, k * 4));
        }
        s.write_byte(u8::from(i32_at(mv, 0x40) != 0));
        for k in 0..8 {
            s.write_float(f32_at(mv, 0x64 + k * 4));
        }
        s.write_string(&cut(&mv[0x44..0x64], 33));
    }

    /// `CompressFrame(frame, stream)` → bytes written (§7.7).
    pub fn compress_frame(&mut self, f: &Uncompressed, s: &mut BitBuffer) -> usize {
        let mut hdr = Header { full: true, instanced_available: self.max_instanced > 0, ..Header::default() };
        self.dtime.time = f64::from(f.time);
        let start = s.byte_pos();
        s.start_bit_mode();
        let n = (f.entitynum as usize).min(f.ents.len());
        for i in 0..n {
            let e = &f.ents[i];
            let num = i32_at(e, 4);
            let custom = (i32_at(e, 0) >> 1) & 1 != 0;
            let Some(d) = self.entity_delta(custom, num) else { continue };
            hdr.num = num;
            hdr.custom = custom;
            hdr.newbl = false;
            hdr.newblindex = 0;
            let base = self.baselines.get(num.max(0) as usize).copied().unwrap_or([0; ENTITY_SIZE]);
            let mut best = i;
            let mut cost = d.test(&base, e) - 6;
            let (mut j, mut k) = (i as i32 - 1, 1);
            while cost > 0 && j >= 0 && k < 63 {
                let other = &f.ents[j as usize];
                if i32_at(e, 0x28) == i32_at(other, 0x28) {
                    let c = d.test(other, e);
                    if c < cost {
                        cost = c;
                        best = j as usize;
                    }
                }
                j -= 1;
                k += 1;
            }
            hdr.offset = (i - best) as i32;
            let from = if hdr.offset != 0 { f.ents[best] } else { base };
            d.write(s, &from, e, true, Some(&mut hdr), self.dtime);
        }
        s.write_bits(0, 16);
        s.end_bit_mode();
        s.byte_pos() - start
    }

    /// `WritePacketEntities(stream, F, D)` (§7.8).
    fn write_packet_entities(&mut self, s: &mut BitBuffer, f: &Uncompressed, d: &Uncompressed) {
        let mut hdr = Header { instanced_available: self.max_instanced > 0, ..Header::default() };
        self.dtime.time = f64::from(f.time);
        s.start_bit_mode();
        let (fn_, dn) = ((f.entitynum as usize).min(f.ents.len()), (d.entitynum as usize).min(d.ents.len()));
        let (mut ni, mut oi) = (0usize, 0usize);
        loop {
            if ni >= fn_ && oi >= dn {
                break;
            }
            let newnum = if ni < fn_ { i32_at(&f.ents[ni], 4) } else { 9999 };
            let oldnum = if oi < dn { i32_at(&d.ents[oi], 4) } else { 9999 };
            if newnum == oldnum {
                let custom = (i32_at(&f.ents[ni], 0) >> 1) & 1 != 0;
                hdr.num = newnum;
                hdr.custom = custom;
                hdr.remove = false;
                hdr.newbl = false;
                hdr.newblindex = 0;
                if let Some(dl) = self.entity_delta(custom, newnum) {
                    dl.write(s, &d.ents[oi], &f.ents[ni], false, Some(&mut hdr), self.dtime);
                }
                ni += 1;
                oi += 1;
            } else if newnum < oldnum {
                let custom = (i32_at(&f.ents[ni], 0) >> 1) & 1 != 0;
                hdr.num = newnum;
                hdr.custom = custom;
                hdr.remove = false;
                hdr.newbl = false;
                hdr.newblindex = 0;
                let base = self.baselines.get(newnum.max(0) as usize).copied().unwrap_or([0; ENTITY_SIZE]);
                if let Some(dl) = self.entity_delta(custom, newnum) {
                    dl.write(s, &base, &f.ents[ni], true, Some(&mut hdr), self.dtime);
                }
                ni += 1;
            } else {
                hdr.num = oldnum;
                hdr.remove = true;
                hdr.newbl = false;
                hdr.newblindex = 0;
                crate::delta::write_header(s, &mut hdr);
                oi += 1;
            }
        }
        s.write_bits(0, 16);
        s.end_bit_mode();
    }

    /// `WriteFrame(frame, lastFrameSeqnr, rel/unrel, deltaSeqNr, clientDelta)` (§7.6); DemoPlayer
    /// passes one stream for both.
    pub fn write_frame(&mut self, id: usize, last_seq: u32, s: &mut BitBuffer, delta_seq: u32, client_delta: u32) {
        let Some(seqnr) = self.frame(id).map(|f| f.seqnr) else { return };
        let Some(fu) = self.uncompressed_by_seq(seqnr) else {
            self.sys.dprint("World::WriteFrame: couldn't uncompress frame.\n");
            return;
        };
        if self.is_hltv {
            s.write_byte(15);
        } else {
            let mut cd = [0u8; CLIENTDATA_SIZE];
            if self.client_data(id, &mut cd)
                && let Some(dc) = self.delta("clientdata_t")
            {
                let zero = [0u8; CLIENTDATA_SIZE];
                s.write_byte(15);
                s.start_bit_mode();
                s.write_bit(false);
                dc.write(s, &zero, &cd, true, None, self.dtime);
                s.write_bit(false);
                s.end_bit_mode();
            }
        }
        let mut written = false;
        let mut full = delta_seq == 0;
        if delta_seq != 0 {
            let hit = self.delta_cache.iter().position(|c| c.0 == fu.seqnr && c.1 == delta_seq);
            let slot = match hit {
                Some(i) => {
                    self.cache_hits += 1;
                    Some(i)
                }
                None => {
                    let i = self.delta_cache_index;
                    self.delta_cache_index = (self.delta_cache_index + 1) % self.delta_cache.len();
                    self.delta_cache[i].0 = fu.seqnr;
                    self.delta_cache[i].1 = delta_seq;
                    self.cache_faults += 1;
                    match self.uncompressed_by_seq(delta_seq) {
                        Some(du) => {
                            let mut buf = BitBuffer::new(fu.entitynum as usize * ENTITY_SIZE);
                            self.write_packet_entities(&mut buf, &fu, &du);
                            self.delta_cache[i].2 = buf;
                            Some(i)
                        }
                        None => {
                            self.sys.dprint(&format!("WARNING! World::WriteFrame: delta frame too old ({delta_seq}).\n"));
                            full = true;
                            None
                        }
                    }
                }
            };
            if let Some(i) = slot {
                s.write_byte(41);
                s.write_short(fu.entitynum as i32);
                s.write_byte(client_delta as u8);
                let bytes = self.delta_cache[i].2.bytes().to_vec();
                s.write_buf(&bytes);
                written = true;
            }
        }
        if full {
            let n = self.frame(id).map_or(0, |f| f.entitynum);
            s.write_byte(40);
            s.write_short(n as i32);
            if self.compress_frame(&fu, s) > 0 {
                written = true;
            } else {
                self.sys.dprint("WARNING! World::WriteFrame: could write entities!\n");
            }
        }
        if written && let Some(ev) = self.frame(id).map(|f| f.events.clone()).filter(|e| !e.is_empty()) {
            s.write_byte(3);
            s.write_buf(&ev);
        }
        let mut cur = self.by_seq.find_exact(f64::from(last_seq.wrapping_add(1)) as f32);
        while let Some(fid) = cur {
            let Some(f) = self.frames.get(fid).and_then(Option::as_ref) else { break };
            if f.seqnr > seqnr || s.overflowed {
                break;
            }
            if !f.reliable.is_empty() && s.space_left() > f.reliable.len() as i64 {
                s.write_buf(&f.reliable);
            }
            if !f.user_messages.is_empty() && s.space_left() > f.user_messages.len() as i64 {
                s.write_buf(&f.user_messages);
            }
            if f.seqnr + 8 > seqnr {
                if !f.unreliable.is_empty() && s.space_left() > f.unreliable.len() as i64 {
                    s.write_buf(&f.unreliable);
                }
                if !f.voice.is_empty() && s.space_left() > f.voice.len() as i64 {
                    s.write_buf(&f.voice);
                }
            }
            cur = self.by_seq.get_next();
        }
    }

    /// The level name's file base (`COM_FileBase(GetLevelName())`).
    pub fn level_base(&self) -> Vec<u8> {
        file_base(&self.level_name)
    }

    /// The game directory's file base.
    pub fn game_dir_base(&self) -> Vec<u8> {
        file_base(&self.game_dir)
    }

    /// Every stored frame id in seqnr order.
    pub fn frame_ids(&self) -> Vec<usize> {
        self.by_seq.iter().map(|(id, _)| id).collect()
    }
}

/// `COM_FileBase`: the name without directories and extension.
pub fn file_base(s: &[u8]) -> Vec<u8> {
    let start = s.iter().rposition(|&c| c == b'/' || c == b'\\').map_or(0, |i| i + 1);
    let rest = &s[start..];
    let end = rest.iter().rposition(|&c| c == b'.').unwrap_or(rest.len());
    rest[..end].to_vec()
}

/// C `atoi` over bytes.
pub fn atoi(s: &[u8]) -> i32 {
    gs_common_atoi(&String::from_utf8_lossy(s))
}

fn gs_common_atoi(s: &str) -> i32 {
    let t = s.trim_start();
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let mut v: i64 = 0;
    for b in rest.bytes().take_while(u8::is_ascii_digit) {
        v = (v * 10 + i64::from(b - b'0')).min(i64::from(i32::MAX) + 1);
    }
    let v = if neg { -v } else { v };
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// `InfoString::ValueForKey`.
pub fn value_for_key(s: &[u8], key: &[u8]) -> Vec<u8> {
    let mut parts = s.split(|&c| c == b'\\');
    if s.first() == Some(&b'\\') {
        parts.next();
    }
    loop {
        let Some(k) = parts.next() else { return Vec::new() };
        let v = parts.next().unwrap_or(&[]);
        if k == key {
            return v.to_vec();
        }
    }
}

/// `InfoString::RemovePrefixedKeys(prefix)` with its tail-shift bug (Q12).
pub fn remove_prefixed_keys(s: &mut Vec<u8>, prefix: u8) {
    s.push(0);
    let at = |s: &Vec<u8>, i: usize| s.get(i).copied().unwrap_or(0);
    let mut p = 0usize;
    if at(s, p) != b'\\' {
        s.pop();
        return;
    }
    loop {
        let key0 = at(s, p + 1);
        p += 1;
        let keystart = p;
        if key0 != b'\\' {
            let mut c = key0;
            while c != 0 {
                p += 1;
                c = at(s, p);
                if c == b'\\' {
                    break;
                }
            }
            if c == 0 {
                break;
            }
        }
        p += 1;
        let mut c = at(s, p);
        while c != b'\\' && c != 0 {
            p += 1;
            c = at(s, p);
        }
        if key0 == prefix {
            if at(s, p) == 0 {
                break;
            }
            p += 1;
            let mut c = at(s, p);
            let mut q = keystart;
            while c != 0 {
                if let Some(d) = s.get_mut(q) {
                    *d = c;
                }
                q += 1;
                p += 1;
                c = at(s, p);
            }
            p = keystart;
        }
        if at(s, p) == 0 {
            break;
        }
    }
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    s.truncate(end);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_keys_and_file_base() {
        assert_eq!(value_for_key(b"\\name\\Bob\\model\\gign", b"model"), b"gign");
        assert_eq!(value_for_key(b"\\name\\Bob", b"x"), b"");
        assert_eq!(file_base(b"maps/de_dust2.bsp"), b"de_dust2");
    }
}
