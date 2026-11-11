//! In-game voice, Steam path (docs/re/voice-engine.md, docs/re/audio.md §11): five receive
//! channels with a 1 s ring, `CAutoGain`, the 0.999-fraction "upsampler", the jitter delay, the
//! output tail fade and ×2 upsampling callback the mixer pulls in its voice pass, the
//! overdrive ducking, and the local-player ack. `HUD_VoiceStatus` calls are queued in
//! [`Voice::hud_events`]. Mixer glue (pseudo-sfx channels 7..11) lives in `mixer.rs`.

/// `VOICE_NUM_CHANNELS`.
pub const CHANNELS: usize = 5;
/// `CHAN_NETWORKVOICE_BASE`.
pub const CHAN_BASE: i32 = 7;
/// `TWEAKMODE_ENTITYINDEX`: the channel owner while the voice options test the microphone.
pub const TWEAK_ENTITY: i32 = -500;
/// `VOICE_RECEIVE_BUFFER_SIZE` in bytes (1 s of 16-bit mono at 11025 Hz).
const RING_BYTES: i32 = 22050;
/// Initial `resampleFrac` (f64 0x3FEFF7CED916872B).
const INITIAL_FRAC: f64 = 0.999;
/// The jitter delay and the ack timeout (f32 0x3E4CCCCD).
const DELAY: f32 = 0.2;
/// The literal π of `VoiceSE_Idle` (0x400921FB54411744), short of `PI` in the last digits.
#[allow(clippy::approx_constant, reason = "the binary's truncated literal, kept bit-exact")]
const OVERDRIVE_PI: f64 = 3.141_592_653_5;
/// `g_VoiceSampleFormat.nSamplesPerSec` (never written).
const OUTPUT_SAMPLE_RATE: u32 = 11025;

/// The voice codec of this build: Steam voice through `ISteamUser` (`g_bUsingSteamVoice`), or
/// without a Steam client the same wire format from `gs-voicecodec`.
pub trait VoiceCodec {
    /// `ISteamUser::StartVoiceRecording`.
    fn start_recording(&mut self);
    /// `ISteamUser::StopVoiceRecording`.
    fn stop_recording(&mut self);
    /// `GetAvailableVoice(…, uncompressed ? 11025 : 0)`, and when that succeeds
    /// `GetVoice(true, out, …, uncompressed, …, 11025)` (its own result ignored):
    /// `(compressed bytes, uncompressed bytes)` written.
    fn get_voice(&mut self, out: &mut [u8], uncompressed: Option<&mut [u8]>) -> (usize, usize);
    /// `DecompressVoice(data, out, 11025)` of a packet from `sender` (its entity, or
    /// [`TWEAK_ENTITY`] for the local mic; Steam's call takes none, the packet carries the
    /// SteamID): 16-bit mono bytes written to `out`, `None` when the result is not
    /// `k_EVoiceResultOK`.
    fn decompress(&mut self, sender: i32, data: &[u8], out: &mut [u8]) -> Option<usize>;
}

/// `voice_recordtofile`'s 1 MiB capture buffers (`voice_micdata.wav`,
/// `voice_decompressed.wav`), written as 11025 Hz 16-bit mono WAVs by `Voice_RecordStop`.
#[derive(Debug, Clone)]
pub struct FileRecording {
    pub name: String,
    pub data: Vec<u8>,
    pub used: usize,
}

impl FileRecording {
    pub const SIZE: usize = 0x10_0000;

    pub fn new(name: &str) -> Self {
        Self { name: name.to_owned(), data: vec![0; Self::SIZE], used: 0 }
    }

    /// Appends what fits.
    fn append(&mut self, bytes: &[u8]) {
        let k = bytes.len().min(Self::SIZE - self.used);
        self.data[self.used..self.used + k].copy_from_slice(&bytes[..k]);
        self.used += k;
    }

    /// `WriteWaveFile(name, data, used, 16, 1, 11025)`.
    pub fn wave(&self) -> Vec<u8> {
        let data = &self.data[..self.used];
        let mut w = Vec::with_capacity(44 + data.len());
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&0u32.to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&11025u32.to_le_bytes());
        w.extend_from_slice(&22050u32.to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(data.len() as u32).to_le_bytes());
        w.extend_from_slice(data);
        let riff = (w.len() - 8) as u32;
        w[4..8].copy_from_slice(&riff.to_le_bytes());
        w
    }
}

