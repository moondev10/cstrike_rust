//! `CL_ParseServerMessage`: the stateful svc decoder. Parsing depends on state set earlier in the
//! same stream (delta descriptions, user message sizes, maxclients, frames), so the decoder owns
//! that state and applies entity/client-data/baseline updates as it parses.

use std::collections::HashMap;

use gs_common::bytestr::truncate_encoded;
use gs_net::munge::{MungeTable, unmunge};

use crate::ProtocolError;
use crate::bitbuf::MsgReader;
use crate::delta::{Delta, names, read_description};
use crate::messages::*;
use crate::structs::{ClientData, EntityState, EventArgs, WeaponData};
use crate::tempent;

pub const UPDATE_BACKUP: usize = 64;
pub const UPDATE_MASK: u32 = 63;
pub const MAX_PACKET_ENTITIES: usize = 1024;
pub const MAX_WEAPONS: usize = 64;
pub const MAX_USER_MESSAGE: usize = 192;
pub const MAX_LIGHTSTYLES: u8 = 64;
pub const MAX_CLIENTS: u8 = 32;
const FIRST_USER_MESSAGE: u8 = 64;
const LAST_SVC: u8 = 59;

pub mod entity_type {
    pub const NORMAL: i32 = 1;
    pub const BEAM: i32 = 2;
}

#[derive(Debug, Clone)]
pub struct UserMessageDef {
    pub name: String,
    /// Fixed payload size, or `None` for variable (length byte on the wire).
    pub size: Option<u8>,
}

/// net_graph's per-frame byte counters (`frame_t` +17156): zeroed by `svc_clientdata`, then
/// filled while the rest of the packet parses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetBytes {
    pub client: u16,
    pub playerinfo: u16,
    pub packetentities: u16,
    pub tentity: u16,
    pub sound: u16,
    pub event: u16,
    pub usr: u16,
    pub voice: u16,
    /// The whole packet (in game only).
    pub msgbytes: u16,
}

#[derive(Debug, Clone, Default)]
pub struct Frame {
    /// Entities arrived for this frame.
    pub valid: bool,
    pub choked: bool,
    /// `cl.mtime[0]` when the frame was parsed.
    pub time: f64,
    pub entities: Vec<EntityState>,
    /// Entity just appeared (not in the delta source frame).
    pub new_entity: Vec<bool>,
    pub clientdata: ClientData,
    pub weapondata: Vec<WeaponData>,
    /// Weapon slots this packet carried (bit = slot); the others came from the delta base and
    /// are replaced by the client's predicted values (`HUD_TxferPredictionData`).
    pub weapondata_sent: u64,
    pub bytes: NetBytes,
}

pub struct ProtocolState {
    deltas: HashMap<String, Delta>,
    user_messages: Vec<Option<UserMessageDef>>,
    pub maxclients: u8,
    pub playernum: u8,
    pub max_edicts: usize,
    pub baselines: Vec<EntityState>,
    pub instanced_baselines: Vec<EntityState>,
    pub frames: Vec<Frame>,
    /// `cl.mtime`: server time from the last two `svc_time`.
    pub mtime: [f64; 2],
    /// Sequence of the last valid entity frame (0 = none); echoed in `clc_delta`.
    pub valid_sequence: u32,
    current: usize,
    /// Bits of player-entity deltas in the last packet entities (net_graph's `playerinfo`).
    player_bits: usize,
    /// `cls.spectator`: set by `svc_hltv` HLTV_ACTIVE (an HLTV proxy connection or demo).
    pub spectator: bool,
    /// `cl_messages` counters.
    pub msg_stats: MsgStats,
    /// `cls.passive` (the client sets it before parsing).
    pub passive: bool,
    /// Record [`ParseTrace`]s while parsing (`cl_shownet`, `cl_showmessages`,
    /// `cl_needinstanced`); the client sets it before parsing.
    pub trace: bool,
    traces: Vec<(usize, ParseTrace)>,
    /// Index the next trace belongs before: the number of messages already produced.
    trace_index: usize,
    /// `cl_max_baseline_num`: the highest entity number of the last `svc_spawnbaseline`.
    max_baseline_num: usize,
}

/// The svc names (`cl_parsefuncs`' name column), indexed by opcode.
pub const SVC_NAMES: [&str; 60] = [
    "svc_bad", "svc_nop", "svc_disconnect", "svc_event", "svc_version", "svc_setview", "svc_sound", "svc_time", "svc_print", "svc_stufftext",
    "svc_setangle", "svc_serverinfo", "svc_lightstyle", "svc_updateuserinfo", "svc_deltadescription", "svc_clientdata", "svc_stopsound",
    "svc_pings", "svc_particle", "svc_damage", "svc_spawnstatic", "svc_event_reliable", "svc_spawnbaseline", "svc_temp_entity",
    "svc_setpause", "svc_signonnum", "svc_centerprint", "svc_killedmonster", "svc_foundsecret", "svc_spawnstaticsound", "svc_intermission",
    "svc_finale", "svc_cdtrack", "svc_restore", "svc_cutscene", "svc_weaponanim", "svc_decalname", "svc_roomtype", "svc_addangle",
    "svc_newusermsg", "svc_packetentities", "svc_deltapacketentities", "svc_choke", "svc_resourcelist", "svc_newmovevars",
    "svc_resourcerequest", "svc_customization", "svc_crosshairangle", "svc_soundfade", "svc_filetxferfailed", "svc_hltv", "svc_director",
    "svc_voiceinit", "svc_voicedata", "svc_sendextrainfo", "svc_timescale", "svc_resourcelocation", "svc_sendcvarvalue",
    "svc_sendcvarvalue2", "svc_exec",
];

/// What `CL_ParseServerMessage` traces while parsing (net-messages.md §2.4), each printed
/// before the message it was recorded ahead of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseTrace {
    /// An svc opcode at `offset` in the payload.
    Svc { offset: usize, op: u8 },
    /// A user message and its size.
    User { name: String, size: usize },
    /// An entity created above every baseline without an instanced baseline.
    NonInstanced { number: usize, modelindex: i32 },
    /// The clean end of the payload (`msg_readcount − 1` there).
    End { offset: usize },
}

