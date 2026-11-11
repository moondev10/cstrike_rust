//! Reassembly of `FE FF FF FF` split datagrams (docs/re/net-transport.md §4).
//!
//! Layout: `i32 -2`, `i32 sequence`, `u8 (index << 4) | count`, then up to 1391 payload bytes.
//! The original keeps a single global context; a new sequence restarts it.

pub const SPLIT_HEADER: usize = 9;
pub const SPLIT_CHUNK: usize = 1391;
pub const MAX_SPLIT_PARTS: usize = 5;
/// Largest reassembled (and largest accepted single) datagram.
pub const MAX_DATAGRAM: usize = 6010;

#[derive(Debug)]
pub struct SplitReassembler {
    sequence: Option<i32>,
    remaining: i32,
    count: usize,
    total: usize,
    part_seq: [Option<i32>; MAX_SPLIT_PARTS],
    buffer: Vec<u8>,
    /// `net_showpackets 4`: `NET_GetLong`'s lines for a new split sequence and each new part.
    pub show: bool,
    /// Lines produced since the owner last took them (`Con_Printf`).
    pub prints: Vec<String>,
}

impl Default for SplitReassembler {
    fn default() -> Self {
        Self {
            sequence: None,
            remaining: 0,
            count: 0,
            total: 0,
            part_seq: [None; MAX_SPLIT_PARTS],
            buffer: vec![0; MAX_DATAGRAM],
            show: false,
            prints: Vec::new(),
        }
    }
}

impl SplitReassembler {
    /// Feeds one split part; returns the full datagram once every part has arrived.
    pub fn accept(&mut self, part: &[u8]) -> Option<Vec<u8>> {
        if part.len() < SPLIT_HEADER {
            log::debug!("invalid split packet length {}", part.len());
            return None;
        }
        let sequence = i32::from_le_bytes([part[4], part[5], part[6], part[7]]);
        let index = usize::from(part[8] >> 4);
        let count = usize::from(part[8] & 0x0F);
        if index >= MAX_SPLIT_PARTS || count > MAX_SPLIT_PARTS || count == 0 {
            log::debug!("malformed split packet {index}/{count}");
            return None;
        }
        let payload = &part[SPLIT_HEADER..];
        if self.sequence != Some(sequence) {
            self.sequence = Some(sequence);
            self.remaining = count as i32;
            self.count = count;
            if self.show {
                self.prints.push(format!("<-- Split packet restart {count} count {sequence} seq\n"));
            }
        }
        if self.part_seq[index] == Some(sequence) {
            log::debug!("duplicate split packet {index} of sequence {sequence}");
        } else {
            if index == count - 1 {
                self.total = payload.len() + SPLIT_CHUNK * (count - 1);
            }
            self.remaining -= 1;
            self.part_seq[index] = Some(sequence);
            if self.show {
                self.prints.push(format!("<-- Split packet {} of {count}, {} bytes {sequence} seq\n", index + 1, payload.len()));
            }
            let start = SPLIT_CHUNK * index;
            if start + payload.len() > MAX_DATAGRAM {
                log::debug!("malformed split packet size");
                return None;
            }
            self.buffer[start..start + payload.len()].copy_from_slice(payload);
        }
        if self.remaining > 0 {
            return None;
        }
        self.sequence = None;
        if self.part_seq[..self.count].iter().any(|&s| s != Some(sequence)) {
            log::debug!("split packet without all {} parts", self.count);
            return None;
        }
        if self.total > MAX_DATAGRAM {
            log::debug!("split packet too large ({})", self.total);
            return None;
        }
        Some(self.buffer[..self.total].to_vec())
    }
}

/// Splits `data` into wire parts exactly like `NET_SendLong` (used by tests and tools).
pub fn split_datagram(data: &[u8], sequence: i32) -> Vec<Vec<u8>> {
    let count = data.len().div_ceil(SPLIT_CHUNK);
    data.chunks(SPLIT_CHUNK)
        .enumerate()
        .map(|(i, chunk)| {
            let mut part = Vec::with_capacity(SPLIT_HEADER + chunk.len());
            part.extend_from_slice(&(-2i32).to_le_bytes());
            part.extend_from_slice(&sequence.to_le_bytes());
            part.push(((i as u8) << 4) | count as u8);
            part.extend_from_slice(chunk);
            part
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7) as u8).collect()
    }

    #[test]
    fn reassembles_out_of_order_and_ignores_duplicates() {
        let data = payload(3000);
        let parts = split_datagram(&data, 5);
        assert_eq!(parts.len(), 3);
        let mut r = SplitReassembler::default();
        assert_eq!(r.accept(&parts[2]), None);
        assert_eq!(r.accept(&parts[2]), None, "duplicate does not complete");
        assert_eq!(r.accept(&parts[0]), None);
        assert_eq!(r.accept(&parts[1]), Some(data));
    }

    #[test]
    fn new_sequence_restarts_reassembly() {
        let (a, b) = (payload(2000), payload(1500));
        let pa = split_datagram(&a, 1);
        let pb = split_datagram(&b, 2);
        let mut r = SplitReassembler::default();
        assert_eq!(r.accept(&pa[0]), None);
        assert_eq!(r.accept(&pb[0]), None);
        assert_eq!(r.accept(&pb[1]), Some(b));
    }

    #[test]
    fn rejects_bad_headers() {
        let mut r = SplitReassembler::default();
        let mut part = split_datagram(&payload(100), 1).remove(0);
        part[8] = 0x50 | 6;
        assert_eq!(r.accept(&part), None);
        part[8] = 0x00;
        assert_eq!(r.accept(&part), None, "zero part count is rejected");
    }
}
