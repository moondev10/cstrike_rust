//! `DirectorCmd` (docs/re/demoplayer.md §10, GameUI's copy gameui-demo.md §7) and `TokenLine`
//! (demoplayer.md §12.4).

use crate::bitbuf::BitBuffer;

/// `DirectorCmd` names by type (`GetName`).
pub const NAMES: [&str; 16] = ["NONE", "START", "EVENT", "MODE", "CAMERA", "TIMESCALE", "MESSAGE", "SOUND", "STATUS", "BANNER", "STUFFTEXT", "CHASE", "INEYE", "MAP", "CAMPATH", "WAYPOINTS"];

pub const START: i32 = 1;
pub const EVENT: i32 = 2;
pub const MODE: i32 = 3;
pub const CAMERA: i32 = 4;
pub const TIMESCALE: i32 = 5;
pub const MESSAGE: i32 = 6;
pub const SOUND: i32 = 7;
pub const STATUS: i32 = 8;
pub const BANNER: i32 = 9;
pub const STUFFTEXT: i32 = 10;
pub const CHASE: i32 = 11;
pub const INEYE: i32 = 12;
pub const MAP: i32 = 13;
pub const CAMPATH: i32 = 14;
pub const WAYPOINTS: i32 = 15;

/// One director command: time, type, payload, list index.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DirectorCmd {
    pub time: f32,
    pub kind: i32,
    pub data: Vec<u8>,
    pub index: i32,
}

/// The payload reader (`ReadCoord` unsigned: `(uint16)·0.125`).
struct Rd(BitBuffer);

impl Rd {
    fn new(d: &[u8]) -> Self {
        Self(BitBuffer::from_bytes(d))
    }
    fn byte(&mut self) -> i32 {
        self.0.read_byte()
    }
    fn word(&mut self) -> i32 {
        self.0.read_word()
    }
    fn long(&mut self) -> i32 {
        self.0.read_long()
    }
    fn float(&mut self) -> f32 {
        self.0.read_float()
    }
    fn coord(&mut self) -> f32 {
        (self.0.read_bits(16) as u16) as f32 * 0.125
    }
    fn string(&mut self) -> String {
        gs_common::bytestr::decode(&self.0.read_string())
    }
}