/// The voice cvars read at the moment of use.
#[derive(Debug, Clone, Copy)]
pub struct VoiceSettings {
    pub enable: f32,
    pub maxgain: f32,
    pub avggain: f32,
    pub scale: f32,
    pub fadeouttime: f32,
    pub overdrive: f32,
    pub overdrivefadetime: f32,
    pub showchannels: f32,
    pub showincoming: f32,
    /// `voice_profile`.
    pub profile: f32,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            enable: 1.0,
            maxgain: 5.0,
            avggain: 0.5,
            scale: 1.0,
            fadeouttime: 0.1,
            overdrive: 2.0,
            overdrivefadetime: 0.4,
            showchannels: 0.0,
            showincoming: 0.0,
            profile: 0.0,
        }
    }
}

/// `CCircularBuffer` (byte ring; voice only moves whole samples).
#[derive(Debug, Clone)]
struct Ring {
    count: i32,
    read: i32,
    write: i32,
    data: Vec<u8>,
}

impl Ring {
    fn new() -> Self {
        Self { count: 0, read: 0, write: 0, data: vec![0; RING_BYTES as usize] }
    }

    fn flush(&mut self) {
        (self.read, self.write, self.count) = (0, 0, 0);
    }

    /// `Write`: when full the oldest bytes are overwritten and the read index follows.
    fn write(&mut self, src: &[u8]) {
        let n = src.len() as i32;
        let size = RING_BYTES;
        let mut w = self.write;
        let mut left = n;
        let mut s = 0usize;
        while left != 0 {
            let end = (w + left).min(size);
            let k = (end - w) as usize;
            self.data[w as usize..end as usize].copy_from_slice(&src[s..s + k]);
            s += k;
            left += w - end;
            w = end % size;
            self.write = w;
        }
        if n > size - self.count {
            self.read = self.write;
            self.count = size;
        } else {
            self.count += n;
        }
    }

    /// `Read`: at most `n` bytes.
    fn read(&mut self, dst: &mut [u8], n: i32) -> i32 {
        if self.count == 0 {
            return 0;
        }
        let size = RING_BYTES;
        let k = n.min(self.count);
        let r = self.read;
        if k + r <= size {
            dst[..k as usize].copy_from_slice(&self.data[r as usize..(r + k) as usize]);
        } else {
            let first = (size - r) as usize;
            dst[..first].copy_from_slice(&self.data[r as usize..]);
            dst[first..k as usize].copy_from_slice(&self.data[..k as usize - first]);
        }
        if k == 0 {
            return 0;
        }
        self.read = (r + k) % size;
        self.count -= k;
        if self.count == 0 {
            self.read = 0;
            self.write = 0;
        }
        k
    }
}

/// `CAutoGain`.
#[derive(Debug, Clone, Copy)]
struct AutoGain {
    block: i32,
    max_gain: f32,
    avg_to_max: f32,
    /// `currentBlock`: never advanced, so every chunk updates the gain.
    current_block: i32,
    sum: i32,
    max: i32,
    scale: f32,
    prev_gain: f32,
    next_gain: f32,
    fixed_gain: i32,
    gain_delta: i32,
}

impl Default for AutoGain {
    fn default() -> Self {
        Self { block: 128, max_gain: 5.0, avg_to_max: 0.5, current_block: 0, sum: 0, max: 0, scale: 1.0, prev_gain: 1.0, next_gain: 1.0, fixed_gain: 128, gain_delta: 0 }
    }
}

impl AutoGain {
    fn reset(&mut self, block: i32, max_gain: f32, avg_to_max: f32, scale: f32) {
        *self = Self { block, max_gain, avg_to_max, scale, ..Self::default() };
    }

