//! Demo recording and playback (docs/re/demo.md): the `.dem` container, the `demoheader.dmf`
//! spool, the per-frame writers, the time-gated reader with its view ring, and the inspection
//! and editing commands. Every `cls.demo*` field lives in [`Demo`]; the client feeds it the
//! clock (`realtime`, `host_framecount`) and acts on what it returns.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use gs_common::bytestr::truncate_encoded;
use gs_fs::{FileSystem, PathId};
use gs_protocol::messages::MoveVars;
use gs_protocol::structs::{EventArgs, UserCmd};

pub const DEMO_PROTOCOL: i32 = 5;
pub const NET_PROTOCOL: i32 = 48;
const HEADER_SIZE: usize = 544;
const ENTRY_SIZE: usize = 92;
const FRAME_HEADER_SIZE: usize = 9;
const INFO_SIZE: usize = 436;
const EVENT_ARGS_SIZE: usize = 72;
const MAX_POSSIBLE_MSG: i32 = 0x10000;
const RING_SIZE: usize = 32;
/// `CL_DemoReadBuffer`'s buffer.
const MAX_BUFFER: u32 = 32768;
/// `&g_demoinfo.cmd` / `&g_demoinfo.movevars` at the image base (readers ignore them).
const INFO_CMD_PTR: u32 = 0x1145_E7EC;
const INFO_MOVEVARS_PTR: u32 = 0x1145_E820;
/// `CL_GGSpeeds` keeps this many one-second samples.
const GG_SAMPLES: usize = 300;
/// `cls.demos[32][16]`.
pub const MAX_DEMOS: usize = 32;

/// Frame types (`cmd` byte of the 9-byte frame header).
pub mod kind {
    pub const STARTUP: u8 = 0;
    pub const NETWORK: u8 = 1;
    pub const START: u8 = 2;
    pub const COMMAND: u8 = 3;
    pub const CLIENT_DATA: u8 = 4;
    pub const NEXT_SECTION: u8 = 5;
    pub const EVENT: u8 = 6;
    pub const WEAPON_ANIM: u8 = 7;
    pub const SOUND: u8 = 8;
    pub const BUFFER: u8 = 9;
}

/// Directory entry flags (`FDEMO_*`).
pub mod seg {
    pub const TITLE: i32 = 0x01;
    pub const PLAY: i32 = 0x04;
    pub const FADE_IN_SLOW: i32 = 0x08;
    pub const FADE_IN_FAST: i32 = 0x10;
    pub const FADE_OUT_SLOW: i32 = 0x20;
    pub const FADE_OUT_FAST: i32 = 0x40;
}

fn put_f32(b: &mut Vec<u8>, v: f32) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn put_i32(b: &mut Vec<u8>, v: i32) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn put_vec3(b: &mut Vec<u8>, v: [f32; 3]) {
    v.iter().for_each(|&c| put_f32(b, c));
}

/// Sequential little-endian fields of a fixed-size image.
struct Fields<'a> {
    b: &'a [u8],
    o: usize,
}

impl<'a> Fields<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, o: 0 }
    }

    fn take<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0u8; N];
        if let Some(s) = self.b.get(self.o..self.o + N) {
            out.copy_from_slice(s);
        }
        self.o += N;
        out
    }

    fn f32(&mut self) -> f32 {
        f32::from_le_bytes(self.take())
    }

    fn i32(&mut self) -> i32 {
        i32::from_le_bytes(self.take())
    }

    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }

    fn vec3(&mut self) -> [f32; 3] {
        [self.f32(), self.f32(), self.f32()]
    }
}

/// `ref_params_t` (232 bytes, client-sim.md Part F §1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RefParamsRaw {
    pub vieworg: [f32; 3],
    pub viewangles: [f32; 3],
    pub forward: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub frametime: f32,
    pub time: f32,
    pub intermission: i32,
    pub paused: i32,
    pub spectator: i32,
    pub onground: i32,
    pub waterlevel: i32,
    pub simvel: [f32; 3],
    pub simorg: [f32; 3],
    pub viewheight: [f32; 3],
    pub idealpitch: f32,
    pub cl_viewangles: [f32; 3],
    pub health: i32,
    pub crosshairangle: [f32; 3],
    pub viewsize: f32,
    pub punchangle: [f32; 3],
    pub maxclients: i32,
    pub viewentity: i32,
    pub playernum: i32,
    pub max_entities: i32,
    pub demoplayback: i32,
    pub hardware: i32,
    pub smoothing: i32,
    pub cmd: u32,
    pub movevars: u32,
    pub viewport: [i32; 4],
    pub next_view: i32,
    pub only_client_draw: i32,
}

impl RefParamsRaw {
    pub const SIZE: usize = 232;

    fn write(&self, b: &mut Vec<u8>) {
        for v in [self.vieworg, self.viewangles, self.forward, self.right, self.up] {
            put_vec3(b, v);
        }
        put_f32(b, self.frametime);
        put_f32(b, self.time);
        for v in [self.intermission, self.paused, self.spectator, self.onground, self.waterlevel] {
            put_i32(b, v);
        }
        for v in [self.simvel, self.simorg, self.viewheight] {
            put_vec3(b, v);
        }
        put_f32(b, self.idealpitch);
        put_vec3(b, self.cl_viewangles);
        put_i32(b, self.health);
        put_vec3(b, self.crosshairangle);
        put_f32(b, self.viewsize);
        put_vec3(b, self.punchangle);
        for v in [self.maxclients, self.viewentity, self.playernum, self.max_entities, self.demoplayback, self.hardware, self.smoothing] {
            put_i32(b, v);
        }
        b.extend_from_slice(&self.cmd.to_le_bytes());
        b.extend_from_slice(&self.movevars.to_le_bytes());
        self.viewport.iter().for_each(|&v| put_i32(b, v));
        put_i32(b, self.next_view);
        put_i32(b, self.only_client_draw);
    }

    fn read(f: &mut Fields) -> Self {
        Self {
            vieworg: f.vec3(),
            viewangles: f.vec3(),
            forward: f.vec3(),
            right: f.vec3(),
            up: f.vec3(),
            frametime: f.f32(),
            time: f.f32(),
            intermission: f.i32(),
            paused: f.i32(),
            spectator: f.i32(),
            onground: f.i32(),
            waterlevel: f.i32(),
            simvel: f.vec3(),
            simorg: f.vec3(),
            viewheight: f.vec3(),
            idealpitch: f.f32(),
            cl_viewangles: f.vec3(),
            health: f.i32(),
            crosshairangle: f.vec3(),
            viewsize: f.f32(),
            punchangle: f.vec3(),
            maxclients: f.i32(),
            viewentity: f.i32(),
            playernum: f.i32(),
            max_entities: f.i32(),
            demoplayback: f.i32(),
            hardware: f.i32(),
            smoothing: f.i32(),
            cmd: f.u32(),
            movevars: f.u32(),
            viewport: [f.i32(), f.i32(), f.i32(), f.i32()],
            next_view: f.i32(),
            only_client_draw: f.i32(),
        }
    }

    /// A `ref_params_t` image (232 bytes).
    pub fn from_bytes(b: &[u8]) -> Self {
        Self::read(&mut Fields::new(b))
    }
}

/// `movevars_t` (132 bytes).
pub fn movevars_bytes(mv: &MoveVars) -> [u8; 132] {
    let mut b = Vec::with_capacity(132);
    for v in [
        mv.gravity,
        mv.stopspeed,
        mv.maxspeed,
        mv.spectatormaxspeed,
        mv.accelerate,
        mv.airaccelerate,
        mv.wateraccelerate,
        mv.friction,
        mv.edgefriction,
        mv.waterfriction,
        mv.entgravity,
        mv.bounce,
        mv.stepsize,
        mv.maxvelocity,
        mv.zmax,
        mv.wave_height,
    ] {
        put_f32(&mut b, v);
    }
    put_i32(&mut b, i32::from(mv.footsteps));
    let mut sky = [0u8; 32];
    for (d, s) in sky.iter_mut().zip(mv.skyname.bytes().take(31)) {
        *d = s;
    }
    b.extend_from_slice(&sky);
    for v in [mv.rollangle, mv.rollspeed] {
        put_f32(&mut b, v);
    }
    put_vec3(&mut b, mv.skycolor);
    put_vec3(&mut b, mv.skyvec);
    let mut out = [0u8; 132];
    out.copy_from_slice(&b[..132]);
    out
}

/// `demo_info_t` (436 bytes): the view snapshot written with every network packet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DemoInfo {
    pub timestamp: f32,
    pub rp: RefParamsRaw,
    pub cmd: [u8; 52],
    pub movevars: [u8; 132],
    /// `cl.viewent.origin`.
    pub view: [f32; 3],
    /// `cl.stats[STAT_WEAPON]`.
    pub viewmodel: i32,
}

impl Default for DemoInfo {
    fn default() -> Self {
        Self { timestamp: 0.0, rp: RefParamsRaw::default(), cmd: [0; 52], movevars: [0; 132], view: [0.0; 3], viewmodel: 0 }
    }
}

