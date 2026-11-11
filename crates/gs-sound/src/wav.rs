//! `GetWavinfo` + `ResampleSfx` (docs/re/audio.md §5): mono PCM WAVs with optional loop points,
//! converted to the mixer's 11025 Hz base rate (22050 Hz kept as is with `hisound`).

/// The mixer's base rate (`shm->speed`).
pub const BASE_RATE: i32 = 11025;
/// RIFF chunks larger than this abort the walk.
const MAX_CHUNK: u32 = 100 * 1024 * 1024;

/// Sample data of a loaded sound.
#[derive(Debug, Clone, PartialEq)]
pub enum Samples {
    /// Signed 8-bit.
    S8(Vec<i8>),
    S16(Vec<i16>),
}

/// `sfxcache_t`.
#[derive(Debug, Clone, PartialEq)]
pub struct SfxCache {
    /// Samples at `speed`.
    pub length: i32,
    /// Loop start sample, -1 for none.
    pub loopstart: i32,
    /// 11025 or 22050.
    pub speed: i32,
    pub samples: Samples,
}

impl SfxCache {
    pub fn sample(&self, i: i32) -> i32 {
        let Ok(i) = usize::try_from(i) else { return 0 };
        match &self.samples {
            Samples::S8(d) => d.get(i).map_or(0, |&s| i32::from(s)),
            Samples::S16(d) => d.get(i).map_or(0, |&s| i32::from(s)),
        }
    }

    pub fn is_8bit(&self) -> bool {
        matches!(self.samples, Samples::S8(_))
    }
}

/// `wavinfo_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo {
    pub rate: i32,
    pub width: i32,
    pub channels: i32,
    pub loopstart: i32,
    pub samples: i32,
    pub dataofs: usize,
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(at)?, *d.get(at + 1)?]))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*d.get(at)?, *d.get(at + 1)?, *d.get(at + 2)?, *d.get(at + 3)?]))
}

/// `FindNextChunk`: the next chunk named `name` from `from` (chunk start) to `end`; returns the
/// chunk start and where the walk continues.
fn next_chunk(d: &[u8], from: usize, end: usize, name: &[u8; 4]) -> Option<(usize, usize)> {
    let mut at = from;
    loop {
        if at + 8 > end {
            return None;
        }
        let len = u32_at(d, at + 4)?;
        if len > MAX_CHUNK {
            return None;
        }
        let next = at + 8 + ((len as usize + 1) & !1);
        if &d[at..at + 4] == name {
            // A LIST chunk running past the end aborts the search.
            if name == b"LIST" && next > end + 1 {
                return None;
            }
            return Some((at, next));
        }
        at = next;
    }
}

/// `GetWavinfo`.
pub fn wav_info(d: &[u8]) -> Result<WavInfo, String> {
    let (riff, _) = next_chunk(d, 0, d.len(), b"RIFF").ok_or("Missing RIFF/WAVE chunks")?;
    if d.get(riff + 8..riff + 12) != Some(b"WAVE") {
        return Err("Missing RIFF/WAVE chunks".into());
    }
    let body = riff + 12;
    let (fmt, _) = next_chunk(d, body, d.len(), b"fmt ").ok_or("Missing fmt chunk")?;
    let p = fmt + 8;
    if u16_at(d, p).ok_or("Missing fmt chunk")? != 1 {
        return Err("Microsoft PCM format only".into());
    }
    let channels = i32::from(u16_at(d, p + 2).ok_or("short fmt chunk")?);
    let rate = u32_at(d, p + 4).ok_or("short fmt chunk")? as i32;
    let width = i32::from(u16_at(d, p + 14).ok_or("short fmt chunk")?) / 8;
    if width == 0 {
        return Err("Invalid sample width".into());
    }
    let mut loopstart = -1;
    let mut marked = None;
    if let Some((cue, after)) = next_chunk(d, body, d.len(), b"cue ") {
        loopstart = u32_at(d, cue + 32).ok_or("short cue chunk")? as i32;
        // A following LIST/adtl `mark` gives the loop length.
        if let Some((list, _)) = next_chunk(d, after, d.len(), b"LIST")
            && d.get(list + 28..list + 32) == Some(b"mark")
        {
            marked = Some(loopstart + u32_at(d, list + 24).ok_or("short LIST chunk")? as i32);
        }
    }
    let (data, _) = next_chunk(d, body, d.len(), b"data").ok_or("Missing data chunk")?;
    let datasamples = u32_at(d, data + 4).ok_or("Missing data chunk")? as i32 / width;
    let samples = match marked {
        // Sys_Error in the original; refused here.
        Some(s) if datasamples < s => return Err("Sound has a bad loop length".into()),
        Some(s) => s,
        None => datasamples,
    };
    if rate < 0 || samples < 0 {
        return Err("Invalid wav info".into());
    }
    Ok(WavInfo { rate, width, channels, loopstart, samples, dataofs: data + 8 })
}

