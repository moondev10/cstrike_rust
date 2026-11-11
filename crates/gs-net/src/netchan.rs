//! Netchan: sequenced datagrams with one in-flight reliable message, fragmented streams and
//! per-packet munging (docs/re/net-transport.md §9).

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::SocketAddrV4;

use crate::munge::{MungeTable, munge, unmunge};

pub const MAX_MSGLEN: usize = 5990;
pub const NET_MAX_MESSAGE: usize = 6037;
pub const MAX_ROUTEABLE_PACKET: usize = 1400;
pub const MAX_RELIABLE_PAYLOAD: usize = 1200;
pub const FRAGMENT_SIZE: usize = 1400;
pub const UDP_HEADER_SIZE: usize = 28;
pub const DEFAULT_RATE: f64 = 30000.0;
const MIN_PACKET: usize = 16;
const MAX_FRAGMENT_ID: u32 = 25000;
const MAX_INCOMING_FRAGMENT: u32 = 2048;
const MAX_INCOMING_OFFSET: u32 = 16384;
const MAX_REASSEMBLED: usize = 65536;
/// Largest file accepted on the file stream (decompressed).
const MAX_FILE_SIZE: usize = 64 * 1024 * 1024;
const BZ2_MAGIC: &[u8; 4] = b"BZ2\0";
const SEQUENCE_MASK: u32 = 0x3FFF_FFFF;
const FLAG_RELIABLE: u32 = 1 << 31;
const FLAG_FRAGMENTS: u32 = 1 << 30;

/// Stream 0 carries large reliable messages, stream 1 carries files.
pub const STREAM_NORMAL: usize = 0;
pub const STREAM_FILE: usize = 1;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NetchanError {
    #[error("outgoing reliable message overflow")]
    Overflow,
    #[error("bad fragmented message: {0}")]
    Fragments(String),
}

/// Result of [`Netchan::process`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Processed {
    /// Message bytes to parse (reliable then unreliable svc data, may end in `0x01` padding).
    pub payload: Option<Vec<u8>>,
    /// A gap in incoming fragment numbers: the original client reconnects (`retry`).
    pub fragment_stall: bool,
    /// The header of a datagram that passed the fragment checks (`net_showpackets`,
    /// `net_showdrop`), whether or not its sequence was then accepted.
    pub header: Option<PacketHeader>,
}

/// A datagram's header as `Netchan_Process` reads it before the sequence test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    /// The whole datagram, header included.
    pub size: usize,
    pub sequence: u32,
    pub ack: u32,
    pub reliable: bool,
    /// `incoming_sequence` before this datagram.
    pub previous: u32,
}

/// A file received on stream 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedFile {
    pub name: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
struct Fragment {
    /// 1-based fragment number.
    number: u32,
    data: Vec<u8>,
}

#[derive(Debug, Clone)]
struct FragmentSet {
    fragments: VecDeque<Fragment>,
    count: u32,
}

/// `flow_t`: the last 32 packet sizes and times of one direction, and the rates net_graph
/// shows.
#[derive(Debug, Clone, Copy, Default)]
struct Flow {
    stats: [(i32, f64); 32],
    current: i32,
    nextcompute: f64,
    kbytespersec: f32,
    avgkbytespersec: f32,
}

#[derive(Debug)]
pub struct Netchan {
    pub remote: SocketAddrV4,
    pub last_received: f64,
    pub connect_time: f64,
    pub rate: f64,
    cleartime: f64,

    pub incoming_sequence: u32,
    pub incoming_acknowledged: u32,
    incoming_reliable_acknowledged: u32,
    incoming_reliable_sequence: u32,
    pub outgoing_sequence: u32,
    reliable_sequence: u32,
    last_reliable_sequence: u32,
    /// Packets lost before the last accepted one.
    pub dropped: u32,

