//! Channels, spatialization and the software mixer (docs/re/audio.md §3, §6–§9): dynamic and
//! static channels, `S_AlterChannel`, sentences with per-word pitch / trimming / time
//! compression, the 11025 Hz base pass, the ×2 upsample, the 22050 Hz pass, the room DSP and
//! the final volume transfer to 16-bit stereo at 22050 Hz.

use std::collections::HashMap;
use std::sync::Arc;

use gs_fs::FileSystem;

use crate::dsp::RoomFx;
use crate::voice::{self, FileRecording, MixerRequest, Voice, VoiceCodec, VoiceSettings};
use crate::vox::{self, MAX_WORDS, Sentences};
use crate::wav::{self, BASE_RATE, Samples, SfxCache};

/// `MAX_CHANNELS`.
pub const MAX_CHANNELS: usize = 128;
/// Channels 0..4 are the (silent) ambients, 4..36 dynamic, 36.. static.
const FIRST_DYNAMIC: usize = 4;
const FIRST_STATIC: usize = 36;
/// `MAX_SFX`.
const MAX_SFX: usize = 1024;
/// Sentence / pitched-sound slots.
const VOX_SLOTS: usize = 16;
/// Paint chunk in 11025 Hz ticks.
const PAINT_CHUNK: i32 = 512;
/// Output rate (`dma.speed`).
pub const OUTPUT_RATE: u32 = 22050;
/// `sound_nominal_clip_dist`.
const NOMINAL_CLIP_DIST: f32 = 1000.0;
/// `sfxcache_t.length` of the voice pseudo-sfx (marks voice channels for the mixer).
const VOICE_LENGTH: i32 = 0x4000_0000;

/// Entity channels.
pub mod chan {
    pub const AUTO: i32 = 0;
    pub const WEAPON: i32 = 1;
    pub const VOICE: i32 = 2;
    pub const ITEM: i32 = 3;
    pub const BODY: i32 = 4;
    pub const STREAM: i32 = 5;
    pub const STATIC: i32 = 6;
    /// `pfnPlaySoundVoiceByName`.
    pub const VOICE_BY_NAME: i32 = 501;
}

/// `SND_*` flags.
pub mod flags {
    pub const STOP: i32 = 0x20;
    pub const CHANGE_VOL: i32 = 0x40;
    pub const CHANGE_PITCH: i32 = 0x80;
}

/// A known sound (`sfx_t` index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SfxId(usize);

#[derive(Debug, Clone)]
struct Sfx {
    name: String,
    cache: Option<Arc<SfxCache>>,
    /// Loading failed once; not retried.
    failed: bool,
}

/// `voxword_t`.
#[derive(Debug, Clone, Copy, Default)]
struct VoxWord {
    volume: i32,
    pitch: i32,
    start: i32,
    end: i32,
    cbtrim: i32,
    samplefrac: i32,
    timecompress: i32,
    sfx: Option<SfxId>,
}

/// `channel_t`.
#[derive(Debug, Clone, Copy)]
struct Channel {
    sfx: Option<SfxId>,
    leftvol: i32,
    rightvol: i32,
    end: i32,
    pos: i32,
    entnum: i32,
    entchannel: i32,
    origin: [f32; 3],
    dist_mult: f32,
    master_vol: i32,
    isentence: i32,
    iword: i32,
    pitch: i32,
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            sfx: None,
            leftvol: 0,
            rightvol: 0,
            end: 0,
            pos: 0,
            entnum: 0,
            entchannel: 0,
            origin: [0.0; 3],
            dist_mult: 0.0,
            master_vol: 0,
            isentence: -1,
            iword: 0,
            pitch: 100,
        }
    }
}

/// Mixer settings from the cvars.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// `volume`.
    pub volume: f32,
    /// `hisound`.
    pub hisound: bool,
    /// `room_type`, `waterroom_type`, `room_off`.
    pub room_type: i32,
    pub waterroom_type: i32,
    pub room_off: bool,
    /// The nine `room_*` cvars the DSP checks read.
    pub room: crate::dsp::RoomParams,
    /// `_snd_mixahead` (seconds painted ahead of the DMA position).
    pub mixahead: f32,
    /// `sys_timescale`.
    pub timescale: f32,
    /// `nosound`: no new dynamic sounds.
    pub nosound: bool,
    /// `snd_show`: each update lists the audible channels.
    pub snd_show: bool,
    /// `fs_precache_timings`: each sound load is timed into [`SoundSystem::dprints`].
    pub precache_timings: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: 0.7,
            hisound: true,
            room_type: 0,
            waterroom_type: 14,
            room_off: false,
            room: crate::dsp::RoomParams::default(),
            mixahead: 0.1,
            timescale: 1.0,
            nosound: false,
            snd_show: false,
            precache_timings: false,
        }
    }
}

/// The listener and the world state spatialization reads, set each frame by `S_Update`.
#[derive(Debug, Clone, Default)]
pub struct Listener {
    pub origin: [f32; 3],
    pub right: [f32; 3],
    /// `cl.viewentity`.
    pub view_entity: i32,
    /// Origins of the entities in the latest packet (brush models at their centre), by index.
    pub entity_origins: Vec<Option<[f32; 3]>>,
    /// `cl.waterlevel > 2` selects `waterroom_type`.
    pub underwater: bool,
    /// `cl.paused`.
    pub paused: bool,
}

/// The engine sound system.
pub struct SoundSystem {
    fs: FileSystem,
    known: Vec<Sfx>,
    by_name: HashMap<String, usize>,
    channels: Vec<Channel>,
    total_channels: usize,
    vox: [[VoxWord; MAX_WORDS + 1]; VOX_SLOTS],
    sentences: Sentences,
    /// `cl.sound_precache`.
    precache: Vec<Option<SfxId>>,
    /// In 11025 Hz ticks.
    paintedtime: i32,
    listener: Listener,
    room: RoomFx,
    /// The last base-rate frame of the previous chunk (upsampling).
    prev: [i32; 2],
    /// Paint buffers (`paintbuffer` at 11025 Hz, and its 22050 Hz upsample), reused.
    base_buf: Vec<[i32; 2]>,
    hi_buf: Vec<[i32; 2]>,
    /// The engine generator behind the de-phasing `RandomLong`.
    rng: gs_common::rng::EngineRng,
    /// `S_Play`'s fake entity counter.
    fake_entity: i32,
    pub settings: Settings,
    /// The voice engine; its channels play through the five `?VoiceData` pseudo-sfx, which
    /// are `SfxId(0..5)`.
    pub voice: Voice,
    pub voice_settings: VoiceSettings,
    /// Scratch for the voice callback (`v72[4096]`).
    voice_buf: Vec<i16>,
    /// `Con_DPrintf` lines (`fs_precache_timings`) for the owner to show at developer level.
    pub dprints: Vec<String>,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Reborrows an optional codec for a nested call (shortening the trait-object lifetime).
fn reborrow<'a>(codec: &'a mut Option<&mut dyn VoiceCodec>) -> Option<&'a mut dyn VoiceCodec> {
    match codec {
        Some(c) => Some(&mut **c),
        None => None,
    }
}

