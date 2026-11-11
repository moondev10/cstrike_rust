//! Core.dll's DELTA codec (docs/re/hltv-world.md §7.2): descriptions come only from
//! `svc_deltadescription` and keep their raw 68-byte field records (re-sent verbatim through
//! `g_MetaDelta`); records are C-layout byte images addressed by field offsets.

use crate::bitbuf::BitBuffer;

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

/// Size of a `delta_description_t` record.
pub const FIELD_RECORD: usize = 0x44;

/// One field: the raw record and its decoded parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub raw: [u8; FIELD_RECORD],
    pub kind: u32,
    pub offset: usize,
    pub bits: u32,
    pub pre: f32,
}

impl Field {
    fn from_raw(raw: [u8; FIELD_RECORD]) -> Self {
        let i32_at = |o: usize| i32::from_le_bytes([raw[o], raw[o + 1], raw[o + 2], raw[o + 3]]);
        Self { kind: i32_at(0) as u32, offset: i32_at(36).max(0) as usize, bits: i32_at(44) as u32, pre: f32::from_bits(i32_at(48) as u32), raw }
    }

    fn base(&self) -> u32 {
        self.kind & !dt::SIGNED
    }

    fn signed(&self) -> bool {
        self.kind & dt::SIGNED != 0
    }
}

/// A `delta_t`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Delta {
    pub fields: Vec<Field>,
}

/// The registry entries in Core's order (§7.2.1: prepended, so the reverse of `DELTA_Init`'s).
pub const REGISTRY_ORDER: [&str; 7] = ["event_t", "weapon_data_t", "usercmd_t", "custom_entity_state_t", "entity_state_player_t", "entity_state_t", "clientdata_t"];

/// The time base and the stored-frame mode (`g_DeltaTime`, `g_LargeTimeBuffers`).
#[derive(Debug, Clone, Copy, Default)]
pub struct DeltaTime {
    pub time: f64,
    pub large: bool,
}

/// `g_MetaDelta`: the `delta_description_t` layout (net-messages §4.2).
pub fn meta() -> Delta {
    let mut fields = Vec::new();
    for (kind, name, off, bits, pre) in [
        (dt::INTEGER, "fieldType", 0i32, 32i32, 1.0f32),
        (dt::STRING, "fieldName", 4, 1, 1.0),
        (dt::INTEGER, "fieldOffset", 36, 16, 1.0),
        (dt::INTEGER, "fieldSize", 40, 8, 1.0),
        (dt::INTEGER, "significant_bits", 44, 8, 1.0),
        (dt::FLOAT, "premultiply", 48, 32, 4000.0),
        (dt::FLOAT, "postmultiply", 52, 32, 4000.0),
    ] {
        let mut raw = [0u8; FIELD_RECORD];
        raw[0..4].copy_from_slice(&kind.to_le_bytes());
        raw[4..4 + name.len()].copy_from_slice(name.as_bytes());
        raw[36..40].copy_from_slice(&off.to_le_bytes());
        raw[44..48].copy_from_slice(&bits.to_le_bytes());
        raw[48..52].copy_from_slice(&pre.to_bits().to_le_bytes());
        fields.push(Field::from_raw(raw));
    }
    Delta { fields }
}

/// `pre < 0.9999f || pre > 1.0001f`.
fn not_one(pre: f32) -> bool {
    !(0.9999..=1.0001).contains(&pre)
}

fn rd<const N: usize>(b: &[u8], o: usize) -> [u8; N] {
    let mut out = [0u8; N];
    if let Some(s) = b.get(o..o + N) {
        out.copy_from_slice(s);
    }
    out
}

fn wr(b: &mut [u8], o: usize, v: &[u8]) {
    if let Some(d) = b.get_mut(o..o + v.len()) {
        d.copy_from_slice(v);
    }
}

fn width(base: u32) -> usize {
    match base {
        dt::BYTE => 1,
        dt::SHORT => 2,
        _ => 4,
    }
}

fn cstr(b: &[u8], o: usize) -> &[u8] {
    let s = b.get(o..).unwrap_or(&[]);
    &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(rd(b, o))
}

/// `MarkSendFields`'s change test for one field.
fn changed(f: &Field, from: &[u8], to: &[u8]) -> bool {
    let o = f.offset;
    match f.base() {
        dt::BYTE => rd::<1>(from, o) != rd::<1>(to, o),
        dt::SHORT => rd::<2>(from, o) != rd::<2>(to, o),
        dt::TIMEWINDOW_8 => (f64::from(f32_at(from, o)) * 100.0) as i32 != (f64::from(f32_at(to, o)) * 100.0) as i32,
        dt::TIMEWINDOW_BIG => (f64::from(f32_at(from, o)) * 1000.0) as i32 != (f64::from(f32_at(to, o)) * 1000.0) as i32,
        dt::STRING => !cstr(from, o).eq_ignore_ascii_case(cstr(to, o)),
        _ => rd::<4>(from, o) != rd::<4>(to, o),
    }
}

