//! Steam's voice packet, what `ISteamUser::GetVoice` returns and `DecompressVoice` takes (as read
//! by ReVoice's `CSteamP2PCodec` and Xash3D's `Voice_ProcessGSData`): the sender's SteamID
//! (8 bytes), a stream of opcode-tagged payloads, and the CRC-32 of everything before it, all
//! little-endian.

use gs_common::crc::crc32;

/// `PLT_Silence`: a count of zero samples.
const SILENCE: u8 = 0;
/// `PLT_Silk`: SILK SDK frames.
pub(crate) const SILK: u8 = 4;
/// `PLT_OPUS_PLC`: Opus frames with sequence numbers for loss concealment.
pub(crate) const OPUS_PLC: u8 = 6;
/// `PLT_SamplingRate`.
const SAMPLE_RATE: u8 = 11;

/// Where [`finish`] expects the Opus payload: after the SteamID, the rate opcode and the
/// payload's opcode and length.
pub const PAYLOAD_AT: usize = 8 + 3 + 3;
/// Bytes around the payload: the header and the CRC.
pub const OVERHEAD: usize = PAYLOAD_AT + 4;

/// One payload of a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payload<'a> {
    SampleRate(u16),
    Silence(u16),
    Silk(&'a [u8]),
    OpusPlc(&'a [u8]),
}

/// The payloads of a packet whose CRC matches, `None` otherwise. An unknown opcode or a
/// truncated payload ends the stream (ReVoice's `StreamDecode` gives up there too).
pub fn parse(data: &[u8]) -> Option<Vec<Payload<'_>>> {
    let body_len = data.len().checked_sub(4).filter(|&n| n >= 8)?;
    let (body, crc) = data.split_at(body_len);
    if crc32(body) != u32::from_le_bytes(crc.try_into().ok()?) {
        return None;
    }
    let mut rest = &body[8..];
    let mut out = Vec::new();
    while let Some((&op, tail)) = rest.split_first() {
        let Some((word, tail)) = tail.split_first_chunk::<2>() else { break };
        let word = u16::from_le_bytes(*word);
        rest = tail;
        match op {
            SAMPLE_RATE => out.push(Payload::SampleRate(word)),
            SILENCE => out.push(Payload::Silence(word)),
            SILK | OPUS_PLC => {
                let Some((payload, tail)) = rest.split_at_checked(usize::from(word)) else { break };
                rest = tail;
                out.push(if op == SILK { Payload::Silk(payload) } else { Payload::OpusPlc(payload) });
            }
            _ => break,
        }
    }
    Some(out)
}

/// `Voice_CreateGSVoicePacket` around the `len`-byte Opus payload already at
/// `out[PAYLOAD_AT..]`: the SteamID, `PLT_SamplingRate`, `PLT_OPUS_PLC` and the CRC written,
/// the packet's length returned (`out` must hold `len + OVERHEAD` bytes).
pub fn finish(out: &mut [u8], steam_id: u64, rate: u16, len: u16) -> usize {
    out[..8].copy_from_slice(&steam_id.to_le_bytes());
    out[8] = SAMPLE_RATE;
    out[9..11].copy_from_slice(&rate.to_le_bytes());
    out[11] = OPUS_PLC;
    out[12..14].copy_from_slice(&len.to_le_bytes());
    let end = PAYLOAD_AT + usize::from(len);
    let crc = crc32(&out[..end]);
    out[end..end + 4].copy_from_slice(&crc.to_le_bytes());
    end + 4
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A packet the way ReVoice's `CSteamP2PCodec::Compress` builds one.
    pub(crate) fn revoice_packet(rate: u16, op: u8, payload: &[u8]) -> Vec<u8> {
        let mut p = 0x0110_0001_0000_0011u64.to_le_bytes().to_vec();
        p.push(SAMPLE_RATE);
        p.extend_from_slice(&rate.to_le_bytes());
        p.push(op);
        p.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        p.extend_from_slice(payload);
        let crc = crc32(&p);
        p.extend_from_slice(&crc.to_le_bytes());
        p
    }

    #[test]
    fn a_built_packet_parses_back_and_a_flipped_bit_fails_the_crc() {
        let payload = [3u8, 0, 7, 0, 0xAA, 0xBB, 0xCC];
        let mut buf = [0u8; 64];
        buf[PAYLOAD_AT..PAYLOAD_AT + payload.len()].copy_from_slice(&payload);
        let n = finish(&mut buf, 0x0110_0001_0000_0011, 24000, payload.len() as u16);
        assert_eq!(buf[..n], revoice_packet(24000, OPUS_PLC, &payload)[..]);
        assert_eq!(parse(&buf[..n]), Some(vec![Payload::SampleRate(24000), Payload::OpusPlc(&payload)]));
        buf[PAYLOAD_AT] ^= 1;
        assert_eq!(parse(&buf[..n]), None);
    }

    #[test]
    fn a_truncated_payload_ends_the_stream_after_what_parsed() {
        let mut p = revoice_packet(16000, SILK, &[1, 2, 3]);
        // Claim 200 bytes of SILK: the length runs past the CRC'd body.
        p[12..14].copy_from_slice(&200u16.to_le_bytes());
        let body = p.len() - 4;
        let crc = crc32(&p[..body]);
        p[body..].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(parse(&p), Some(vec![Payload::SampleRate(16000)]));
    }
}