/// `cl_messages`'s `msg_count[64]` / `msg_bytes[64]`: svc ids 0..=59 by index, every user
/// message in slot 63.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsgStats {
    pub count: [i32; 64],
    pub bytes: [i32; 64],
}

impl Default for MsgStats {
    fn default() -> Self {
        Self { count: [0; 64], bytes: [0; 64] }
    }
}

impl Default for ProtocolState {
    fn default() -> Self {
        Self {
            deltas: HashMap::new(),
            user_messages: vec![None; 256],
            maxclients: 1,
            playernum: 0,
            max_edicts: 0,
            baselines: Vec::new(),
            instanced_baselines: Vec::new(),
            frames: (0..UPDATE_BACKUP).map(|_| Frame::default()).collect(),
            mtime: [0.0; 2],
            valid_sequence: 0,
            current: 0,
            player_bits: 0,
            spectator: false,
            msg_stats: MsgStats::default(),
            passive: false,
            trace: false,
            traces: Vec::new(),
            trace_index: 0,
            max_baseline_num: 0,
        }
    }
}

fn bad(msg: impl Into<String>) -> ProtocolError {
    ProtocolError::Malformed(msg.into())
}

/// One packet's messages in order. A message that fails ends the packet: `error` says why and
/// `messages` are the ones before it, which `CL_ParseServerMessage` has run by the time it
/// raises its `Host_Error`.
#[derive(Debug, Default)]
pub struct ParsedPacket {
    pub messages: Vec<ServerMessage>,
    pub error: Option<ProtocolError>,
}

impl ProtocolState {
    pub fn delta(&self, name: &str) -> Option<&Delta> {
        self.deltas.get(name)
    }

    pub fn user_message(&self, id: u8) -> Option<&UserMessageDef> {
        self.user_messages[usize::from(id)].as_ref()
    }

    pub fn user_message_id(&self, name: &str) -> Option<u8> {
        self.user_messages
            .iter()
            .position(|m| m.as_ref().is_some_and(|m| m.name.eq_ignore_ascii_case(name)))
            .map(|i| i as u8)
    }

    /// The frame being filled by the packet currently parsed (or last parsed).
    pub fn current_frame(&self) -> &Frame {
        &self.frames[self.current]
    }

    /// The current frame, for the client's `HUD_TxferPredictionData` pass.
    pub fn current_frame_mut(&mut self) -> &mut Frame {
        &mut self.frames[self.current]
    }

    pub fn frame(&self, sequence: u32) -> &Frame {
        &self.frames[(sequence & UPDATE_MASK) as usize]
    }

    fn need_delta(&self, name: &str) -> Result<&Delta, ProtocolError> {
        self.deltas.get(name).ok_or_else(|| bad(format!("no delta description for {name}")))
    }

    fn entity_delta(&self, num: usize, custom: bool) -> Result<&Delta, ProtocolError> {
        let name = if custom {
            names::CUSTOM_ENTITY_STATE
        } else if (1..=usize::from(self.maxclients)).contains(&num) {
            names::ENTITY_STATE_PLAYER
        } else {
            names::ENTITY_STATE
        };
        self.need_delta(name)
    }

    fn baseline(&self, num: usize) -> EntityState {
        self.baselines.get(num).cloned().unwrap_or_default()
    }

    /// Parses one netchan payload. `incoming_sequence` selects the frame slot.
    pub fn parse(&mut self, data: &[u8], incoming_sequence: u32) -> ParsedPacket {
        let mut messages = Vec::new();
        let error = self.parse_into(data, incoming_sequence, &mut messages).err();
        ParsedPacket { messages, error }
    }

    fn parse_into(&mut self, data: &[u8], incoming_sequence: u32, out: &mut Vec<ServerMessage>) -> Result<(), ProtocolError> {
        self.current = (incoming_sequence & UPDATE_MASK) as usize;
        {
            let frame = &mut self.frames[self.current];
            frame.valid = false;
            frame.choked = false;
            frame.time = self.mtime[0];
        }
        let mut r = MsgReader::new(data);
        self.traces.clear();
        // CL_ParseServerMessage's frame_bytes: committed to msg_stats only for a whole packet.
        let mut frame_bytes = [0i32; 64];
        loop {
            // msg_badread is checked before each message, so one that read past the end has
            // run (on the reader's zeros) before the packet is given up.
            if r.bad() {
                return Err(bad("CL_ParseServerMessage: Bad server message"));
            }
            if r.at_end() {
                break;
            }
            let offset = r.pos();
            self.trace_index = out.len();
            let op = r.read_u8();
            // cl_shownet / cl_showmessages: every svc before its handler (svc_bad included).
            if self.trace && op <= LAST_SVC {
                self.traces.push((out.len(), ParseTrace::Svc { offset, op }));
            }
            let slot = if op <= LAST_SVC { usize::from(op) } else { 63 };
            self.msg_stats.count[slot] = self.msg_stats.count[slot].wrapping_add(1);
            let start = r.pos();
            let msg = if op <= LAST_SVC {
                self.parse_svc(op, &mut r, incoming_sequence)?
            } else {
                Some(self.parse_user_message(op, &mut r)?)
            };
            frame_bytes[slot] = frame_bytes[slot].wrapping_add((r.pos() - start + 1) as i32);
            self.count_bytes(op, (r.pos() - start).min(usize::from(u16::MAX)) as u16);
            if let Some(msg) = msg {
                out.push(msg);
            }
        }
        for (total, n) in self.msg_stats.bytes.iter_mut().zip(frame_bytes) {
            *total = total.wrapping_add(n);
        }
        if self.trace {
            self.traces.push((out.len(), ParseTrace::End { offset: data.len().saturating_sub(1) }));
        }
        Ok(())
    }

    /// The traces of the last [`Self::parse`] (also after an error), each with the index of the
    /// message it prints before.
    pub fn take_traces(&mut self) -> Vec<(usize, ParseTrace)> {
        std::mem::take(&mut self.traces)
    }

