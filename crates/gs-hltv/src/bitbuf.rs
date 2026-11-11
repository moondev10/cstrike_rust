//! Core.dll's `BitBuffer` (docs/re/hltv-world.md §7.1, demoplayer.md §12.1): every write is a bit
//! write at `(curByte, curBit)`, LSB first; writes OR into a pre-zeroed buffer and mask their value;
//! reads past the end return −1 and set the overflow flag.

/// Bytes kept after `max_size` (the `n + 4` allocation, plus room for a spilling 32-bit write).
const PAD: usize = 8;

#[derive(Debug, Clone)]
pub struct BitBuffer {
    data: Vec<u8>,
    max_size: usize,
    cur_byte: usize,
    cur_bit: u32,
    pub overflowed: bool,
}

impl Default for BitBuffer {
    fn default() -> Self {
        Self::new(0)
    }
}

impl BitBuffer {
    /// `Resize(n)`: `n` usable bytes, zeroed.
    pub fn new(n: usize) -> Self {
        Self { data: vec![0; n + PAD], max_size: n, cur_byte: 0, cur_bit: 0, overflowed: false }
    }

    /// `BitBuffer(p, n)`: reads `bytes`.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut b = Self::new(bytes.len());
        b.data[..bytes.len()].copy_from_slice(bytes);
        b
    }

    /// `Resize(n)`.
    pub fn resize(&mut self, n: usize) {
        *self = Self::new(n);
    }

    pub fn max_size(&self) -> usize {
        self.max_size
    }

    /// `CurrentSize`: a partial byte counts.
    pub fn current_size(&self) -> usize {
        self.cur_byte + usize::from(self.cur_bit != 0)
    }

    /// `SpaceLeft`.
    pub fn space_left(&self) -> i64 {
        self.max_size as i64 - self.cur_byte as i64
    }

    /// The written bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.data[..self.current_size().min(self.max_size + PAD)]
    }

    /// `Clear`: the whole buffer zeroed, rewound.
    pub fn clear(&mut self) {
        self.data.iter_mut().for_each(|b| *b = 0);
        self.rewind();
    }

    /// `FastClear`: the used bytes (+4, at most `maxSize`) zeroed, rewound.
    pub fn fast_clear(&mut self) {
        let n = (self.current_size() + 4).min(self.max_size);
        self.data[..n].iter_mut().for_each(|b| *b = 0);
        self.rewind();
    }

    /// `Reset`: rewound only.
    pub fn rewind(&mut self) {
        self.cur_byte = 0;
        self.cur_bit = 0;
        self.overflowed = false;
    }

    /// `StartBitMode`: overflow if not byte-aligned.
    pub fn start_bit_mode(&mut self) {
        if self.cur_bit != 0 {
            self.overflowed = true;
        }
    }

    /// `EndBitMode`: rounds up to the next byte.
    pub fn end_bit_mode(&mut self) {
        if self.cur_bit != 0 {
            self.cur_byte += 1;
            self.cur_bit = 0;
        }
    }

    fn advance(&mut self, n: u32) {
        let bits = self.cur_bit + n;
        self.cur_byte += (bits / 8) as usize;
        self.cur_bit = bits % 8;
    }

    /// `WriteBits(v, n)`: masked to `n` bits and ORed in.
    pub fn write_bits(&mut self, v: u32, n: u32) {
        if self.overflowed || n == 0 {
            return;
        }
        if (n >> 8) as usize + self.cur_byte > self.max_size || self.cur_byte + 5 > self.data.len() {
            self.overflowed = true;
            return;
        }
        let v = if n >= 32 { u64::from(v) } else { u64::from(v & ((1u32 << n) - 1)) };
        let shifted = v << self.cur_bit;
        for k in 0..5 {
            self.data[self.cur_byte + k] |= (shifted >> (8 * k)) as u8;
        }
        self.advance(n);
    }

    /// `WriteBit(b)`: sets or clears one bit.
    pub fn write_bit(&mut self, b: bool) {
        if self.cur_byte >= self.max_size {
            self.overflowed = true;
            return;
        }
        if self.overflowed {
            return;
        }
        let mask = 1u8 << self.cur_bit;
        if b {
            self.data[self.cur_byte] |= mask;
        } else {
            self.data[self.cur_byte] &= !mask;
        }
        self.advance(1);
    }

    pub fn write_byte(&mut self, v: u8) {
        self.write_bits(u32::from(v), 8);
    }

    pub fn write_short(&mut self, v: i32) {
        self.write_bits(v as u32, 16);
    }

    pub fn write_word(&mut self, v: i32) {
        self.write_bits(v as u32, 16);
    }

    pub fn write_long(&mut self, v: i32) {
        self.write_bits(v as u32, 32);
    }

    pub fn write_float(&mut self, v: f32) {
        self.write_bits(v.to_bits(), 32);
    }

    /// `WriteCoord(f)`: 16 bits of `(int)(f * 8)`.
    pub fn write_coord(&mut self, f: f32) {
        self.write_bits((f * 8.0) as i32 as u32, 16);
    }

    /// `WriteSBits(v, n)`: clamped, sign bit, magnitude.
    pub fn write_sbits(&mut self, v: i32, n: u32) {
        let mut v = v;
        if n < 32 {
            let max = (1i64 << (n - 1)) - 1;
            v = (i64::from(v)).clamp(-max, max) as i32;
        }
        self.write_bit(v < 0);
        self.write_bits(v.unsigned_abs(), n - 1);
    }

    /// `WriteBitAngle(a, n)`: no `fmod`, C int division by 360.
    pub fn write_bit_angle(&mut self, a: f32, n: u32) {
        if n >= 32 {
            self.overflowed = true;
            return;
        }
        let v = ((1u32 << n) as f32 * a) as i32 / 360;
        self.write_bits((v as u32) & ((1u32 << n) - 1), n);
    }

    /// `WriteBuf(p, len)`: all or nothing; memcpy when byte-aligned.
    pub fn write_buf(&mut self, p: &[u8]) {
        if p.is_empty() || self.overflowed {
            return;
        }
        if self.cur_byte + p.len() > self.max_size {
            self.overflowed = true;
            return;
        }
        if self.cur_bit == 0 {
            self.data[self.cur_byte..self.cur_byte + p.len()].copy_from_slice(p);
            self.cur_byte += p.len();
        } else {
            for &b in p {
                self.write_bits(u32::from(b), 8);
            }
        }
    }

    /// `WriteString(s)`: the bytes and the terminator.
    pub fn write_string(&mut self, s: &[u8]) {
        let mut v = Vec::with_capacity(s.len() + 1);
        v.extend_from_slice(s);
        v.push(0);
        self.write_buf(&v);
    }

    /// `WriteBitString(s)`: 8 bits per char, then 0.
    pub fn write_bit_string(&mut self, s: &[u8]) {
        for &c in s {
            self.write_bits(u32::from(c), 8);
        }
        self.write_bits(0, 8);
    }

    /// `WriteBitData(p, n)`.
    pub fn write_bit_data(&mut self, p: &[u8]) {
        for &c in p {
            if self.cur_byte > self.max_size {
                self.overflowed = true;
                return;
            }
            self.write_bits(u32::from(c), 8);
        }
    }

    // --- reading ------------------------------------------------------------------------------

    /// `ReadBits(n)`: −1 (all ones) with the overflow flag once the end is reached.
    pub fn read_bits(&mut self, n: u32) -> u32 {
        if self.cur_byte >= self.max_size {
            self.overflowed = true;
            return u32::MAX;
        }
        if n == 0 {
            return 0;
        }
        let mut window = 0u64;
        for k in 0..5 {
            window |= u64::from(self.data.get(self.cur_byte + k).copied().unwrap_or(0)) << (8 * k);
        }
        let v = (window >> self.cur_bit) as u32;
        let v = if n >= 32 { v } else { v & ((1u32 << n) - 1) };
        self.advance(n);
        v
    }

    pub fn read_bit(&mut self) -> bool {
        self.read_bits(1) == 1
    }

    /// `PeekBits(n)`.
    pub fn peek_bits(&mut self, n: u32) -> u32 {
        let (b, bit, of) = (self.cur_byte, self.cur_bit, self.overflowed);
        let v = self.read_bits(n);
        self.cur_byte = b;
        self.cur_bit = bit;
        self.overflowed = of;
        v
    }

    /// `ReadByte`: −1 at the end.
    pub fn read_byte(&mut self) -> i32 {
        let v = self.read_bits(8);
        if v == u32::MAX { -1 } else { v as i32 }
    }

    pub fn read_short(&mut self) -> i32 {
        let v = self.read_bits(16);
        if v == u32::MAX { -1 } else { i32::from(v as u16 as i16) }
    }

    pub fn read_word(&mut self) -> i32 {
        let v = self.read_bits(16);
        if v == u32::MAX { -1 } else { v as i32 }
    }

    pub fn read_long(&mut self) -> i32 {
        self.read_bits(32) as i32
    }

    pub fn read_float(&mut self) -> f32 {
        f32::from_bits(self.read_bits(32))
    }

    /// `ReadSBits(n)`.
    pub fn read_sbits(&mut self, n: u32) -> i32 {
        let neg = self.read_bit();
        let v = self.read_bits(n - 1) as i32;
        if neg { -v } else { v }
    }

    /// `ReadBitAngle(n)`.
    pub fn read_bit_angle(&mut self, n: u32) -> f32 {
        self.read_bits(n) as f32 * (360.0 / f64::from((1u32 << n) as f32)) as f32
    }

    /// `ReadString()`: up to 8191 bytes, without the terminator.
    pub fn read_string(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let c = self.read_byte();
            if c <= 0 {
                break;
            }
            if out.len() < 8191 {
                out.push(c as u8);
            }
        }
        out
    }

    /// `ReadBitString`: 8-bit chars up to the terminator.
    pub fn read_bit_string(&mut self) -> Vec<u8> {
        self.read_string()
    }

    /// `ReadBuf(n)`: `None` (overflow, nothing read) past the end.
    pub fn read_buf(&mut self, n: usize) -> Option<Vec<u8>> {
        if self.cur_byte + n > self.max_size {
            self.overflowed = true;
            return None;
        }
        if self.cur_bit == 0 {
            let v = self.data[self.cur_byte..self.cur_byte + n].to_vec();
            self.cur_byte += n;
            Some(v)
        } else {
            Some((0..n).map(|_| self.read_bits(8) as u8).collect())
        }
    }

    pub fn skip_bits(&mut self, n: u32) {
        let mut left = n;
        while left > 0 {
            let k = left.min(32);
            self.read_bits(k);
            left -= k;
        }
    }

    pub fn skip_bytes(&mut self, n: usize) {
        for _ in 0..n {
            self.read_bits(8);
        }
    }

    /// `SkipString`: the bytes skipped, terminator included.
    pub fn skip_string(&mut self) -> usize {
        let mut n = 0;
        loop {
            n += 1;
            if self.read_byte() <= 0 {
                return n;
            }
        }
    }

    /// The read position in bytes (`curByte - data`).
    pub fn byte_pos(&self) -> usize {
        self.cur_byte
    }

    /// The raw bytes `[a, b)` of the buffer.
    pub fn slice(&self, a: usize, b: usize) -> &[u8] {
        let b = b.min(self.data.len());
        &self.data[a.min(b)..b]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_mask_and_read_back_lsb_first() {
        let mut b = BitBuffer::new(16);
        b.write_bits(0x1ff, 3);
        b.write_bits(0xabc, 12);
        b.write_byte(0x5a);
        assert_eq!(b.current_size(), 3);
        let mut r = BitBuffer::from_bytes(b.bytes());
        assert_eq!(r.read_bits(3), 7);
        assert_eq!(r.read_bits(12), 0xabc);
        assert_eq!(r.read_byte(), 0x5a);
    }

    #[test]
    fn overflow_reads_minus_one_and_writes_all_or_nothing() {
        let mut b = BitBuffer::new(2);
        b.write_buf(&[1, 2, 3]);
        assert!(b.overflowed);
        assert_eq!(b.current_size(), 0);
        let mut r = BitBuffer::from_bytes(&[9]);
        assert_eq!(r.read_byte(), 9);
        assert_eq!(r.read_byte(), -1);
        assert!(r.overflowed);
    }

    #[test]
    fn sbits_clamp_and_angle_has_no_fmod() {
        let mut b = BitBuffer::new(8);
        b.write_sbits(-1000, 8);
        b.write_bit_angle(90.0, 8);
        let mut r = BitBuffer::from_bytes(b.bytes());
        assert_eq!(r.read_sbits(8), -127);
        assert_eq!(r.read_bits(8), 64);
    }
}