impl Delta {
    /// `DELTA_WriteDelta(stream, from, to, force, desc, hdr)`: `false` (nothing written, not even
    /// the header) when no field changed and `force` is off.
    pub fn write(&self, s: &mut BitBuffer, from: &[u8], to: &[u8], force: bool, hdr: Option<&mut Header>, t: DeltaTime) -> bool {
        let marked: Vec<bool> = self.fields.iter().map(|f| changed(f, from, to)).collect();
        if !force && !marked.iter().any(|&m| m) {
            return false;
        }
        let mut mask = [0u8; 8];
        let mut last: i32 = -1;
        for i in (0..self.fields.len().min(64)).rev() {
            if marked[i] {
                mask[i / 8] |= 1 << (i % 8);
                if last == -1 {
                    last = i as i32;
                }
            }
        }
        let nbytes = if last < 0 { 0 } else { (last >> 3) + 1 };
        if let Some(h) = hdr {
            write_header(s, h);
        }
        s.write_bits(nbytes as u32, 3);
        for &m in mask.iter().take(nbytes as usize) {
            s.write_bits(u32::from(m), 8);
        }
        for (f, _) in self.fields.iter().zip(&marked).filter(|(_, m)| **m) {
            write_field(s, f, to, t);
        }
        true
    }

    /// `DELTA_TestDelta(from, to, desc)`: the bit cost of a delta.
    pub fn test(&self, from: &[u8], to: &[u8]) -> i32 {
        let mut bits: i32 = 0;
        let mut highest: i32 = -1;
        for (i, f) in self.fields.iter().enumerate() {
            if f.base() == dt::STRING {
                let t = cstr(to, f.offset);
                if t.is_empty() {
                    continue;
                }
                let fr = from.get(f.offset..).unwrap_or(&[]);
                let differs = t.iter().enumerate().any(|(k, &c)| !fr.get(k).copied().unwrap_or(0).eq_ignore_ascii_case(&c));
                if differs {
                    bits += 8 * (t.len() as i32 + 1);
                }
                continue;
            }
            if changed(f, from, to) {
                bits += f.bits as i32;
                highest = i as i32;
            }
        }
        if highest == -1 { bits } else { bits + (highest & !7) + 8 }
    }

    /// `DELTA_ParseDelta(stream, from, to, desc)`.
    pub fn parse(&self, s: &mut BitBuffer, from: &[u8], to: &mut [u8], t: DeltaTime) {
        let nbytes = s.read_bits(3) as usize;
        let mut mask = [0u8; 8];
        for m in mask.iter_mut().take(nbytes.min(8)) {
            *m = s.read_bits(8) as u8;
        }
        for (i, f) in self.fields.iter().enumerate() {
            let present = i < 64 && (mask[i / 8] >> (i % 8)) & 1 != 0;
            let o = f.offset;
            if !present {
                if f.base() == dt::STRING {
                    let src = cstr(from, o).to_vec();
                    if src.is_empty() {
                        wr(to, o, &[0]);
                    } else {
                        wr(to, o, &src);
                    }
                } else {
                    let w = width(f.base());
                    let v = from.get(o..o + w).map(<[u8]>::to_vec).unwrap_or_default();
                    wr(to, o, &v);
                }
                continue;
            }
            read_field(s, f, to, t);
        }
    }
}

/// The entity header (`hdr`, 0x1C bytes) of `DELTA_WriteHeader`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Header {
    pub numbase: i32,
    pub num: i32,
    pub remove: bool,
    pub custom: bool,
    pub newbl: bool,
    pub newblindex: i32,
    pub full: bool,
    pub offset: i32,
    pub instanced_available: bool,
}

/// `DELTA_WriteHeader(stream, hdr)` (§7.2.4).
pub fn write_header(s: &mut BitBuffer, h: &mut Header) {
    let d = h.num - h.numbase;
    if h.full {
        if d == 1 {
            s.write_bit(true);
        } else {
            s.write_bit(false);
            if (1..=63).contains(&d) {
                s.write_bit(false);
                s.write_bits(d as u32, 6);
            } else {
                s.write_bit(true);
                s.write_bits(h.num as u32, 11);
            }
        }
    } else {
        s.write_bit(h.remove);
        if (1..=63).contains(&d) {
            s.write_bit(false);
            s.write_bits(d as u32, 6);
        } else {
            s.write_bit(true);
            s.write_bits(h.num as u32, 11);
        }
    }
    h.numbase = h.num;
    if !h.remove {
        s.write_bit(h.custom);
        if h.instanced_available {
            if h.newbl {
                s.write_bit(true);
                s.write_bits(h.newblindex as u32, 6);
            } else {
                s.write_bit(false);
            }
        }
        if h.full && !h.newbl {
            if h.offset != 0 {
                s.write_bit(true);
                s.write_bits(h.offset as u32, 6);
            } else {
                s.write_bit(false);
            }
        }
    }
}