    /// `cmd_delta_stats`: the received descriptions in `g_deltaregistry` order with their
    /// per-field receive counts (sends are never counted on a client).
    pub fn delta_stats_text(&self) -> String {
        const REGISTRY: [&str; 7] =
            ["event_t", "weapon_data_t", "usercmd_t", "custom_entity_state_t", "entity_state_player_t", "entity_state_t", "clientdata_t"];
        let space_sign = |v: i32| if v >= 0 { format!("{:>5}", format!(" {v}")) } else { format!("{v:>5}") };
        let mut out = String::from("Delta Stats\n");
        for name in REGISTRY {
            let Some(d) = self.deltas.get(name) else { continue };
            out.push_str(&format!("Stats for '{name}'\n"));
            for (k, f) in d.fields.iter().enumerate() {
                out.push_str(&format!("  {:02} {:>10}:  s {} r {}\n", k + 1, f.name, space_sign(0), space_sign(f.recv.get())));
            }
            out.push('\n');
        }
        out
    }

    /// `cmd_delta_clear`'s counter reset.
    pub fn clear_delta_stats(&self) {
        self.deltas.values().flat_map(|d| &d.fields).for_each(|f| f.recv.set(0));
    }

    /// net_graph's accounting of one parsed message of `n` bytes (after its opcode):
    /// `svc_clientdata` restarts the counters; player deltas inside packet entities count
    /// as `playerinfo` (their bits / 8).
    fn count_bytes(&mut self, op: u8, n: u16) {
        let player = (self.player_bits / 8).min(usize::from(u16::MAX)) as u16;
        let b = &mut self.frames[self.current].bytes;
        let add = |v: &mut u16, n: u16| *v = v.wrapping_add(n);
        match op {
            15 => *b = NetBytes { client: n, ..NetBytes::default() },
            40 | 41 => {
                add(&mut b.playerinfo, player);
                add(&mut b.packetentities, n.wrapping_sub(player));
            }
            23 => add(&mut b.tentity, n),
            6 => add(&mut b.sound, n),
            3 | 21 => add(&mut b.event, n),
            53 => add(&mut b.voice, n),
            o if o > LAST_SVC => add(&mut b.usr, n),
            _ => {}
        }
    }

    fn parse_user_message(&mut self, id: u8, r: &mut MsgReader) -> Result<ServerMessage, ProtocolError> {
        let def = self
            .user_message(id)
            .cloned()
            .ok_or_else(|| bad(format!("UserMsg: Not Present on Client {id}")))?;
        let len = match def.size {
            Some(n) => usize::from(n),
            None => usize::from(r.read_u8()),
        };
        if self.trace {
            self.traces.push((self.trace_index, ParseTrace::User { name: def.name.clone(), size: len }));
        }
        if len > MAX_USER_MESSAGE {
            return Err(bad(format!("UserMsg {}: size {len} > 192", def.name)));
        }
        Ok(ServerMessage::UserMessage { id, name: def.name, data: r.read_bytes(len).to_vec() })
    }

