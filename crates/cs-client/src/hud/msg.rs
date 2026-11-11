//! `parsemsg.cpp` readers (docs/re/client-hud.md §3): little-endian fields; reading past the
//! end yields -1 (COORD -0.125, ANGLE -1.40625) and sets the bad-read flag; strings stop at NUL
//! or 0xFF and never flag a bad read.

pub struct MsgReader<'a> {
    data: &'a [u8],
    pos: usize,
    bad: bool,
}

impl<'a> MsgReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, bad: false }
    }

    /// `READ_OK`.
    pub fn ok(&self) -> bool {
        !self.bad
    }

    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let bytes = self.data.get(self.pos..self.pos + N).and_then(|b| b.try_into().ok());
        match bytes {
            Some(b) => {
                self.pos += N;
                Some(b)
            }
            None => {
                self.bad = true;
                None
            }
        }
    }

    pub fn byte(&mut self) -> i32 {
        self.take::<1>().map_or(-1, |[b]| i32::from(b))
    }

    pub fn char(&mut self) -> i32 {
        self.take::<1>().map_or(-1, |[b]| i32::from(b as i8))
    }

    pub fn short(&mut self) -> i32 {
        self.take::<2>().map_or(-1, |b| i32::from(i16::from_le_bytes(b)))
    }

    pub fn long(&mut self) -> i32 {
        self.take::<4>().map_or(-1, i32::from_le_bytes)
    }

    pub fn coord(&mut self) -> f32 {
        self.take::<2>().map_or(-0.125, |b| f32::from(i16::from_le_bytes(b)) * 0.125)
    }

    pub fn angle(&mut self) -> f32 {
        self.take::<1>().map_or(-1.40625, |[b]| f32::from(b as i8) * (360.0 / 256.0))
    }

    /// `READ_FLOAT` (unchecked in the original; an overrun reads 0 here).
    pub fn float(&mut self) -> f32 {
        self.take::<4>().map_or(0.0, f32::from_le_bytes)
    }

    /// `READ_STRING`'s bytes: up to NUL or 0xFF (2047 bytes at most).
    pub fn string_bytes(&mut self) -> &'a [u8] {
        let rest = &self.data[self.pos.min(self.data.len())..];
        let len = rest.iter().position(|&b| b == 0 || b == 0xFF).unwrap_or(rest.len());
        self.pos += (len + 1).min(rest.len());
        &rest[..len.min(2047)]
    }

    /// [`Self::string_bytes`] as text.
    pub fn string(&mut self) -> String {
        gs_common::bytestr::decode(self.string_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overruns_read_minus_one_and_flag() {
        let mut r = MsgReader::new(&[0xFF, 0x34, 0x12, b'h', b'i', 0, b'x']);
        assert_eq!(r.char(), -1);
        assert_eq!(r.short(), 0x1234);
        assert_eq!(r.string(), "hi");
        assert!(r.ok());
        assert_eq!(r.string(), "x");
        assert_eq!(r.long(), -1);
        assert_eq!(r.coord(), -0.125);
        assert!(!r.ok());
        assert_eq!(r.string(), "", "strings never flag");
    }
}