    message: Vec<u8>,
    reliable_buf: Vec<u8>,
    reliable_fragment: [bool; 2],
    reliable_fragid: [u32; 2],
    frag_startpos: [u16; 2],
    frag_length: [u16; 2],
    waitlist: [VecDeque<FragmentSet>; 2],
    fragbufs: [Option<FragmentSet>; 2],
    incoming: [Vec<(u32, Vec<u8>)>; 2],
    incoming_ready: [bool; 2],
    /// Bytes per outgoing fragment (`pfnNetchan_Blocksize`); the client uses 128 until active.
    pub fragment_size: usize,
    /// `flow[0]` outgoing, `flow[1]` incoming.
    flow: [Flow; 2],
    /// `Con_DPrintf` lines the channel produced (`Compressing split packet …`), for the
    /// owner to show at developer level.
    pub dprints: Vec<String>,
}

impl Netchan {
    /// `Netchan_Setup`.
    pub fn new(remote: SocketAddrV4, now: f64) -> Self {
        Self {
            remote,
            last_received: now,
            connect_time: now,
            rate: DEFAULT_RATE,
            cleartime: 0.0,
            incoming_sequence: 0,
            incoming_acknowledged: 0,
            incoming_reliable_acknowledged: 0,
            incoming_reliable_sequence: 0,
            outgoing_sequence: 1,
            reliable_sequence: 0,
            last_reliable_sequence: 0,
            dropped: 0,
            message: Vec::new(),
            reliable_buf: Vec::new(),
            reliable_fragment: [false; 2],
            reliable_fragid: [0; 2],
            frag_startpos: [0; 2],
            frag_length: [0; 2],
            waitlist: Default::default(),
            fragbufs: Default::default(),
            incoming: Default::default(),
            incoming_ready: [false; 2],
            fragment_size: 128,
            flow: [Flow::default(); 2],
            dprints: Vec::new(),
        }
    }

    /// `Netchan_Clear`: drops queued fragments and the in-flight reliable; keeps sequences.
    pub fn clear(&mut self) {
        self.waitlist = Default::default();
        self.fragbufs = Default::default();
        self.incoming = Default::default();
        if !self.reliable_buf.is_empty() {
            self.reliable_buf.clear();
            self.reliable_sequence ^= 1;
        }
        self.cleartime = 0.0;
        self.reliable_fragment = [false; 2];
        self.reliable_fragid = [0; 2];
        self.frag_startpos = [0; 2];
        self.frag_length = [0; 2];
        self.incoming_ready = [false; 2];
    }

    /// Reliable message being assembled for the next send (`chan->message`).
    pub fn reliable(&mut self) -> &mut Vec<u8> {
        &mut self.message
    }

    pub fn clear_reliable(&mut self) {
        self.message.clear();
    }

    /// The seven counters a demo network frame stores (`incoming_sequence` …
    /// `last_reliable_sequence`, netchan_t +56..+80).
    pub fn demo_sequences(&self) -> [i32; 7] {
        [
            self.incoming_sequence,
            self.incoming_acknowledged,
            self.incoming_reliable_acknowledged,
            self.incoming_reliable_sequence,
            self.outgoing_sequence,
            self.reliable_sequence,
            self.last_reliable_sequence,
        ]
        .map(|v| v as i32)
    }

    /// Playback restores the recorded counters before each packet is parsed.
    pub fn set_demo_sequences(&mut self, s: [i32; 7]) {
        let s = s.map(|v| v as u32);
        self.incoming_sequence = s[0];
        self.incoming_acknowledged = s[1];
        self.incoming_reliable_acknowledged = s[2];
        self.incoming_reliable_sequence = s[3];
        self.outgoing_sequence = s[4];
        self.reliable_sequence = s[5];
        self.last_reliable_sequence = s[6];
    }

    /// `Netchan_CanPacket`: the rate limiter allows sending now.
    pub fn can_packet(&self, now: f64) -> bool {
        self.cleartime < now
    }

    /// Advances the outgoing sequence without sending (the client does this on choked frames).
    pub fn skip_sequence(&mut self) {
        self.outgoing_sequence += 1;
    }

    /// `avgkbytespersec` of the outgoing and incoming flows (net_graph).
    pub fn flow_kbytes(&self) -> [f32; 2] {
        [self.flow[0].avgkbytespersec, self.flow[1].avgkbytespersec]
    }