    fn parse_svc(&mut self, op: u8, r: &mut MsgReader, seq: u32) -> Result<Option<ServerMessage>, ProtocolError> {
        use ServerMessage as M;
        let msg = match op {
            0 => return Err(bad("Illegible server message - svc_bad")),
            1 | 19 | 27 | 28 => return Ok(None),
            2 => M::Disconnect(r.read_string()),
            3 => M::Events(self.parse_events(r)?),
            4 => {
                let v = r.read_i32();
                if v != 48 {
                    return Err(bad(format!("server is protocol {v}, not 48")));
                }
                M::Version(v)
            }
            5 => M::SetView(r.read_i16()),
            6 => M::Sound(parse_sound(r)),
            7 => {
                let t = r.read_f32();
                self.mtime = [f64::from(t), self.mtime[0]];
                self.frames[self.current].time = f64::from(t);
                M::Time(t)
            }
            8 => M::Print(r.read_string()),
            9 => M::StuffText(r.read_string()),
            10 => M::SetAngle([r.read_hires_angle(), r.read_hires_angle(), r.read_hires_angle()]),
            11 => M::ServerInfo(self.parse_serverinfo(r)?),
            12 => {
                let index = r.read_u8();
                if index >= MAX_LIGHTSTYLES {
                    return Err(bad(format!("svc_lightstyle > MAX_LIGHTSTYLES ({index})")));
                }
                M::LightStyle { index, pattern: truncate_encoded(&r.read_string(), 63).into_owned() }
            }
            13 => {
                let slot = r.read_u8();
                if slot >= MAX_CLIENTS {
                    return Err(bad(format!("svc_updateuserinfo slot {slot}")));
                }
                let userid = r.read_i32();
                let info = r.read_string();
                let mut hash = [0u8; 16];
                hash.copy_from_slice(r.read_bytes(16).get(..16).unwrap_or(&[0; 16]));
                M::UpdateUserInfo { slot, userid, info, hash }
            }
            14 => {
                let name = r.read_string();
                if name.is_empty() {
                    return Err(bad("Illegible description name"));
                }
                let delta = read_description(r, name.clone());
                self.deltas.insert(name.clone(), delta);
                M::DeltaDescription(name)
            }
            15 => {
                // CL_ParseClientdata: an HLTV client gets the bare opcode and only
                // cl.stats[STAT_HEALTH] = 1 (kept here in the frame's health).
                if self.spectator {
                    self.frames[self.current].clientdata.bytes_mut()[64..68].copy_from_slice(&1.0f32.to_le_bytes());
                } else {
                    self.parse_clientdata(r)?;
                }
                M::ClientData
            }
            16 => {
                let v = r.read_i16() as u16;
                M::StopSound { entity: v >> 3, channel: (v & 7) as u8 }
            }
            17 => M::Pings(parse_pings(r)),
            18 => {
                let origin = r.read_coord3();
                let dir = [0; 3].map(|_| f32::from(r.read_i8()) * 0.0625);
                let count = match r.read_u8() {
                    255 => 1024,
                    c => u16::from(c),
                };
                M::Particle { origin, dir, count, color: r.read_u8() }
            }
            20 => M::SpawnStatic(parse_static_entity(r)),
            21 => self.parse_reliable_event(r)?,
            22 => {
                self.parse_baselines(r)?;
                M::SpawnBaseline
            }
            23 => M::TempEntity(tempent::parse(r)?),
            24 => M::SetPause(r.read_u8() != 0),
            25 => M::SignonNum(r.read_u8()),
            26 => M::CenterPrint(r.read_string()),
            29 => M::SpawnStaticSound(parse_static_sound(r)),
            30 => M::Intermission,
            31 => M::Finale(r.read_string()),
            32 => M::CdTrack { track: r.read_u8(), loop_track: r.read_u8() },
            33 => {
                let save_name = r.read_string();
                let n = r.read_u8();
                M::Restore { save_name, maps: (0..n).map(|_| r.read_string()).collect() }
            }
            34 => M::Cutscene(r.read_string()),
            35 => M::WeaponAnim { sequence: r.read_u8(), body: r.read_u8() },
            36 => M::DecalName { index: r.read_u8(), name: r.read_string() },
            37 => M::RoomType(r.read_i16()),
            38 => M::AddAngle(r.read_hires_angle()),
            39 => {
                let id = r.read_u8();
                let raw_size = r.read_u8();
                let raw = r.read_bytes(16);
                let mut name_bytes = [0u8; 16];
                name_bytes[..raw.len()].copy_from_slice(raw);
                name_bytes[15] = 0;
                let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(16);
                let name = gs_common::bytestr::decode(&name_bytes[..end]);
                for slot in self.user_messages.iter_mut() {
                    if slot.as_ref().is_some_and(|m| m.name.eq_ignore_ascii_case(&name)) {
                        *slot = None;
                    }
                }
                let size = (raw_size != 255).then_some(raw_size);
                if id >= FIRST_USER_MESSAGE {
                    self.user_messages[usize::from(id)] = Some(UserMessageDef { name: name.clone(), size });
                }
                M::NewUserMsg { id, size: if raw_size == 255 { -1 } else { i16::from(raw_size) }, name }
            }
            40 | 41 => self.parse_packet_entities(r, op == 41, seq)?,
            42 => {
                self.frames[self.current].choked = true;
                M::Choke
            }
            43 => M::ResourceList(parse_resource_list(r)),
            44 => M::NewMoveVars(parse_movevars(r)),
            45 => M::ResourceRequest { spawncount: r.read_i32(), start_index: r.read_i32() },
            46 => {
                let player = r.read_u8();
                if player >= MAX_CLIENTS {
                    return Err(bad(format!("svc_customization player {player}")));
                }
                M::Customization(Customization { player, resource: parse_byte_resource(r) })
            }
            // Parse_svc_crosshairangle: `f32((f64)MSG_ReadChar() * 0.2)` per axis.
            47 => M::CrosshairAngle { pitch: (f64::from(r.read_i8()) * 0.2) as f32, yaw: (f64::from(r.read_i8()) * 0.2) as f32 },
            48 => M::SoundFade([r.read_u8(), r.read_u8(), r.read_u8(), r.read_u8()]),
            49 => {
                let name = r.read_string();
                if name.is_empty() {
                    return Err(bad("svc_filetxferfailed with empty name"));
                }
                M::FileTransferFailed(name)
            }
            50 => M::Hltv(match r.read_u8() {
                0 => {
                    self.spectator = true;
                    Hltv::Active
                }
                1 => Hltv::Status {
                    clients: r.read_i32(),
                    proxies: r.read_i16(),
                    slots: r.read_u16(),
                    total_viewers: r.read_i32(),
                    total_slots: r.read_i32(),
                    total_relays: r.read_u16(),
                },
                2 => Hltv::Listen(r.read_string()),
                other => Hltv::Unknown(other),
            }),
            51 => {
                let len = usize::from(r.read_u8());
                M::Director(r.read_bytes(len).to_vec())
            }
            52 => M::VoiceInit { codec: r.read_string(), quality: r.read_u8() },
            53 => {
                let sender = r.read_u8();
                // Sign-extended i16 compared unsigned: negative or ≥ 0x2000 reads 8192 bytes.
                let len = u32::from(r.read_i16() as u16).min(0x2000) as usize;
                M::VoiceData { sender, data: r.read_bytes(len).to_vec() }
            }
            54 => M::SendExtraInfo { fallback_dir: r.read_string(), cheats: r.read_u8() != 0 },
            55 => M::TimeScale(r.read_f32()),
            56 => M::ResourceLocation(r.read_string()),
            57 => M::SendCvarValue(r.read_string()),
            58 => M::SendCvarValue2 { request_id: r.read_i32(), name: r.read_string() },
            59 => {
                let mode = r.read_u8();
                let class = (mode == 1).then(|| r.read_u8());
                M::Exec { mode, class }
            }
            _ => return Err(bad(format!("unhandled svc {op}"))),
        };
        Ok(Some(msg))
    }

    fn parse_serverinfo(&mut self, r: &mut MsgReader) -> Result<ServerInfo, ProtocolError> {
        let protocol = r.read_i32();
        if protocol != 48 {
            return Err(bad(format!("Server returned version {protocol}, not 48")));
        }
        let spawncount = r.read_i32();
        let mut crc = r.read_i32().to_le_bytes();
        let mut md5 = [0u8; 16];
        md5.copy_from_slice(r.read_bytes(16).get(..16).unwrap_or(&[0; 16]));
        let maxclients = r.read_u8();
        let playernum = r.read_u8();
        unmunge(&mut crc, i32::from(!playernum), MungeTable::Three);
        if !(1..=MAX_CLIENTS).contains(&maxclients) {
            return Err(bad(format!("Bad maxclients ({maxclients}) from server")));
        }
        let info = ServerInfo {
            protocol,
            spawncount,
            map_crc: i32::from_le_bytes(crc),
            client_dll_md5: md5,
            maxclients,
            playernum,
            deathmatch: r.read_u8(),
            gamedir: r.read_string(),
            hostname: r.read_string(),
            map_model: truncate_encoded(&r.read_string(), 39).into_owned(),
            mapcycle: r.read_string(),
            extra: r.read_u8(),
        };
        self.maxclients = maxclients;
        self.playernum = playernum;
        self.max_edicts = 1200 + 15 * (usize::from(maxclients) - 1);
        self.baselines = vec![EntityState::default(); self.max_edicts];
        self.instanced_baselines.clear();
        self.frames = (0..UPDATE_BACKUP).map(|_| Frame::default()).collect();
        self.valid_sequence = 0;
        Ok(info)
    }