/// `S_LoadSound`'s checks plus `ResampleSfx`: nearest-neighbour to 11025 Hz (22050 Hz data
/// stays at its rate when `hisound` is on), 8-bit data made signed.
pub fn load(d: &[u8], hisound: bool) -> Result<SfxCache, String> {
    let info = wav_info(d)?;
    if info.channels != 1 {
        return Err("is a stereo sample".into());
    }
    if info.rate == 0 || info.dataofs >= d.len() {
        return Err("Invalid wav info".into());
    }
    if !matches!(info.width, 1 | 2) {
        return Err("Invalid sample width".into());
    }
    let data = &d[info.dataofs..];
    let mut stepscale = info.rate as f32 / BASE_RATE as f32;
    let mut speed = BASE_RATE;
    if stepscale == 2.0 && hisound {
        stepscale = 1.0;
        speed = 2 * BASE_RATE;
    }
    let length = (info.samples as f32 / stepscale) as i32;
    let loopstart = if info.loopstart == -1 { -1 } else { (info.loopstart as f32 / stepscale) as i32 };
    let input = |i: usize| -> i32 {
        if info.width == 2 {
            data.get(i * 2..i * 2 + 2).map_or(0, |b| i32::from(i16::from_le_bytes([b[0], b[1]])))
        } else {
            data.get(i).map_or(0, |&b| (i32::from(b) - 128) << 8)
        }
    };
    let count = usize::try_from(length).unwrap_or(0);
    let samples = if info.width == 1 && stepscale == 1.0 {
        Samples::S8(data.iter().take(count).map(|&b| b.wrapping_add(0x80) as i8).collect())
    } else {
        let step = (stepscale * 256.0) as usize;
        let mut frac = 0usize;
        let src = (0..count).map(|_| {
            let s = input(frac >> 8);
            frac += step;
            s
        });
        if info.width == 2 { Samples::S16(src.map(|s| s as i16).collect()) } else { Samples::S8(src.map(|s| (s >> 8) as i8).collect()) }
    };
    Ok(SfxCache { length, loopstart, speed, samples })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A mono PCM WAV with optional cue/mark loop chunks.
    pub(crate) fn wav(rate: u32, bits: u16, data: &[u8], cue: Option<(u32, u32)>) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend(b"WAVE");
        body.extend(b"fmt ");
        body.extend(16u32.to_le_bytes());
        body.extend(1u16.to_le_bytes());
        body.extend(1u16.to_le_bytes());
        body.extend(rate.to_le_bytes());
        body.extend((rate * u32::from(bits) / 8).to_le_bytes());
        body.extend((bits / 8).to_le_bytes());
        body.extend(bits.to_le_bytes());
        if let Some((start, len)) = cue {
            body.extend(b"cue ");
            body.extend(28u32.to_le_bytes());
            let mut c = vec![0u8; 28];
            c[24..28].copy_from_slice(&start.to_le_bytes());
            body.extend(c);
            body.extend(b"LIST");
            body.extend(24u32.to_le_bytes());
            let mut l = vec![0u8; 24];
            l[16..20].copy_from_slice(&len.to_le_bytes());
            l[20..24].copy_from_slice(b"mark");
            body.extend(l);
        }
        body.extend(b"data");
        body.extend((data.len() as u32).to_le_bytes());
        body.extend(data);
        let mut out = b"RIFF".to_vec();
        out.extend((body.len() as u32).to_le_bytes());
        out.extend(body);
        out
    }

    #[test]
    fn eight_bit_data_becomes_signed_and_loops_come_from_cue_and_mark() {
        let w = wav(11025, 8, &[128, 255, 0, 130], Some((1, 2)));
        let info = wav_info(&w).unwrap();
        assert_eq!((info.loopstart, info.samples), (1, 3));
        let sc = load(&w, true).unwrap();
        assert_eq!(sc.samples, Samples::S8(vec![0, 127, -128]));
        assert_eq!((sc.length, sc.loopstart, sc.speed), (3, 1, 11025));
    }

    #[test]
    fn rates_resample_to_the_base_rate_unless_hisound_keeps_22050() {
        let data: Vec<u8> = (0..8i16).flat_map(|i| (i * 100).to_le_bytes()).collect();
        let sc = load(&wav(22050, 16, &data, None), true).unwrap();
        assert_eq!((sc.length, sc.speed), (8, 22050));
        let sc = load(&wav(22050, 16, &data, None), false).unwrap();
        assert_eq!((sc.length, sc.speed), (4, 11025));
        assert_eq!(sc.samples, Samples::S16(vec![0, 200, 400, 600]));
        let sc = load(&wav(44100, 16, &data, None), true).unwrap();
        assert_eq!(sc.samples, Samples::S16(vec![0, 400]));
        assert!(load(&wav(11025, 24, &[0; 6], None), true).is_err());
    }
}