    /// A packet of `len` bytes (+28 for the UDP/IP headers) in flow `i`, then
    /// `Netchan_UpdateFlow`.
    fn record_flow(&mut self, i: usize, len: usize, now: f64) {
        let f = &mut self.flow[i];
        f.stats[(f.current & 31) as usize] = (len as i32 + UDP_HEADER_SIZE as i32, now);
        f.current = f.current.wrapping_add(1);
        self.update_flow(now);
    }

    /// `Netchan_UpdateFlow`: every 0.2 s in effect (`realtime - nextcompute >= 0.1`, then
    /// `nextcompute = realtime + 0.1`), the 15 samples before the newest give `kbytespersec`,
    /// smoothed 2/3 old + 1/3 new. The byte and time sums are shared by the two flows, so the
    /// incoming rate includes the outgoing samples when both recompute together.
    fn update_flow(&mut self, now: f64) {
        let mut bytes = 0i32;
        let mut span = 0.0f32;
        for f in &mut self.flow {
            if now - f.nextcompute < 0.1 {
                continue;
            }
            f.nextcompute = now + 0.1;
            let start = f.current.wrapping_sub(1);
            let mut prev = f.stats[(start & 31) as usize];
            for i in 1..16 {
                let s = f.stats[(start.wrapping_sub(i) & 31) as usize];
                bytes = bytes.wrapping_add(s.0);
                span += (prev.1 - s.1) as f32;
                prev = s;
            }
            f.kbytespersec = if span == 0.0 { 0.0 } else { (f64::from(bytes as f32 / span) / 1024.0) as f32 };
            f.avgkbytespersec = (f64::from(f.avgkbytespersec) * (2.0 / 3.0) + f64::from(f.kbytespersec) * (1.0 - 2.0 / 3.0)) as f32;
        }
    }