    /// `ProcessSamples`: chunks of up to `block` samples ramped from the previous gain to the
    /// next, the gain recomputed after every chunk (a short chunk's average still / block).
    fn process(&mut self, s: &mut [i16]) {
        let mut rest = s;
        while !rest.is_empty() {
            let chunk = ((self.block - self.current_block).min(rest.len() as i32)).max(0) as usize;
            let (head, tail) = rest.split_at_mut(chunk);
            for (i, x) in head.iter_mut().enumerate() {
                let v = i32::from(*x);
                let a = v.abs();
                self.sum = self.sum.wrapping_add(a);
                if a > self.max {
                    self.max = a;
                }
                let o = (self.fixed_gain.wrapping_add((i as i32).wrapping_mul(self.gain_delta))).wrapping_mul(v) >> 7;
                *x = o.clamp(-32768, 32767) as i16;
            }
            rest = tail;
            if self.current_block % self.block == 0 {
                let prev = self.next_gain;
                self.prev_gain = prev;
                self.fixed_gain = (f64::from(prev) * 128.0) as i32;
                let avg = self.sum / self.block;
                let level = (self.max - avg) as f32 * self.avg_to_max + avg as f32;
                let g = 32767.0f32 / level;
                let gain = if g > self.max_gain { self.max_gain } else { g };
                self.sum = 0;
                self.max = 0;
                self.next_gain = gain * self.scale;
                self.gain_delta = (f64::from((self.next_gain - prev) / (self.block - 1) as f32) * 128.0) as i32;
            }
            if chunk == 0 {
                break;
            }
        }
    }
}

/// `VoiceChannel`.
#[derive(Debug, Clone)]
struct Channel {
    /// Talker entity, -1 free.
    ent: i32,
    ring: Ring,
    resample_frac: f64,
    starved: bool,
    /// The jitter delay, counting down forever.
    time_pad: f32,
    gain: AutoGain,
}

impl Channel {
    fn new() -> Self {
        Self { ent: -1, ring: Ring::new(), resample_frac: 0.0, starved: false, time_pad: 0.0, gain: AutoGain::default() }
    }
}

/// What the voice engine asks of the mixer in `Voice_Idle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixerRequest {
    /// `VoiceSE_StartChannel(i)`.
    Start(usize),
    /// `VoiceSE_EndChannel(i)` (`S_StopSound(cl.viewentity, 7 + i)`).
    End(usize),
}

/// The engine voice state (`g_VoiceChannels`, the pseudo-sfx carries, overdrive, ack, recording).
#[derive(Debug, Clone)]
pub struct Voice {
    channels: Vec<Channel>,
    /// `g_VoiceSfx[i].lastOutSample`: the ×2 upsampler's carried sample.
    carry: [i16; CHANNELS],
    /// `g_VoiceOverdriveFadeTime`, `g_bVoiceOverdriveOn`, `g_SND_VoiceOverdrive`.
    overdrive_t: f32,
    overdrive_on: bool,
    pub overdrive: f32,
    /// `g_bVoiceInitted`, `g_bUsingSteamVoice`, `g_bVoiceRecording`.
    initted: bool,
    using_steam: bool,
    recording: bool,
    /// `g_bLocalPlayerTalkingAck`, `g_LocalPlayerTalkingTimeout` (counts up).
    ack: bool,
    ack_timeout: f32,
    /// `g_nVoiceFadeSamples`, `g_VoiceFadeSamplesMul`.
    fade_samples: i32,
    fade_mul: f32,
    /// Queued `HUD_VoiceStatus(ent, talking)` calls.
    pub hud_events: Vec<(i32, bool)>,
    /// Console lines from the debug cvars.
    pub prints: Vec<String>,
    /// `voice_profile`'s sums since the last `Voice_Idle` (seconds): decompress, gain, upsample.
    profile: [f64; 3],
    /// `voice_recordtofile` captures: the local mic (uncompressed from Steam) and the
    /// received post-AGC samples.
    pub mic_rec: Option<FileRecording>,
    pub decomp_rec: Option<FileRecording>,
    /// `g_bInTweakMode` and `g_VoiceTweakSpeakingVolume` (voice-engine.md §9).
    pub in_tweak: bool,
    pub tweak_speaking_volume: i32,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            channels: (0..CHANNELS).map(|_| Channel::new()).collect(),
            carry: [0; CHANNELS],
            overdrive_t: 0.0,
            overdrive_on: false,
            overdrive: 1.0,
            initted: false,
            using_steam: true,
            recording: false,
            ack: false,
            ack_timeout: 0.0,
            fade_samples: 1,
            fade_mul: 1.0,
            hud_events: Vec::new(),
            prints: Vec::new(),
            profile: [0.0; 3],
            mic_rec: None,
            decomp_rec: None,
            in_tweak: false,
            tweak_speaking_volume: 0,
        }
    }
}

