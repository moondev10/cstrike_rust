//! Voice without a Steam client. hl.exe always runs under Steam, whose `ISteamUser` records,
//! encodes and decodes voice (`gs-steam`); this client also runs without one, and then speaks
//! Steam's wire format itself, so servers and other players see no difference
//! (docs/deviations.md): Steam's packet ([`packet`]) carrying Opus frames like Steam's encoder
//! (24 kHz, 20 ms, sequence numbers for loss concealment), sent from the default input device;
//! received Opus from Steam clients and the SILK that ReVoice transcodes old Speex clients'
//! voice into are decoded per talker, the way ReVoice decodes them.

mod capture;
mod ffi;
mod packet;
mod resample;

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use gs_sound::voice::VoiceCodec;

use crate::capture::Capture;
use crate::ffi::{OpusDecoder, OpusEncoder, SilkDecoder};
use crate::packet::Payload;
use crate::resample::{Resampler, to_i16};

/// Steam's Opus voice rate and frame (Xash3D's `GS_DEFAULT_SAMPLE_RATE`, `GS_DEFAULT_FRAME_SIZE`).
const ENCODE_RATE: u16 = 24000;
const FRAME: usize = 480;
/// ReVoice's `VoiceEncoder_Opus` bitrate.
const BITRATE: i32 = 32000;
/// The SteamID ReVoice writes into the packets it builds (there is no Steam account here).
const STEAM_ID: u64 = 0x0110_0001_0000_0011;
/// ReVoice's `MAX_OPUS_DATA_LEN`: it drops longer packets (and within it never rate-limits).
const MAX_PACKET: usize = 960;
/// Payload room left before another frame is encoded: its 4-byte header and twice a frame at
/// the bitrate.
const FRAME_ROOM: usize = 4 + 2 * (BITRATE as usize / 8 / 50);
/// The engine's voice rate (`g_VoiceSampleFormat`).
const VOICE_RATE: u32 = 11025;
/// Received voice decodes at 12 kHz (the codecs filter it) and is resampled to 11025 Hz.
const DECODE_RATE: i32 = 12000;
/// 20 ms and the longest Opus frame (120 ms) at the decode rate.
const DECODE_FRAME: usize = 240;
const DECODE_MAX_FRAME: usize = 1440;
/// The most samples one packet decodes to (Xash3D's `GS_MAX_DECOMPRESSED_SAMPLES`).
const MAX_DECODED: usize = 32768;
/// ReVoice's `MAX_PACKET_LOSS`: lost frames concealed at most.
const MAX_LOSS: u16 = 10;
/// The `0xFFFF` frame length that ends a stream.
const END: u16 = 0xFFFF;

/// One talker's decoders (Steam's `DecompressVoice` keeps them per sender too).
struct Talker {
    opus: OpusDecoder,
    silk: SilkDecoder,
    /// `m_nDecodeSeq`: the Opus sequence number expected next.
    seq: u16,
    /// The last `PLT_SamplingRate` (silence counts are in its samples).
    rate: u16,
    to_voice: Resampler,
    /// Decoded samples at the decode rate.
    pcm: Vec<i16>,
}

/// `n` more samples at the decode rate made room for in `pcm` (`None` past the packet's cap).
fn grow(pcm: &mut Vec<i16>, n: usize) -> Option<&mut [i16]> {
    let at = pcm.len();
    if at + n > MAX_DECODED {
        return None;
    }
    pcm.resize(at + n, 0);
    Some(&mut pcm[at..])
}

impl Talker {
    fn new() -> Result<Self, String> {
        Ok(Self {
            opus: OpusDecoder::new(DECODE_RATE).map_err(|e| format!("opus_decoder_create: {e}"))?,
            silk: SilkDecoder::new(DECODE_RATE).map_err(|e| format!("SKP_Silk_SDK_InitDecoder: {e}"))?,
            seq: 0,
            rate: ENCODE_RATE,
            to_voice: Resampler::new(DECODE_RATE as u32, VOICE_RATE),
            pcm: Vec::new(),
        })
    }