    /// `Netchan_Transmit`: builds the next datagram carrying the reliable stream and `unreliable`.
    pub fn transmit(&mut self, unreliable: &[u8], now: f64) -> Result<Vec<u8>, NetchanError> {
        if self.message.len() > MAX_MSGLEN {
            return Err(NetchanError::Overflow);
        }
        let send_resending = self.incoming_acknowledged > self.last_reliable_sequence
            && self.incoming_reliable_acknowledged != self.reliable_sequence;
        let mut send_reliable = send_resending;

        if self.reliable_buf.is_empty() {
            self.frag_send();
            let send_from_frag = [self.fragbufs[0].is_some(), self.fragbufs[1].is_some()];
            let mut send_from_regular = !self.message.is_empty();
            if send_from_regular && send_from_frag[STREAM_NORMAL] {
                send_from_regular = false;
                if self.message.len() > MAX_RELIABLE_PAYLOAD {
                    // Netchan_CreateFragments_: BZip2 when it saves 4 bytes, then the split.
                    let msg = std::mem::take(&mut self.message);
                    let packed = self.compress(&msg);
                    self.enqueue_fragments(STREAM_NORMAL, &packed);
                }
            }
            self.frag_startpos = [0; 2];
            self.reliable_fragment = [false; 2];
            self.reliable_fragid = [0; 2];
            self.frag_length = [0; 2];
            if send_from_regular || send_from_frag.contains(&true) {
                send_reliable = true;
                self.reliable_sequence ^= 1;
            }
            if send_from_regular {
                self.reliable_buf = std::mem::take(&mut self.message);
                let len = self.reliable_buf.len() as u16;
                self.frag_startpos = [len, len];
            }
            for (stream, &from_frag) in send_from_frag.iter().enumerate() {
                if !from_frag {
                    continue;
                }
                let Some(set) = self.fragbufs[stream].as_mut() else { continue };
                let Some(front) = set.fragments.front() else { continue };
                if front.data.len() + self.reliable_buf.len() >= MAX_RELIABLE_PAYLOAD {
                    continue;
                }
                let count = set.count;
                let Some(frag) = set.fragments.pop_front() else { continue };
                if set.fragments.is_empty() {
                    self.fragbufs[stream] = None;
                }
                self.reliable_fragid[stream] = (frag.number << 16) | (count & 0xFFFF);
                self.reliable_buf.extend_from_slice(&frag.data);
                self.frag_length[stream] = frag.data.len() as u16;
                self.reliable_fragment[stream] = true;
                for later in stream + 1..2 {
                    self.frag_startpos[later] += self.frag_length[stream];
                }
            }
        }

        let mut out = Vec::with_capacity(MAX_ROUTEABLE_PACKET);
        let has_fragments = send_reliable && self.reliable_fragment.contains(&true);
        let mut w1 = self.outgoing_sequence & SEQUENCE_MASK;
        if send_reliable {
            w1 |= FLAG_RELIABLE;
        }
        if has_fragments {
            w1 |= FLAG_FRAGMENTS;
        }
        let w2 = (self.incoming_sequence & SEQUENCE_MASK) | (self.incoming_reliable_sequence << 31);
        out.extend_from_slice(&w1.to_le_bytes());
        out.extend_from_slice(&w2.to_le_bytes());
        if has_fragments {
            for stream in 0..2 {
                out.push(u8::from(self.reliable_fragment[stream]));
                if self.reliable_fragment[stream] {
                    out.extend_from_slice(&self.reliable_fragid[stream].to_le_bytes());
                    out.extend_from_slice(&self.frag_startpos[stream].to_le_bytes());
                    out.extend_from_slice(&self.frag_length[stream].to_le_bytes());
                }
            }
        }
        self.outgoing_sequence += 1;
        if send_reliable {
            out.extend_from_slice(&self.reliable_buf);
            self.last_reliable_sequence = self.outgoing_sequence - 1;
        }
        let limit = if send_resending { MAX_ROUTEABLE_PACKET } else { NET_MAX_MESSAGE };
        if limit.saturating_sub(out.len()) >= unreliable.len() {
            out.extend_from_slice(unreliable);
        } else {
            log::debug!("netchan: unreliable would overflow, dropping {} bytes", unreliable.len());
        }
        if out.len() < MIN_PACKET {
            out.resize(MIN_PACKET, 1);
        }
        self.record_flow(0, out.len(), now);
        munge(&mut out[8..], i32::from((self.outgoing_sequence - 1) as u8), MungeTable::Two);

        let per_byte = (1.0 / self.rate) as f32;
        if self.cleartime < now {
            self.cleartime = now;
        }
        self.cleartime += f64::from((out.len() + UDP_HEADER_SIZE) as f32 * per_byte);
        Ok(out)
    }

    /// `Netchan_FragSend`: moves the next waiting fragment set of each idle stream into service.
    fn frag_send(&mut self) {
        for stream in 0..2 {
            if self.fragbufs[stream].is_none() {
                self.fragbufs[stream] = self.waitlist[stream].pop_front();
            }
        }
    }

    /// Splits `data` into `fragment_size` pieces and queues them on `stream`.
    fn enqueue_fragments(&mut self, stream: usize, data: &[u8]) {
        let chunk = self.fragment_size.max(1);
        let fragments: VecDeque<Fragment> = data
            .chunks(chunk)
            .enumerate()
            .map(|(i, c)| Fragment { number: i as u32 + 1, data: c.to_vec() })
            .collect();
        let count = fragments.len() as u32;
        if count > 0 {
            self.waitlist[stream].push_back(FragmentSet { fragments, count });
        }
    }

    /// `Netchan_CreateFragments`: sends `msg` (and any pending reliable data first) as stream-0
    /// fragments, BZip2-compressed when that makes it smaller.
    pub fn queue_fragmented(&mut self, msg: &[u8]) {
        if !self.message.is_empty() {
            let pending = std::mem::take(&mut self.message);
            let packed = self.compress(&pending);
            self.enqueue_fragments(STREAM_NORMAL, &packed);
        }
        if !msg.is_empty() {
            let packed = self.compress(msg);
            self.enqueue_fragments(STREAM_NORMAL, &packed);
        }
    }