impl Voice {
    /// The tail of `Voice_Init` after `Voice_Deinit` (`VoiceSE_Init` with sound running):
    /// initialised, the pseudo-sfx carries reset, Steam voice when there is a codec.
    pub fn init_done(&mut self, steam: bool) {
        self.initted = true;
        self.carry = [0; CHANNELS];
        self.using_steam = steam;
    }

    /// `Voice_Deinit`'s gate (`g_bVoiceInitted`); clears it.
    pub fn take_initted(&mut self) -> bool {
        std::mem::take(&mut self.initted)
    }

    /// `g_bUsingSteamVoice`.
    pub fn using_steam(&self) -> bool {
        self.using_steam
    }

    /// `g_bVoiceInitted` (the tweak API's mixer controls exist only then).
    pub fn initted(&self) -> bool {
        self.initted
    }

    /// `Voice_IsRecording`: recording and not in tweak mode.
    pub fn is_recording(&self) -> bool {
        self.recording && !self.in_tweak
    }

    /// `Voice_RecordStart`'s result.
    pub fn record_started(&mut self, ok: bool) {
        self.recording = ok;
        if ok {
            self.hud_events.push((-1, true));
        }
    }

    /// `Voice_RecordStop`'s state change.
    pub fn record_stopped(&mut self) {
        if self.recording {
            self.hud_events.push((-1, false));
        }
        self.recording = false;
    }

    /// `Voice_LocalPlayerTalkingAck`.
    pub fn local_ack(&mut self) {
        if !self.ack {
            self.hud_events.push((-2, true));
        }
        self.ack = true;
        self.ack_timeout = 0.0;
    }

    /// `Voice_GetChannel`.
    pub fn channel_of(&self, ent: i32) -> Option<usize> {
        self.channels.iter().position(|c| c.ent == ent)
    }

    /// `Voice_AssignChannel`: the talker's channel or the first free one (reset, 0.2 s delay,
    /// overdrive on).
    pub fn assign_channel(&mut self, ent: i32, s: &VoiceSettings) -> Option<usize> {
        // In tweak mode Voice_AssignChannel returns -2: network voice is dropped.
        if self.in_tweak {
            return None;
        }
        self.assign_any(ent, s)
    }

    /// `Voice_AssignChannel`'s body (also the tweak start's inlined copy for entity −500).
    pub fn assign_any(&mut self, ent: i32, s: &VoiceSettings) -> Option<usize> {
        for (i, c) in self.channels.iter_mut().enumerate() {
            if c.ent == ent {
                return Some(i);
            }
            if c.ent == -1 {
                c.ent = ent;
                c.starved = false;
                c.ring.flush();
                c.resample_frac = INITIAL_FRAC;
                c.time_pad = DELAY;
                c.gain.reset(128, s.maxgain, s.avggain, s.scale);
                self.overdrive_on = true;
                return Some(i);
            }
        }
        None
    }

    /// `Voice_AddIncomingData` after the decode: `pcm` are the decoded 11025 Hz samples (`None`
    /// when decoding failed), `decode_secs` how long the decode took (`voice_profile`). Any
    /// non-empty packet un-starves the channel first.
    pub fn add_incoming(&mut self, ch: usize, pcm: Option<&mut [i16]>, decode_secs: f64, s: &VoiceSettings) {
        let Some(c) = self.channels.get_mut(ch) else { return };
        c.starved = false;
        let Some(pcm) = pcm else { return };
        let n = pcm.len() as i32;
        let profile = s.profile != 0.0;
        let mut mark = std::time::Instant::now();
        let mut lap = |slot: &mut f64| {
            if profile {
                *slot += mark.elapsed().as_secs_f64();
                mark = std::time::Instant::now();
            }
        };
        if profile {
            self.profile[0] += decode_secs;
        }
        c.gain.process(pcm);
        lap(&mut self.profile[1]);
        // The "upsample" at a ratio of 1: n-1 samples, each ~the next input sample.
        let step = f64::from(OUTPUT_SAMPLE_RATE) / 11025.0;
        let mut pos = c.resample_frac;
        let mut x = pos;
        let limit = n - 1;
        while pos < f64::from(limit) {
            let i = pos as i32 as usize;
            let s0 = f64::from(pcm[i]);
            let fl = x.floor();
            let s1 = f64::from(pcm[i + 1]);
            let y = (pos - fl) * (s1 - s0) + s0;
            let out = (y as i32) as u16;
            c.ring.write(&out.to_le_bytes());
            pos += step;
            x = pos;
        }
        c.resample_frac = pos - x.floor();
        lap(&mut self.profile[2]);
        if let Some(rec) = &mut self.decomp_rec {
            let bytes: Vec<u8> = pcm.iter().flat_map(|v| v.to_le_bytes()).collect();
            rec.append(&bytes);
        }
        if s.showincoming != 0.0 {
            self.prints.push(format!("Voice - {n} incoming samples added to channel {ch}\n"));
        }
    }