impl SoundSystem {
    /// `S_Init` + `VOX_ReadSentenceFile`; `rng` is the engine's shared generator.
    pub fn new(fs: FileSystem, rng: gs_common::rng::EngineRng) -> Self {
        let sentences = match fs.read("sound/sentences.txt") {
            Ok(d) => Sentences::parse(&String::from_utf8_lossy(&d)),
            Err(e) => {
                log::warn!("sound/sentences.txt: {e}");
                Sentences::default()
            }
        };
        // g_VoiceSfx: length 0x40000000 marks a voice cache; the "loop start" holds the data
        // callback, a positive address, so the mixer never frees a silent voice channel.
        let voice_cache = Arc::new(SfxCache { length: VOICE_LENGTH, loopstart: 1, speed: 2 * BASE_RATE, samples: Samples::S16(Vec::new()) });
        let known = (0..voice::CHANNELS).map(|_| Sfx { name: "?VoiceData".into(), cache: Some(voice_cache.clone()), failed: false }).collect();
        Self {
            fs,
            known,
            by_name: HashMap::new(),
            channels: vec![Channel::default(); MAX_CHANNELS],
            total_channels: FIRST_STATIC,
            vox: [[VoxWord::default(); MAX_WORDS + 1]; VOX_SLOTS],
            sentences,
            precache: Vec::new(),
            paintedtime: 0,
            listener: Listener::default(),
            room: RoomFx::default(),
            prev: [0; 2],
            base_buf: Vec::new(),
            hi_buf: Vec::new(),
            rng,
            fake_entity: 345,
            settings: Settings::default(),
            voice: Voice::default(),
            voice_settings: VoiceSettings::default(),
            voice_buf: Vec::new(),
            dprints: Vec::new(),
        }
    }

    /// The listener origin of the last update.
    pub fn listener_origin(&self) -> [f32; 3] {
        self.listener.origin
    }

    /// `cmd_soundlist`: every resident sample of `known_sfx` (the voice pseudo-sfx are not in
    /// that table) with its loop mark, bits, size in bytes and name, then the total.
    pub fn soundlist_text(&self) -> String {
        let mut out = String::new();
        let mut total = 0i32;
        for sfx in self.known.iter().skip(voice::CHANNELS) {
            let Some(sc) = &sfx.cache else { continue };
            let width = if sc.is_8bit() { 1 } else { 2 };
            let size = sc.length.wrapping_mul(width);
            total = total.wrapping_add(size);
            out.push_str(if sc.loopstart >= 0 { "L" } else { " " });
            out.push_str(&format!("({:2}b) {size:6} : {}\n", width * 8, sfx.name));
        }
        out.push_str(&format!("Total resident: {total}\n"));
        out
    }

    /// `total_channels`: the channel table's high-water mark (36 after a stop, up to 128).
    pub fn total_channels(&self) -> usize {
        self.total_channels
    }

    /// `cl.viewentity` of the last update.
    pub fn view_entity(&self) -> i32 {
        self.listener.view_entity
    }

    /// `S_Play`: a fresh entity number per played sample.
    pub fn next_fake_entity(&mut self) -> i32 {
        self.fake_entity += 1;
        self.fake_entity - 1
    }

    /// `S_FindName`: the sfx for `name`, registered on first use.
    pub fn find_name(&mut self, name: &str) -> Option<SfxId> {
        if name.is_empty() || gs_common::bytestr::encoded_len(name) >= 64 {
            return None;
        }
        let key = name.to_ascii_lowercase();
        if let Some(&i) = self.by_name.get(&key) {
            return Some(SfxId(i));
        }
        if self.known.len() - voice::CHANNELS >= MAX_SFX {
            log::warn!("S_FindName: out of sfx_t");
            return None;
        }
        self.known.push(Sfx { name: name.to_owned(), cache: None, failed: false });
        self.by_name.insert(key, self.known.len() - 1);
        Some(SfxId(self.known.len() - 1))
    }

    /// `CL_PrecacheResources` for sounds: `cl.sound_precache[i] = S_PrecacheSound(name)`.
    pub fn precache(&mut self, names: &[Option<String>]) {
        self.precache = names.iter().map(|n| n.as_deref().and_then(|n| self.precache_sound(n))).collect();
    }

    /// `S_PrecacheSound`: register, and load plain WAVs now.
    pub fn precache_sound(&mut self, name: &str) -> Option<SfxId> {
        let id = self.find_name(name)?;
        if !name.starts_with(['*', '!', '#']) {
            self.load(id);
        }
        Some(id)
    }

    /// `cl.sound_precache[index]`.
    pub fn precached(&self, index: usize) -> Option<SfxId> {
        self.precache.get(index).copied().flatten()
    }

    /// `CL_LookupSound`: an exact-case precache name, else `S_PrecacheSound`.
    pub fn lookup(&mut self, name: &str) -> Option<SfxId> {
        let found = self.precache.iter().flatten().copied().find(|id| self.known[id.0].name == name);
        found.or_else(|| self.precache_sound(name))
    }

    /// The `!NAME` sfx of wire sentence `index`.
    pub fn sentence_sfx(&mut self, index: usize) -> Option<SfxId> {
        let name = format!("!{}", self.sentences.name(index)?);
        self.find_name(&name)
    }

    /// `S_LoadSound`: the cached data, loading `sound/<name>` on first use.
    fn load(&mut self, id: SfxId) -> Option<Arc<SfxCache>> {
        let hisound = self.settings.hisound;
        // VoiceSE_GetSFXCache: the pseudo-sfx caches, whatever the rate setting.
        if id.0 < voice::CHANNELS {
            return self.known[id.0].cache.clone();
        }
        let sfx = self.known.get_mut(id.0)?;
        if let Some(c) = &sfx.cache {
            // A 22050 Hz cache reloads when hisound went off.
            if hisound || c.speed == BASE_RATE {
                return Some(c.clone());
            }
        }
        if sfx.failed || sfx.name.starts_with(['!', '#', '?']) {
            return None;
        }
        let name = sfx.name.trim_start_matches('*');
        let path = if name.starts_with('/') { format!("sound{name}") } else { format!("sound/{name}") };
        let started = std::time::Instant::now();
        let loaded = self.fs.read(&path).map_err(|e| e.to_string()).and_then(|d| wav::load(&d, hisound));
        let sfx = &mut self.known[id.0];
        match loaded {
            Ok(c) => {
                if self.settings.precache_timings {
                    let secs = started.elapsed().as_secs_f64() as f32;
                    self.dprints.push(format!("fs_precache_timings: loaded sound {path} in time {secs:.3} sec\n"));
                }
                let c = Arc::new(c);
                sfx.cache = Some(c.clone());
                Some(c)
            }
            Err(e) => {
                log::debug!("S_LoadSound: Couldn't load {path}: {e}");
                sfx.failed = true;
                None
            }
        }
    }