    /// [`compress_message`] with `Netchan_CreateFragments_`'s developer line.
    fn compress(&mut self, data: &[u8]) -> Vec<u8> {
        let packed = compress_message(data);
        if !data.starts_with(BZ2_MAGIC) && packed.starts_with(BZ2_MAGIC) {
            self.dprints.push(format!("Compressing split packet ({} -> {} bytes)\n", data.len(), packed.len() - 4));
        }
        packed
    }

    /// `Netchan_CreateFileFragmentsFromBuffer`: sends a named buffer on the file stream.
    pub fn queue_file(&mut self, name: &str, data: &[u8]) {
        let (kind, body) = match bz2_compress(data).filter(|c| c.len() < data.len()) {
            Some(c) => ("bz2", c),
            None => ("uncompressed", data.to_vec()),
        };
        let mut header = Vec::new();
        header.extend_from_slice(name.as_bytes());
        header.push(0);
        header.extend_from_slice(kind.as_bytes());
        header.push(0);
        header.extend_from_slice(&(data.len() as i32).to_le_bytes());
        let chunk = self.fragment_size.max(header.len() + 1);
        let first = (chunk - header.len()).min(body.len());
        let mut fragments = VecDeque::new();
        let mut head = header;
        head.extend_from_slice(&body[..first]);
        fragments.push_back(Fragment { number: 1, data: head });
        for (i, c) in body[first..].chunks(chunk).enumerate() {
            fragments.push_back(Fragment { number: i as u32 + 2, data: c.to_vec() });
        }
        let count = fragments.len() as u32;
        self.waitlist[STREAM_FILE].push_back(FragmentSet { fragments, count });
    }

    /// `Netchan_Process`: validates, unmunges and sequences an incoming datagram from `remote`.
    pub fn process(&mut self, datagram: &[u8], now: f64) -> Processed {
        let mut result = Processed::default();
        if datagram.len() < 8 {
            return result;
        }
        self.last_received = now;
        let w1 = u32::from_le_bytes([datagram[0], datagram[1], datagram[2], datagram[3]]);
        let w2 = u32::from_le_bytes([datagram[4], datagram[5], datagram[6], datagram[7]]);
        if w2 & FLAG_FRAGMENTS != 0 {
            return result;
        }
        let mut data = datagram.to_vec();
        unmunge(&mut data[8..], i32::from(w1 as u8), MungeTable::Two);

        let reliable_message = w1 >> 31;
        let has_fragments = (w1 >> 30) & 1 != 0;
        let reliable_ack = w2 >> 31;
        let mut pos = 8;
        let mut frags: [Option<(u32, u32, u32)>; 2] = [None; 2];
        if has_fragments {
            for slot in &mut frags {
                let Some(&flag) = data.get(pos) else { return result };
                pos += 1;
                if flag == 0 {
                    continue;
                }
                let Some(h) = data.get(pos..pos + 8) else { return result };
                let fragid = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
                let offset = i16::from_le_bytes([h[4], h[5]]) as u32;
                let length = i16::from_le_bytes([h[6], h[7]]) as u32;
                pos += 8;
                if fragid >> 16 > MAX_FRAGMENT_ID
                    || fragid & 0xFFFF > MAX_FRAGMENT_ID
                    || length > MAX_INCOMING_FRAGMENT
                    || offset > MAX_INCOMING_OFFSET
                {
                    return result;
                }
                *slot = Some((fragid, offset, length));
            }
        }

        let sequence = w1 & SEQUENCE_MASK;
        let ack = w2 & SEQUENCE_MASK;
        result.header = Some(PacketHeader { size: datagram.len(), sequence, ack, reliable: reliable_message != 0, previous: self.incoming_sequence });
        if sequence <= self.incoming_sequence {
            return result;
        }
        self.dropped = sequence - self.incoming_sequence - 1;
        if reliable_ack == self.reliable_sequence && self.incoming_acknowledged + 1 >= self.last_reliable_sequence {
            self.reliable_buf.clear();
        }
        self.incoming_sequence = sequence;
        self.incoming_acknowledged = ack;
        self.incoming_reliable_acknowledged = reliable_ack;
        if reliable_message != 0 {
            self.incoming_reliable_sequence ^= 1;
        }
        self.record_flow(1, datagram.len(), now);

        if !has_fragments {
            result.payload = Some(data[pos..].to_vec());
            return result;
        }
        for stream in 0..2 {
            let Some((fragid, offset, length)) = frags[stream] else { continue };
            let start = pos + offset as usize;
            let Some(bytes) = data.get(start..start + length as usize) else { return Processed { header: result.header, ..Processed::default() } };
            if fragid != 0 {
                if length as usize > FRAGMENT_SIZE {
                    return Processed { header: result.header, ..Processed::default() };
                }
                let list = &mut self.incoming[stream];
                match list.iter_mut().find(|(id, _)| *id == fragid) {
                    Some((_, stored)) => *stored = bytes.to_vec(),
                    None => {
                        let at = list.partition_point(|(id, _)| id >> 16 <= fragid >> 16);
                        list.insert(at, (fragid, bytes.to_vec()));
                    }
                }
                let mut complete = 0;
                for (k, (id, _)) in list.iter().enumerate() {
                    if id >> 16 != k as u32 + 1 {
                        result.fragment_stall = true;
                    }
                    complete = k as u32 + 1;
                }
                if complete == fragid & 0xFFFF {
                    self.incoming_ready[stream] = true;
                }
            }
            data.drain(start..start + length as usize);
            if stream == 0
                && let Some((_, later_offset, _)) = frags[1].as_mut()
            {
                *later_offset -= length;
            }
        }
        if data.len() > MIN_PACKET {
            result.payload = Some(data[pos..].to_vec());
        }
        result
    }