    /// `VoiceEncoder_Opus::Decompress` with loss concealment: `[len][seq][frame]` until the
    /// `0xFFFF` end; a gap in the sequence concealed (at most 10 frames), a step back resets.
    fn opus_plc(&mut self, mut data: &[u8]) {
        while let Some((len, rest)) = data.split_first_chunk::<2>() {
            let len = u16::from_le_bytes(*len);
            if len == END {
                self.opus.reset();
                self.seq = 0;
                return;
            }
            let Some((seq, rest)) = rest.split_first_chunk::<2>() else { return };
            let seq = u16::from_le_bytes(*seq);
            if seq < self.seq {
                self.opus.reset();
            } else {
                for _ in 0..(seq - self.seq).min(MAX_LOSS) {
                    let Some(out) = grow(&mut self.pcm, DECODE_FRAME) else { return };
                    let n = self.opus.decode(None, out).unwrap_or(0);
                    self.pcm.truncate(self.pcm.len() - DECODE_FRAME + n);
                }
            }
            self.seq = seq.wrapping_add(1);
            let Some((frame, rest)) = rest.split_at_checked(usize::from(len)) else { return };
            data = rest;
            // A zero length is a DTX frame: 20 ms of silence.
            let room = if frame.is_empty() { DECODE_FRAME } else { DECODE_MAX_FRAME };
            let Some(out) = grow(&mut self.pcm, room) else { return };
            let n = if frame.is_empty() { DECODE_FRAME } else { self.opus.decode(Some(frame), out).unwrap_or(0) };
            self.pcm.truncate(self.pcm.len() - room + n);
        }
    }

    /// `VoiceEncoder_Silk::Decompress`: `[len][payload]` until the `0xFFFF` end, a zero length
    /// being 20 ms of silence.
    fn silk(&mut self, mut data: &[u8]) {
        while let Some((len, rest)) = data.split_first_chunk::<2>() {
            let len = u16::from_le_bytes(*len);
            if len == END {
                self.silk.reset();
                return;
            }
            if len == 0 {
                data = rest;
                if grow(&mut self.pcm, DECODE_FRAME).is_none() {
                    return;
                }
                continue;
            }
            let Some((payload, rest)) = rest.split_at_checked(usize::from(len)) else { return };
            data = rest;
            let Some(out) = grow(&mut self.pcm, DECODE_MAX_FRAME) else { return };
            let n = self.silk.decode(payload, out);
            self.pcm.truncate(self.pcm.len() - DECODE_MAX_FRAME + n);
        }
    }

