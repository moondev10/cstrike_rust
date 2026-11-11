//! `MSG_Read*` / `MSG_Write*`: byte mode and embedded LSB-first bit streams
//! (docs/re/net-messages.md §1). Reads never fail; an overrun sets a sticky `bad` flag like the
//! original `msg_badread`, and callers check it at message boundaries.

/// Longest string `MSG_ReadString` stores.
const MAX_STRING: usize = 8191;
const MAX_STRING_LINE: usize = 2047;

#[derive(Debug, Clone, Copy)]
struct BitCursor {
    /// Byte where the bit stream started.
    start: usize,
    /// Bits consumed since `start`.
    bit: usize,
}

#[derive(Debug, Clone)]
pub struct MsgReader<'a> {
    data: &'a [u8],
    pos: usize,
    bad: bool,
    bits: Option<BitCursor>,
}

impl<'a> MsgReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, bad: false, bits: None }
    }

    pub fn bad(&self) -> bool {
        self.bad
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// Bits consumed in the current bit stream (0 outside one).
    pub fn bits_read(&self) -> usize {
        self.bits.map_or(0, |c| c.bit)
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        debug_assert!(self.bits.is_none(), "byte read inside a bit stream");
        match self.data.get(self.pos..self.pos + n) {
            Some(s) => {
                self.pos += n;
                Some(s)
            }
            None => {
                self.bad = true;
                None
            }
        }
    }

    pub fn read_u8(&mut self) -> u8 {
        self.take(1).map_or(0, |b| b[0])
    }

    pub fn read_i8(&mut self) -> i8 {
        self.read_u8() as i8
    }

    pub fn read_i16(&mut self) -> i16 {
        self.take(2).map_or(-1, |b| i16::from_le_bytes([b[0], b[1]]))
    }

    pub fn read_u16(&mut self) -> u16 {
        self.take(2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn read_i32(&mut self) -> i32 {
        self.take(4).map_or(-1, |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_f32(&mut self) -> f32 {
        self.take(4).map_or(0.0, |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_bytes(&mut self, n: usize) -> &'a [u8] {
        self.take(n).unwrap_or(&[])
    }

    /// `MSG_ReadCoord`: int16 / 8.
    pub fn read_coord(&mut self) -> f32 {
        f32::from(self.read_i16()) * 0.125
    }

    pub fn read_coord3(&mut self) -> [f32; 3] {
        [self.read_coord(), self.read_coord(), self.read_coord()]
    }

    /// `MSG_ReadAngle`: int8 * 360/256.
    pub fn read_angle(&mut self) -> f32 {
        f32::from(self.read_i8()) * 1.40625
    }

    /// `MSG_ReadHiresAngle`: int16 * 360/65536.
    pub fn read_hires_angle(&mut self) -> f32 {
        f32::from(self.read_i16()) * 0.005_493_164
    }

    /// `MSG_ReadString`: terminated (and consumed) by 0x00 or 0xFF; stops after 8191 characters
    /// without consuming the rest, exactly like the original.
    pub fn read_string(&mut self) -> String {
        self.read_terminated(MAX_STRING, false)
    }

    /// `MSG_ReadStringLine`: also stops at (and consumes) `\n`; 2047 characters max.
    pub fn read_string_line(&mut self) -> String {
        self.read_terminated(MAX_STRING_LINE, true)
    }

    fn read_terminated(&mut self, limit: usize, line: bool) -> String {
        let mut out = Vec::new();
        while out.len() < limit {
            let Some(&c) = self.data.get(self.pos) else {
                self.bad = true;
                break;
            };
            self.pos += 1;
            if c == 0 || c == 0xFF || (line && c == b'\n') {
                break;
            }
            out.push(c);
        }
        gs_common::bytestr::decode(&out)
    }

    // ---------------------------------------------------------------- bit mode

    /// `MSG_StartBitReading`.
    pub fn start_bits(&mut self) {
        if self.pos + 1 > self.data.len() {
            self.bad = true;
        }
        self.bits = Some(BitCursor { start: self.pos, bit: 0 });
    }

    /// `MSG_EndBitReading`: the stream occupies `max(1, ceil(bits/8))` whole bytes.
    pub fn end_bits(&mut self) {
        let Some(cursor) = self.bits.take() else { return };
        self.pos = cursor.start + cursor.bit.div_ceil(8).max(1);
        if self.pos > self.data.len() {
            self.bad = true;
        }
    }

    fn cursor(&mut self) -> &mut BitCursor {
        let start = self.pos;
        self.bits.get_or_insert(BitCursor { start, bit: 0 })
    }

    /// `MSG_ReadBits`: LSB-first, `n` in 0..=32. Past the end it returns 1 and sets `bad`.
    pub fn read_bits(&mut self, n: u32) -> u32 {
        if self.bad {
            return 1;
        }
        let len = self.data.len();
        let cursor = *self.cursor();
        let first = cursor.start * 8 + cursor.bit;
        if first + n as usize > len * 8 {
            self.bad = true;
            return 1;
        }
        let mut value: u64 = 0;
        for k in 0..n as usize {
            let bit = first + k;
            let b = (self.data[bit / 8] >> (bit % 8)) & 1;
            value |= u64::from(b) << k;
        }
        self.cursor().bit += n as usize;
        value as u32
    }

    pub fn read_bit(&mut self) -> bool {
        self.read_bits(1) != 0
    }

    /// `MSG_PeekBits`.
    pub fn peek_bits(&mut self, n: u32) -> u32 {
        let (saved_bits, saved_bad) = (self.bits, self.bad);
        let v = self.read_bits(n);
        self.bits = saved_bits;
        self.bad = saved_bad;
        v
    }

    /// `MSG_ReadSBitLong`: sign bit, then `n-1` magnitude bits.
    pub fn read_sbitlong(&mut self, n: u32) -> i32 {
        let negative = self.read_bit();
        let magnitude = self.read_bits(n.saturating_sub(1)) as i32;
        if negative { magnitude.wrapping_neg() } else { magnitude }
    }

    /// `MSG_ReadBitAngle`.
    pub fn read_bit_angle(&mut self, n: u32) -> f32 {
        let scale = 360.0 / (1u64 << n) as f32;
        self.read_bits(n) as f32 * scale
    }

    /// `MSG_ReadBitCoord`: int/frac flags, sign, 12-bit integer, 3-bit eighths.
    pub fn read_bit_coord(&mut self) -> f32 {
        let int_flag = self.read_bit();
        let frac_flag = self.read_bit();
        if !int_flag && !frac_flag {
            return 0.0;
        }
        let negative = self.read_bit();
        let int = if int_flag { self.read_bits(12) } else { 0 };
        let frac = if frac_flag { self.read_bits(3) } else { 0 };
        let value = int as f32 + frac as f32 * 0.125;
        if negative { -value } else { value }
    }

    /// `MSG_ReadBitVec3Coord`; unflagged components are 0.
    pub fn read_bit_vec3_coord(&mut self) -> [f32; 3] {
        let flags = [self.read_bit(), self.read_bit(), self.read_bit()];
        flags.map(|f| if f { self.read_bit_coord() } else { 0.0 })
    }

    /// `MSG_ReadBitString`: 8-bit characters until a 0 (0xFF does not terminate).
    pub fn read_bit_string(&mut self) -> String {
        let mut out = Vec::new();
        loop {
            let c = self.read_bits(8) as u8;
            if c == 0 || self.bad {
                break;
            }
            if out.len() < MAX_STRING {
                out.push(c);
            }
        }
        gs_common::bytestr::decode(&out)
    }

    /// `MSG_ReadBitData`.
    pub fn read_bit_data(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.read_bits(8) as u8).collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct MsgWriter {
    buf: Vec<u8>,
    bits: Option<(usize, usize)>,
}

impl MsgWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.buf
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn write_i16(&mut self, v: i16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_bytes(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// `MSG_WriteString`: the string's original bytes plus terminating NUL.
    pub fn write_string(&mut self, s: &str) {
        self.buf.extend_from_slice(&gs_common::bytestr::encode(s));
        self.buf.push(0);
    }

    /// `MSG_StartBitWriting`.
    pub fn start_bits(&mut self) {
        self.bits = Some((self.buf.len(), 0));
    }

    /// `MSG_EndBitWriting`: pads to whole bytes, at least one.
    pub fn end_bits(&mut self) {
        let Some((start, bit)) = self.bits.take() else { return };
        self.buf.resize(start + bit.div_ceil(8).max(1), 0);
    }

    /// `MSG_WriteBits`: saturates (not masks) values that do not fit in `n` bits.
    pub fn write_bits(&mut self, value: u32, n: u32) {
        let value = if n < 32 && value >= (1 << n) { (1u32 << n) - 1 } else { value };
        let start = self.buf.len();
        let (s, bit) = *self.bits.get_or_insert((start, 0));
        for k in 0..n as usize {
            let pos = bit + k;
            let byte = s + pos / 8;
            if byte >= self.buf.len() {
                self.buf.push(0);
            }
            if (value >> k) & 1 != 0 {
                self.buf[byte] |= 1 << (pos % 8);
            }
        }
        self.bits = Some((s, bit + n as usize));
    }

    pub fn write_bit(&mut self, on: bool) {
        self.write_bits(u32::from(on), 1);
    }

    /// `MSG_WriteSBitLong`: clamps to the representable range, sign bit then magnitude.
    pub fn write_sbitlong(&mut self, value: i32, n: u32) {
        let value = if n < 32 {
            let max = (1i64 << (n - 1)) - 1;
            i64::from(value).clamp(1 - (1i64 << (n - 1)), max) as i32
        } else {
            value
        };
        self.write_bit(value < 0);
        self.write_bits(value.unsigned_abs(), n - 1);
    }

    /// `MSG_WriteBitAngle`: the integer division by 360 happens after the int conversion.
    pub fn write_bit_angle(&mut self, angle: f32, n: u32) {
        let scaled = ((angle % 360.0) * (1u64 << n) as f32) as i32 / 360;
        let mask = if n >= 32 { u32::MAX } else { (1u32 << n) - 1 };
        self.write_bits(scaled as u32 & mask, n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_are_lsb_first_and_streams_use_whole_bytes() {
        let mut w = MsgWriter::new();
        w.write_u8(0xAA);
        w.start_bits();
        w.write_bits(0b101, 3);
        w.write_bits(0x1FF, 9);
        w.end_bits();
        w.write_u8(0x55);
        assert_eq!(w.as_bytes(), &[0xAA, 0b1111_1101, 0b0000_1111, 0x55]);

        let mut r = MsgReader::new(w.as_bytes());
        assert_eq!(r.read_u8(), 0xAA);
        r.start_bits();
        assert_eq!(r.read_bits(3), 0b101);
        assert_eq!(r.read_bits(9), 0x1FF);
        r.end_bits();
        assert_eq!(r.read_u8(), 0x55);
        assert!(!r.bad());
    }

    #[test]
    fn empty_bit_stream_still_consumes_a_byte() {
        let mut r = MsgReader::new(&[0, 7]);
        r.start_bits();
        r.end_bits();
        assert_eq!(r.read_u8(), 7);
    }

    #[test]
    fn overrun_reads_ones_and_marks_bad() {
        let mut r = MsgReader::new(&[0]);
        r.start_bits();
        assert_eq!(r.read_bits(8), 0);
        assert_eq!(r.read_bits(1), 1);
        assert!(r.bad());
    }

    #[test]
    fn write_bits_saturates() {
        let mut w = MsgWriter::new();
        w.write_bits(300, 8);
        w.end_bits();
        assert_eq!(w.as_bytes(), &[0xFF]);
    }

    #[test]
    fn sbitlong_round_trip_and_clamp() {
        let mut w = MsgWriter::new();
        w.write_sbitlong(-5, 8);
        w.write_sbitlong(1000, 8);
        w.end_bits();
        let mut r = MsgReader::new(w.as_bytes());
        assert_eq!(r.read_sbitlong(8), -5);
        assert_eq!(r.read_sbitlong(8), 127);
    }

    #[test]
    fn strings_stop_at_nul_or_ff() {
        let mut r = MsgReader::new(b"ab\0cd\xffef\nx\0");
        assert_eq!(r.read_string(), "ab");
        assert_eq!(r.read_string(), "cd");
        assert_eq!(r.read_string_line(), "ef");
        assert_eq!(r.read_string(), "x");
        assert!(r.at_end() && !r.bad());
    }

    #[test]
    fn bit_coord_with_fraction() {
        // int flag, frac flag, sign, int=3, frac=4 → -3.5
        let mut w = MsgWriter::new();
        w.write_bit(true);
        w.write_bit(true);
        w.write_bit(true);
        w.write_bits(3, 12);
        w.write_bits(4, 3);
        w.end_bits();
        let mut r = MsgReader::new(w.as_bytes());
        assert_eq!(r.read_bit_coord(), -3.5);
    }

    #[test]
    fn bit_angle_round_trip() {
        let mut w = MsgWriter::new();
        w.write_bit_angle(90.0, 16);
        w.end_bits();
        let mut r = MsgReader::new(w.as_bytes());
        assert_eq!(r.read_bit_angle(16), 90.0);
    }
}