    /// `S_StopAllSounds`: every channel with an sfx freed (voice notified), then all zeroed.
    pub fn stop_all(&mut self) {
        for c in 0..self.total_channels {
            if self.channels[c].sfx.is_some() {
                self.free_channel(c);
            }
        }
        self.channels.iter_mut().for_each(|c| *c = Channel::default());
        self.vox.iter_mut().for_each(|s| s[0].sfx = None);
        self.total_channels = FIRST_STATIC;
    }

    /// `S_StopSound(ent, chan)` (`svc_stopsound`).
    pub fn stop(&mut self, ent: i32, chan: i32) {
        for c in FIRST_DYNAMIC..self.total_channels {
            if self.channels[c].entnum == ent && self.channels[c].entchannel == chan {
                self.free_channel(c);
            }
        }
    }

    /// `Voice_Init` (startup and `svc_voiceinit`): nothing with `voice_enable 0`; else
    /// `Voice_Deinit`, then voice is up, on Steam when there is a codec.
    pub fn voice_init(&mut self, mut codec: Option<&mut dyn VoiceCodec>) -> bool {
        if self.voice_settings.enable == 0.0 {
            return false;
        }
        self.voice_deinit(reborrow(&mut codec));
        self.voice.init_done(codec.is_some());
        true
    }

    /// `Voice_Deinit`: every channel ended through the mixer, recording stopped.
    pub fn voice_deinit(&mut self, codec: Option<&mut dyn VoiceCodec>) {
        if !self.voice.take_initted() {
            return;
        }
        // Voice_EndAllChannels.
        for i in 0..voice::CHANNELS {
            self.stop(self.listener.view_entity, voice::CHAN_BASE + i as i32);
            self.voice.end_channel(i);
        }
        self.voice_record_stop(codec);
    }

    /// `Voice_RecordStart(micFile, decompFile, …)` (`+voicerecord`; `voice_inputfromfile` is
    /// loaded but unused on the Steam path, so it is not read).
    pub fn voice_record_start(&mut self, mut codec: Option<&mut dyn VoiceCodec>, to_file: bool) {
        self.voice_record_stop(reborrow(&mut codec));
        if to_file {
            self.voice.mic_rec = Some(FileRecording::new("voice_micdata.wav"));
            self.voice.decomp_rec = Some(FileRecording::new("voice_decompressed.wav"));
        }
        let ok = match codec {
            Some(codec) if self.voice.using_steam() => {
                codec.start_recording();
                true
            }
            _ => false,
        };
        self.voice.record_started(ok);
    }

    /// `Voice_RecordStop`: the capture files written, Steam recording stopped.
    pub fn voice_record_stop(&mut self, codec: Option<&mut dyn VoiceCodec>) {
        for rec in [self.voice.mic_rec.take(), self.voice.decomp_rec.take()].into_iter().flatten() {
            if let Err(e) = self.fs.write(&rec.name, gs_fs::PathId::Game, &rec.wave()) {
                log::warn!("WriteWaveFile {}: {e}", rec.name);
            }
        }
        if let Some(codec) = codec
            && self.voice.using_steam()
        {
            codec.stop_recording();
        }
        self.voice.record_stopped();
    }

    /// `Voice_GetCompressedData` (Steam branch): the next compressed chunk into `out`, the
    /// uncompressed copy appended to `voice_micdata.wav` when recording to file.
    pub fn voice_compressed(&mut self, codec: Option<&mut dyn VoiceCodec>, out: &mut [u8]) -> usize {
        let Some(codec) = codec.filter(|_| self.voice.using_steam()) else { return 0 };
        match &mut self.voice.mic_rec {
            Some(rec) => {
                let used = rec.used;
                let (written, uncompressed) = codec.get_voice(out, Some(&mut rec.data[used..]));
                rec.used += uncompressed.min(FileRecording::SIZE - used);
                written
            }
            None => codec.get_voice(out, None).0,
        }
    }

    /// `CL_ParseVoiceData` from the entity checks on: the local ack, the talker's channel
    /// (assigned on first data), and `Voice_AddIncomingData`.
    pub fn voice_data(&mut self, codec: Option<&mut dyn VoiceCodec>, ent: i32, local: bool, data: &[u8]) {
        if local {
            self.voice.local_ack();
        }
        if data.is_empty() {
            return;
        }
        let s = self.voice_settings;
        let Some(ch) = self.voice.channel_of(ent).or_else(|| self.voice.assign_channel(ent, &s)) else {
            log::debug!("CL_ParseVoiceData: Voice_AssignChannel failed for client {}!", ent - 1);
            return;
        };
        let mut raw = [0u8; 8192];
        let started = std::time::Instant::now();
        let decoded = if self.voice.using_steam() {
            // No codec on the Steam path cannot happen in the original (SteamUser() is live).
            match codec.and_then(|c| c.decompress(ent, data, &mut raw)) {
                Some(0) | None => None,
                Some(w) => Some(w.min(raw.len()) >> 1),
            }
        } else {
            // Legacy path without a codec DLL: zero samples decoded.
            Some(0)
        };
        let decode_secs = started.elapsed().as_secs_f64();
        let mut pcm: Vec<i16> = raw[..decoded.unwrap_or(0) * 2].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        self.voice.add_incoming(ch, decoded.map(|_| pcm.as_mut_slice()), decode_secs, &s);
    }

    /// `VoiceTweak_StartVoiceTweakMode`: without voice nothing starts; else every channel
    /// ended, recording restarted, the local mic routed to a channel of entity −500.
    pub fn voice_tweak_start(&mut self, mut codec: Option<&mut dyn VoiceCodec>) -> bool {
        if self.voice.in_tweak || !self.voice.initted() {
            return false;
        }
        for i in 0..voice::CHANNELS {
            self.stop(self.listener.view_entity, voice::CHAN_BASE + i as i32);
            self.voice.end_channel(i);
        }
        self.voice_record_stop(reborrow(&mut codec));
        let ok = match codec {
            Some(codec) if self.voice.using_steam() => {
                codec.start_recording();
                true
            }
            _ => false,
        };
        self.voice.record_started(ok);
        let s = self.voice_settings;
        self.voice.assign_any(voice::TWEAK_ENTITY, &s);
        self.voice.in_tweak = true;
        true
    }

    /// `VoiceTweak_EndVoiceTweakMode`.
    pub fn voice_tweak_end(&mut self, codec: Option<&mut dyn VoiceCodec>) {
        if !self.voice.in_tweak {
            return;
        }
        self.voice.in_tweak = false;
        self.voice_record_stop(codec);
    }