    fn parse_clientdata(&mut self, r: &mut MsgReader) -> Result<(), ProtocolError> {
        let cd_delta = self.need_delta(names::CLIENT_DATA)?.clone();
        let wd_delta = self.need_delta(names::WEAPON_DATA)?.clone();
        r.start_bits();
        let from = if r.read_bit() { Some((r.read_bits(8) & UPDATE_MASK) as usize) } else { None };
        let (from_cd, from_wd) = match from {
            Some(i) => {
                let f = &self.frames[i];
                (f.clientdata.clone(), f.weapondata.clone())
            }
            None => (ClientData::default(), Vec::new()),
        };
        let mut weapons = from_wd;
        weapons.resize(MAX_WEAPONS, WeaponData::default());
        let mut cd = ClientData::default();
        cd_delta.decode(r, from_cd.bytes(), cd.bytes_mut(), self.mtime[0]);
        let mut sent = 0u64;
        while !r.bad() && r.read_bit() {
            let idx = r.read_bits(6) as usize;
            let mut wd = WeaponData::default();
            let base = weapons[idx].clone();
            wd_delta.decode(r, base.bytes(), wd.bytes_mut(), self.mtime[0]);
            weapons[idx] = wd;
            sent |= 1 << idx;
        }
        r.end_bits();
        let frame = &mut self.frames[self.current];
        frame.clientdata = cd;
        frame.weapondata = weapons;
        frame.weapondata_sent = sent;
        Ok(())
    }

    fn parse_baselines(&mut self, r: &mut MsgReader) -> Result<(), ProtocolError> {
        let zero = EntityState::default();
        r.start_bits();
        self.max_baseline_num = 0;
        while !r.bad() && r.peek_bits(16) != 0xFFFF {
            let num = r.read_bits(11) as usize;
            self.max_baseline_num = self.max_baseline_num.max(num);
            let ty = r.read_bits(2) as i32;
            if num >= self.max_edicts {
                return Err(bad(format!("CL_EntityNum: {num} is an invalid number")));
            }
            let delta = self.entity_delta(num, ty & 2 != 0)?;
            let mut state = EntityState::default();
            delta.decode(r, zero.bytes(), state.bytes_mut(), self.mtime[0]);
            state.set_entity_type(ty);
            state.set_number(num as i32);
            self.baselines[num] = state;
        }
        r.read_bits(16);
        let count = r.read_bits(6) as usize;
        let delta = self.need_delta(names::ENTITY_STATE)?.clone();
        self.instanced_baselines = (0..count)
            .map(|_| {
                let mut state = EntityState::default();
                delta.decode(r, zero.bytes(), state.bytes_mut(), self.mtime[0]);
                state
            })
            .collect();
        r.end_bits();
        Ok(())
    }

    fn parse_packet_entities(&mut self, r: &mut MsgReader, delta_update: bool, seq: u32) -> Result<ServerMessage, ProtocolError> {
        self.player_bits = 0;
        let count = r.read_i16().max(0) as u16;
        let from_seq = delta_update.then(|| r.read_u8());
        let old: Vec<EntityState> = match from_seq {
            Some(from) => {
                if seq.wrapping_sub(u32::from(from)) as u8 >= 63 || self.max_edicts == 0 {
                    // CL_FlushEntityPacket: the source frame is gone; discard and ask for a full update.
                    self.skip_packet_entities(r)?;
                    self.valid_sequence = 0;
                    return Ok(ServerMessage::PacketEntities { delta_from: from_seq, count: 0 });
                }
                self.frames[(u32::from(from) & UPDATE_MASK) as usize].entities.clone()
            }
            None => Vec::new(),
        };
        self.valid_sequence = seq;
        let full = !delta_update;
        let mut new: Vec<EntityState> = Vec::with_capacity(usize::from(count).max(1));
        let mut is_new: Vec<bool> = Vec::with_capacity(usize::from(count).max(1));
        let mut old_idx = 0;
        let mut last_num = 0usize;
        r.start_bits();
        loop {
            if r.bad() {
                return Err(bad("msg_badread in packetentities"));
            }
            if r.peek_bits(16) == 0 {
                r.read_bits(16);
                break;
            }
            let bits_before = r.bits_read();
            let header = read_entity_header(r, full, &mut last_num, !self.instanced_baselines.is_empty());
            let num = header.num;
            if num >= self.max_edicts {
                return Err(bad(format!("CL_EntityNum: {num} is an invalid number")));
            }
            while old_idx < old.len() && (old[old_idx].number() as usize) < num {
                if full {
                    return Err(bad("WARNING: oldcopy invalid on non-delta compressed update"));
                }
                new.push(old[old_idx].clone());
                is_new.push(false);
                old_idx += 1;
            }
            let old_num = old.get(old_idx).map_or(usize::MAX, |e| e.number() as usize);
            if new.len() >= MAX_PACKET_ENTITIES {
                return Err(bad("CL_ParsePacketEntities: newindex == MAX_PACKET_ENTITIES"));
            }
            if num < old_num {
                if header.remove {
                    if full {
                        return Err(bad("WARNING: remove invalid on non-delta compressed update"));
                    }
                    continue;
                }
                let from = if let Some(i) = header.instanced {
                    self.instanced_baselines.get(i).cloned().unwrap_or_default()
                } else if header.offset > 0 {
                    new.len()
                        .checked_sub(header.offset)
                        .and_then(|i| new.get(i).cloned())
                        .ok_or_else(|| bad("delta offset before start of packet"))?
                } else {
                    self.baseline(num)
                };
                let delta = self.entity_delta(num, header.custom)?;
                let mut state = EntityState::default();
                delta.decode(r, from.bytes(), state.bytes_mut(), self.mtime[0]);
                state.set_entity_type(if header.custom { entity_type::BEAM } else { entity_type::NORMAL });
                state.set_number(num as i32);
                if self.trace && num > self.max_baseline_num && header.instanced.is_none() {
                    self.traces.push((self.trace_index, ParseTrace::NonInstanced { number: num, modelindex: state.modelindex() }));
                }
                if (1..=usize::from(self.maxclients)).contains(&num) {
                    self.player_bits += r.bits_read() - bits_before;
                }
                new.push(state);
                // A passive client's periodic full update does not restart interpolation.
                is_new.push(!(full && self.passive));
            } else {
                if full {
                    log::warn!("WARNING: delta on full update");
                    self.valid_sequence = 0;
                }
                if header.remove {
                    old_idx += 1;
                    continue;
                }
                let delta = self.entity_delta(num, header.custom)?;
                let mut state = EntityState::default();
                delta.decode(r, old[old_idx].bytes(), state.bytes_mut(), self.mtime[0]);
                state.set_entity_type(if header.custom { entity_type::BEAM } else { entity_type::NORMAL });
                state.set_number(num as i32);
                if (1..=usize::from(self.maxclients)).contains(&num) {
                    self.player_bits += r.bits_read() - bits_before;
                }
                new.push(state);
                is_new.push(false);
                old_idx += 1;
            }
        }
        for e in &old[old_idx.min(old.len())..] {
            new.push(e.clone());
            is_new.push(false);
        }
        r.end_bits();
        let frame = &mut self.frames[self.current];
        frame.entities = new;
        frame.new_entity = is_new;
        frame.valid = true;
        Ok(ServerMessage::PacketEntities { delta_from: from_seq, count })
    }