    /// `Netchan_CopyNormalFragments`: a fully received large message, BZip2-decoded if needed.
    pub fn take_message(&mut self) -> Option<Result<Vec<u8>, NetchanError>> {
        if !std::mem::take(&mut self.incoming_ready[STREAM_NORMAL]) {
            return None;
        }
        let parts = std::mem::take(&mut self.incoming[STREAM_NORMAL]);
        let joined: Vec<u8> = parts.into_iter().flat_map(|(_, d)| d).collect();
        if joined.len() > MAX_REASSEMBLED {
            return Some(Err(NetchanError::Fragments(format!("message of {} bytes", joined.len()))));
        }
        Some(match joined.strip_prefix(BZ2_MAGIC) {
            Some(packed) => bz2_decompress(packed, MAX_REASSEMBLED),
            None => Ok(joined),
        })
    }

    /// `Netchan_CopyFileFragments`: a fully received file. Callers must vet the name before saving.
    pub fn take_file(&mut self) -> Option<Result<ReceivedFile, NetchanError>> {
        if !std::mem::take(&mut self.incoming_ready[STREAM_FILE]) {
            return None;
        }
        let parts = std::mem::take(&mut self.incoming[STREAM_FILE]);
        let joined: Vec<u8> = parts.into_iter().flat_map(|(_, d)| d).collect();
        Some(parse_file(&joined))
    }

    /// `Netchan_UpdateProgress` (net-transport.md §9.12) as the loading dialog sees it: while
    /// file fragments are coming in, the percentage done and the file's name (empty for
    /// customizations other than `!ModuleC.dll`); `None` otherwise (`net_drawslider` only
    /// changes the unshown value then).
    pub fn download_progress(&self) -> Option<(f32, String)> {
        if self.incoming[STREAM_FILE].is_empty() {
            return None;
        }
        let (mut best, mut count, mut name) = (0.0f32, 0usize, String::new());
        for stream in [STREAM_FILE, STREAM_NORMAL] {
            if let Some((head_id, head)) = self.incoming[stream].first() {
                // `count` carries over from the file stream to the normal one.
                count += self.incoming[stream].len();
                let total = head_id & 0xFFFF;
                if total != 0 {
                    best = (count as f32 * 100.0 / total as f32).max(best);
                }
                if stream == STREAM_FILE {
                    let end = head.iter().position(|&b| b == 0).unwrap_or(head.len()).min(129);
                    let n = &head[..end];
                    if !n.is_empty() && (n[0] != b'!' || n.starts_with(b"!ModuleC.dl")) {
                        name = gs_common::bytestr::decode(n);
                    }
                }
            } else if let Some(set) = self.fragbufs[stream].as_ref().filter(|s| s.count != 0)
                && let Some(next) = set.fragments.front()
            {
                best = (next.number as f32 * 100.0 / set.count as f32).max(best);
            }
        }
        Some((best, name))
    }
}