    /// `Voice_AddIncomingData(-100, …)` in tweak mode: the local mic's compressed chunk decoded
    /// into channel 0 and `g_VoiceTweakSpeakingVolume` taken from its loudest sample.
    fn voice_tweak_incoming(&mut self, codec: Option<&mut dyn VoiceCodec>, data: &[u8]) {
        self.voice.tweak_speaking_volume = 0;
        if data.is_empty() {
            return;
        }
        let s = self.voice_settings;
        let mut raw = [0u8; 8192];
        let started = std::time::Instant::now();
        let decoded = match codec.filter(|_| self.voice.using_steam()).and_then(|c| c.decompress(voice::TWEAK_ENTITY, data, &mut raw)) {
            Some(0) | None => None,
            Some(w) => Some(w.min(raw.len()) >> 1),
        };
        let mut pcm: Vec<i16> = raw[..decoded.unwrap_or(0) * 2].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        if decoded.is_some() {
            let loudest = pcm.iter().map(|&v| i32::from(v).abs()).max().unwrap_or(0);
            self.voice.tweak_speaking_volume = loudest & 0xFE00;
        }
        // voice_profile's decompress window also covers the tweak-volume scan.
        let decode_secs = started.elapsed().as_secs_f64();
        self.voice.add_incoming(0, decoded.map(|_| pcm.as_mut_slice()), decode_secs, &s);
    }

    /// `Voice_Idle(dt)` with its mixer calls in the original order (`S_StopSound` of starved
    /// channels, `S_StartDynamicSound` of channels whose jitter delay ran out).
    pub fn voice_idle(&mut self, dt: f32, mut codec: Option<&mut dyn VoiceCodec>) {
        let s = self.voice_settings;
        if !self.voice.idle_begin(dt, &s) {
            self.voice_deinit(codec);
            return;
        }
        if self.voice.in_tweak {
            let mut tmp = [0u8; 2048];
            let k = self.voice_compressed(reborrow(&mut codec), &mut tmp);
            self.voice_tweak_incoming(reborrow(&mut codec), &tmp[..k]);
        }
        let mut active = 0;
        for i in 0..voice::CHANNELS {
            let Some((request, counts)) = self.voice.idle_channel(i, dt) else { continue };
            match request {
                Some(MixerRequest::End(i)) => {
                    self.stop(self.listener.view_entity, voice::CHAN_BASE + i as i32);
                    self.voice.end_channel(i);
                }
                Some(MixerRequest::Start(i)) => {
                    // VoiceSE_StartChannel: the view entity, channel 7+i, full volume, ATTN 1.
                    self.start_dynamic(self.listener.view_entity, voice::CHAN_BASE + i as i32, SfxId(i), [0.0; 3], 1.0, 1.0, 0, 100);
                    self.voice.channel_started(i);
                }
                None => {}
            }
            active += usize::from(counts);
        }
        self.voice.idle_end(active, dt, &s);
    }

    /// `S_FreeChannel`. Entity and channel stay, so `S_StopSound` can free a freed channel
    /// again; any entchannel 7..500 ends voice channel `entchannel - 7`
    /// (`VoiceSE_NotifyFreeChannel`; beyond 11 the original asserts, ignored here).
    fn free_channel(&mut self, c: usize) {
        let ch = &mut self.channels[c];
        if (voice::CHAN_BASE..=500).contains(&ch.entchannel) {
            self.voice.end_channel((ch.entchannel - voice::CHAN_BASE) as usize);
        }
        if let Ok(slot) = usize::try_from(ch.isentence) {
            self.vox[slot][0].sfx = None;
        }
        ch.isentence = -1;
        ch.sfx = None;
    }

    fn is_sentence(&self, id: SfxId) -> bool {
        self.known[id.0].name.starts_with(['!', '#'])
    }

    /// `S_AlterChannel`: change or stop the matching sound; `true` when one matched.
    fn alter(&mut self, ent: i32, chan: i32, sfx: SfxId, vol: i32, pitch: i32, flags: i32) -> bool {
        let sentence = self.known[sfx.0].name.starts_with('!');
        for c in FIRST_DYNAMIC..self.total_channels {
            let ch = self.channels[c];
            if ch.entnum != ent || ch.entchannel != chan {
                continue;
            }
            let matched = if sentence { ch.sfx.is_some() && ch.isentence >= 0 } else { ch.sfx == Some(sfx) };
            if !matched {
                continue;
            }
            if flags & flags::CHANGE_PITCH != 0 {
                self.channels[c].pitch = pitch;
            }
            if flags & flags::CHANGE_VOL != 0 {
                self.channels[c].master_vol = vol;
            }
            if flags & flags::STOP != 0 {
                self.free_channel(c);
            }
            return true;
        }
        false
    }

    /// The common prologue: effective channel, volume and pitch, or `None` when the call was
    /// a change/stop that is done (or a pitch of 0).
    fn prologue(&mut self, sfx: SfxId, chan: i32, ent: i32, fvol: f32, pitch: i32, flags: i32) -> Option<(i32, i32, i32)> {
        let mut chan = chan;
        let mut pitch = pitch;
        if self.known[sfx.0].name.starts_with('*') {
            chan = chan::STREAM;
            pitch = 100;
        }
        let vol = ((fvol * 255.0) as i32).min(255);
        if flags & (flags::STOP | flags::CHANGE_VOL | flags::CHANGE_PITCH) != 0 && (self.alter(ent, chan, sfx, vol, pitch, flags) || flags & flags::STOP != 0) {
            return None;
        }
        if pitch == 0 {
            log::debug!("Warning: S_StartSound Ignored, called with pitch 0");
            return None;
        }
        Some((chan, vol, (((self.settings.timescale + 1.0) * 0.5) * pitch as f32) as i32))
    }

    /// `S_StartDynamicSound`.
    #[allow(clippy::too_many_arguments)]
    pub fn start_dynamic(&mut self, ent: i32, chan: i32, sfx: SfxId, origin: [f32; 3], fvol: f32, attn: f32, flags: i32, pitch: i32) {
        if self.settings.nosound {
            return;
        }
        let Some((chan, vol, pitch)) = self.prologue(sfx, chan, ent, fvol, pitch, flags) else { return };
        if chan == chan::STREAM && self.channels[FIRST_DYNAMIC..self.total_channels].iter().any(|c| c.sfx == Some(sfx)) {
            return;
        }
        // SND_PickDynamicChannel.
        let view = self.listener.view_entity;
        let mut pick = None;
        let mut life_left = i32::MAX;
        for c in FIRST_DYNAMIC..FIRST_STATIC {
            let ch = &self.channels[c];
            if chan != 0 && ch.entnum == ent && (ch.entchannel == chan || chan == -1) {
                pick = Some(c);
                break;
            }
            if ch.entnum == view && ent != view && ch.sfx.is_some() {
                continue;
            }
            if ch.end - self.paintedtime < life_left {
                life_left = ch.end - self.paintedtime;
                pick = Some(c);
            }
        }
        let Some(c) = pick else { return };
        if self.channels[c].sfx.is_some() {
            self.free_channel(c);
        }
        self.channels[c] = Channel {
            origin,
            master_vol: vol,
            entnum: ent,
            entchannel: chan,
            pitch,
            isentence: -1,
            dist_mult: attn / NOMINAL_CLIP_DIST,
            ..Channel::default()
        };
        self.spatialize(c);
        if self.channels[c].leftvol == 0 && self.channels[c].rightvol == 0 && chan != chan::STREAM {
            return;
        }
        if !self.start_channel(c, sfx, pitch) {
            return;
        }
        // De-phase two copies of one sound started together.
        let other = (FIRST_DYNAMIC..FIRST_STATIC).find(|&o| o != c && self.channels[o].sfx == Some(sfx) && self.channels[o].pos == 0);
        if other.is_some() {
            let mut n = self.rng.long(0, (BASE_RATE as f32 * 0.1) as i32);
            let ch = &mut self.channels[c];
            if n >= ch.end {
                n = ch.end - 1;
            }
            ch.pos += n;
            ch.end -= n;
        }
    }