impl DemoInfo {
    /// `CL_DemoSnapshotView`: `rp` after the client's `V_CalcRefdef` (pass 0), with its two
    /// pointers aimed at the snapshot's own copies; the timestamp is never written.
    pub fn snapshot(rp: RefParamsRaw, cmd: &UserCmd, movevars: &MoveVars, view: [f32; 3], viewmodel: i32) -> Self {
        let rp = RefParamsRaw { cmd: INFO_CMD_PTR, movevars: INFO_MOVEVARS_PTR, ..rp };
        Self { timestamp: 0.0, rp, cmd: cmd.to_bytes(), movevars: movevars_bytes(movevars), view, viewmodel }
    }

    fn write(&self, b: &mut Vec<u8>) {
        put_f32(b, self.timestamp);
        self.rp.write(b);
        b.extend_from_slice(&self.cmd);
        b.extend_from_slice(&self.movevars);
        put_vec3(b, self.view);
        put_i32(b, self.viewmodel);
    }

    fn read(b: &[u8]) -> Self {
        let mut f = Fields::new(b);
        Self { timestamp: f.f32(), rp: RefParamsRaw::read(&mut f), cmd: f.take(), movevars: f.take(), view: f.vec3(), viewmodel: f.i32() }
    }
}

/// `client_data_t` (32 bytes): what `HUD_UpdateClientData` received.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DemoClientData {
    pub origin: [f32; 3],
    pub viewangles: [f32; 3],
    pub weapons: i32,
    pub fov: f32,
}

impl DemoClientData {
    fn write(&self, b: &mut Vec<u8>) {
        put_vec3(b, self.origin);
        put_vec3(b, self.viewangles);
        put_i32(b, self.weapons);
        put_f32(b, self.fov);
    }

    pub(crate) fn read(b: &[u8]) -> Self {
        let mut f = Fields::new(b);
        Self { origin: f.vec3(), viewangles: f.vec3(), weapons: f.i32(), fov: f.f32() }
    }
}

/// `demoentry_t` (92 bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct DemoEntry {
    pub kind: i32,
    pub description: [u8; 64],
    pub flags: i32,
    pub cd_track: i32,
    pub track_time: f32,
    pub frames: i32,
    pub offset: i32,
    pub file_length: i32,
}

impl Default for DemoEntry {
    fn default() -> Self {
        Self { kind: 0, description: [0; 64], flags: 0, cd_track: 0, track_time: 0.0, frames: 0, offset: 0, file_length: 0 }
    }
}

impl DemoEntry {
    fn new(kind: i32, description: &str, cd_track: i32, offset: i32) -> Self {
        let mut e = Self { kind, cd_track, offset, ..Self::default() };
        e.set_description(description);
        e
    }

    /// `Q_strncpy(desc, s, 63)` with the last byte forced to NUL.
    fn set_description(&mut self, s: &str) {
        self.description = [0; 64];
        for (d, c) in self.description.iter_mut().zip(s.bytes().take(63)) {
            *d = c;
        }
    }

    pub fn description(&self) -> String {
        c_string(&self.description)
    }

    fn write(&self, b: &mut Vec<u8>) {
        put_i32(b, self.kind);
        b.extend_from_slice(&self.description);
        put_i32(b, self.flags);
        put_i32(b, self.cd_track);
        put_f32(b, self.track_time);
        put_i32(b, self.frames);
        put_i32(b, self.offset);
        put_i32(b, self.file_length);
    }

    fn read(b: &[u8]) -> Self {
        let mut f = Fields::new(b);
        Self {
            kind: f.i32(),
            description: f.take(),
            flags: f.i32(),
            cd_track: f.i32(),
            track_time: f.f32(),
            frames: f.i32(),
            offset: f.i32(),
            file_length: f.i32(),
        }
    }
}

/// Text up to the first NUL.
pub(crate) fn c_string(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    gs_common::bytestr::decode(&b[..end])
}

/// `demoheader_t` (544 bytes), kept as its image.
#[derive(Clone)]
struct Header([u8; HEADER_SIZE]);

impl Header {
    fn new(map: &str, dll_dir: &str, map_crc: i32) -> Self {
        let mut h = [0u8; HEADER_SIZE];
        h[..6].copy_from_slice(b"HLDEMO");
        h[8..12].copy_from_slice(&DEMO_PROTOCOL.to_le_bytes());
        h[12..16].copy_from_slice(&NET_PROTOCOL.to_le_bytes());
        for (d, s) in h[16..275].iter_mut().zip(gs_common::bytestr::encode(map)) {
            *d = s;
        }
        for (d, s) in h[276..535].iter_mut().zip(dll_dir.bytes()) {
            *d = s;
        }
        h[536..540].copy_from_slice(&map_crc.to_le_bytes());
        Self(h)
    }

    fn parse(b: &[u8]) -> Self {
        let mut h = [0u8; HEADER_SIZE];
        let n = b.len().min(HEADER_SIZE);
        h[..n].copy_from_slice(&b[..n]);
        Self(h)
    }

    /// `Q_strncmp(header, "HLDEMO", 6)`.
    fn magic_ok(&self) -> bool {
        &self.0[..6] == b"HLDEMO"
    }

    fn i32_at(&self, o: usize) -> i32 {
        i32::from_le_bytes([self.0[o], self.0[o + 1], self.0[o + 2], self.0[o + 3]])
    }

    fn demo_protocol(&self) -> i32 {
        self.i32_at(8)
    }

    fn net_protocol(&self) -> i32 {
        self.i32_at(12)
    }

    fn map_name(&self) -> String {
        c_string(&self.0[16..276])
    }

    fn dll_dir(&self) -> String {
        c_string(&self.0[276..536])
    }

    fn directory_offset(&self) -> i32 {
        self.i32_at(540)
    }

    fn set_directory_offset(&mut self, offset: i32) {
        self.0[540..544].copy_from_slice(&offset.to_le_bytes());
    }
}

/// `sub_101B73B0(name, ext, 260)`: appends `ext` unless the text after the last `.` (up to 7
/// characters) is exactly the extension (case-sensitive).
fn ensure_extension(name: &str, ext: &str) -> String {
    let want = ext.trim_start_matches('.');
    let has = name.rfind('.').is_some_and(|i| truncate_encoded(&name[i + 1..], 7) == want);
    if has { name.to_owned() } else { format!("{name}{ext}") }
}

/// `COM_DefaultExtension`: scanning back from the end, a `.` before any `/` keeps the name;
/// the first character is never looked at.
fn default_extension(name: &str, ext: &str) -> String {
    let b = name.as_bytes();
    let mut i = b.len();
    while i > 1 {
        i -= 1;
        if b[i] == b'/' {
            break;
        }
        if b[i] == b'.' {
            return name.to_owned();
        }
    }
    format!("{name}{ext}")
}

/// `sub_101B7BC0(name, ext)`: the text after the last `.` equals `ext` (case-sensitive).
fn has_extension(name: &str, ext: &str) -> bool {
    name.rfind('.').map_or("", |i| &name[i + 1..]) == ext
}

/// `COM_StripExtension`: everything before the last `.` of the last path component.
fn strip_extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if !name[i..].contains(['/', '\\']) => &name[..i],
        _ => name,
    }
}

/// `COM_FileBase`: the last path component without its extension.
pub fn file_base(path: &str) -> String {
    let tail = path.rsplit(['/', '\\']).next().unwrap_or(path);
    strip_extension(tail).to_owned()
}

/// `Q_atoi`.
pub fn atoi(s: &str) -> i32 {
    let s = s.trim_start();
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let n = digits.bytes().take_while(u8::is_ascii_digit).fold(0i32, |n, d| n.wrapping_mul(10).wrapping_add(i32::from(d - b'0')));
    if neg { n.wrapping_neg() } else { n }
}

/// `Q_atof` for the replay speed.
fn atof(s: &str) -> f64 {
    let s = s.trim_start();
    let end = s
        .char_indices()
        .take_while(|&(i, c)| c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+')))
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    s[..end].parse().unwrap_or(0.0)
}

/// Console output of a demo operation: `(developer only, text)`.
pub type Output = Vec<(bool, String)>;

/// Where the demo layer gets the clock and the file system.
pub struct Ctx<'a> {
    pub fs: &'a FileSystem,
    /// `realtime`.
    pub now: f64,
    /// `host_framecount`.
    pub frame: i32,
    pub out: &'a mut Output,
}

impl Ctx<'_> {
    fn print(&mut self, text: impl Into<String>) {
        self.out.push((false, text.into()));
    }

    fn dprint(&mut self, text: impl Into<String>) {
        self.out.push((true, text.into()));
    }
}