impl DirectorCmd {
    /// `GetName` (no bounds check in the binary; out-of-range types read garbage — `"NONE"` here).
    pub fn name(&self) -> &'static str {
        usize::try_from(self.kind).ok().and_then(|i| NAMES.get(i)).copied().unwrap_or("NONE")
    }

    fn set(&mut self, kind: i32, size: usize, f: impl FnOnce(&mut BitBuffer)) {
        self.kind = kind;
        let mut b = BitBuffer::new(size);
        f(&mut b);
        self.data = b.bytes().to_vec();
        self.data.resize(size, 0);
    }

    /// `WriteToStream`: svc_director, length, type, payload; types outside 1..15 or payloads over 254
    /// bytes are dropped.
    pub fn write_to_stream(&self, s: &mut BitBuffer) {
        if !(1..=15).contains(&self.kind) || self.data.len() > 254 {
            return;
        }
        s.write_byte(51);
        s.write_byte(self.data.len() as u8 + 1);
        s.write_byte(self.kind as u8);
        s.write_buf(&self.data);
    }

    /// `ReadFromStream` (Core, svc_director at ingest): the type, then its payload, whose length
    /// follows the type's wire layout `[INFERENCE: the setters re-encode bit-identically]`.
    pub fn read_from_stream(&mut self, s: &mut BitBuffer) -> bool {
        self.kind = s.read_byte();
        let start = s.byte_pos();
        match self.kind {
            START => {}
            EVENT => s.skip_bytes(8),
            MODE => s.skip_bytes(1),
            CAMERA => s.skip_bytes(15),
            TIMESCALE => s.skip_bytes(4),
            MESSAGE => {
                s.skip_bytes(29);
                s.skip_string();
            }
            SOUND => {
                s.skip_string();
                s.skip_bytes(4);
            }
            STATUS => s.skip_bytes(10),
            BANNER | STUFFTEXT => {
                s.skip_string();
            }
            CHASE => s.skip_bytes(9),
            INEYE => s.skip_bytes(2),
            MAP => s.skip_bytes(10),
            CAMPATH => s.skip_bytes(14),
            WAYPOINTS => {
                let n = s.read_byte().max(0) as usize;
                s.skip_bytes(n * 16);
            }
            _ => return false,
        }
        self.data = s.slice(start, s.byte_pos()).to_vec();
        !s.overflowed
    }

    pub fn set_event_data(&mut self, e1: i32, e2: i32, flags: i32) {
        self.set(EVENT, 8, |b| {
            b.write_word(e1);
            b.write_word(e2);
            b.write_long(flags);
        });
    }
    pub fn set_mode_data(&mut self, m: i32) {
        self.set(MODE, 1, |b| b.write_byte(m as u8));
    }
    pub fn set_camera_data(&mut self, pos: [f32; 3], ang: [f32; 3], fov: f32, target: i32) {
        self.set(CAMERA, 15, |b| {
            pos.iter().chain(&ang).for_each(|&v| b.write_coord(v));
            b.write_byte(fov as i32 as u8);
            b.write_word(target);
        });
    }
    pub fn set_time_scale_data(&mut self, s: f32) {
        self.set(TIMESCALE, 4, |b| b.write_float(s));
    }
    #[allow(clippy::too_many_arguments)]
    pub fn set_message_data(&mut self, effect: i32, color: u32, pos: [f32; 2], fadein: f32, fadeout: f32, hold: f32, fxtime: f32, text: &str) {
        let text = gs_common::bytestr::encode(text);
        self.set(MESSAGE, text.len() + 30, |b| {
            b.write_byte(effect as u8);
            b.write_long(color as i32);
            for v in [pos[0], pos[1], fadein, fadeout, hold, fxtime] {
                b.write_float(v);
            }
            b.write_string(&text);
        });
    }
    pub fn set_sound_data(&mut self, name: &str, vol: f32) {
        let name = gs_common::bytestr::encode(name);
        self.set(SOUND, name.len() + 5, |b| {
            b.write_string(&name);
            b.write_float(vol);
        });
    }
    pub fn set_status_data(&mut self, slots: i32, specs: i32, proxies: i32) {
        self.set(STATUS, 10, |b| {
            b.write_long(slots);
            b.write_long(specs);
            b.write_word(proxies);
        });
    }
    pub fn set_banner_data(&mut self, file: &str) {
        let file = gs_common::bytestr::encode(file);
        self.set(BANNER, file.len() + 1, |b| b.write_string(&file));
    }
    pub fn set_stuff_text_data(&mut self, cmd: &str) {
        let cmd = gs_common::bytestr::encode(cmd);
        self.set(STUFFTEXT, cmd.len() + 1, |b| b.write_string(&cmd));
    }
    pub fn set_chase_data(&mut self, e1: i32, e2: i32, dist: f32, flags: i32) {
        self.set(CHASE, 9, |b| {
            b.write_word(e1);
            b.write_word(e2);
            b.write_float(dist);
            b.write_byte(flags as u8);
        });
    }
    pub fn set_in_eye_data(&mut self, e: i32) {
        self.set(INEYE, 2, |b| b.write_word(e));
    }
    pub fn set_map_data(&mut self, e: i32, angle: f32, dist: f32) {
        self.set(MAP, 10, |b| {
            b.write_word(e);
            b.write_float(angle);
            b.write_float(dist);
        });
    }
    pub fn set_way_point_data(&mut self, pos: [f32; 3], ang: [f32; 3], fov: f32, flags: i32) {
        self.set(CAMPATH, 14, |b| {
            pos.iter().chain(&ang).for_each(|&v| b.write_coord(v));
            b.write_byte(fov as i32 as u8);
            b.write_byte(flags as u8);
        });
    }

    /// `GetCameraData` / `GetWayPointData`: pos, angles, fov, last field (target word / flags byte).
    pub fn camera_data(&self) -> ([f32; 3], [f32; 3], f32, i32) {
        let mut r = Rd::new(&self.data);
        let pos = [r.coord(), r.coord(), r.coord()];
        let ang = [r.coord(), r.coord(), r.coord()];
        let fov = r.byte() as f32;
        let last = if self.kind == CAMPATH { r.byte() } else { r.word() };
        (pos, ang, fov, last)
    }

    /// `GetTimeScaleData`.
    pub fn time_scale_data(&self) -> f32 {
        Rd::new(&self.data).float()
    }

    /// GameUI's `GetEventData` (Word, Word, Long).
    pub fn event_data(&self) -> (i32, i32, i32) {
        let mut r = Rd::new(&self.data);
        (r.word(), r.word(), r.long())
    }
    /// GameUI's `GetModeData`.
    pub fn mode_data(&self) -> i32 {
        Rd::new(&self.data).byte()
    }
    /// GameUI's `GetMessageData`: effect, color, x, y, fadein, fadeout, hold, fxtime, text.
    pub fn message_data(&self) -> (i32, u32, [f32; 6], String) {
        let mut r = Rd::new(&self.data);
        let effect = r.byte();
        let color = r.long() as u32;
        let f = [r.float(), r.float(), r.float(), r.float(), r.float(), r.float()];
        (effect, color, f, r.string())
    }
    /// GameUI's `GetSoundData`.
    pub fn sound_data(&self) -> (String, f32) {
        let mut r = Rd::new(&self.data);
        (r.string(), r.float())
    }
    /// GameUI's `GetStatusData`.
    pub fn status_data(&self) -> (i32, i32, i32) {
        let mut r = Rd::new(&self.data);
        (r.long(), r.long(), r.word())
    }
    /// GameUI's `GetBannerData` / `GetStuffTextData`.
    pub fn string_data(&self) -> String {
        Rd::new(&self.data).string()
    }
    /// GameUI's `GetChaseData`: the entities are read as **bytes** (gameui-demo.md §7.2).
    pub fn chase_data(&self) -> (i32, i32, f32, i32) {
        let mut r = Rd::new(&self.data);
        (r.byte(), r.byte(), r.float(), r.byte())
    }
    /// GameUI's `GetInEyeData`: a byte.
    pub fn in_eye_data(&self) -> i32 {
        Rd::new(&self.data).byte()
    }
    /// GameUI's `GetMapData`: the entity as a byte.
    pub fn map_data(&self) -> (i32, f32, f32) {
        let mut r = Rd::new(&self.data);
        (r.byte(), r.float(), r.float())
    }

    /// GameUI's `ToString()`; `None` for types outside 1..15.
    pub fn to_text(&self) -> Option<String> {
        let n = self.name();
        let mut r = Rd::new(&self.data);
        Some(match self.kind {
            START => n.to_owned(),
            WAYPOINTS | MODE | INEYE => format!("{n} {}", r.byte()),
            EVENT => format!("{n} {} {} {}", r.word(), r.word(), r.long()),
            STATUS => format!("{n} {} {} {}", r.long(), r.long(), r.word()),
            CHASE => format!("{n} {} {} {:.1} {}", r.byte(), r.byte(), r.float(), r.byte()),
            MAP => format!("{n} {} {:.1} {:.1}", r.byte(), r.float(), r.float()),
            CAMERA | CAMPATH => {
                let (p, a, fov, last) = self.camera_data();
                format!("{n} ({:.1} {:.1} {:.1}) ({:.1} {:.1} {:.1}) {:.1} {}", p[0], p[1], p[2], a[0], a[1], a[2], fov, last)
            }
            TIMESCALE => format!("{n} {:.2}", r.float()),
            MESSAGE => {
                let (effect, color, f, text) = self.message_data();
                format!("{n} \"{text}\" {effect} {color:x} ({:.2} {:.2}) {:.1}, {:.1} {:.1} {:.1}", f[0], f[1], f[2], f[3], f[4], f[5])
            }
            SOUND => {
                let (name, vol) = self.sound_data();
                format!("{n} \"{name}\" {vol:.2}")
            }
            BANNER | STUFFTEXT => format!("{n} \"{}\"", r.string()),
            _ => return None,
        })
    }
}