    /// `S_StartStaticSound`: never stolen, ends with the sample.
    #[allow(clippy::too_many_arguments)]
    pub fn start_static(&mut self, ent: i32, chan: i32, sfx: SfxId, origin: [f32; 3], fvol: f32, attn: f32, flags: i32, pitch: i32) {
        let Some((chan, vol, pitch)) = self.prologue(sfx, chan, ent, fvol, pitch, flags) else { return };
        if chan == chan::STREAM && self.channels[FIRST_DYNAMIC..self.total_channels].iter().any(|c| c.sfx == Some(sfx)) {
            return;
        }
        let c = match (FIRST_STATIC..self.total_channels).find(|&c| self.channels[c].sfx.is_none()) {
            Some(c) => c,
            None if self.total_channels < MAX_CHANNELS => {
                self.total_channels += 1;
                self.total_channels - 1
            }
            None => {
                log::debug!("total_channels == MAX_CHANNELS");
                return;
            }
        };
        self.channels[c] = Channel { origin, master_vol: vol, entnum: ent, entchannel: chan, pitch, dist_mult: attn / NOMINAL_CLIP_DIST, ..Channel::default() };
        if self.start_channel(c, sfx, pitch) {
            self.spatialize(c);
        }
    }

    /// Loads the sound (or sentence) onto channel `c` and sets its span; frees it on failure.
    fn start_channel(&mut self, c: usize, sfx: SfxId, pitch: i32) -> bool {
        let sentence = self.is_sentence(sfx);
        let loaded = if sentence {
            let name = self.known[sfx.0].name[1..].to_owned();
            self.vox_load(c, &name)
        } else {
            self.channels[c].sfx = Some(sfx);
            self.load(sfx)
        };
        let Some(sc) = loaded else {
            self.free_channel(c);
            return false;
        };
        let ch = &mut self.channels[c];
        ch.pos = 0;
        ch.end = self.paintedtime + sc.length;
        if !sentence && pitch != 100 {
            self.single_word_sentence(c, pitch);
        }
        self.trim_start_end(c, &sc);
        true
    }

    /// `VOX_LoadSound`: the sentence's words into a free slot, the first word loaded.
    fn vox_load(&mut self, c: usize, name: &str) -> Option<Arc<SfxCache>> {
        if name.starts_with('#') {
            log::debug!("VOX_LoadSound: sequence sentences are not supported ({name})");
            return None;
        }
        let Some(text) = self.sentences.text(name).map(str::to_owned) else {
            log::debug!("VOX_LoadSound: Couldn't find sentence {name}");
            return None;
        };
        let (dir, words) = vox::parse_sentence(&text);
        let Some(slot) = (0..VOX_SLOTS).find(|&s| self.vox[s][0].sfx.is_none()) else {
            log::debug!("VOX_IFindEmptySentence: Sentence or Pitch shift ignored. > 16 playing!");
            return None;
        };
        let mut n = 0;
        for (word, p) in words {
            let Some(sfx) = self.find_name(&format!("{dir}{word}.wav")) else { continue };
            self.vox[slot][n] = VoxWord {
                volume: p.volume,
                pitch: p.pitch,
                start: p.start,
                end: p.end,
                cbtrim: 0,
                samplefrac: 0,
                timecompress: p.timecompress,
                sfx: Some(sfx),
            };
            n += 1;
        }
        self.vox[slot][n].sfx = None;
        let first = self.vox[slot][0].sfx?;
        let ch = &mut self.channels[c];
        ch.isentence = slot as i32;
        ch.iword = 0;
        ch.sfx = Some(first);
        let sc = self.load(first);
        if sc.is_none() {
            self.vox[slot][0].sfx = None;
            self.channels[c].isentence = -1;
        }
        sc
    }

    /// `VOX_MakeSingleWordSentence`: a pitched sound takes a VOX slot (or plays unpitched).
    fn single_word_sentence(&mut self, c: usize, pitch: i32) {
        match (0..VOX_SLOTS).find(|&s| self.vox[s][0].sfx.is_none()) {
            Some(slot) => {
                let sfx = self.channels[c].sfx;
                self.vox[slot][0] = VoxWord { volume: 100, pitch: 100, start: 0, end: 100, cbtrim: 0, samplefrac: 0, timecompress: 0, sfx };
                self.vox[slot][1].sfx = None;
                let ch = &mut self.channels[c];
                ch.isentence = slot as i32;
                ch.iword = 0;
                ch.pitch = pitch;
            }
            None => self.channels[c].pitch = 100,
        }
    }

    /// `VOX_TrimStartEndTimes`: a word's `s`/`e` percentages, snapped to near-zero samples.
    fn trim_start_end(&mut self, c: usize, sc: &SfxCache) {
        let ch = &mut self.channels[c];
        let Ok(slot) = usize::try_from(ch.isentence) else { return };
        let word = &mut self.vox[slot][ch.iword as usize];
        let length = sc.length;
        word.cbtrim = length;
        if word.start >= word.end {
            return;
        }
        if word.start > 0 && word.start < 100 {
            let skip = (word.start as f32 / 100.0 * length as f32) as i32;
            ch.pos += skip;
            if let Some(scanned) = (0..255).take_while(|k| ch.pos + k < length).find(|&k| sc.sample(ch.pos + k).abs() <= 2) {
                ch.pos += scanned;
                ch.end -= skip + scanned;
            }
            if word.pitch != 100 {
                word.samplefrac += ch.pos << 8;
            }
        }
        if word.end > 0 && word.end < 100 {
            let skip = ((100 - word.end) as f32 / 100.0 * length as f32) as i32;
            ch.end -= skip;
            let from = length - skip;
            if let Some(scanned) = (0..255).take_while(|k| from - k > ch.pos).find(|&k| sc.sample(from - k).abs() <= 2) {
                ch.end -= scanned;
                word.cbtrim -= scanned + skip;
            }
        }
    }

    /// `SND_Spatialize`.
    fn spatialize(&mut self, c: usize) {
        let ch = &mut self.channels[c];
        if ch.entnum == self.listener.view_entity {
            ch.leftvol = ch.master_vol;
            ch.rightvol = ch.master_vol;
        } else {
            if ch.entnum > 0
                && let Some(Some(o)) = usize::try_from(ch.entnum).ok().and_then(|i| self.listener.entity_origins.get(i))
            {
                ch.origin = *o;
            }
            let v = [0, 1, 2].map(|i| ch.origin[i] - self.listener.origin[i]);
            let dist = dot(v, v).sqrt();
            let v = if dist > 0.0 { v.map(|x| x / dist) } else { v };
            let d = dist * ch.dist_mult;
            let side = dot(v, self.listener.right);
            let (rscale, lscale) = (1.0 + side, 1.0 - side);
            ch.rightvol = (rscale * (1.0 - d) * ch.master_vol as f32) as i32;
            ch.leftvol = (lscale * (1.0 - d) * ch.master_vol as f32) as i32;
        }
        // VOX_SetChanVol.
        if let Ok(slot) = usize::try_from(ch.isentence) {
            let volume = self.vox[slot][ch.iword as usize].volume;
            if volume != 100 {
                ch.leftvol = ch.leftvol * volume / 100;
                ch.rightvol = ch.rightvol * volume / 100;
            }
        }
        if ch.entnum != self.listener.view_entity {
            ch.leftvol = ch.leftvol.max(0);
            ch.rightvol = ch.rightvol.max(0);
        }
    }