    /// `Voice_Idle(dt)` up to the channel loop: the ack timer and the fade length. `false`
    /// when `voice_enable` is 0 (the caller runs `Voice_Deinit` and nothing else).
    pub fn idle_begin(&mut self, dt: f32, s: &VoiceSettings) -> bool {
        if s.enable == 0.0 {
            return false;
        }
        if self.ack {
            self.ack_timeout += dt;
            if self.ack_timeout > DELAY {
                self.ack = false;
                self.hud_events.push((-2, false));
            }
        }
        let fs = ((s.fadeouttime * 11025.0f32) as i32).max(2);
        self.fade_samples = fs;
        self.fade_mul = 1.0 / (fs - 1) as f32;
        true
    }

    /// One channel of `Voice_Idle`'s loop: `None` for a free channel, else what the mixer has
    /// to do now (`End` for a starved channel, then [`Self::end_channel`]; `Start` when the
    /// jitter delay just ran out, then [`Self::channel_started`]) and whether it counts as
    /// active.
    pub fn idle_channel(&mut self, i: usize, dt: f32) -> Option<(Option<MixerRequest>, bool)> {
        let c = &mut self.channels[i];
        if c.ent == -1 {
            return None;
        }
        // Starved channels are kept while tweaking.
        if c.starved && !self.in_tweak {
            return Some((Some(MixerRequest::End(i)), false));
        }
        let t = c.time_pad;
        c.time_pad = t - dt;
        let start = t > 0.0 && c.time_pad <= 0.0;
        Some((start.then_some(MixerRequest::Start(i)), true))
    }

    /// The tail of `Voice_Idle`: overdrive off without active channels, `VoiceSE_Idle`, and
    /// `voice_showchannels`.
    pub fn idle_end(&mut self, active: usize, dt: f32, s: &VoiceSettings) {
        if active == 0 && !self.in_tweak {
            self.overdrive_on = false;
        }
        self.overdrive_idle(dt, s);
        if s.showchannels >= 1.0 {
            for (i, c) in self.channels.iter().enumerate().filter(|(_, c)| c.ent != -1) {
                self.prints.push(format!("Voice - chan {i}, ent {}, bufsize: {}\n", c.ent, c.ring.count));
            }
        }
        // voice_profile: the frame's sums in µs; compress is only timed by the legacy encoder.
        if s.profile != 0.0 {
            let ([decomp, gain, ups], comp) = (self.profile, 0.0f64);
            self.prints.push(format!(
                "Voice - compress: {:7.2}u, decompress: {:7.2}u, gain: {:7.2}u, upsample: {:7.2}u, total: {:7.2}u\n",
                comp * 1e6,
                decomp * 1e6,
                gain * 1e6,
                ups * 1e6,
                (decomp + comp + gain + ups) * 1e6
            ));
            self.profile = [0.0; 3];
        }
    }

    /// After the mixer started channel `i` (`VoiceSE_StartChannel`): the carry reset and the
    /// talker's `HUD(ent, 1)` (re-reading the entity after the start).
    pub fn channel_started(&mut self, i: usize) {
        self.carry[i] = 0;
        let ent = self.channels[i].ent;
        self.hud_events.push((ent, true));
    }

    /// `Voice_EndChannel(i)` (also `VoiceSE_NotifyFreeChannel` from `S_FreeChannel`).
    pub fn end_channel(&mut self, i: usize) {
        let Some(c) = self.channels.get_mut(i) else { return };
        if c.ent >= 0 {
            self.hud_events.push((c.ent, false));
        }
        c.ent = -1;
    }