    /// One packet's payloads decoded and resampled into `out` as 16-bit mono at 11025 Hz: the
    /// bytes written.
    fn decode(&mut self, payloads: &[Payload<'_>], out: &mut [u8]) -> usize {
        self.pcm.clear();
        for p in payloads {
            match *p {
                Payload::SampleRate(rate) => self.rate = rate,
                Payload::Silence(n) => {
                    let n = usize::from(n) * DECODE_RATE as usize / usize::from(self.rate.max(1));
                    let room = MAX_DECODED - self.pcm.len();
                    let _ = grow(&mut self.pcm, n.min(room));
                }
                Payload::OpusPlc(data) => self.opus_plc(data),
                Payload::Silk(data) => self.silk(data),
            }
        }
        let mut written = 0;
        let pcm = &self.pcm;
        self.to_voice.process(pcm.iter().map(|&s| f32::from(s)), |y| {
            if let Some(dst) = out.get_mut(written..written + 2) {
                dst.copy_from_slice(&to_i16(y).to_le_bytes());
                written += 2;
            }
        });
        written
    }
}

/// The voice codec without Steam.
pub struct SteamFormatVoice {
    encoder: OpusEncoder,
    /// `OPUS_PLC` sequence number of the next frame.
    seq: u16,
    /// The microphone while recording, and its samples: as taken, at 24 kHz not yet encoded,
    /// and the converters to 24 kHz and to the 11025 Hz `voice_recordtofile` copy.
    capture: Option<Capture>,
    raw: Vec<f32>,
    pending: Vec<i16>,
    to_encode: Resampler,
    to_record: Resampler,
    talkers: HashMap<i32, Talker>,
}

impl SteamFormatVoice {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            encoder: OpusEncoder::new(i32::from(ENCODE_RATE), BITRATE).map_err(|e| format!("opus_encoder_create: {e}"))?,
            seq: 0,
            capture: None,
            raw: Vec::new(),
            pending: Vec::new(),
            to_encode: Resampler::new(u32::from(ENCODE_RATE), u32::from(ENCODE_RATE)),
            to_record: Resampler::new(u32::from(ENCODE_RATE), VOICE_RATE),
            talkers: HashMap::new(),
        })
    }

    /// 24 kHz samples queued for encoding (what the microphone delivers, resampled).
    fn queue(&mut self, samples: impl IntoIterator<Item = f32>, uncompressed: Option<&mut [u8]>) -> usize {
        let start = self.pending.len();
        let pending = &mut self.pending;
        self.to_encode.process(samples, |y| pending.push(to_i16(y)));
        // At most a second waits; a stalled caller loses the oldest.
        let excess = self.pending.len().saturating_sub(usize::from(ENCODE_RATE));
        self.pending.drain(..excess);
        let Some(u) = uncompressed else { return 0 };
        let mut written = 0;
        let fresh = self.pending[start.saturating_sub(excess)..].iter().map(|&s| f32::from(s));
        self.to_record.process(fresh, |y| {
            if let Some(dst) = u.get_mut(written..written + 2) {
                dst.copy_from_slice(&to_i16(y).to_le_bytes());
                written += 2;
            }
        });
        written
    }

    /// The whole 20 ms frames of `pending` that fit, encoded into a packet in `out`: its length
    /// (0 without a frame). Digital silence is skipped, like Xash3D's encoder.
    fn encode(&mut self, out: &mut [u8]) -> usize {
        let cap = out.len().min(MAX_PACKET);
        if cap < packet::OVERHEAD + FRAME_ROOM {
            return 0;
        }
        let payload = &mut out[packet::PAYLOAD_AT..cap - 4];
        let (mut len, mut used) = (0, 0);
        while self.pending.len() - used >= FRAME && payload.len() - len >= FRAME_ROOM {
            let frame = &self.pending[used..used + FRAME];
            used += FRAME;
            if frame.iter().all(|&s| s == 0) {
                continue;
            }
            let (head, body) = payload[len..].split_at_mut(4);
            match self.encoder.encode(frame, body) {
                Ok(n) => {
                    head[..2].copy_from_slice(&(n as u16).to_le_bytes());
                    head[2..].copy_from_slice(&self.seq.to_le_bytes());
                    self.seq = self.seq.wrapping_add(1);
                    len += 4 + n;
                }
                Err(e) => log::warn!("opus_encode: {e}"),
            }
        }
        self.pending.drain(..used);
        if len == 0 {
            return 0;
        }
        packet::finish(out, STEAM_ID, ENCODE_RATE, len as u16)
    }

    /// The encoder restarted at sequence 0 (which resets the receivers' decoders), nothing queued.
    fn restart(&mut self) {
        self.encoder.reset();
        self.seq = 0;
        self.pending.clear();
        self.raw.clear();
    }
}

impl VoiceCodec for SteamFormatVoice {
    /// The encoder restarted and the microphone opened.
    fn start_recording(&mut self) {
        self.restart();
        self.capture = match Capture::open() {
            Ok(c) => {
                self.to_encode = Resampler::new(c.rate, u32::from(ENCODE_RATE));
                Some(c)
            }
            Err(e) => {
                log::warn!("voice capture: {e}");
                None
            }
        };
    }

    fn stop_recording(&mut self) {
        self.capture = None;
    }

    fn get_voice(&mut self, out: &mut [u8], uncompressed: Option<&mut [u8]>) -> (usize, usize) {
        let mut raw = std::mem::take(&mut self.raw);
        if let Some(c) = &self.capture {
            c.take(&mut raw);
        }
        let recorded = self.queue(raw.drain(..), uncompressed);
        self.raw = raw;
        (self.encode(out), recorded)
    }