    /// The preset `SX_RoomFX` applied since the last call: its values go into the `room_*`
    /// cvars (`Cvar_SetValue`).
    pub fn take_room_preset(&mut self) -> Option<crate::dsp::RoomParams> {
        self.room.preset_written.take()
    }

    /// `S_Update`: spatialize, then paint up to `soundtime + mixahead` (22050 Hz frames);
    /// `out` receives the new output frames. With `snd_show` the result is its listing: each
    /// audible channel after spatialization, then their count.
    pub fn update(&mut self, listener: Listener, soundtime: u64, out: &mut Vec<[i16; 2]>) -> Option<String> {
        self.listener = listener;
        for c in FIRST_DYNAMIC..self.total_channels {
            if self.channels[c].sfx.is_some() {
                self.spatialize(c);
            }
        }
        let shown = self.settings.snd_show.then(|| {
            let mut text = String::new();
            let mut audible = 0;
            for ch in self.channels[..self.total_channels].iter().filter(|ch| ch.leftvol != 0 || ch.rightvol != 0) {
                if let Some(sfx) = ch.sfx {
                    text.push_str(&format!("{:3} {:3} {}\n", ch.leftvol, ch.rightvol, self.sfx_name(sfx)));
                    audible += 1;
                }
            }
            text.push_str(&format!("----({audible})----\n"));
            text
        });
        let played = (soundtime / 2) as i32;
        if self.paintedtime < played {
            self.paintedtime = played;
        }
        // endtime = soundtime + speed·_snd_mixahead, at most the DMA buffer (64 KB of 16-bit
        // stereo: 16384 frames) ahead.
        let ahead = i64::from((self.settings.mixahead * OUTPUT_RATE as f32) as i32).min(16384);
        let end = ((soundtime as i64 + ahead) / 2) as i32;
        out.clear();
        while self.paintedtime < end {
            let n = (end - self.paintedtime).min(PAINT_CHUNK);
            self.paint(n, out);
        }
        shown
    }

    /// `S_PrintChannels` (`s_show`): every channel slot below `total_channels` as its origin,
    /// entity, entity channel and sound, `None` when free.
    pub fn channel_lines(&self) -> Vec<Option<String>> {
        self.channels[..self.total_channels]
            .iter()
            .map(|ch| {
                let sfx = ch.sfx?;
                let [x, y, z] = ch.origin;
                Some(format!("{x:.4} {y:.4} {z:.4} {:03} {:02} {:>20}", ch.entnum, ch.entchannel, self.sfx_name(sfx)))
            })
            .collect()
    }

    fn sfx_name(&self, sfx: SfxId) -> &str {
        self.known.get(sfx.0).map_or("", |s| s.name.as_str())
    }

    /// `S_PaintChannels` for `n` ticks.
    fn paint(&mut self, n: i32, out: &mut Vec<[i16; 2]>) {
        let s = self.settings;
        let room_type = if self.listener.underwater { s.waterroom_type } else { s.room_type };
        let end = self.paintedtime + n;
        let (mut base, mut hi) = (std::mem::take(&mut self.base_buf), std::mem::take(&mut self.hi_buf));
        base.clear();
        base.resize(n as usize, [0; 2]);
        if !self.listener.paused {
            self.mix(&mut base, end, false);
        }
        if !s.hisound {
            self.room.process(&mut base, room_type, false, s.room_off, s.room);
        }
        // ×2 upsample: odd frames copy, even frames average with the previous input.
        hi.clear();
        for f in &base {
            hi.push([(f[0] + self.prev[0]) >> 1, (f[1] + self.prev[1]) >> 1]);
            hi.push(*f);
            self.prev = *f;
        }
        if s.hisound {
            if !self.listener.paused {
                self.mix(&mut hi, end, true);
            }
            self.room.process(&mut hi, room_type, true, s.room_off, s.room);
        }
        // Pass C: voice, after the DSP and the upsample, at 22050 Hz.
        if !self.listener.paused {
            self.mix_voice(&mut hi, end);
        }
        // S_TransferStereo16.
        let vol = (s.volume * 256.0) as i32;
        out.extend(hi.iter().map(|f| f.map(|v| ((v * vol) >> 8).clamp(-32768, 32767) as i16)));
        self.paintedtime = end;
        (self.base_buf, self.hi_buf) = (base, hi);
    }

    /// `S_MixChannelsToPaintbuffer`: the channels of one rate into `buf` (from `paintedtime`);
    /// voice channels only in [`Self::mix_voice`]. Volumes are ducked by the voice overdrive.
    fn mix(&mut self, buf: &mut [[i32; 2]], end: i32, hires: bool) {
        let overdrive = self.voice.overdrive;
        for c in 0..self.total_channels {
            let Some(sfx) = self.channels[c].sfx else { continue };
            let Some(mut sc) = self.load(sfx) else { continue };
            if sc.length == VOICE_LENGTH || (hires && sc.speed <= BASE_RATE) || (!hires && sc.speed != BASE_RATE) {
                continue;
            }
            let ch = self.channels[c];
            if ch.leftvol == 0 && ch.rightvol == 0 {
                if sc.loopstart < 0 {
                    self.free_channel(c);
                }
                continue;
            }
            let (lv, rv) = ((ch.leftvol as f32 / overdrive) as i32, (ch.rightvol as f32 / overdrive) as i32);
            let mut ltime = self.paintedtime;
            while ltime < end {
                let ch = self.channels[c];
                let (mut p, mut tc) = (ch.pitch, 0);
                if let Ok(slot) = usize::try_from(ch.isentence) {
                    let word = self.vox[slot][ch.iword as usize];
                    if word.pitch > 0 {
                        p = word.pitch + ch.pitch - 100;
                    }
                    tc = word.timecompress;
                }
                let ch_end = if hires { ch.end + (sc.length >> 1) - sc.length } else { ch.end };
                let cnt = ch_end.min(end) - ltime;
                let shift = u32::from(hires);
                let ns = cnt << shift;
                let ofs = ((ltime - self.paintedtime) << shift) as usize;
                let mut finished = false;
                if ns > 0 {
                    let dst = &mut buf[ofs..ofs + ns as usize];
                    if sc.is_8bit() && (p != 100 || tc != 0) && ch.isentence >= 0 {
                        finished = self.vox_mix(c, &sc, dst, p, tc, lv, rv);
                    } else {
                        mix_plain(dst, &sc, ch.pos, lv, rv);
                        self.channels[c].pos += ns;
                    }
                    ltime += cnt;
                }
                if finished || ltime >= ch_end {
                    match self.loop_or_next_word(c, ltime, &sc) {
                        Some(next) => sc = next,
                        None => break,
                    }
                }
            }
        }
    }