    /// `VoiceSE_Idle`: the raised-cosine ducking ramp toward `voice_overdrive`.
    fn overdrive_idle(&mut self, dt: f32, s: &VoiceSettings) {
        let tt = s.overdrivefadetime;
        let mut t = self.overdrive_t;
        if self.overdrive_on {
            if t == tt {
                return;
            }
            t = if tt < t + dt { tt } else { t + dt };
        } else {
            if t == 0.0 {
                return;
            }
            t = if 0.0 > t - dt { 0.0 } else { t - dt };
        }
        self.overdrive_t = t;
        let c = (f64::from(t / tt) * OVERDRIVE_PI).cos();
        let w = (0.5 - c * 0.5) as f32;
        self.overdrive = w * (s.overdrive - 1.0) + 1.0;
    }

    /// `VoiceSE_GetSoundDataCallback` + `Voice_GetOutputData`: `ns` samples at 22050 Hz for
    /// mixer channel `i` (`ns` even), the buffered tail faded, zeros past the end, `starved`
    /// once the ring is empty.
    pub fn pull(&mut self, i: usize, ns: i32, out: &mut Vec<i16>, s: &VoiceSettings) -> i32 {
        let n_samples = ns >> 1;
        out.clear();
        out.resize((n_samples * 2) as usize, 0);
        let (fade_samples, fade_mul) = (self.fade_samples, self.fade_mul);
        let Some(c) = self.channels.get_mut(i) else { return 0 };
        // Voice_GetOutputData (maxSamples is compared with the byte count; the cap never binds).
        let max_samples = 2048;
        let mut bytes = c.ring.count.min(max_samples).min(2 * n_samples);
        let mut raw = vec![0u8; bytes.max(0) as usize];
        bytes = c.ring.read(&mut raw, bytes);
        let mut got = bytes / 2;
        let mut buf: Vec<i16> = raw[..(got * 2) as usize].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let left = c.ring.count / 2;
        if left < fade_samples {
            let start = (left - fade_samples + got).max(0);
            let offset = (fade_samples - left - got).max(0);
            for k in 0..(got - start).max(0) {
                let idx = (start + k) as usize;
                let x = f32::from(buf[idx]);
                buf[idx] = ((1.0f32 - (k + offset) as f32 * fade_mul) * x) as i32 as i16;
            }
        }
        if got < n_samples {
            let m = max_samples.min(n_samples);
            buf.resize(m as usize, 0);
            got = m;
        }
        if c.ring.count == 0 {
            c.starved = true;
        }
        if s.showchannels >= 2.0 {
            self.prints.push(format!("Voice - mixed {got} samples from channel {i}\n"));
        }
        // ×2 upsample with the carried sample.
        let n = got as usize;
        for k in (1..n).rev() {
            out[2 * k + 1] = buf[k];
            out[2 * k] = ((i32::from(buf[k - 1]) + i32::from(buf[k])) >> 1) as i16;
        }
        if n >= 1 {
            out[1] = buf[0];
            out[0] = ((i32::from(buf[0]) + i32::from(self.carry[i])) >> 1) as i16;
            self.carry[i] = out[2 * n - 1];
        }
        (2 * n) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packet_writes_one_sample_less_each_close_to_the_next_input() {
        let mut v = Voice::default();
        let s = VoiceSettings::default();
        let ch = v.assign_channel(3, &s).unwrap_or(0);
        let mut pcm = vec![0i16, 1000, 2000, 3000];
        v.add_incoming(ch, Some(&mut pcm), 0.0, &s);
        assert_eq!(v.channels[ch].ring.count, 6, "n-1 samples");
        let mut out = Vec::new();
        assert_eq!(v.pull(ch, 6, &mut out, &s), 6);
        assert_eq!(out[1], 999, "~pcm[1] (0.999 blend, first chunk at gain 1)");
        assert_eq!(out[0], 499, "averaged with the zero carry");
        assert!(v.channels[ch].starved, "the ring ran dry");
    }

    #[test]
    fn auto_gain_ramps_each_chunk_toward_the_gain_of_the_previous_one() {
        let mut g = AutoGain::default();
        g.reset(128, 5.0, 0.5, 1.0);
        let mut quiet = vec![1000i16; 128];
        g.process(&mut quiet);
        assert_eq!(quiet[127], 1000, "the first chunk plays at gain 1");
        // level 1000 → 32767/1000 > maxgain: the next chunk ramps 1 → 5.
        let mut next = vec![1000i16; 128];
        g.process(&mut next);
        assert_eq!(next[0], 1000);
        assert!(next[127] > 4900 && next[127] <= 5000, "{}", next[127]);
    }
}