/// `TokenLine::SetLine` + `CountToken`/`GetToken`.
pub fn tokens(line: &str) -> Vec<String> {
    let b = line.as_bytes();
    if b.len() >= 0x7FF {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() && out.len() < 128 {
        while i < b.len() && (b[i] <= 0x20 || b[i] > 0x7E) {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        if b[i] == b'"' {
            i += 1;
            let s = i;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            out.push(gs_common::bytestr::decode(&b[s..i]));
            i += 1;
        } else {
            let s = i;
            while i < b.len() && (0x21..=0x7E).contains(&b[i]) {
                i += 1;
            }
            out.push(gs_common::bytestr::decode(&b[s..i]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_payload_round_trips_and_writes_svc_director() {
        let mut c = DirectorCmd::default();
        c.set_camera_data([10.0, -2.0, 3.5], [0.0, 90.0, 0.0], 90.0, 3);
        assert_eq!(c.data.len(), 15);
        let mut s = BitBuffer::new(64);
        c.write_to_stream(&mut s);
        let mut r = BitBuffer::from_bytes(s.bytes());
        assert_eq!(r.read_byte(), 51);
        assert_eq!(r.read_byte(), 16);
        let mut back = DirectorCmd::default();
        assert!(back.read_from_stream(&mut r));
        assert_eq!(back.data, c.data);
        assert_eq!(back.camera_data().0[1], 8190.0);
    }

    #[test]
    fn token_line_splits_quotes() {
        assert_eq!(tokens("dem_speed  \"2 5\" x"), vec!["dem_speed", "2 5", "x"]);
    }
}