/// `x87 _ftol`: truncation to int64, low 32 bits.
fn ftol(x: f32) -> i32 {
    if x.is_finite() { (x as i64) as i32 } else { i32::MIN }
}

/// `WriteMarkedFields`' value of one field.
fn write_field(s: &mut BitBuffer, f: &Field, to: &[u8], t: DeltaTime) {
    let o = f.offset;
    let pre = f.pre;
    let bits = f.bits;
    match f.base() {
        dt::BYTE => {
            if f.signed() {
                let v = (f32::from(rd::<1>(to, o)[0] as i8) * pre) as i32 as i8;
                s.write_sbits(i32::from(v), bits);
            } else {
                let v = (f32::from(rd::<1>(to, o)[0]) * pre) as i32 as u8;
                s.write_bits(u32::from(v), bits);
            }
        }
        dt::SHORT => {
            let raw = rd::<2>(to, o);
            if f.signed() {
                let v = (f32::from(i16::from_le_bytes(raw)) * pre) as i32 as i16;
                s.write_sbits(i32::from(v), bits);
            } else {
                let v = (f32::from(u16::from_le_bytes(raw)) * pre) as i32 as u16;
                s.write_bits(u32::from(v), bits);
            }
        }
        dt::FLOAT => {
            let v = ftol(f32_at(to, o) * pre);
            if f.signed() {
                s.write_sbits(v, bits);
            } else {
                s.write_bits(v as u32, bits);
            }
        }
        dt::INTEGER => {
            let v = i32::from_le_bytes(rd(to, o));
            if f.signed() {
                let w = if not_one(pre) { (v as f32 * pre) as i32 } else { v };
                s.write_sbits(w, bits);
            } else {
                let w = if not_one(pre) { ftol(f64::from(v as u32) as f32 * pre) as u32 } else { v as u32 };
                s.write_bits(w, bits);
            }
        }
        dt::ANGLE => s.write_bit_angle(f32_at(to, o), bits),
        dt::TIMEWINDOW_8 => {
            let x = f32_at(to, o);
            if t.large {
                s.write_float(x);
            } else {
                s.write_sbits((t.time * 100.0 - f64::from(x * 100.0)) as i32, 8);
            }
        }
        dt::TIMEWINDOW_BIG => {
            let x = f32_at(to, o);
            if t.large {
                s.write_float(x);
            } else {
                s.write_sbits((f64::from(pre) * t.time) as i32 - (pre * x) as i32, bits);
            }
        }
        dt::STRING => {
            for &c in cstr(to, o) {
                s.write_bits(u32::from(c), 8);
            }
            s.write_bits(0, 8);
        }
        _ => {}
    }
}

/// `DELTA_ParseDelta`'s decode of one present field.
fn read_field(s: &mut BitBuffer, f: &Field, to: &mut [u8], t: DeltaTime) {
    let o = f.offset;
    let pre = f.pre;
    let bits = f.bits;
    match f.base() {
        dt::BYTE => {
            if f.signed() {
                let mut v = s.read_sbits(bits) as i8;
                if not_one(pre) {
                    v = (f32::from(v) / pre) as i32 as i8;
                }
                wr(to, o, &[v as u8]);
            } else {
                let mut v = s.read_bits(bits) as u8;
                if not_one(pre) {
                    v = (f32::from(v) / pre) as i32 as u8;
                }
                wr(to, o, &[v]);
            }
        }
        dt::SHORT => {
            if f.signed() {
                let mut v = s.read_sbits(bits) as i16;
                if not_one(pre) {
                    v = (f32::from(v) / pre) as i32 as i16;
                }
                wr(to, o, &v.to_le_bytes());
            } else {
                let mut v = s.read_bits(bits) as u16;
                if not_one(pre) {
                    v = (f32::from(v) / pre) as i32 as u16;
                }
                wr(to, o, &v.to_le_bytes());
            }
        }
        dt::FLOAT => {
            let mut x = if f.signed() { s.read_sbits(bits) as f32 } else { f64::from(s.read_bits(bits)) as f32 };
            if not_one(pre) {
                x /= pre;
            }
            wr(to, o, &x.to_le_bytes());
        }
        dt::INTEGER => {
            let v = if f.signed() {
                let v = s.read_sbits(bits);
                if not_one(pre) { ftol(f64::from(v as u32) as f32 / pre) } else { v }
            } else {
                let v = s.read_bits(bits) as i32;
                if not_one(pre) { (v as f32 / pre) as i32 } else { v }
            };
            wr(to, o, &v.to_le_bytes());
        }
        dt::ANGLE => {
            let x = s.read_bit_angle(bits);
            wr(to, o, &x.to_le_bytes());
        }
        dt::TIMEWINDOW_8 => {
            let x = if t.large { s.read_float() } else { t.time as f32 - s.read_sbits(8) as f32 / 100.0 };
            wr(to, o, &x.to_le_bytes());
        }
        dt::TIMEWINDOW_BIG => {
            let x = if t.large {
                s.read_float()
            } else {
                let mut x = t.time as f32 * pre - s.read_sbits(bits) as f32;
                if not_one(pre) {
                    x /= pre;
                }
                x
            };
            wr(to, o, &x.to_le_bytes());
        }
        dt::STRING => {
            let mut k = 0;
            loop {
                let c = s.read_bits(8) as u8;
                if let Some(d) = to.get_mut(o + k) {
                    *d = c;
                }
                k += 1;
                if c == 0 || s.overflowed {
                    break;
                }
            }
        }
        _ => {}
    }
}