    /// Pass C (`voiceOnly`): voice channels pulled through `VoiceSE_GetSoundDataCallback` at
    /// 22050 Hz, not ducked; a silent one is skipped but never freed.
    fn mix_voice(&mut self, buf: &mut [[i32; 2]], end: i32) {
        let s = self.voice_settings;
        let mut tmp = std::mem::take(&mut self.voice_buf);
        for c in 0..self.total_channels {
            let Some(sfx) = self.channels[c].sfx else { continue };
            if sfx.0 >= voice::CHANNELS {
                continue;
            }
            let ch = self.channels[c];
            if ch.leftvol == 0 && ch.rightvol == 0 {
                continue;
            }
            let ch_end = ch.end + (VOICE_LENGTH >> 1) - VOICE_LENGTH;
            let mut ltime = self.paintedtime;
            while ltime < end {
                let cnt = ch_end.min(end) - ltime;
                let got = self.voice.pull(sfx.0, cnt << 1, &mut tmp, &s);
                if got <= 0 {
                    break;
                }
                let ofs = ((ltime - self.paintedtime) << 1) as usize;
                for (f, &v) in buf[ofs..ofs + got as usize].iter_mut().zip(&tmp) {
                    f[0] += (i32::from(v) * ch.leftvol) >> 8;
                    f[1] += (i32::from(v) * ch.rightvol) >> 8;
                }
                self.channels[c].pos += got;
                ltime += got >> 1;
            }
        }
        self.voice_buf = tmp;
    }

    /// `SND_LoopOrNextWord`: loop, go to the next word, or free; the cache to continue with.
    fn loop_or_next_word(&mut self, c: usize, ltime: i32, sc: &Arc<SfxCache>) -> Option<Arc<SfxCache>> {
        let ch = self.channels[c];
        if sc.loopstart >= 0 {
            let chm = &mut self.channels[c];
            chm.pos = sc.loopstart;
            chm.end = ltime + sc.length - sc.loopstart;
            if let Ok(slot) = usize::try_from(ch.isentence) {
                self.vox[slot][ch.iword as usize].samplefrac = 0;
            }
            return Some(sc.clone());
        }
        if let Ok(slot) = usize::try_from(ch.isentence) {
            let next = ch.iword as usize + 1;
            if next <= MAX_WORDS
                && let Some(sfx) = self.vox[slot][next].sfx
                && let Some(nsc) = self.load(sfx)
            {
                let chm = &mut self.channels[c];
                chm.sfx = Some(sfx);
                chm.pos = 0;
                chm.iword = next as i32;
                chm.end = ltime + nsc.length;
                self.trim_start_end(c, &nsc);
                return Some(nsc);
            }
        }
        self.free_channel(c);
        None
    }

    /// `VOX_MixChannel`: an 8-bit word with pitch (nearest-neighbour 8.8 step) and time
    /// compression; `true` when the word is done.
    #[allow(clippy::too_many_arguments)]
    fn vox_mix(&mut self, c: usize, sc: &SfxCache, dst: &mut [[i32; 2]], p: i32, tc: i32, lv: i32, rv: i32) -> bool {
        let ch = self.channels[c];
        let slot = ch.isentence as usize;
        let word = self.vox[slot][ch.iword as usize];
        let (lq, rq) = ((lv.min(255) >> 3) << 3, (rv.min(255) >> 3) << 3);
        let rate = (p as f64 / 100.0 * 256.0) as i32;
        let cbtrim = word.cbtrim;
        let count = dst.len();
        let mut acc = word.samplefrac + rate;
        let mut src = word.samplefrac >> 8;
        let mut i = 0;
        let mut put = |i: &mut usize, src: i32, acc: &mut i32| -> i32 {
            let s = sc.sample(src);
            dst[*i][0] += s * lq;
            dst[*i][1] += s * rq;
            *i += 1;
            let next = *acc >> 8;
            *acc += rate;
            next
        };
        if tc == 0 {
            while i < count && src < cbtrim {
                src = put(&mut i, src, &mut acc);
            }
        } else {
            let chunk = (cbtrim / 8).max(1);
            let skiplen = tc * chunk / 100;
            let near_zero = |s: i32| (s as i8 as u8) <= 2;
            while i < count && src < cbtrim {
                if src >= skiplen && src % chunk < skiplen {
                    // Inside a later chunk's skip zone: finish on a near-zero sample, jump past
                    // the zone, and resume on a near-zero sample.
                    let mut k = 0;
                    while i < count && src < cbtrim && k < 255 && !near_zero(sc.sample(src)) {
                        src = put(&mut i, src, &mut acc);
                        k += 1;
                    }
                    while src < cbtrim && src >= skiplen && src % chunk < skiplen {
                        let jump = skiplen - src % chunk;
                        src += jump;
                        acc += jump << 8;
                        let mut k = 0;
                        while src < cbtrim && k < 255 && !near_zero(sc.sample(src)) {
                            src = acc >> 8;
                            acc += rate;
                            k += 1;
                        }
                    }
                }
                // Play the rest of this chunk.
                let m = chunk - src % chunk;
                let outs = (((m << 8) / rate.max(1)).max(1) as usize).min(count - i);
                for _ in 0..outs {
                    if src >= cbtrim {
                        break;
                    }
                    src = put(&mut i, src, &mut acc);
                }
            }
        }
        let newpos = acc >> 8;
        self.vox[slot][ch.iword as usize].samplefrac = acc - rate;
        let chm = &mut self.channels[c];
        chm.end += i as i32 + (ch.pos - newpos);
        chm.pos = newpos;
        src >= cbtrim
    }
}