    /// Consumes a delta entity packet whose source frame is unavailable.
    fn skip_packet_entities(&mut self, r: &mut MsgReader) -> Result<(), ProtocolError> {
        let mut last_num = 0;
        let mut scratch = EntityState::default();
        let zero = EntityState::default();
        r.start_bits();
        while !r.bad() {
            if r.peek_bits(16) == 0 {
                r.read_bits(16);
                break;
            }
            let header = read_entity_header(r, false, &mut last_num, !self.instanced_baselines.is_empty());
            if !header.remove {
                let delta = self.entity_delta(header.num, header.custom)?;
                delta.decode(r, zero.bytes(), scratch.bytes_mut(), self.mtime[0]);
            }
        }
        r.end_bits();
        self.frames[self.current].valid = false;
        Ok(())
    }

    /// `svc_event`: wire entries only (see [`WireEvent`]).
    fn parse_events(&mut self, r: &mut MsgReader) -> Result<Vec<WireEvent>, ProtocolError> {
        let delta = self.need_delta(names::EVENT)?.clone();
        let zero = EventArgs::default();
        r.start_bits();
        let n = r.read_bits(5);
        let mut events = Vec::with_capacity(n as usize);
        for _ in 0..n {
            if r.bad() {
                break;
            }
            let index = r.read_bits(10) as u16;
            let packet_index = r.read_bit().then(|| r.read_bits(11) as u16);
            let args = r.read_bit().then(|| {
                let mut args = EventArgs::default();
                delta.decode(r, zero.bytes(), args.bytes_mut(), self.mtime[0]);
                args
            });
            let delay = r.read_bit().then(|| r.read_bits(16) as f32 / 100.0);
            events.push(WireEvent { index, packet_index, args, delay });
        }
        r.end_bits();
        Ok(events)
    }

    /// `svc_event_reliable`.
    fn parse_reliable_event(&mut self, r: &mut MsgReader) -> Result<ServerMessage, ProtocolError> {
        let delta = self.need_delta(names::EVENT)?.clone();
        let zero = EventArgs::default();
        r.start_bits();
        let index = r.read_bits(10) as u16;
        let mut args = EventArgs::default();
        delta.decode(r, zero.bytes(), args.bytes_mut(), self.mtime[0]);
        let delay = if r.read_bit() { r.read_bits(16) as f32 / 100.0 } else { 0.0 };
        r.end_bits();
        Ok(ServerMessage::ReliableEvent { index, args, delay })
    }
}

struct EntityHeader {
    num: usize,
    remove: bool,
    custom: bool,
    instanced: Option<usize>,
    offset: usize,
}

/// `CL_ParseDeltaHeader`.
fn read_entity_header(r: &mut MsgReader, full: bool, last_num: &mut usize, has_instanced: bool) -> EntityHeader {
    let remove = !full && r.read_bit();
    let num = if full && r.read_bit() {
        *last_num + 1
    } else if r.read_bit() {
        r.read_bits(11) as usize
    } else {
        *last_num + r.read_bits(6) as usize
    };
    *last_num = num;
    let mut header = EntityHeader { num, remove, custom: false, instanced: None, offset: 0 };
    if remove {
        return header;
    }
    header.custom = r.read_bit();
    if has_instanced && r.read_bit() {
        header.instanced = Some(r.read_bits(6) as usize);
    }
    if full && header.instanced.is_none() && r.read_bit() {
        header.offset = r.read_bits(6) as usize;
    }
    header
}