fn parse_file(joined: &[u8]) -> Result<ReceivedFile, NetchanError> {
    let bad = |what: &str| NetchanError::Fragments(format!("file header: {what}"));
    let name_end = joined.iter().position(|&b| b == 0).ok_or_else(|| bad("name"))?;
    let name = String::from_utf8_lossy(&joined[..name_end]).into_owned();
    let rest = &joined[name_end + 1..];
    let kind_end = rest.iter().position(|&b| b == 0).ok_or_else(|| bad("type"))?;
    let compressed = rest[..kind_end].eq_ignore_ascii_case(b"bz2");
    let size_bytes = rest.get(kind_end + 1..kind_end + 5).ok_or_else(|| bad("size"))?;
    let size = i32::from_le_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]]);
    let size = usize::try_from(size).map_err(|_| bad("negative size"))?;
    // The original decompresses into a buffer of the declared size (up to 2 GB): a small bzip2
    // bomb would fill memory. Servers refuse files over sv_filetransfermaxsize (10 MB).
    if size > MAX_FILE_SIZE {
        return Err(bad("declared size too large"));
    }
    let body = &rest[kind_end + 5..];
    let data = if compressed { bz2_decompress(body, size)? } else { body.to_vec() };
    if name.is_empty() {
        return Err(bad("empty name"));
    }
    Ok(ReceivedFile { name, data })
}

/// `"BZ2\0"` + BZip2 when that is at least 4 bytes smaller than `data`, else `data` unchanged.
fn compress_message(data: &[u8]) -> Vec<u8> {
    if data.starts_with(BZ2_MAGIC) {
        return data.to_vec();
    }
    match bz2_compress(data).filter(|c| c.len() <= data.len().saturating_sub(4)) {
        Some(packed) => {
            let mut out = BZ2_MAGIC.to_vec();
            out.extend_from_slice(&packed);
            out
        }
        None => data.to_vec(),
    }
}

fn bz2_compress(data: &[u8]) -> Option<Vec<u8>> {
    let mut enc = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::best());
    enc.write_all(data).ok()?;
    enc.finish().ok()
}

