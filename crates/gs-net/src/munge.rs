//! `COM_Munge` family: the XOR/byteswap obfuscation applied to netchan packets and some message
//! payloads (docs/re/net-transport.md §10). Only whole 32-bit words are touched; 0..3 trailing
//! bytes stay as they are.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MungeTable {
    /// `COM_Munge`: clc_move payload, file consistency.
    One,
    /// `COM_Munge2`: every netchan packet after the 8-byte header.
    Two,
    /// `COM_Munge3`: 4-byte values in sign-on messages.
    Three,
}

const TABLE1: [u8; 16] = [0x7A, 0x64, 0x05, 0xF1, 0x1B, 0x9B, 0xA0, 0xB5, 0xCA, 0xED, 0x61, 0x0D, 0x4A, 0xDF, 0x8E, 0xC7];
const TABLE2: [u8; 16] = [0x05, 0x61, 0x7A, 0xED, 0x1B, 0xCA, 0x0D, 0x9B, 0x4A, 0xF1, 0x64, 0xC7, 0xB5, 0x8E, 0xDF, 0xA0];
const TABLE3: [u8; 16] = [0x20, 0x07, 0x13, 0x61, 0x03, 0x45, 0x17, 0x72, 0x0A, 0x2D, 0x48, 0x0C, 0x4A, 0x12, 0xA9, 0xB5];
/// `0xA5 | (j << j) | j` for byte position j.
const K: [u8; 4] = [0xA5, 0xA7, 0xAF, 0xBF];

impl MungeTable {
    fn bytes(self) -> &'static [u8; 16] {
        match self {
            Self::One => &TABLE1,
            Self::Two => &TABLE2,
            Self::Three => &TABLE3,
        }
    }
}

fn mix(word: u32, i: usize, table: &[u8; 16]) -> u32 {
    let mut b = word.to_le_bytes();
    for (j, byte) in b.iter_mut().enumerate() {
        *byte ^= K[j] | table[(i + j) & 15];
    }
    u32::from_le_bytes(b)
}

pub fn munge(data: &mut [u8], seq: i32, table: MungeTable) {
    let seq = seq as u32;
    for (i, chunk) in data.chunks_exact_mut(4).enumerate() {
        let c = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) ^ !seq;
        let c = mix(c.swap_bytes(), i, table.bytes()) ^ seq;
        chunk.copy_from_slice(&c.to_le_bytes());
    }
}

pub fn unmunge(data: &mut [u8], seq: i32, table: MungeTable) {
    let seq = seq as u32;
    for (i, chunk) in data.chunks_exact_mut(4).enumerate() {
        let c = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) ^ seq;
        let c = mix(c, i, table.bytes()).swap_bytes() ^ !seq;
        chunk.copy_from_slice(&c.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_hw_dll_test_vectors() {
        let mut a: Vec<u8> = (0..10).collect();
        munge(&mut a, 0x12, MungeTable::Two);
        assert_eq!(a, [0x4B, 0x1A, 0x01, 0x12, 0x0F, 0x06, 0x15, 0x56, 0x08, 0x09]);
        let mut b = [1u8; 8];
        munge(&mut b, 0x01, MungeTable::Two);
        assert_eq!(b, [0x5A, 0x19, 0x01, 0x00, 0x1A, 0x01, 0x11, 0x40]);
    }

    #[test]
    fn unmunge_inverts_munge_for_every_table() {
        for table in [MungeTable::One, MungeTable::Two, MungeTable::Three] {
            for seq in [0, 1, 0x7F, 0xFF, 0x0123_4567, -7] {
                let original: Vec<u8> = (0..23u8).map(|i| i.wrapping_mul(37)).collect();
                let mut data = original.clone();
                munge(&mut data, seq, table);
                assert_ne!(data, original);
                unmunge(&mut data, seq, table);
                assert_eq!(data, original, "{table:?} seq {seq}");
            }
        }
    }
}