/// The non-VOX mix: 16-bit `(s * vol) >> 8`; 8-bit through the scale table (volume clamped to
/// 255 and quantized to steps of 8).
fn mix_plain(dst: &mut [[i32; 2]], sc: &SfxCache, pos: i32, lv: i32, rv: i32) {
    match &sc.samples {
        wav::Samples::S16(d) => {
            for (k, f) in dst.iter_mut().enumerate() {
                let s = d.get(pos as usize + k).map_or(0, |&s| i32::from(s));
                f[0] += (s * lv) >> 8;
                f[1] += (s * rv) >> 8;
            }
        }
        wav::Samples::S8(d) => {
            let (lq, rq) = ((lv.min(255) >> 3) << 3, (rv.min(255) >> 3) << 3);
            for (k, f) in dst.iter_mut().enumerate() {
                let s = d.get(pos as usize + k).map_or(0, |&s| i32::from(s));
                f[0] += s * lq;
                f[1] += s * rq;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gs_fs::{PathId, SearchPath};

    /// A sound system over a temporary game directory holding `sound/t.wav`.
    fn system(name: &str, wav: &[u8]) -> SoundSystem {
        let dir = std::env::temp_dir().join(format!("gs-sound-{name}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sound")).unwrap();
        std::fs::write(dir.join("sound/t.wav"), wav).unwrap();
        SoundSystem::new(FileSystem::new(vec![SearchPath { dir, id: PathId::Game, writable: false }]), gs_common::rng::EngineRng::new(1))
    }

    #[test]
    fn a_sound_to_the_right_pans_and_fades_steals_its_channel_and_ends() {
        let data: Vec<u8> = std::iter::repeat_n(1000i16.to_le_bytes(), 1000).flatten().collect();
        let mut s = system("pan", &crate::wav::tests::wav(11025, 16, &data, None));
        s.precache(&[None, Some("t.wav".into())]);
        let sfx = s.precached(1).unwrap();
        let listener = Listener { right: [0.0, -1.0, 0.0], view_entity: 1, ..Listener::default() };
        let mut out = Vec::new();
        s.update(listener.clone(), 0, &mut out);
        assert_eq!(out.len(), 2204, "0.1 s mixed ahead");
        assert!(out.iter().all(|f| *f == [0, 0]));
        // Entity 5, 100 units to the right, ATTN_NORM: right = 2·(1 − 100·0.8/1000)·255 = 469,
        // left 0. The second start on the same entity and channel replaces the first.
        for _ in 0..2 {
            s.start_dynamic(5, chan::WEAPON, sfx, [0.0, -100.0, 0.0], 1.0, 0.8, 0, 100);
        }
        s.update(listener, 2205, &mut out);
        // (1000·469)>>8 = 1832 at 11025 Hz; ×2 upsampled (the first frame averages with the
        // silence before: 916), then volume 0.7 → (v·179)>>8: 640 and 1280.
        assert_eq!(out[0], [0, 640]);
        assert_eq!(out[1], [0, 1280]);
        assert_eq!(out[1999], [0, 1280]);
        assert_eq!(out[2000], [0, 640], "the 1000 samples end here");
        assert_eq!(out[2001], [0, 0]);
        assert!(s.channels.iter().all(|c| c.sfx.is_none()), "a finished one-shot frees its channel");
    }

    /// Decodes "compressed" bytes as raw 16-bit PCM.
    struct RawCodec;

    impl VoiceCodec for RawCodec {
        fn start_recording(&mut self) {}
        fn stop_recording(&mut self) {}
        fn get_voice(&mut self, _: &mut [u8], _: Option<&mut [u8]>) -> (usize, usize) {
            (0, 0)
        }
        fn decompress(&mut self, _: i32, data: &[u8], out: &mut [u8]) -> Option<usize> {
            out[..data.len()].copy_from_slice(data);
            Some(data.len())
        }
    }

    #[test]
    fn a_talker_starts_after_the_jitter_delay_plays_at_full_volume_and_ends_when_dry() {
        let mut s = system("voice", &crate::wav::tests::wav(11025, 16, &[0; 2], None));
        let mut codec = RawCodec;
        assert!(s.voice_init(Some(&mut codec)));
        let listener = Listener { view_entity: 1, ..Listener::default() };
        let mut out = Vec::new();
        s.update(listener.clone(), 0, &mut out);
        // 4000 samples of 1000 from entity 3: 3999 buffered; AGC gain 1, then ramping to 5.
        let pcm: Vec<u8> = std::iter::repeat_n(1000i16.to_le_bytes(), 4000).flatten().collect();
        s.voice_data(Some(&mut codec), 3, false, &pcm);
        s.voice_idle(0.1, Some(&mut codec));
        assert!(s.voice.hud_events.is_empty(), "inside the 0.2 s jitter delay");
        s.voice_idle(0.11, Some(&mut codec));
        assert_eq!(s.voice.hud_events, vec![(3, true)]);
        assert!(s.voice.overdrive > 1.0 && s.voice.overdrive < 2.0, "ducking ramps up: {}", s.voice.overdrive);
        s.update(listener.clone(), 2204, &mut out);
        // Volume 255 at the view entity, then the master volume 0.7 ((v·179)>>8):
        // 1000 → 696; the first frame averages with the zero carry.
        assert_eq!(out[0], [348, 348]);
        assert_eq!(out[1], [696, 696]);
        assert_eq!(out[1000], [3482, 3482], "gain 5 from the third chunk");
        for t in 2..5 {
            s.voice_idle(0.01, Some(&mut codec));
            s.update(listener.clone(), 2204 * t, &mut out);
        }
        assert_eq!(out.last(), Some(&[0, 0]), "ran dry: zeros");
        s.voice_idle(0.01, Some(&mut codec));
        assert_eq!(s.voice.hud_events, vec![(3, true), (3, false)], "one stop when starved");
        assert!(s.channels.iter().all(|c| c.sfx.is_none()), "the mixer channel is stopped");
    }

    /// A microphone whose "compressed" chunks are raw 16-bit PCM handed out by `get_voice`.
    struct MicCodec {
        chunk: Vec<u8>,
        recording: bool,
    }

    impl VoiceCodec for MicCodec {
        fn start_recording(&mut self) {
            self.recording = true;
        }
        fn stop_recording(&mut self) {
            self.recording = false;
        }
        fn get_voice(&mut self, out: &mut [u8], _: Option<&mut [u8]>) -> (usize, usize) {
            let n = self.chunk.len().min(out.len());
            out[..n].copy_from_slice(&self.chunk[..n]);
            (n, 0)
        }
        fn decompress(&mut self, _: i32, data: &[u8], out: &mut [u8]) -> Option<usize> {
            out[..data.len()].copy_from_slice(data);
            Some(data.len())
        }
    }

    #[test]
    fn test_microphone_plays_the_local_mic_back_and_mutes_the_network() {
        let mut s = system("voice", &crate::wav::tests::wav(11025, 16, &[0; 2], None));
        let samples: Vec<i16> = (0..400).map(|i| if i == 7 { -20000 } else { 300 }).collect();
        let mut mic = MicCodec { chunk: samples.iter().flat_map(|v| v.to_le_bytes()).collect(), recording: false };
        assert!(s.voice_init(Some(&mut mic)));
        assert!(s.voice_tweak_start(Some(&mut mic)));
        assert!(mic.recording, "tweak mode records");
        assert!(!s.voice.is_recording(), "nothing is sent to the server meanwhile");
        s.voice_idle(0.1, Some(&mut mic));
        assert_eq!(s.voice.tweak_speaking_volume, 20000 & 0xFE00, "the loudest sample, low bits cut");
        s.voice_idle(0.11, Some(&mut mic));
        assert_eq!(s.voice.hud_events, vec![(-1, true), (voice::TWEAK_ENTITY, true)], "the mic channel starts after 0.2 s");
        s.voice_data(Some(&mut mic), 3, false, &[1, 0, 2, 0]);
        assert_eq!(s.voice.channel_of(3), None, "network voice is dropped");
        s.voice_tweak_end(Some(&mut mic));
        assert!(!mic.recording);
    }
}