/// What a frame of the reader asks the engine to do, in file order.
#[derive(Debug, Clone, PartialEq)]
pub enum DemoItem {
    /// Type 3: a client command, for the filtered buffer when `CL_FilteredCmdAllowed` passes.
    Command(String),
    /// Type 4: `ClientDLL_DemoUpdateClientData`.
    ClientData(DemoClientData),
    /// Type 6: `CL_QueueEvent`.
    Event { flags: i32, index: i32, delay: f32, args: EventArgs },
    /// Type 7: `cle_pfnWeaponAnim`.
    WeaponAnim { anim: i32, body: i32 },
    /// Type 8: `CL_DemoReadSound` with the file's volume and attenuation (still swapped).
    Sound { channel: i32, sample: String, volume: f32, attenuation: f32, flags: i32, pitch: i32 },
    /// Type 9: `ClientDLL_ReadDemoBuffer`.
    Buffer(Vec<u8>),
    /// `Cbuf_AddText`.
    EngineText(String),
    /// `V_ScreenFade` with a `screenfade_t` image (the ScreenFade message layout).
    Fade([u8; 10]),
    /// `SetDemoMessage` + `CL_HudMessage("__DEMOMESSAGE__")`.
    Title(String),
}

/// One call of `CL_GetDemoMessage`.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadStatus {
    /// The next frame is not due (or timedemo already read a packet this host frame).
    Wait,
    /// A network packet with the seven netchan counters to restore before parsing it.
    Packet { data: Vec<u8>, sequences: [i32; 7] },
    /// `Host_EndGame` with this message.
    End(&'static str),
}

/// What `CL_StopPlayback` leaves for the host.
#[derive(Debug, Default)]
pub struct StopResult {
    /// `Cvar_Set("cl_gg", "0")` ran.
    pub gg_reset: bool,
    /// The game gauge wants `quit`.
    pub quit: bool,
}

/// The view `CL_DemoInterpolateView` substitutes for the client's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewReplay {
    pub rp: RefParamsRaw,
    /// `cl.viewent.origin` and `cl.stats[STAT_WEAPON]`, when a pair qualified (or timedemo).
    pub view: Option<[f32; 3]>,
    pub viewmodel: Option<i32>,
}

#[derive(Debug, Default)]
struct GameGauge {
    last: f32,
    last_frame: i32,
    max: i32,
    min: i32,
    samples: Vec<i32>,
}

/// The open recording.
struct Recorder {
    file: BufWriter<File>,
    header: Header,
}

/// The `cls.demo*` state.
pub struct Demo {
    recorder: Option<Recorder>,
    /// `cls.demoplayback` with the file read into memory.
    playback: Option<Vec<u8>>,
    /// `cls.demoplayback` without a file: `viewdemo` through the DemoPlayer module.
    pub module: bool,
    pos: usize,
    pub timedemo: bool,
    /// `cls.demowaiting`: no type-1 frames until the first full entity update.
    pub waiting: bool,
    appending: bool,
    /// `cls.demofilename` (appenddemo).
    filename: String,
    starttime: f32,
    startframe: i32,
    /// `cls.forcetrack`: 0 (BSS) until a demo sets it.
    pub forcetrack: i32,
    /// `cls.demoheader`: the `demoheader.dmf` spool in GAMECONFIG (`w+b`).
    spool: Option<File>,
    framecount: i32,
    td_lastframe: i32,
    td_startframe: i32,
    td_starttime: f32,
    /// `g_demoinfo`: the last rendered frame's snapshot.
    pub info: DemoInfo,
    dir: Vec<DemoEntry>,
    cur_entry: usize,
    ring: [DemoInfo; RING_SIZE],
    lastdemoframe: i32,
    /// `g_demospeed`.
    speed: f32,
    fade_out_started: bool,
    /// The stale header fields of the last read (FS_Read results are not checked).
    last_cmd: u8,
    last_time: f32,
    gauge: GameGauge,
    /// `cls.demonum` (-1 = loop off) and `cls.demos`.
    pub demonum: i32,
    pub demos: [String; MAX_DEMOS],
}

impl Default for Demo {
    fn default() -> Self {
        Self {
            recorder: None,
            playback: None,
            module: false,
            pos: 0,
            timedemo: false,
            waiting: false,
            appending: false,
            filename: String::new(),
            starttime: 0.0,
            startframe: 0,
            forcetrack: 0,
            spool: None,
            framecount: 0,
            td_lastframe: 0,
            td_startframe: 0,
            td_starttime: 0.0,
            info: DemoInfo::default(),
            dir: Vec::new(),
            cur_entry: 0,
            ring: [DemoInfo::default(); RING_SIZE],
            lastdemoframe: 0,
            speed: 1.0,
            fade_out_started: false,
            last_cmd: 0,
            last_time: 0.0,
            gauge: GameGauge::default(),
            demonum: 0,
            demos: Default::default(),
        }
    }
}

/// `"%f"`.
fn c_f(v: f64) -> String {
    format!("{v:.6}")
}

impl Demo {
    pub fn recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// `cls.demoplayback`.
    pub fn playing(&self) -> bool {
        self.playback.is_some() || self.module
    }

    /// `cls.timedemo` (only meaningful while playing).
    pub fn timedemo(&self) -> bool {
        self.timedemo
    }

    /// The demo clock of the recorder: `(f32)realtime − demostarttime` and the frame offset.
    fn stamp(&self, now: f64, frame: i32) -> (f32, i32) {
        ((now as f32) - self.starttime, frame.wrapping_sub(self.startframe))
    }

    fn frame_header(b: &mut Vec<u8>, cmd: u8, time: f32, frame: i32) {
        b.push(cmd);
        put_f32(b, time);
        put_i32(b, frame);
    }

    /// Appends a frame to the open recording (the writers only check that the file is open).
    fn write(&mut self, bytes: &[u8]) {
        if let Some(rec) = self.recorder.as_mut()
            && let Err(e) = rec.file.write_all(bytes)
        {
            log::warn!("demo write: {e}");
        }
    }

    fn packet_frame(&self, cmd: u8, payload: &[u8], sequences: [i32; 7], now: f64, frame: i32) -> Vec<u8> {
        let mut b = Vec::with_capacity(FRAME_HEADER_SIZE + INFO_SIZE + 32 + payload.len());
        let (t, f) = self.stamp(now, frame);
        Self::frame_header(&mut b, cmd, t, f);
        self.info.write(&mut b);
        sequences.iter().for_each(|&s| put_i32(&mut b, s));
        put_i32(&mut b, payload.len() as i32);
        b.extend_from_slice(payload);
        b
    }

    // ------------------------------------------------------------------ recording

