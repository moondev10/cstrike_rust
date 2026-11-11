//! Delta codec (docs/re/net-messages.md §4). Descriptions come from the server
//! (`svc_deltadescription`); records are encoded against C-layout byte images of the structs,
//! addressed by the field offsets the server sends.

use crate::bitbuf::{MsgReader, MsgWriter};

pub mod dt {
    pub const BYTE: u32 = 1;
    pub const SHORT: u32 = 2;
    pub const FLOAT: u32 = 4;
    pub const INTEGER: u32 = 8;
    pub const ANGLE: u32 = 16;
    pub const TIMEWINDOW_8: u32 = 32;
    pub const TIMEWINDOW_BIG: u32 = 64;
    pub const STRING: u32 = 128;
    pub const SIGNED: u32 = 0x8000_0000;
}

/// C struct sizes of the delta-encoded records.
pub mod size {
    pub const ENTITY_STATE: usize = 340;
    pub const CLIENT_DATA: usize = 476;
    pub const WEAPON_DATA: usize = 88;
    pub const USERCMD: usize = 52;
    pub const EVENT_ARGS: usize = 72;
    pub const DELTA_DESCRIPTION: usize = 68;
}

/// Names the server describes in `svc_deltadescription`.
pub mod names {
    pub const CLIENT_DATA: &str = "clientdata_t";
    pub const ENTITY_STATE: &str = "entity_state_t";
    pub const ENTITY_STATE_PLAYER: &str = "entity_state_player_t";
    pub const CUSTOM_ENTITY_STATE: &str = "custom_entity_state_t";
    pub const USERCMD: &str = "usercmd_t";
    pub const WEAPON_DATA: &str = "weapon_data_t";
    pub const EVENT: &str = "event_t";
}

const MAX_FIELDS: usize = 64;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeltaField {
    pub kind: u32,
    pub name: String,
    pub offset: i32,
    pub size: i32,
    pub bits: u32,
    pub pre: f32,
    pub post: f32,
    /// `recvcount`: fields of this kind received since the description arrived (`delta_stats`).
    pub recv: std::cell::Cell<i32>,
}

impl DeltaField {
    fn base(&self) -> u32 {
        self.kind & !dt::SIGNED
    }