fn bz2_decompress(data: &[u8], limit: usize) -> Result<Vec<u8>, NetchanError> {
    let mut out = Vec::new();
    bzip2::read::BzDecoder::new(data)
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| NetchanError::Fragments(format!("bzip2: {e}")))?;
    if out.len() > limit {
        return Err(NetchanError::Fragments(format!("decompressed data exceeds {limit} bytes")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn pair() -> (Netchan, Netchan) {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 27015);
        (Netchan::new(addr, 0.0), Netchan::new(addr, 0.0))
    }

    #[test]
    fn first_client_packet_matches_original_bytes() {
        let (mut client, _) = pair();
        client.reliable().extend_from_slice(b"\x03new\0");
        let packet = client.transmit(&[], 0.0).unwrap_or_default();
        assert_eq!(packet, [0x01, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x2C, 0x7D, 0x6E, 0x02, 0x1A, 0x01, 0x11, 0x41]);
    }

    #[test]
    fn reliable_is_resent_after_a_later_ack_and_delivered_once() {
        let (mut a, mut b) = pair();
        a.reliable().extend_from_slice(b"\x03hello\0");
        let p1 = a.transmit(b"U1", 0.0).unwrap_or_default();
        // p1 is lost. Until the peer acks something newer than p1, nothing is resent.
        let p2 = a.transmit(b"U2", 0.0).unwrap_or_default();
        assert_eq!(b.process(&p2, 0.0).payload.as_deref().map(|p| &p[..2]), Some(&b"U2"[..]));
        let q1 = b.transmit(&[], 0.0).unwrap_or_default();
        assert!(a.process(&q1, 0.0).payload.is_some());
        let p3 = a.transmit(b"U3", 0.0).unwrap_or_default();
        let got = b.process(&p3, 0.0).payload.unwrap_or_default();
        assert!(got.starts_with(b"\x03hello\0U3"), "reliable resent with new unreliable: {got:?}");
        assert_eq!(b.process(&p1, 0.0).payload, None, "stale packet is dropped");
        let q2 = b.transmit(&[], 0.0).unwrap_or_default();
        assert!(a.process(&q2, 0.0).payload.is_some());
        let p4 = a.transmit(b"U4", 0.0).unwrap_or_default();
        let got = b.process(&p4, 0.0).payload.unwrap_or_default();
        assert!(got.starts_with(b"U4"), "acked reliable is not sent again: {got:?}");
    }

    #[test]
    fn fragmented_message_round_trips_with_compression() {
        let (mut a, mut b) = pair();
        a.fragment_size = 256;
        let big: Vec<u8> = (0..20_000u32).map(|i| (i % 97) as u8).collect();
        a.queue_fragmented(&big);
        let mut received = None;
        for _ in 0..200 {
            let p = a.transmit(&[], 0.0).unwrap_or_default();
            let processed = b.process(&p, 0.0);
            assert!(!processed.fragment_stall);
            if let Some(msg) = b.take_message() {
                received = Some(msg);
                break;
            }
            let ack = b.transmit(&[], 0.0).unwrap_or_default();
            a.process(&ack, 0.0);
        }
        assert_eq!(received, Some(Ok(big)));
    }

    #[test]
    fn file_stream_round_trips() {
        let (mut a, mut b) = pair();
        a.fragment_size = 128;
        let data: Vec<u8> = (0..3000u32).map(|i| (i * 31 % 251) as u8).collect();
        a.queue_file("!MD5abc", &data);
        let mut received = None;
        for _ in 0..100 {
            let p = a.transmit(&[], 0.0).unwrap_or_default();
            b.process(&p, 0.0);
            if let Some(file) = b.take_file() {
                received = Some(file);
                break;
            }
            let ack = b.transmit(&[], 0.0).unwrap_or_default();
            a.process(&ack, 0.0);
        }
        assert_eq!(received, Some(Ok(ReceivedFile { name: "!MD5abc".into(), data })));
    }

    /// While a file comes in, the loading dialog's percentage counts its fragments and the
    /// name comes from the first one; customizations other than `!ModuleC.dll` show none; a
    /// finished file shows nothing.
    #[test]
    fn download_progress_counts_incoming_file_fragments() {
        let noise: Vec<u8> = (0..1000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        for (name, shown) in [("sound/gungame/bell.wav", "sound/gungame/bell.wav"), ("!MD5abc", ""), ("!ModuleC.dll", "!ModuleC.dll")] {
            let (mut a, mut b) = pair();
            a.fragment_size = 256;
            a.queue_file(name, &noise);
            assert_eq!(b.download_progress(), None);
            let mut seen = Vec::new();
            let mut done = false;
            for _ in 0..100 {
                let p = a.transmit(&[], 0.0).unwrap_or_default();
                b.process(&p, 0.0);
                if b.take_file().is_some() {
                    done = true;
                    break;
                }
                seen.push(b.download_progress());
                let ack = b.transmit(&[], 0.0).unwrap_or_default();
                a.process(&ack, 0.0);
            }
            assert!(done, "{name} never completed");
            let total = seen.len() + 1;
            let expected: Vec<_> = (1..total).map(|k| Some((k as f32 * 100.0 / total as f32, shown.to_owned()))).collect();
            assert_eq!(seen, expected, "{name}");
            assert_eq!(b.download_progress(), None);
        }
    }

    #[test]
    fn rate_limit_blocks_until_cleartime() {
        let (mut a, _) = pair();
        a.rate = 1000.0;
        let p = a.transmit(&[0u8; 72], 10.0).unwrap_or_default();
        assert_eq!(p.len(), 80);
        assert!(!a.can_packet(10.05), "108 bytes at 1000 B/s blocks for ~0.108 s");
        assert!(a.can_packet(10.2));
    }
}