    /// `CL_BeginDemoStartup` (from `CL_ParseServerInfo`): the previous spool closed,
    /// `demoheader.dmf` truncated and reopened.
    pub fn begin_startup(&mut self, fs: &FileSystem, out: &mut Output) {
        self.spool = None;
        self.spool = fs
            .write_path("demoheader.dmf", PathId::GameConfig)
            .ok()
            .and_then(|path| std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path).ok());
        let text = if self.spool.is_some() { "Spooling demo header.\n" } else { "ERROR: couldn't open temporary header file.\n" };
        out.push((true, text.into()));
    }

    /// `CL_WriteDemoStartup`: a type-0 frame into the spool, timed with the stale demo clock.
    pub fn spool_packet(&mut self, payload: &[u8], sequences: [i32; 7], now: f64, frame: i32) {
        if self.spool.is_none() || payload.is_empty() {
            return;
        }
        let bytes = self.packet_frame(kind::STARTUP, payload, sequences, now, frame);
        if let Some(spool) = self.spool.as_mut()
            && let Err(e) = spool.write_all(&bytes)
        {
            log::warn!("demoheader.dmf: {e}");
        }
    }

    /// `CL_DemoShutdown` (from `Host_Shutdown`): the spool closed and `demoheader.dmf` deleted.
    pub fn shutdown(&mut self, fs: &FileSystem) {
        self.spool = None;
        fs.remove("demoheader.dmf", Some(PathId::GameConfig));
    }

    /// `CL_WriteDemoMessage`: a type-1 frame of a parsed packet (`len >= 0`).
    pub fn record_packet(&mut self, payload: &[u8], sequences: [i32; 7], now: f64, frame: i32) {
        if !self.recording() || self.waiting {
            return;
        }
        let bytes = self.packet_frame(kind::NETWORK, payload, sequences, now, frame);
        self.write(&bytes);
        self.framecount += 1;
    }

    /// `CL_RecordHUDCommand`: the registered name of a client-DLL command, 63 chars at most.
    pub fn record_hud_command(&mut self, name: &str, now: f64, frame: i32) {
        if !self.recording() {
            return;
        }
        let mut b = Vec::with_capacity(73);
        let (t, f) = self.stamp(now, frame);
        Self::frame_header(&mut b, kind::COMMAND, t, f);
        let mut text = [0u8; 64];
        for (d, s) in text.iter_mut().zip(name.bytes().take(63)) {
            *d = s;
        }
        b.extend_from_slice(&text);
        self.write(&b);
    }

    /// `CL_DemoWriteClientData`.
    pub fn record_client_data(&mut self, cd: &DemoClientData, now: f64, frame: i32) {
        if !self.recording() {
            return;
        }
        let mut b = Vec::with_capacity(41);
        let (t, f) = self.stamp(now, frame);
        Self::frame_header(&mut b, kind::CLIENT_DATA, t, f);
        cd.write(&mut b);
        self.write(&b);
    }

    /// `CL_DemoEvent`.
    pub fn record_event(&mut self, flags: i32, index: u16, delay: f32, args: &EventArgs, now: f64, frame: i32) {
        if !self.recording() {
            return;
        }
        let mut b = Vec::with_capacity(93);
        let (t, f) = self.stamp(now, frame);
        Self::frame_header(&mut b, kind::EVENT, t, f);
        put_i32(&mut b, flags);
        put_i32(&mut b, i32::from(index));
        put_f32(&mut b, delay);
        b.extend_from_slice(args.bytes());
        self.write(&b);
    }

    /// `CL_DemoAnim`.
    pub fn record_anim(&mut self, anim: i32, body: i32, now: f64, frame: i32) {
        if !self.recording() {
            return;
        }
        let mut b = Vec::with_capacity(17);
        let (t, f) = self.stamp(now, frame);
        Self::frame_header(&mut b, kind::WEAPON_ANIM, t, f);
        put_i32(&mut b, anim);
        put_i32(&mut b, body);
        self.write(&b);
    }

    /// `CL_DemoWriteSound`: the raw sample name without its NUL.
    #[allow(clippy::too_many_arguments)]
    pub fn record_sound(&mut self, channel: i32, sample: &str, volume: f32, attenuation: f32, flags: i32, pitch: i32, now: f64, frame: i32) {
        if !self.recording() {
            return;
        }
        let sample = gs_common::bytestr::encode(sample);
        let mut b = Vec::with_capacity(33 + sample.len());
        let (t, f) = self.stamp(now, frame);
        Self::frame_header(&mut b, kind::SOUND, t, f);
        put_i32(&mut b, channel);
        put_i32(&mut b, sample.len() as i32);
        b.extend_from_slice(&sample);
        put_f32(&mut b, volume);
        put_f32(&mut b, attenuation);
        put_i32(&mut b, flags);
        put_i32(&mut b, pitch);
        self.write(&b);
    }

    /// `pDemoAPI->WriteBuffer`.
    pub fn record_buffer(&mut self, data: &[u8], now: f64, frame: i32) {
        if !self.recording() {
            return;
        }
        let mut b = Vec::with_capacity(13 + data.len());
        let (t, f) = self.stamp(now, frame);
        Self::frame_header(&mut b, kind::BUFFER, t, f);
        put_i32(&mut b, data.len() as i32);
        b.extend_from_slice(data);
        self.write(&b);
    }

    fn open_recording(&mut self, ctx: &mut Ctx, name: &str, header: Header, mode_append: Option<Vec<u8>>) -> Option<u64> {
        let path = ctx.fs.write_path(name, PathId::GameConfig).ok()?;
        let file = File::create(&path).ok()?;
        let mut file = BufWriter::new(file);
        let data = mode_append.unwrap_or_else(|| header.0.to_vec());
        file.write_all(&data).ok()?;
        let pos = file.stream_position().ok()?;
        self.recorder = Some(Recorder { file, header });
        Some(pos)
    }

    fn tell(&mut self) -> i32 {
        self.recorder.as_mut().and_then(|r| r.file.stream_position().ok()).map_or(0, |p| p as i32)
    }

    /// `cmd_record`: `active` is `cls.state == 5`; `map` the world model name, `map_crc` the
    /// serverinfo CRC.
    pub fn cmd_record(&mut self, ctx: &mut Ctx, argv: &[&str], active: bool, map: &str, map_crc: i32) {
        if argv.len() < 2 {
            return ctx.print("record <demoname> <cd>\n");
        }
        if self.recording() {
            return ctx.print("Already recording.\n");
        }
        if self.playing() {
            return ctx.print("Can't record during demo playback.\n");
        }
        if self.spool.is_none() || !active {
            let dir = ctx.fs.game_dir().map(|d| d.display().to_string()).unwrap_or_default();
            return ctx.print(format!("You must be in an active map spawned on a machine that allows creation of files in {dir}\n"));
        }
        if !(2..=3).contains(&argv.len()) {
            return ctx.print("record <demoname> <cd track>\n");
        }
        if argv[1].contains("..") {
            return ctx.print("Relative pathnames are not allowed.\n");
        }
        let track = if argv.len() == 3 {
            let t = atoi(argv[2]);
            ctx.print(format!("Forcing CD track to {t}\n"));
            t
        } else {
            -1
        };
        let name = ensure_extension(&truncate_encoded(argv[1], 255), ".dem");
        ctx.print(format!("recording to {name}.\n"));
        let dll_dir = ctx.fs.game_dir().map(|d| d.display().to_string()).unwrap_or_default();
        let header = Header::new(&file_base(map), &file_base(&dll_dir), map_crc);
        let Some(start) = self.open_recording(ctx, &name, header, None) else {
            return ctx.print(format!("Error: couldn't open demo file {name}, demo files must have .dem extension.\n"));
        };
        self.dir = vec![DemoEntry::new(0, "LOADING", track, start as i32), DemoEntry::default()];
        self.waiting = true;
        // The spool ends with a type-5 frame stamped on the stale clock, and is copied whole.
        let (t, f) = self.stamp(ctx.now, ctx.frame);
        let mut five = Vec::with_capacity(FRAME_HEADER_SIZE);
        Self::frame_header(&mut five, kind::NEXT_SECTION, t, f);
        let mut copy = Vec::new();
        if let Some(spool) = self.spool.as_mut() {
            let read = spool.write_all(&five).and_then(|()| spool.seek(SeekFrom::Start(0))).and_then(|_| spool.read_to_end(&mut copy)).and_then(|_| spool.seek(SeekFrom::End(0)));
            if let Err(e) = read {
                log::warn!("demoheader.dmf: {e}");
            }
        }
        self.write(&copy);
        self.framecount = 0;
        self.startframe = ctx.frame;
        self.starttime = ctx.now as f32;
        let pos = self.tell();
        self.dir[0].file_length = pos - self.dir[0].offset;
        self.dir[1] = DemoEntry::new(1, "Playback", track, pos);
        let mut two = Vec::with_capacity(FRAME_HEADER_SIZE);
        let (t, f) = self.stamp(ctx.now, ctx.frame);
        Self::frame_header(&mut two, kind::START, t, f);
        self.write(&two);
        self.td_startframe = ctx.frame;
        self.td_lastframe = -1;
        self.appending = false;
    }

    /// `cmd_stop`: the closing frame, the directory, the header with its offset.
    pub fn cmd_stop(&mut self, ctx: &mut Ctx) {
        if !self.recording() {
            return ctx.print("Not recording a demo.\n");
        }
        let t = ctx.now as f32;
        let mut five = Vec::with_capacity(FRAME_HEADER_SIZE);
        Self::frame_header(&mut five, kind::NEXT_SECTION, t - self.starttime, ctx.frame.wrapping_sub(self.startframe));
        self.write(&five);
        let dirpos = self.tell();
        let (starttime, framecount) = (self.starttime, self.framecount);
        if let Some(cur) = self.dir.last_mut() {
            cur.file_length = dirpos - cur.offset;
            cur.track_time = t - starttime;
            cur.frames = framecount;
        }
        let mut b = Vec::with_capacity(4 + self.dir.len() * ENTRY_SIZE);
        put_i32(&mut b, self.dir.len() as i32);
        self.dir.iter().for_each(|e| e.write(&mut b));
        self.write(&b);
        self.dir.clear();
        if let Some(mut rec) = self.recorder.take() {
            rec.header.set_directory_offset(dirpos);
            let res = rec.file.seek(SeekFrom::Start(0)).and_then(|_| rec.file.write_all(&rec.header.0)).and_then(|()| rec.file.flush());
            if let Err(e) = res {
                log::warn!("demo finalise: {e}");
            }
        }
        self.td_lastframe = ctx.frame;
        if self.appending {
            ctx.print("Replacing old demo with appended version.\n");
            let temp = format!("{}.dm2", strip_extension(&self.filename));
            crt_replace(ctx.fs, &self.filename, &temp);
        }
        ctx.print("Completed demo\n");
        ctx.dprint(format!(
            "Recording time {:.2}\nHost frames {}\n",
            f64::from(t - self.starttime),
            self.td_lastframe.wrapping_sub(self.td_startframe)
        ));
    }

    /// `cmd_appenddemo`: a new segment over the old directory of a copy (`.dm2`).
    pub fn cmd_appenddemo(&mut self, ctx: &mut Ctx, argv: &[&str], active: bool, map: &str) {
        if self.recording() || self.playing() {
            return ctx.print("Appenddemo only available when not running or recording a demo.\n");
        }
        if !active {
            let dir = ctx.fs.game_dir().map(|d| d.display().to_string()).unwrap_or_default();
            return ctx.print(format!("You must be in an active map spawned on a machine that allows creation of files in {dir}\n"));
        }
        if !(2..=3).contains(&argv.len()) {
            return ctx.print("appenddemo <demoname> <cd track>\n");
        }
        if argv[1].contains("..") {
            return ctx.print("Relative pathnames are not allowed.\n");
        }
        let track = if argv.len() == 3 {
            let t = atoi(argv[2]);
            ctx.print(format!("Forcing CD track to {t}\n"));
            t
        } else {
            -1
        };
        let name = default_extension(&truncate_encoded(argv[1], 255), ".dem");
        if !has_extension(&name, "dem") {
            return ctx.print(format!("Error: couldn't open {name}, demo files must have .dem extension.\n"));
        }
        ctx.print(format!("Appending to demo {name}.\n"));
        self.filename = truncate_encoded(&name, 255).into_owned();
        let Some(src) = ctx.fs.resolve_in(&name, Some(PathId::GameConfig)).ok().and_then(|p| std::fs::read(p).ok()) else {
            return ctx.print(format!("Error:  couldn't open demo file {name} for appending.\n"));
        };
        let temp = ensure_extension(strip_extension(&name), ".dm2");
        let header = Header::parse(&src);
        let Some(_) = self.open_recording(ctx, &temp, header.clone(), Some(src.clone())) else {
            return ctx.print(format!("ERROR: couldn't open {name}.\n"));
        };
        let fail = |demo: &mut Self, ctx: &mut Ctx, text: String, unlink: bool| {
            ctx.print(text);
            demo.recorder = None;
            if unlink && let Ok(p) = ctx.fs.write_path(&temp, PathId::GameConfig) {
                let _ = std::fs::remove_file(p);
            }
        };
        if !header.magic_ok() {
            return fail(self, ctx, format!("{name} is not a demo file\n"), true);
        }
        if file_base(map) != header.map_name() {
            return fail(self, ctx, format!("{name} was recorded on a different map, cannot append.\n"), true);
        }
        if header.net_protocol() != NET_PROTOCOL || header.demo_protocol() != DEMO_PROTOCOL {
            let text = format!(
                "ERROR: demo protocol outdated\nDemo file protocols {}N:{}D\nServer protocol is at {NET_PROTOCOL}N:{DEMO_PROTOCOL}D\n",
                header.net_protocol(),
                header.demo_protocol()
            );
            return fail(self, ctx, text, true);
        }
        let Some(mut dir) = read_directory(&src, header.directory_offset()) else {
            let count = directory_count(&src, header.directory_offset());
            return fail(self, ctx, format!("ERROR: demo had bogus # of directory entries:  {count}\n"), false);
        };
        let n = dir.len();
        let old_offset = header.directory_offset();
        if let Some(rec) = self.recorder.as_mut()
            && let Err(e) = rec.file.seek(SeekFrom::Start(old_offset as u64))
        {
            log::warn!("appenddemo seek: {e}");
        }
        let pos = self.tell();
        dir.push(DemoEntry::new(1, &format!("Playback '{n}'"), track, pos));
        self.dir = dir;
        self.startframe = ctx.frame;
        self.framecount = 0;
        self.starttime = ctx.now as f32;
        let mut two = Vec::with_capacity(FRAME_HEADER_SIZE);
        Self::frame_header(&mut two, kind::START, 0.0, 0);
        self.write(&two);
        self.waiting = true;
        self.td_startframe = ctx.frame;
        self.td_lastframe = -1;
        self.appending = true;
    }

    // ------------------------------------------------------------------ playback

    /// `cmd_playdemo` after its `CL_Disconnect`: open, check, read the directory, start the
    /// LOADING segment. `false` when nothing plays (`cls.demonum = -1` already set); `corrupt`
    /// asks for the extra disconnect of a bad directory.
    pub fn open_playback(&mut self, ctx: &mut Ctx, argv1: &str, speed: f32, corrupt: &mut bool) -> bool {
        let name = default_extension(&truncate_encoded(argv1, 250), ".dem");
        ctx.print(format!("Playing demo from {name}.\n"));
        let Some(data) = ctx.fs.resolve(&name).ok().and_then(|p| std::fs::read(p).ok()) else {
            ctx.print("ERROR: couldn't open.\n");
            self.demonum = -1;
            return false;
        };
        let header = Header::parse(&data);
        if !header.magic_ok() {
            ctx.print(format!("{name} is not a demo file\n"));
            self.demonum = -1;
            return false;
        }
        if header.net_protocol() != NET_PROTOCOL || header.demo_protocol() != DEMO_PROTOCOL {
            ctx.print(format!(
                "WARNING! demo protocol outdated.\nDemo file protocols {}:{}, Engine protocol is at {NET_PROTOCOL}:{DEMO_PROTOCOL}\n",
                header.net_protocol(),
                header.demo_protocol()
            ));
        }
        let Some(dir) = read_directory(&data, header.directory_offset()) else {
            log::info!("COM_ExplainDisconnection: Error: Corrupt demo file.");
            self.demonum = -1;
            *corrupt = true;
            return false;
        };
        self.speed = speed;
        self.cur_entry = 0;
        self.pos = usize::try_from(dir[0].offset).unwrap_or(usize::MAX);
        self.forcetrack = dir[0].cd_track;
        self.dir = dir;
        self.startframe = ctx.frame;
        self.starttime = (f64::from(speed) * ctx.now) as f32;
        self.playback = Some(data);
        self.framecount = 0;
        true
    }

    /// `cmd_timedemo` / `cmd_gg` after a successful `cmd_playdemo`.
    pub fn start_timedemo(&mut self, frame: i32, now: f64, gauge: bool) {
        if gauge {
            self.gauge = GameGauge { last: 0.0, last_frame: frame, max: 0, min: 0x7FFF_FFFF, samples: Vec::new() };
        }
        self.timedemo = true;
        self.td_startframe = frame;
        self.td_lastframe = -1;
        self.td_starttime = now as f32;
    }

    /// `FS_Read` into `dst`: a short read fills what it can and leaves the rest.
    fn read_into(&mut self, dst: &mut [u8]) -> usize {
        let file = self.playback.as_deref().unwrap_or(&[]);
        let start = self.pos.min(file.len());
        let n = dst.len().min(file.len() - start);
        dst[..n].copy_from_slice(&file[start..start + n]);
        self.pos = start + n;
        n
    }

    fn read_i32(&mut self) -> i32 {
        let mut b = [0u8; 4];
        self.read_into(&mut b);
        i32::from_le_bytes(b)
    }

    fn read_f32(&mut self) -> f32 {
        let mut b = [0u8; 4];
        self.read_into(&mut b);
        f32::from_le_bytes(b)
    }

    fn fade(duration: u16, hold: u16, flags: u16) -> DemoItem {
        let mut b = [0u8; 10];
        b[0..2].copy_from_slice(&duration.to_le_bytes());
        b[2..4].copy_from_slice(&hold.to_le_bytes());
        b[4..6].copy_from_slice(&flags.to_le_bytes());
        b[9] = 255;
        DemoItem::Fade(b)
    }

    /// `CL_GetDemoMessage`: sub-frames go to `items` in file order until a network packet is
    /// due; `timedemo` releases everything at once but one packet per host frame.
    pub fn read(&mut self, now: f64, frame: i32, items: &mut Vec<DemoItem>) -> ReadStatus {
        if !self.playing() {
            return ReadStatus::Wait;
        }
        let mut rel;
        loop {
            let pos = self.pos;
            let mut head = [self.last_cmd, 0, 0, 0, 0, 0, 0, 0, 0];
            head[1..5].copy_from_slice(&self.last_time.to_le_bytes());
            self.read_into(&mut head);
            let cmd = head[0];
            let ftime = f32::from_le_bytes([head[1], head[2], head[3], head[4]]);
            (self.last_cmd, self.last_time) = (cmd, ftime);
            let clock = (f64::from(self.speed) * now) as f32;
            rel = clock - self.starttime;
            // comiss/setae: a NaN time is never "later".
            let due_later = !self.timedemo && ftime >= rel;
            if cmd != kind::STARTUP && cmd != kind::NEXT_SECTION && due_later {
                self.pos = pos;
                return ReadStatus::Wait;
            }
            match cmd {
                kind::START => {
                    self.starttime = clock;
                    self.startframe = frame;
                }
                kind::NEXT_SECTION => {
                    if self.cur_entry >= 1 && self.dir.get(self.cur_entry).is_some_and(|e| e.flags & seg::PLAY != 0) {
                        items.push(DemoItem::EngineText("cd stop\n".into()));
                    }
                    self.cur_entry += 1;
                    let Some(entry) = self.dir.get(self.cur_entry) else { return ReadStatus::End("End of demo.\n") };
                    self.forcetrack = entry.cd_track;
                    self.pos = usize::try_from(entry.offset).unwrap_or(usize::MAX);
                    self.startframe = frame;
                    self.framecount = 0;
                    self.starttime = (f64::from(self.speed) * now) as f32;
                }
                kind::COMMAND => {
                    let mut s = [0u8; 64];
                    self.read_into(&mut s);
                    items.push(DemoItem::Command(c_string(&s)));
                }
                kind::CLIENT_DATA => {
                    let mut b = [0u8; 32];
                    self.read_into(&mut b);
                    items.push(DemoItem::ClientData(DemoClientData::read(&b)));
                }
                kind::EVENT => {
                    let flags = self.read_i32();
                    let index = self.read_i32();
                    let delay = self.read_f32();
                    let mut args = EventArgs::default();
                    let mut raw = [0u8; EVENT_ARGS_SIZE];
                    self.read_into(&mut raw);
                    args.bytes_mut().copy_from_slice(&raw);
                    items.push(DemoItem::Event { flags, index, delay, args });
                }
                kind::WEAPON_ANIM => {
                    let anim = self.read_i32();
                    let body = self.read_i32();
                    items.push(DemoItem::WeaponAnim { anim, body });
                }
                kind::SOUND => {
                    let channel = self.read_i32();
                    let len = self.read_i32();
                    // Clamped (unsigned) to 255 without skipping the rest.
                    let n = (len as u32).min(255) as usize;
                    let mut name = vec![0u8; n];
                    self.read_into(&mut name);
                    let volume = self.read_f32();
                    let attenuation = self.read_f32();
                    let flags = self.read_i32();
                    let pitch = self.read_i32();
                    items.push(DemoItem::Sound { channel, sample: c_string(&name), volume, attenuation, flags, pitch });
                }
                kind::BUFFER => {
                    let size = self.read_i32();
                    let n = (size as u32).min(MAX_BUFFER) as usize;
                    let mut buf = vec![0u8; n];
                    self.read_into(&mut buf);
                    items.push(DemoItem::Buffer(buf));
                }
                _ => break,
            }
        }
        if self.timedemo && self.lastdemoframe == frame {
            self.pos = self.pos.saturating_sub(FRAME_HEADER_SIZE);
            return ReadStatus::Wait;
        }
        self.lastdemoframe = frame;
        if frame == self.td_startframe.wrapping_add(1) {
            self.td_starttime = now as f32;
        }
        self.segment_effects(rel, now, items);
        let mut raw = [0u8; INFO_SIZE];
        self.read_into(&mut raw);
        let info = DemoInfo::read(&raw);
        self.info.cmd = info.cmd;
        self.info.movevars = info.movevars;
        self.info.view = info.view;
        self.info.viewmodel = info.viewmodel;
        self.ring.copy_within(0..RING_SIZE - 1, 1);
        self.ring[0] = DemoInfo { timestamp: rel, ..info };
        let mut sequences = [0i32; 7];
        for s in &mut sequences {
            *s = self.read_i32();
        }
        let mut len = [0u8; 4];
        if self.read_into(&mut len) != 4 {
            return ReadStatus::End("Bad demo length.\n");
        }
        let len = i32::from_le_bytes(len);
        if len < 0 {
            return ReadStatus::End("Demo message length < 0.\n");
        }
        if len > MAX_POSSIBLE_MSG {
            return ReadStatus::End("Demo message > MAX_POSSIBLE_MSG");
        }
        let mut data = vec![0u8; len as usize];
        if len > 0 && self.read_into(&mut data) != data.len() {
            return ReadStatus::End("Error reading demo message data.\n");
        }
        ReadStatus::Packet { data, sequences }
    }

    /// The per-packet segment effects of segments after LOADING (§3.4).
    fn segment_effects(&mut self, rel: f32, now: f64, items: &mut Vec<DemoItem>) {
        if self.cur_entry == 0 {
            return;
        }
        let Some(entry) = self.dir.get(self.cur_entry).cloned() else { return };
        if self.framecount == 0 {
            if entry.flags & seg::PLAY != 0 {
                items.push(DemoItem::EngineText(format!("cd stop\ncd play {}\n", entry.cd_track)));
            }
            if entry.flags & seg::FADE_IN_FAST != 0 {
                items.push(Self::fade(0x0800, 0, 0));
            }
            if entry.flags & seg::FADE_IN_SLOW != 0 {
                items.push(Self::fade(0x2000, 0, 0));
            }
        }
        if self.framecount == 1 {
            self.fade_out_started = false;
            if entry.flags & seg::TITLE != 0 {
                let title = entry.description();
                if !title.is_empty() {
                    items.push(DemoItem::Title(title));
                }
            }
            if !self.timedemo {
                self.starttime = (f64::from(self.speed) * now) as f32;
            }
        }
        self.framecount += 1;
        if !self.fade_out_started {
            if entry.flags & seg::FADE_OUT_FAST != 0 && f64::from(rel) + 0.5 >= f64::from(entry.track_time) + 0.02 {
                self.fade_out_started = true;
                items.push(Self::fade(0x0800, 0x0199, 1));
            }
            if !self.fade_out_started && entry.flags & seg::FADE_OUT_SLOW != 0 && f64::from(rel) + 2.0 >= f64::from(entry.track_time) + 0.02 {
                self.fade_out_started = true;
                items.push(Self::fade(0x2000, 0x0199, 1));
            }
        }
    }

    /// `CL_DemoInterpolateView`: the newest packet's refdef, with the camera blended between
    /// the two ring entries around `clock − 0.02`.
    pub fn interpolate_view(&self, now: f64) -> ViewReplay {
        let newest = &self.ring[0];
        let mut out = ViewReplay { rp: newest.rp, view: None, viewmodel: None };
        if self.timedemo {
            out.view = Some(newest.view);
            out.viewmodel = Some(newest.viewmodel);
            return out;
        }
        let t = (f64::from((f64::from(self.speed) * now) as f32 - self.starttime) - 0.02) as f32;
        let mut prev = newest;
        let mut cur = None;
        for entry in &self.ring[1..=30] {
            cur = Some(entry);
            if entry.timestamp == 0.0 || t > entry.timestamp {
                break;
            }
            prev = entry;
        }
        let Some(cur) = cur.filter(|c| c.timestamp != 0.0 && c.timestamp != prev.timestamp) else { return out };
        let mut f = (t - cur.timestamp) / (prev.timestamp - cur.timestamp);
        if f64::from(f) > 1.0 {
            f = 1.0;
        } else if f < 0.0 {
            f = 0.0;
        }
        let lerp = |a: [f32; 3], b: [f32; 3]| [0, 1, 2].map(|i| a[i] + f * (b[i] - a[i]));
        let (c, p) = (&cur.rp, &prev.rp);
        out.rp.vieworg = lerp(c.vieworg, p.vieworg);
        out.rp.viewangles = [0, 1, 2].map(|i| {
            let mut d = p.viewangles[i] - c.viewangles[i];
            if d > 180.0 {
                d -= 360.0;
            } else if d < -180.0 {
                d += 360.0;
            }
            d * f + c.viewangles[i]
        });
        out.rp.viewangles = out.rp.viewangles.map(|a| {
            if a > 180.0 {
                (f64::from(a % 360.0) - 360.0) as f32
            } else if a < -180.0 {
                (f64::from(a % 360.0) + 360.0) as f32
            } else {
                a
            }
        });
        out.rp.simvel = lerp(c.simvel, p.simvel);
        out.rp.simorg = lerp(c.simorg, p.simorg);
        out.rp.viewheight = [0, 1, 2].map(|i| (p.viewheight[i] - c.viewheight[i]) * f + c.viewheight[i]);
        out.view = Some(lerp(cur.view, prev.view));
        out.viewmodel = Some(prev.viewmodel);
        out
    }

    /// `CL_StopPlayback`: close, report timedemo/gauge results; `cd stop` is queued by the host.
    pub fn stop_playback(&mut self, ctx: &mut Ctx, cl_gg: bool, version: &str) -> StopResult {
        let mut result = StopResult::default();
        if !self.playing() {
            return result;
        }
        self.playback = None;
        self.module = false;
        self.framecount = 0;
        self.dir.clear();
        if cl_gg {
            result.gg_reset = true;
            let mut secs = (ctx.now - f64::from(self.td_starttime)) as f32;
            if secs == 0.0 {
                secs = 1.0;
            }
            let fps = ctx.frame.wrapping_sub(self.td_startframe).wrapping_sub(1) as f32 / secs;
            let mut lines = vec![
                format!("{:5.2} Half-Life v{version}\n", f64::from(fps)),
                format!("{} Min\n", self.gauge.min),
                format!("{} Max\n", self.gauge.max),
            ];
            lines.extend(self.gauge.samples.iter().enumerate().map(|(i, n)| format!("{n} Second {}\n", i + 1)));
            lines.iter().for_each(|l| ctx.print(l.clone()));
            if let Err(e) = ctx.fs.write("fps.txt", PathId::GameConfig, lines.concat().as_bytes()) {
                log::warn!("fps.txt: {e}");
            }
            result.quit = true;
        }
        if self.timedemo {
            self.timedemo = false;
            let frames = ctx.frame.wrapping_sub(self.td_startframe).wrapping_sub(1);
            let mut secs = (ctx.now - f64::from(self.td_starttime)) as f32;
            if secs == 0.0 {
                secs = 1.0;
            }
            ctx.print(format!("{frames} frames {:5.3} seconds {:5.3} fps\n", f64::from(secs), f64::from(frames as f32 / secs)));
        }
        result
    }

    /// `CL_GGSpeeds(t)`: one-second frame counts while `cl_gg` is on.
    pub fn gg_speeds(&mut self, t: f32, frame: i32, console_up: bool, now: f64) {
        let g = &mut self.gauge;
        if g.last == 0.0 {
            g.last = t;
            return;
        }
        if t - g.last < 1.0 {
            return;
        }
        if console_up {
            self.td_startframe = frame;
            self.td_starttime = now as f32;
        } else {
            let n = frame.wrapping_sub(g.last_frame);
            g.max = g.max.max(n);
            g.min = g.min.min(n);
            if g.samples.len() < GG_SAMPLES {
                g.samples.push(n);
            }
        }
        g.last_frame = frame;
        g.last = t;
    }

    /// `CL_NextDemo`: the `playdemo` line for the next loop entry (`None` = loop off).
    pub fn next_demo(&mut self, out: &mut Output) -> Option<String> {
        if self.demonum == -1 {
            return None;
        }
        if self.demonum as usize >= MAX_DEMOS || self.demos[self.demonum as usize].is_empty() {
            self.demonum = 0;
            if self.demos[0].is_empty() {
                out.push((false, "No demos listed with startdemos\n".into()));
                self.demonum = -1;
                return None;
            }
        }
        let line = format!("playdemo {}\n", self.demos[self.demonum as usize]);
        self.demonum += 1;
        Some(line)
    }

    // ------------------------------------------------------------------ inspection / editing

    fn busy(&self) -> bool {
        self.recording() || self.playing()
    }

    /// `cmd_listdemo`.
    pub fn cmd_listdemo(&self, ctx: &mut Ctx, argv: &[&str]) {
        if self.busy() {
            return ctx.print("Listdemo only available when not running or recording.\n");
        }
        let name = default_extension(&truncate_encoded(argv.get(1).copied().unwrap_or(""), 255), ".dem");
        ctx.print(format!("Demo contents for {name}.\n"));
        let Some(data) = ctx.fs.resolve(&name).ok().and_then(|p| std::fs::read(p).ok()) else {
            return ctx.print("ERROR: couldn't open.\n");
        };
        let header = Header::parse(&data);
        if !header.magic_ok() {
            return ctx.print(format!("{name} is not a demo file\n"));
        }
        ctx.print(format!(
            "Demo file protocols {}N:{}D\nServer protocols at {NET_PROTOCOL}N:{DEMO_PROTOCOL}D\n",
            header.net_protocol(),
            header.demo_protocol()
        ));
        ctx.print(format!("Map name :  {}\n", header.map_name()));
        ctx.print(format!("Game .dll:  {}\n", header.dll_dir()));
        let Some(dir) = read_directory(&data, header.directory_offset()) else {
            let count = directory_count(&data, header.directory_offset());
            return ctx.print(format!("ERROR: demo had bogus # of directory entries:  {count}\n"));
        };
        ctx.print("\n");
        for (i, e) in dir.iter().enumerate() {
            let kind = if e.kind == 0 { "Start segment" } else { "Normal segment" };
            ctx.print(format!(
                "{}:  {kind}\n  \"{:>20}\"\nTime:  {:.2} s.\nFrames:  {}\nSize:  {:.2}K\n",
                i + 1,
                e.description(),
                f64::from(e.track_time),
                e.frames,
                f64::from(e.file_length as f32) * (1.0 / 1024.0)
            ));
            if e.flags != 0 {
                ctx.print("  Flags:\n");
                if e.flags & seg::TITLE != 0 {
                    ctx.print(format!("     Title \"{}\"\n", e.description()));
                }
                if e.flags & seg::PLAY != 0 {
                    ctx.print(format!("     Play {}\n", e.cd_track));
                }
                for (flag, text) in [
                    (seg::FADE_IN_SLOW, "     Fade in slow\n"),
                    (seg::FADE_IN_FAST, "     Fade in fast\n"),
                    (seg::FADE_OUT_SLOW, "     Fade out slow\n"),
                    (seg::FADE_OUT_FAST, "     Fade out fast\n"),
                ] {
                    if e.flags & flag != 0 {
                        ctx.print(text);
                    }
                }
            }
            ctx.print("\n");
        }
    }

    /// The shared front of `swapdemo` / `setdemoinfo`: copy the demo into `<name>.dm2`
    /// (GAMECONFIG) and validate it. `Some((temp path, data, header, directory))`.
    fn edit_copy(ctx: &mut Ctx, name: &str, open_error: String) -> Option<(PathBuf, Vec<u8>, Header, Vec<DemoEntry>)> {
        let Some(data) = ctx.fs.resolve(name).ok().and_then(|p| std::fs::read(p).ok()) else {
            ctx.print(open_error);
            return None;
        };
        let temp = ensure_extension(strip_extension(name), ".dm2");
        let Some(path) = ctx.fs.write_path(&temp, PathId::GameConfig).ok().filter(|p| std::fs::write(p, &data).is_ok()) else {
            ctx.print(format!("Error: couldn't open {name}.\n"));
            return None;
        };
        let header = Header::parse(&data);
        if !header.magic_ok() {
            ctx.print(format!("{name} is not a demo file\n"));
            return None;
        }
        if header.net_protocol() != NET_PROTOCOL || header.demo_protocol() != DEMO_PROTOCOL {
            ctx.print(format!(
                "ERROR: demo protocol outdated\nDemo file protocols {}N:{}D\nServer protocol is at {NET_PROTOCOL}N:{DEMO_PROTOCOL}D\n",
                header.net_protocol(),
                header.demo_protocol()
            ));
            return None;
        }
        let Some(dir) = read_directory(&data, header.directory_offset()) else {
            let count = directory_count(&data, header.directory_offset());
            ctx.print(format!("ERROR: demo had bogus # of directory entries:  {count}\n"));
            return None;
        };
        Some((path, data, header, dir))
    }

    /// Rewrites the directory of the `.dm2` copy, then the CRT replace of the original.
    fn edit_finish(ctx: &mut Ctx, name: &str, path: &PathBuf, mut data: Vec<u8>, header: &Header, dir: &[DemoEntry]) {
        let offset = header.directory_offset() as usize;
        let mut b = Vec::with_capacity(4 + dir.len() * ENTRY_SIZE);
        put_i32(&mut b, dir.len() as i32);
        dir.iter().for_each(|e| e.write(&mut b));
        if data.len() < offset + b.len() {
            data.resize(offset + b.len(), 0);
        }
        data[offset..offset + b.len()].copy_from_slice(&b);
        if let Err(e) = std::fs::write(path, &data) {
            log::warn!("{}: {e}", path.display());
        }
        ctx.print("Replacing old demo with edited version.\n");
        let temp = format!("{}.dm2", strip_extension(name));
        crt_replace(ctx.fs, name, &temp);
    }

    /// `cmd_swapdemo`: swaps two directory entries of a copy.
    pub fn cmd_swapdemo(&self, ctx: &mut Ctx, argv: &[&str]) {
        if self.busy() {
            return ctx.print("Swapdemo only available when not running or recording a demo.\n");
        }
        if argv.len() != 4 {
            return ctx.print("Swapdemo <demoname> <seg#> <seg#>\nSwaps segments, segment 1 cannot be moved\n");
        }
        if argv[1].contains("..") {
            return ctx.print("Relative pathnames are not allowed.\n");
        }
        let (a, b) = (atoi(argv[2]), atoi(argv[3]));
        if a <= 1 {
            return ctx.print("Cannot swap the STARTUP segment.\n");
        }
        let name = default_extension(&truncate_encoded(argv[1], 255), ".dem");
        if !has_extension(&name, "dem") {
            return ctx.print(format!("Error: couldn't open {name}, demo files must have .dem extension.\n"));
        }
        ctx.print(format!("Swapping segment {a} for {b} from demo {name}.\n"));
        let Some((path, data, header, mut dir)) = Self::edit_copy(ctx, &name, format!("Error: couldn't open demo file {name} for swapping.\n")) else {
            return;
        };
        let (ia, ib) = ((a - 1) as usize, b.wrapping_sub(1));
        if ia >= dir.len() || ib < 0 || ib as usize >= dir.len() {
            return ctx.print(format!("ERROR: demo only has {} entries\n", dir.len()));
        }
        dir.swap(ia, ib as usize);
        Self::edit_finish(ctx, &name, &path, data, &header, &dir);
    }

    /// `cmd_setdemoinfo`.
    pub fn cmd_setdemoinfo(&self, ctx: &mut Ctx, argv: &[&str]) {
        if self.busy() {
            return ctx.print("Setdemoinfo only available when not running or recording a demo.\n");
        }
        if argv.len() < 3 {
            ctx.print("Setdemoinfo <demoname> <seg#> <info ...>\n");
            ctx.print("   title \"text\"\n");
            ctx.print("   play tracknum\n");
            ctx.print("   fade <in | out> <fast | slow>\n\n");
            return ctx.print("Use -option to disable, e.g., -title\n");
        }
        if argv[1].contains("..") {
            return ctx.print("Relative pathnames are not allowed.\n");
        }
        let segment = atoi(argv[2]);
        if segment <= 1 {
            return ctx.print("Cannot Setdemoinfo the STARTUP segment.\n");
        }
        let name = default_extension(&truncate_encoded(argv[1], 255), ".dem");
        if !has_extension(&name, "dem") {
            return ctx.print(format!("Error: couldn't open file {name}, demo files must have .dem extension.\n"));
        }
        ctx.print(format!("Setting info for segment {segment} in demo {name}.\n"));
        let Some((path, data, header, mut dir)) = Self::edit_copy(ctx, &name, format!("Error: couldn't open demo file {name} for Setdemoinfo.\n"))
        else {
            return;
        };
        let Some(entry) = dir.get_mut((segment - 1) as usize) else {
            return ctx.print(format!("ERROR: demo only has {} entries\n", dir.len()));
        };
        let mut i = 3;
        while let Some(tok) = argv.get(i).filter(|t| !t.is_empty()) {
            let next = |k: usize| argv.get(i + k).copied().filter(|t| !t.is_empty());
            match tok.to_ascii_uppercase().as_str() {
                "-TITLE" => entry.flags &= !seg::TITLE,
                "-PLAY" => entry.flags &= !seg::PLAY,
                "-FADE" => entry.flags &= !0x78,
                "TITLE" => {
                    match next(1) {
                        Some(text) => {
                            entry.set_description(text);
                            entry.flags |= seg::TITLE;
                        }
                        None => ctx.print("Title flag requires a double-quoted value.\n"),
                    }
                    i += 1;
                }
                "PLAY" => {
                    match next(1) {
                        Some(n) => {
                            entry.cd_track = atoi(n);
                            entry.flags |= seg::PLAY;
                        }
                        None => ctx.print("Play flag requires a cd track #.\n"),
                    }
                    i += 1;
                }
                "FADE" => {
                    match (next(1).map(str::to_ascii_uppercase), next(2).map(str::to_ascii_uppercase)) {
                        (None, _) => ctx.print("Fade flag requires a direction and speed (in fast, e.g.)\n"),
                        (Some(dir), speed) if dir == "IN" || dir == "OUT" => match (dir.as_str(), speed.as_deref()) {
                            ("IN", Some("FAST")) => entry.flags |= seg::FADE_IN_FAST,
                            ("IN", Some("SLOW")) => entry.flags |= seg::FADE_IN_SLOW,
                            ("OUT", Some("FAST")) => entry.flags |= seg::FADE_OUT_FAST,
                            ("OUT", Some("SLOW")) => entry.flags |= seg::FADE_OUT_SLOW,
                            _ => ctx.print("Fade flag requires a speed (fast or slow).\n"),
                        },
                        _ => ctx.print("Fade flag requires a direction (in or out).\n"),
                    }
                    i += 2;
                }
                _ => {
                    ctx.print(format!("Setdemoinfo, unrecognized flag:  {tok}\n"));
                    break;
                }
            }
            i += 1;
        }
        Self::edit_finish(ctx, &name, &path, data, &header, &dir);
    }

    /// `cmd_removedemo`: always stops at its temporary-name check (it tests the demo name for
    /// a `dm2` extension).
    pub fn cmd_removedemo(&self, ctx: &mut Ctx, argv: &[&str]) {
        if self.busy() {
            return ctx.print("Removedemo only available when not running or recording a demo.\n");
        }
        if argv.len() != 3 {
            return ctx.print("removedemo <demoname> <segment to remove>\nSegment 1 cannot be removed\n");
        }
        if argv[1].contains("..") {
            return ctx.print("Relative pathnames are not allowed.\n");
        }
        let segment = atoi(argv[2]);
        if segment <= 1 {
            return ctx.print("Cannot remove the STARTUP segment.\n");
        }
        let name = default_extension(&truncate_encoded(argv[1], 255), ".dem");
        if !has_extension(&name, "dem") {
            return ctx.print(format!("Error: couldn't open demo file {name}, demo files must have .dem extension.\n"));
        }
        ctx.print(format!("Removing segment {segment} from demo {name}.\n"));
        if ctx.fs.resolve(&name).is_err() {
            return ctx.print(format!("Error: couldn't open demo file {name} for segment removal.\n"));
        }
        if !has_extension(&name, "dm2") {
            ctx.print(format!("Error: couldn't open temporary file {name}.\n"));
        }
    }
}