    fn decompress(&mut self, sender: i32, data: &[u8], out: &mut [u8]) -> Option<usize> {
        let payloads = packet::parse(data)?;
        let talker = match self.talkers.entry(sender) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => match Talker::new() {
                Ok(t) => e.insert(t),
                Err(err) => {
                    log::warn!("voice decoder: {err}");
                    return None;
                }
            },
        };
        Some(talker.decode(&payloads, out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::SilkEncoder;
    use crate::packet::tests::revoice_packet;

    /// `seconds` of a 300 Hz tone at `rate`.
    fn tone(rate: u32, seconds: f32) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        (0..n).map(|i| (i as f32 * 300.0 * std::f32::consts::TAU / rate as f32).sin() * 8000.0).collect()
    }

    fn samples(bytes: &[u8]) -> Vec<i16> {
        bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
    }

    fn rms(s: &[i16]) -> f32 {
        (s.iter().map(|&v| f32::from(v).powi(2)).sum::<f32>() / s.len().max(1) as f32).sqrt()
    }

    #[test]
    fn spoken_frames_round_trip_through_steam_packets_at_11025_hz() {
        let mut tx = SteamFormatVoice::new().unwrap();
        let mut rx = SteamFormatVoice::new().unwrap();
        let (mut decoded, mut packets) = (Vec::new(), 0);
        // 1 s of tone fed in 10 ms client frames: a one-frame packet every other call.
        for chunk in tone(24000, 1.0).chunks(240) {
            tx.queue(chunk.iter().copied(), None);
            let mut pkt = [0u8; 2048];
            let n = tx.encode(&mut pkt);
            if n == 0 {
                continue;
            }
            packets += 1;
            let mut pcm = [0u8; 8192];
            let w = rx.decompress(3, &pkt[..n], &mut pcm).unwrap();
            decoded.extend(samples(&pcm[..w]));
        }
        // Each 20 ms frame is 220.5 samples at 11025 Hz.
        assert!(packets >= 49);
        assert!((decoded.len() as f32 - packets as f32 * 220.5).abs() <= 2.0, "{packets} {}", decoded.len());
        // Past the codec's start-up, the tone comes back near its 5657 rms.
        let level = rms(&decoded[2000..]);
        assert!((4000.0..7000.0).contains(&level), "{level}");
    }

    #[test]
    fn a_backlog_is_sent_in_packets_revoice_accepts() {
        let mut tx = SteamFormatVoice::new().unwrap();
        tx.queue(tone(24000, 0.5), None);
        let mut sizes = Vec::new();
        loop {
            let mut pkt = [0u8; 4096];
            match tx.encode(&mut pkt) {
                0 => break,
                n => sizes.push(n),
            }
        }
        assert!(sizes.len() > 1 && sizes.iter().all(|&n| n <= MAX_PACKET), "{sizes:?}");
        assert!(tx.pending.len() < FRAME);
    }

    #[test]
    fn lost_frames_are_concealed_and_a_restart_resets() {
        let mut tx = SteamFormatVoice::new().unwrap();
        let mut rx = SteamFormatVoice::new().unwrap();
        let mut packets = Vec::new();
        for chunk in tone(24000, 0.2).chunks(FRAME) {
            tx.queue(chunk.iter().copied(), None);
            let mut pkt = [0u8; 2048];
            let n = tx.encode(&mut pkt);
            packets.push(pkt[..n].to_vec());
        }
        let mut pcm = [0u8; 8192];
        let one = rx.decompress(1, &packets[0], &mut pcm).unwrap();
        // Frames 1..=3 lost: the next packet brings them back as concealment plus its own.
        let four = rx.decompress(1, &packets[4], &mut pcm).unwrap();
        assert!((four as f32 / one as f32 - 4.0).abs() < 0.05, "{one} {four}");
        // A talker starting over at sequence 0 is decoded afresh, nothing concealed.
        tx.restart();
        tx.queue(tone(24000, 0.02), None);
        let mut pkt = [0u8; 2048];
        let n = tx.encode(&mut pkt);
        let again = rx.decompress(1, &pkt[..n], &mut pcm).unwrap();
        assert!(again.abs_diff(one) <= 2, "{one} {again}");
    }

    #[test]
    fn revoice_silk_from_speex_clients_decodes() {
        // VoiceEncoder_Silk at 8 kHz: [len][payload] per 20 ms frame, then the 0xFFFF end.
        let mut enc = SilkEncoder::new(8000);
        let mut payload = Vec::new();
        let pcm: Vec<i16> = tone(8000, 0.4).into_iter().map(to_i16).collect();
        for frame in pcm.chunks(160) {
            let bytes = enc.encode(frame);
            payload.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
            payload.extend_from_slice(&bytes);
        }
        payload.extend_from_slice(&END.to_le_bytes());
        let pkt = revoice_packet(16000, packet::SILK, &payload);
        let mut rx = SteamFormatVoice::new().unwrap();
        let mut out = [0u8; 16384];
        let w = rx.decompress(2, &pkt, &mut out).unwrap();
        let decoded = samples(&out[..w]);
        // 0.4 s at 11025 Hz.
        assert!((4405..=4415).contains(&decoded.len()), "{}", decoded.len());
        let level = rms(&decoded[1000..]);
        assert!((3000.0..8000.0).contains(&level), "{level}");
    }
}