/// The delta registry (`DELTA_LookupRegistration`), in Core's order.
#[derive(Debug, Clone)]
pub struct Registry {
    pub entries: Vec<(String, Option<Delta>)>,
}

impl Default for Registry {
    fn default() -> Self {
        Self { entries: REGISTRY_ORDER.iter().map(|n| ((*n).to_owned(), None)).collect() }
    }
}

impl Registry {
    pub fn get(&self, name: &str) -> Option<&Delta> {
        self.entries.iter().find(|(n, _)| n == name).and_then(|(_, d)| d.as_ref())
    }

    /// `ParseDeltaDescription`'s store: `false` for an unregistered name.
    pub fn set(&mut self, name: &str, d: Delta) -> bool {
        match self.entries.iter_mut().find(|(n, _)| n == name) {
            Some(e) => {
                e.1 = Some(d);
                true
            }
            None => false,
        }
    }
}

/// `ParseDeltaDescription`'s field loop: `n` records through `g_MetaDelta` from zero.
pub fn parse_description(s: &mut BitBuffer, n: usize) -> Delta {
    let meta = meta();
    let zero = [0u8; FIELD_RECORD];
    let mut fields = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        let mut raw = [0u8; FIELD_RECORD];
        meta.parse(s, &zero, &mut raw, DeltaTime::default());
        fields.push(Field::from_raw(raw));
    }
    Delta { fields }
}

/// `WriteNewData`'s description body: `fieldCount:16`, each record against zero through the meta delta.
pub fn write_description(s: &mut BitBuffer, d: &Delta) {
    let meta = meta();
    let zero = [0u8; FIELD_RECORD];
    s.write_bits(d.fields.len() as u32, 16);
    for f in &d.fields {
        meta.write(s, &zero, &f.raw, true, None, DeltaTime::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int_field(off: i32, bits: i32) -> Field {
        let mut raw = [0u8; FIELD_RECORD];
        raw[0..4].copy_from_slice(&dt::INTEGER.to_le_bytes());
        raw[36..40].copy_from_slice(&off.to_le_bytes());
        raw[44..48].copy_from_slice(&bits.to_le_bytes());
        raw[48..52].copy_from_slice(&1.0f32.to_bits().to_le_bytes());
        Field::from_raw(raw)
    }

    #[test]
    fn description_round_trips_through_the_meta_delta() {
        let d = Delta { fields: vec![int_field(4, 11), int_field(8, 16)] };
        let mut s = BitBuffer::new(256);
        write_description(&mut s, &d);
        let mut r = BitBuffer::from_bytes(s.bytes());
        let n = r.read_bits(16) as usize;
        assert_eq!(parse_description(&mut r, n), d);
    }

    #[test]
    fn unchanged_records_write_nothing_unless_forced() {
        let d = Delta { fields: vec![int_field(0, 8), int_field(4, 8)] };
        let a = [1u8, 0, 0, 0, 2, 0, 0, 0];
        let mut s = BitBuffer::new(16);
        assert!(!d.write(&mut s, &a, &a, false, None, DeltaTime::default()));
        assert_eq!(s.current_size(), 0);
        let mut b = a;
        b[4] = 7;
        assert!(d.write(&mut s, &a, &b, false, None, DeltaTime::default()));
        let mut r = BitBuffer::from_bytes(s.bytes());
        let mut out = [0u8; 8];
        d.parse(&mut r, &a, &mut out, DeltaTime::default());
        assert_eq!(out, b);
        assert_eq!(d.test(&a, &b), 8 + 8);
    }
}