/// `_unlink(name); rename(temp, name)` relative to the original's working directory (the
/// install directory): the GAMECONFIG files are not where these look.
fn crt_replace(fs: &FileSystem, name: &str, temp: &str) {
    let Some(base) = fs.base_dir() else { return };
    let _ = std::fs::remove_file(base.join(name));
    let _ = std::fs::rename(base.join(temp), base.join(name));
}

/// The `count` at the directory offset.
fn directory_count(data: &[u8], offset: i32) -> i32 {
    usize::try_from(offset)
        .ok()
        .and_then(|o| data.get(o..o + 4))
        .map_or(0, |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The directory at `offset`: `1..=1024` entries, else `None`.
fn read_directory(data: &[u8], offset: i32) -> Option<Vec<DemoEntry>> {
    let count = directory_count(data, offset);
    if (count as u32).wrapping_sub(1) > 0x3FF {
        return None;
    }
    let start = usize::try_from(offset).ok()? + 4;
    Some(
        (0..count as usize)
            .map(|i| DemoEntry::read(data.get(start + i * ENTRY_SIZE..start + (i + 1) * ENTRY_SIZE).unwrap_or(&[])))
            .collect(),
    )
}

/// `playdemo`'s speed argument: `(f32)Q_atof`, 1.0 when not above 0.001.
pub fn parse_speed(arg: Option<&str>) -> f32 {
    let speed = arg.map_or(1.0, |a| atof(a) as f32);
    if f64::from(speed) <= 0.001 { 1.0 } else { speed }
}

/// `"Playdemo:  Time scale %f\n"`.
pub fn speed_line(speed: f32) -> String {
    format!("Playdemo:  Time scale {}\n", c_f(f64::from(speed)))
}

/// `playdemo`'s usage.
pub const PLAYDEMO_USAGE: [&str; 2] = ["playdemo <demoname> <replayspeed>: plays a demo\n", "replayspeed == 1.0 default\n"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_have_their_c_sizes() {
        let mut b = Vec::new();
        RefParamsRaw::default().write(&mut b);
        assert_eq!(b.len(), RefParamsRaw::SIZE);
        b.clear();
        DemoInfo::default().write(&mut b);
        assert_eq!(b.len(), INFO_SIZE);
        b.clear();
        DemoEntry::default().write(&mut b);
        assert_eq!(b.len(), ENTRY_SIZE);
        b.clear();
        DemoClientData::default().write(&mut b);
        assert_eq!(b.len(), 32);
    }

    #[test]
    fn extension_rules_match_the_engine() {
        assert_eq!(ensure_extension("foo", ".dem"), "foo.dem");
        assert_eq!(ensure_extension("foo.DEM", ".dem"), "foo.DEM.dem");
        assert_eq!(ensure_extension("foo.dem", ".dem"), "foo.dem");
        assert_eq!(default_extension("foo.DEM", ".dem"), "foo.DEM");
        assert_eq!(default_extension("dir.x/foo", ".dem"), "dir.x/foo.dem");
        assert!(!has_extension("foo.DEM", "dem"));
    }

    fn info_at(ts: f32, x: f32) -> DemoInfo {
        let mut i = DemoInfo { timestamp: ts, ..DemoInfo::default() };
        i.rp.vieworg = [x, 0.0, 0.0];
        i.rp.viewangles = [0.0, 170.0 + x / 10.0, 0.0];
        i.view = [x; 3];
        i.viewmodel = x as i32;
        i
    }

    #[test]
    fn view_replay_blends_the_two_entries_around_the_lagged_clock() {
        let mut d = Demo { speed: 1.0, starttime: 0.0, ..Demo::default() };
        d.ring[0] = info_at(1.0, 100.0);
        d.ring[1] = info_at(0.9, 0.0);
        // t = 0.97 − 0.02 = 0.95: halfway between 0.9 and 1.0.
        let r = d.interpolate_view(0.97);
        assert!((r.rp.vieworg[0] - 50.0).abs() < 0.01, "{:?}", r.rp.vieworg);
        // 170 → 180 across the wrap: 175.
        assert!((r.rp.viewangles[1] - 175.0).abs() < 0.01, "{:?}", r.rp.viewangles);
        assert_eq!(r.viewmodel, Some(100), "the newer entry's view model");
        // No older entry: the newest refdef only, live view model kept.
        d.ring[1] = DemoInfo::default();
        let r = d.interpolate_view(0.97);
        assert_eq!(r.rp.vieworg[0], 100.0);
        assert_eq!(r.view, None);
    }
}