    fn signed(&self) -> bool {
        self.kind & dt::SIGNED != 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Delta {
    pub name: String,
    pub fields: Vec<DeltaField>,
}

/// "≈1.0": multipliers within this range are not applied.
fn near_one(m: f32) -> bool {
    (0.9999..=1.0001).contains(&m)
}

fn get<const N: usize>(buf: &[u8], off: usize) -> [u8; N] {
    let mut out = [0u8; N];
    if let Some(s) = buf.get(off..off + N) {
        out.copy_from_slice(s);
    }
    out
}

fn put(buf: &mut [u8], off: usize, bytes: &[u8]) {
    if let Some(d) = buf.get_mut(off..off + bytes.len()) {
        d.copy_from_slice(bytes);
    }
}

fn get_f32(buf: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(get(buf, off))
}

fn get_i32(buf: &[u8], off: usize) -> i32 {
    i32::from_le_bytes(get(buf, off))
}

/// Bytes a non-string field occupies.
fn width(base: u32) -> usize {
    match base {
        dt::BYTE => 1,
        dt::SHORT => 2,
        _ => 4,
    }
}

fn c_strlen(buf: &[u8], off: usize) -> usize {
    buf.get(off..).map_or(0, |s| s.iter().position(|&b| b == 0).unwrap_or(s.len()))
}

impl Delta {
    /// `DELTA_ParseDelta`: decodes one record from `r` (in bit mode) into `to`, starting from `from`.
    /// `time` is `cl.mtime[0]` for time-window fields.
    pub fn decode(&self, r: &mut MsgReader, from: &[u8], to: &mut [u8], time: f64) {
        debug_assert_eq!(from.len(), to.len());
        let size = to.len();
        let nbytes = r.read_bits(3) as usize;
        let mut mask = [0u8; 8];
        for byte in mask.iter_mut().take(nbytes) {
            *byte = r.read_bits(8) as u8;
        }
        for (i, f) in self.fields.iter().enumerate().take(MAX_FIELDS) {
            let Ok(off) = usize::try_from(f.offset) else { continue };
            if off >= size {
                continue;
            }
            let present = (mask[i / 8] >> (i % 8)) & 1 != 0;
            let base = f.base();
            if !present {
                copy_field(from, to, off, base);
                continue;
            }
            f.recv.set(f.recv.get().wrapping_add(1));
            decode_field(r, f, to, off, time);
        }
    }

    /// `DELTA_WriteDelta`: encodes `to` against `from`. Returns false (and writes nothing) when
    /// nothing changed and `force` is off.
    pub fn encode(&self, w: &mut MsgWriter, from: &[u8], to: &[u8], force: bool, time: f64) -> bool {
        let size = to.len();
        let mut send = [false; MAX_FIELDS];
        let mut highest = None;
        for (i, f) in self.fields.iter().enumerate().take(MAX_FIELDS) {
            let Ok(off) = usize::try_from(f.offset) else { continue };
            if off >= size {
                continue;
            }
            let changed = match f.base() {
                dt::TIMEWINDOW_8 => (get_f32(from, off) * 100.0) as i32 != (get_f32(to, off) * 100.0) as i32,
                dt::TIMEWINDOW_BIG => (get_f32(from, off) * 1000.0) as i32 != (get_f32(to, off) * 1000.0) as i32,
                dt::STRING => {
                    let (a, b) = (&from[off..off + c_strlen(from, off)], &to[off..off + c_strlen(to, off)]);
                    !a.eq_ignore_ascii_case(b)
                }
                base => from.get(off..off + width(base)) != to.get(off..off + width(base)),
            };
            if changed {
                send[i] = true;
                highest = Some(i);
            }
        }
        if highest.is_none() && !force {
            return false;
        }
        let nbytes = highest.map_or(0, |h| (h >> 3) + 1);
        w.write_bits(nbytes as u32, 3);
        for b in 0..nbytes {
            let byte = (0..8).fold(0u32, |acc, k| acc | (u32::from(send[b * 8 + k]) << k));
            w.write_bits(byte, 8);
        }
        for (i, f) in self.fields.iter().enumerate().take(MAX_FIELDS) {
            if send[i] {
                encode_field(w, f, to, f.offset as usize, time);
            }
        }
        true
    }
}

fn copy_field(from: &[u8], to: &mut [u8], off: usize, base: u32) {
    if base == dt::STRING {
        let len = (c_strlen(from, off) + 1).min(to.len() - off);
        to[off..off + len].copy_from_slice(&from[off..off + len]);
        return;
    }
    let w = width(base);
    if off + w <= to.len() {
        to[off..off + w].copy_from_slice(&from[off..off + w]);
    }
}

fn decode_field(r: &mut MsgReader, f: &DeltaField, to: &mut [u8], off: usize, time: f64) {
    let (pre, post, bits) = (f.pre, f.post, f.bits);
    let scale_int = |v: i32, unsigned_conv: bool| -> i32 {
        let mut v = v;
        if !near_one(pre) {
            v = if unsigned_conv { ((v as u32) as f64 / f64::from(pre)) as i64 as i32 } else { (v as f32 / pre) as i32 };
        }
        if !near_one(post) {
            v = if unsigned_conv { ((v as u32) as f64 * f64::from(post)) as i64 as i32 } else { (v as f32 * post) as i32 };
        }
        v
    };
    match f.base() {
        dt::BYTE => {
            let v = if f.signed() { i32::from(r.read_sbitlong(bits) as i8) } else { i32::from(r.read_bits(bits) as u8) };
            put(to, off, &[scale_int(v, false) as u8]);
        }
        dt::SHORT => {
            if off + 2 > to.len() {
                return;
            }
            let v = if f.signed() { i32::from(r.read_sbitlong(bits) as i16) } else { i32::from(r.read_bits(bits) as u16) };
            put(to, off, &(scale_int(v, false) as u16).to_le_bytes());
        }
        dt::FLOAT => {
            let mut x = if f.signed() { r.read_sbitlong(bits) as f32 } else { r.read_bits(bits) as f32 };
            if !near_one(pre) {
                x /= pre;
            }
            if !near_one(post) {
                x *= post;
            }
            put(to, off, &x.to_le_bytes());
        }
        dt::INTEGER => {
            let v = if f.signed() {
                scale_int(r.read_sbitlong(bits), true)
            } else {
                let raw = r.read_bits(bits);
                let mut v = raw as i32;
                if !near_one(pre) {
                    v = (raw as f32 / pre) as i32;
                }
                if !near_one(post) {
                    v = (v as u32 as f32 * post) as i32;
                }
                v
            };
            put(to, off, &v.to_le_bytes());
        }
        dt::ANGLE => put(to, off, &r.read_bit_angle(bits).to_le_bytes()),
        dt::TIMEWINDOW_8 => {
            let d = r.read_sbitlong(8);
            let x = (((time * 100.0) as i32) as f32 - d as f32) / 100.0;
            put(to, off, &x.to_le_bytes());
        }
        dt::TIMEWINDOW_BIG => {
            let d = r.read_sbitlong(bits);
            let mut x = (f64::from(pre) * time) as f32 - d as f32;
            if !near_one(pre) {
                x /= pre;
            }
            put(to, off, &x.to_le_bytes());
        }
        dt::STRING => {
            let mut k = off;
            loop {
                let c = r.read_bits(8) as u8;
                if k >= to.len() {
                    if let Some(last) = to.last_mut() {
                        *last = 0;
                    }
                    break;
                }
                to[k] = c;
                k += 1;
                if c == 0 || r.bad() {
                    break;
                }
            }
        }
        other => log::warn!("delta field {}: unparseable type {other:#x}", f.name),
    }
}

fn encode_field(w: &mut MsgWriter, f: &DeltaField, src: &[u8], off: usize, time: f64) {
    let (pre, bits, signed) = (f.pre, f.bits, f.signed());
    match f.base() {
        dt::BYTE => {
            let raw = src[off];
            if signed {
                w.write_sbitlong(i32::from(((raw as i8) as f32 * pre) as i32 as i8), bits);
            } else {
                w.write_bits(u32::from((f32::from(raw) * pre) as i32 as u8), bits);
            }
        }
        dt::SHORT => {
            let raw = u16::from_le_bytes(get(src, off));
            if signed {
                w.write_sbitlong(i32::from(((raw as i16) as f32 * pre) as i32 as i16), bits);
            } else {
                w.write_bits(u32::from((f32::from(raw) * pre) as i32 as u16), bits);
            }
        }
        dt::FLOAT => {
            let v = (get_f32(src, off) * pre) as i32;
            if signed { w.write_sbitlong(v, bits) } else { w.write_bits(v as u32, bits) }
        }
        dt::INTEGER => {
            let raw = get_i32(src, off);
            let v = if near_one(pre) {
                raw
            } else if signed {
                (raw as f32 * pre) as i32
            } else {
                (raw as u32 as f32 * pre) as i32
            };
            if signed { w.write_sbitlong(v, bits) } else { w.write_bits(v as u32, bits) }
        }
        dt::ANGLE => w.write_bit_angle(get_f32(src, off), bits),
        dt::TIMEWINDOW_8 => {
            w.write_sbitlong((time * 100.0) as i32 - (get_f32(src, off) * 100.0) as i32, 8);
        }
        dt::TIMEWINDOW_BIG => {
            w.write_sbitlong((f64::from(pre) * time) as i32 - (get_f32(src, off) * pre) as i32, bits);
        }
        dt::STRING => {
            for &c in &src[off..off + c_strlen(src, off)] {
                w.write_bits(u32::from(c), 8);
            }
            w.write_bits(0, 8);
        }
        _ => {}
    }
}

/// `g_MetaDelta`: how `svc_deltadescription` encodes each `delta_description_t`.
fn meta() -> Delta {
    let field = |kind, name: &str, offset, bits, pre| DeltaField {
        kind,
        name: name.to_owned(),
        offset,
        size: 1,
        bits,
        pre,
        post: 1.0,
        recv: Default::default(),
    };
    Delta {
        name: "g_MetaDelta".to_owned(),
        fields: vec![
            field(dt::INTEGER, "fieldType", 0, 32, 1.0),
            field(dt::STRING, "fieldName", 4, 1, 1.0),
            field(dt::INTEGER, "fieldOffset", 36, 16, 1.0),
            field(dt::INTEGER, "fieldSize", 40, 8, 1.0),
            field(dt::INTEGER, "significant_bits", 44, 8, 1.0),
            field(dt::FLOAT, "premultiply", 48, 32, 4000.0),
            field(dt::FLOAT, "postmultiply", 52, 32, 4000.0),
        ],
    }
}

/// Parses the bit-stream part of `svc_deltadescription` (after the name string).
pub fn read_description(r: &mut MsgReader, name: String) -> Delta {
    let meta = meta();
    let zero = [0u8; size::DELTA_DESCRIPTION];
    r.start_bits();
    let count = r.read_bits(16) as usize;
    let mut fields = Vec::with_capacity(count.min(MAX_FIELDS * 4));
    for _ in 0..count {
        if r.bad() {
            break;
        }
        let mut raw = [0u8; size::DELTA_DESCRIPTION];
        meta.decode(r, &zero, &mut raw, 0.0);
        let name_end = raw[4..36].iter().position(|&b| b == 0).unwrap_or(32);
        fields.push(DeltaField {
            kind: get_i32(&raw, 0) as u32,
            name: String::from_utf8_lossy(&raw[4..4 + name_end]).into_owned(),
            offset: get_i32(&raw, 36),
            size: i32::from(i16::from_le_bytes(get(&raw, 40))),
            bits: get_i32(&raw, 44) as u32,
            pre: get_f32(&raw, 48),
            post: get_f32(&raw, 52),
            recv: Default::default(),
        });
    }
    r.end_bits();
    Delta { name, fields }
}

/// Encodes a description the way the server does (used by tests and tools).
pub fn write_description(w: &mut MsgWriter, delta: &Delta) {
    let meta = meta();
    let zero = [0u8; size::DELTA_DESCRIPTION];
    w.write_string(&delta.name);
    w.start_bits();
    w.write_bits(delta.fields.len() as u32, 16);
    for f in &delta.fields {
        let mut raw = [0u8; size::DELTA_DESCRIPTION];
        raw[0..4].copy_from_slice(&(f.kind as i32).to_le_bytes());
        let n = f.name.len().min(31);
        raw[4..4 + n].copy_from_slice(&f.name.as_bytes()[..n]);
        raw[36..40].copy_from_slice(&f.offset.to_le_bytes());
        raw[40..44].copy_from_slice(&f.size.to_le_bytes());
        raw[44..48].copy_from_slice(&(f.bits as i32).to_le_bytes());
        raw[48..52].copy_from_slice(&f.pre.to_le_bytes());
        raw[52..56].copy_from_slice(&f.post.to_le_bytes());
        meta.encode(w, &zero, &raw, true, 0.0);
    }
    w.end_bits();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(kind: u32, name: &str, offset: i32, bits: u32, pre: f32) -> DeltaField {
        DeltaField { kind, name: name.into(), offset, size: 1, bits, pre, post: 1.0, recv: Default::default() }
    }

    /// CS delta.lst `usercmd_t` in description order.
    fn usercmd_delta() -> Delta {
        Delta {
            name: names::USERCMD.into(),
            fields: vec![
                field(dt::SHORT, "lerp_msec", 0, 9, 1.0),
                field(dt::BYTE, "msec", 2, 8, 1.0),
                field(dt::ANGLE, "viewangles[1]", 8, 16, 1.0),
                field(dt::ANGLE, "viewangles[0]", 4, 16, 1.0),
                field(dt::SHORT, "buttons", 30, 16, 1.0),
                field(dt::FLOAT | dt::SIGNED, "forwardmove", 16, 12, 1.0),
                field(dt::BYTE, "lightlevel", 28, 8, 1.0),
                field(dt::FLOAT | dt::SIGNED, "sidemove", 20, 12, 1.0),
                field(dt::FLOAT | dt::SIGNED, "upmove", 24, 12, 1.0),
                field(dt::BYTE, "impulse", 32, 8, 1.0),
                field(dt::ANGLE, "viewangles[2]", 12, 16, 1.0),
                field(dt::INTEGER, "impact_index", 36, 6, 1.0),
                field(dt::FLOAT | dt::SIGNED, "impact_position[0]", 40, 16, 8.0),
            ],
        }
    }

    #[test]
    fn description_round_trips_through_meta_delta() {
        let d = usercmd_delta();
        let mut w = MsgWriter::new();
        write_description(&mut w, &d);
        let mut r = MsgReader::new(w.as_bytes());
        let name = r.read_string();
        let parsed = read_description(&mut r, name);
        assert!(!r.bad() && r.at_end());
        assert_eq!(parsed, d);
    }

    #[test]
    fn usercmd_round_trip_and_unchanged_fields_copy() {
        let d = usercmd_delta();
        let mut cmd = [0u8; size::USERCMD];
        cmd[2] = 16; // msec
        cmd[8..12].copy_from_slice(&90.0f32.to_le_bytes()); // yaw
        cmd[16..20].copy_from_slice(&(-250.0f32).to_le_bytes()); // forwardmove
        cmd[30..32].copy_from_slice(&0x0401u16.to_le_bytes()); // buttons
        cmd[40..44].copy_from_slice(&12.5f32.to_le_bytes());
        let zero = [0u8; size::USERCMD];
        let mut w = MsgWriter::new();
        w.start_bits();
        assert!(d.encode(&mut w, &zero, &cmd, true, 0.0));
        w.end_bits();
        let mut r = MsgReader::new(w.as_bytes());
        let mut out = [0u8; size::USERCMD];
        r.start_bits();
        d.decode(&mut r, &zero, &mut out, 0.0);
        r.end_bits();
        assert!(!r.bad() && r.at_end());
        assert_eq!(out, cmd);
    }

    #[test]
    fn no_change_writes_nothing_unless_forced() {
        let d = usercmd_delta();
        let cmd = [0u8; size::USERCMD];
        let mut w = MsgWriter::new();
        assert!(!d.encode(&mut w, &cmd, &cmd, false, 0.0));
        assert!(w.is_empty());
        assert!(d.encode(&mut w, &cmd, &cmd, true, 0.0));
        w.end_bits();
        assert_eq!(w.as_bytes(), &[0], "forced empty delta is a 3-bit zero mask length");
    }

    #[test]
    fn signed_integer_with_multiplier_decodes_through_unsigned() {
        // CS weapon_data_t.iuser1 has pre 128: -1 * 128 = -128 on the wire.
        let d = Delta { name: "w".into(), fields: vec![field(dt::INTEGER | dt::SIGNED, "iuser1", 0, 10, 128.0)] };
        let mut w = MsgWriter::new();
        w.write_bits(1, 3);
        w.write_bits(1, 8);
        w.write_sbitlong(-128, 10);
        w.end_bits();
        let mut r = MsgReader::new(w.as_bytes());
        let mut out = [0u8; 4];
        d.decode(&mut r, &[0; 4], &mut out, 0.0);
        let expected = ((-128i32 as u32) as f64 / 128.0) as i64 as i32;
        assert_eq!(i32::from_le_bytes(out), expected);
        assert_eq!(expected, 33554431);
    }

    #[test]
    fn timewindow_8_is_relative_to_server_time() {
        let d = Delta { name: "e".into(), fields: vec![field(dt::TIMEWINDOW_8, "animtime", 0, 8, 1.0)] };
        let mut w = MsgWriter::new();
        w.write_bits(1, 3);
        w.write_bits(1, 8);
        w.write_sbitlong(25, 8);
        w.end_bits();
        let mut r = MsgReader::new(w.as_bytes());
        let mut out = [0u8; 4];
        d.decode(&mut r, &[0; 4], &mut out, 10.0);
        assert_eq!(f32::from_le_bytes(out), 9.75);
    }
}