fn parse_sound(r: &mut MsgReader) -> Sound {
    use sound_flags as F;
    r.start_bits();
    let flags = r.read_bits(9);
    let volume = if flags & F::VOLUME != 0 { r.read_bits(8) as f32 / 255.0 } else { 1.0 };
    let attenuation = if flags & F::ATTENUATION != 0 { r.read_bits(8) as f32 / 64.0 } else { 1.0 };
    let channel = r.read_bits(3) as u8;
    let entity = r.read_bits(11) as u16;
    let index = if flags & F::LARGE_INDEX != 0 { r.read_bits(16) } else { r.read_bits(8) } as u16;
    let origin = r.read_bit_vec3_coord();
    let pitch = if flags & F::PITCH != 0 { r.read_bits(8) as u8 } else { 100 };
    r.end_bits();
    let sound = match (flags & F::SENTENCE != 0, index) {
        (true, i) if i >= 2048 => SoundName::SentenceNumber(i - 2048),
        (true, i) => SoundName::Sentence(i),
        (false, i) => SoundName::Precache(i),
    };
    Sound { flags, volume, attenuation, channel, entity, sound, origin, pitch }
}

fn parse_static_sound(r: &mut MsgReader) -> StaticSound {
    let origin = r.read_coord3();
    let index = r.read_i16() as u16;
    let volume = f32::from(r.read_u8()) / 255.0;
    let attenuation = f32::from(r.read_u8()) / 64.0;
    let entity = r.read_i16();
    let pitch = r.read_u8();
    let flags = r.read_u8();
    let sound = if flags & sound_flags::SENTENCE as u8 != 0 { SoundName::Sentence(index) } else { SoundName::Precache(index) };
    StaticSound { origin, sound, volume, attenuation, entity, pitch, flags }
}

fn parse_static_entity(r: &mut MsgReader) -> StaticEntity {
    let modelindex = r.read_i16();
    let sequence = r.read_u8();
    let frame = r.read_u8();
    let colormap = r.read_i16();
    let skin = r.read_u8();
    let mut origin = [0.0; 3];
    let mut angles = [0.0; 3];
    for i in 0..3 {
        origin[i] = r.read_coord();
        angles[i] = r.read_angle();
    }
    let rendermode = r.read_u8();
    let (renderamt, rendercolor, renderfx) =
        if rendermode != 0 { (r.read_u8(), [r.read_u8(), r.read_u8(), r.read_u8()], r.read_u8()) } else { (0, [0; 3], 0) };
    StaticEntity { modelindex, sequence, frame, colormap, skin, origin, angles, rendermode, renderamt, rendercolor, renderfx }
}

fn parse_pings(r: &mut MsgReader) -> Vec<Ping> {
    let mut out = Vec::new();
    r.start_bits();
    while !r.bad() && r.read_bit() && out.len() < 64 {
        out.push(Ping { slot: r.read_bits(5) as u8, ping: r.read_bits(12) as u16, loss: r.read_bits(7) as u8 });
    }
    r.end_bits();
    out
}

fn parse_movevars(r: &mut MsgReader) -> MoveVars {
    let mut f = || r.read_f32();
    let mut mv = MoveVars {
        gravity: f(),
        stopspeed: f(),
        maxspeed: f(),
        spectatormaxspeed: f(),
        accelerate: f(),
        airaccelerate: f(),
        wateraccelerate: f(),
        friction: f(),
        edgefriction: f(),
        waterfriction: f(),
        entgravity: f(),
        bounce: f(),
        stepsize: f(),
        maxvelocity: f(),
        zmax: f(),
        wave_height: f(),
        ..MoveVars::default()
    };
    mv.footsteps = r.read_u8() != 0;
    mv.rollangle = r.read_f32();
    mv.rollspeed = r.read_f32();
    mv.skycolor = [r.read_f32(), r.read_f32(), r.read_f32()];
    mv.skyvec = [r.read_f32(), r.read_f32(), r.read_f32()];
    mv.skyname = truncate_encoded(&r.read_string(), 31).into_owned();
    mv
}

fn parse_resource_list(r: &mut MsgReader) -> ResourceList {
    let mut list = ResourceList::default();
    r.start_bits();
    let count = r.read_bits(12);
    for _ in 0..count {
        if r.bad() {
            break;
        }
        let kind = r.read_bits(4) as u8;
        let name = truncate_encoded(&r.read_bit_string(), 63).into_owned();
        let index = r.read_bits(12) as u16;
        let download_size = r.read_bits(24);
        let flags = (r.read_bits(3) as u8) & !resource_flags::WAS_MISSING;
        let md5 = (flags & resource_flags::CUSTOM != 0).then(|| {
            let mut m = [0u8; 16];
            m.copy_from_slice(&r.read_bit_data(16));
            m
        });
        let reserved = r.read_bit().then(|| {
            let mut m = [0u8; 32];
            m.copy_from_slice(&r.read_bit_data(32));
            m
        });
        list.resources.push(Resource { kind, name, index, download_size, flags, md5, reserved });
    }
    if r.read_bit() {
        let mut last = 0u16;
        while !r.bad() && r.read_bit() && list.consistency.len() < 512 {
            let idx = if r.read_bit() { last + r.read_bits(5) as u16 } else { r.read_bits(10) as u16 };
            list.consistency.push(idx);
            last = idx;
        }
    }
    r.end_bits();
    list
}

/// Byte-mode resource (svc_customization).
fn parse_byte_resource(r: &mut MsgReader) -> Resource {
    let kind = r.read_u8();
    let name = truncate_encoded(&r.read_string(), 63).into_owned();
    let index = r.read_i16() as u16;
    let download_size = r.read_i32() as u32;
    let flags = r.read_u8() & !resource_flags::WAS_MISSING;
    let md5 = (flags & resource_flags::CUSTOM != 0).then(|| {
        let mut m = [0u8; 16];
        m.copy_from_slice(r.read_bytes(16).get(..16).unwrap_or(&[0; 16]));
        m
    });
    Resource { kind, name, index, download_size, flags, md5, reserved: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitbuf::MsgWriter;
    use crate::delta::{DeltaField, dt, write_description};

    fn field(kind: u32, name: &str, offset: i32, bits: u32) -> DeltaField {
        DeltaField { kind, name: name.into(), offset, size: 1, bits, pre: 1.0, post: 1.0, recv: Default::default() }
    }

    fn entity_delta(name: &str) -> Delta {
        Delta {
            name: name.into(),
            fields: vec![field(dt::FLOAT | dt::SIGNED, "origin[0]", 16, 16), field(dt::INTEGER, "modelindex", 40, 10)],
        }
    }

    fn state_with_descriptions() -> ProtocolState {
        let mut w = MsgWriter::new();
        for name in [names::ENTITY_STATE, names::ENTITY_STATE_PLAYER, names::CUSTOM_ENTITY_STATE] {
            w.write_u8(14);
            write_description(&mut w, &entity_delta(name));
        }
        let mut st = ProtocolState { maxclients: 2, max_edicts: 1215, ..ProtocolState::default() };
        st.baselines = vec![EntityState::default(); st.max_edicts];
        let parsed = st.parse(w.as_bytes(), 1);
        assert_eq!((parsed.messages.len(), parsed.error), (3, None));
        st
    }

    /// Writes one entity record (header written by the caller) with origin[0] and modelindex.
    fn write_entity(w: &mut MsgWriter, origin_x: i32, model: u32) {
        w.write_bits(1, 3);
        w.write_bits(0b11, 8);
        w.write_sbitlong(origin_x, 16);
        w.write_bits(model, 10);
    }

    #[test]
    fn full_then_delta_packet_entities() {
        let mut st = state_with_descriptions();
        // Full update at sequence 10: entities 5 and 6.
        let mut w = MsgWriter::new();
        w.write_u8(40);
        w.write_i16(2);
        w.start_bits();
        w.write_bit(false); // not "num = last + 1"
        w.write_bit(false); // relative number
        w.write_bits(5, 6);
        w.write_bit(false); // custom
        w.write_bit(false); // offset flag
        write_entity(&mut w, 100, 7);
        w.write_bit(true); // num = 6
        w.write_bit(false);
        w.write_bit(false);
        write_entity(&mut w, -50, 8);
        w.write_bits(0, 16);
        w.end_bits();
        assert_eq!(st.parse(w.as_bytes(), 10).error, None);
        let f = st.frame(10);
        assert!(f.valid);
        assert_eq!(f.entities.iter().map(|e| (e.number(), e.origin()[0], e.modelindex())).collect::<Vec<_>>(), [(5, 100.0, 7), (6, -50.0, 8)]);

        // Delta at 11 from 10: remove 5, add 9, keep 6.
        let mut w = MsgWriter::new();
        w.write_u8(41);
        w.write_i16(2);
        w.write_u8(10);
        w.start_bits();
        w.write_bit(true); // remove
        w.write_bit(false);
        w.write_bits(5, 6);
        w.write_bit(false); // not remove
        w.write_bit(false);
        w.write_bits(4, 6); // 5 + 4 = 9
        w.write_bit(false); // custom
        write_entity(&mut w, 1, 2);
        w.write_bits(0, 16);
        w.end_bits();
        assert_eq!(st.parse(w.as_bytes(), 11).error, None);
        let f = st.frame(11);
        assert_eq!(f.entities.iter().map(|e| e.number()).collect::<Vec<_>>(), [6, 9]);
        assert_eq!(f.new_entity, [false, true]);
        assert_eq!(st.valid_sequence, 11);
    }

    #[test]
    fn stale_delta_source_is_flushed() {
        let mut st = state_with_descriptions();
        let mut w = MsgWriter::new();
        w.write_u8(41);
        w.write_i16(1);
        w.write_u8(0);
        w.start_bits();
        w.write_bit(false);
        w.write_bit(false);
        w.write_bits(5, 6);
        w.write_bit(false);
        write_entity(&mut w, 1, 2);
        w.write_bits(0, 16);
        w.end_bits();
        w.write_u8(1); // nop after the bit stream must still parse
        let msgs = st.parse(w.as_bytes(), 200).messages;
        assert_eq!(msgs.len(), 1);
        assert_eq!(st.valid_sequence, 0);
        assert!(!st.frame(200).valid);
    }

    #[test]
    fn user_messages_resolve_sizes_and_unknown_ids_are_fatal() {
        let mut st = ProtocolState::default();
        let mut w = MsgWriter::new();
        w.write_u8(39);
        w.write_u8(64);
        w.write_u8(255);
        w.write_bytes(b"SayText\0\0\0\0\0\0\0\0\0");
        w.write_u8(39);
        w.write_u8(65);
        w.write_u8(2);
        w.write_bytes(b"Money\0\0\0\0\0\0\0\0\0\0\0");
        w.write_u8(64);
        w.write_u8(3);
        w.write_bytes(b"\x01hi");
        w.write_u8(65);
        w.write_bytes(&[0x10, 0x27]);
        let msgs = st.parse(w.as_bytes(), 1).messages;
        assert_eq!(msgs[2], ServerMessage::UserMessage { id: 64, name: "SayText".into(), data: b"\x01hi".to_vec() });
        assert_eq!(msgs[3], ServerMessage::UserMessage { id: 65, name: "Money".into(), data: vec![0x10, 0x27] });
        assert!(st.parse(&[70, 0], 2).error.is_some());
    }

    #[test]
    fn a_bad_message_ends_the_packet_after_the_ones_before_it() {
        let mut st = ProtocolState::default();
        // svc_print, svc_bad, svc_print: the first print stands, nothing after svc_bad is read.
        let parsed = st.parse(&[8, b'h', b'i', 0, 0, 8, b'x', 0], 1);
        assert_eq!(parsed.messages, [ServerMessage::Print("hi".into())]);
        assert_eq!(parsed.error, Some(ProtocolError::Malformed("Illegible server message - svc_bad".into())));
        // An svc_time cut short still runs: msg_badread is only checked before the next message.
        let parsed = st.parse(&[8, b'h', b'i', 0, 7, 0, 0], 2);
        assert_eq!(parsed.messages, [ServerMessage::Print("hi".into()), ServerMessage::Time(0.0)]);
        assert_eq!(parsed.error, Some(ProtocolError::Malformed("CL_ParseServerMessage: Bad server message".into())));
    }
}
