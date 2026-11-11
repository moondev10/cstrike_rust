//! The engine's sound entry points over [`gs_sound`] (docs/re/audio.md §2, §5): `svc_sound`,
//! `svc_spawnstaticsound`, `svc_stopsound`, the event / HUD / effect / pmove sounds, the sound
//! console commands, `S_Update` from the rendered view, and the voice engine
//! (docs/re/voice-engine.md): `svc_voiceinit`, `svc_voicedata`, `Voice_Idle`, recording.

use std::path::PathBuf;

use gs_console::Console;
use gs_console::cvar::Cvars;
use gs_fs::FileSystem;
use gs_protocol::messages::{Sound, SoundName, StaticSound, sound_flags};
use gs_sound::mixer::OUTPUT_RATE;
use gs_sound::music::{CdAudio, MusicInput};
use gs_sound::voice::{VoiceCodec, VoiceSettings};
use gs_sound::{Device, Listener, Settings, SfxId, SoundSystem, chan, flags};

/// Engine commands of the sound system.
pub const COMMANDS: &[&str] = &["play", "playvol", "speak", "spk", "stopsound", "soundlist", "soundinfo"];

/// A sound requested by client code while the audio was borrowed elsewhere.
#[derive(Debug, Clone)]
pub enum SoundRequest {
    /// `EV_PlaySound` / `pfnPlaySoundBy*`.
    Play { ent: i32, origin: [f32; 3], channel: i32, sample: String, volume: f32, attenuation: f32, flags: i32, pitch: i32 },
    /// `EV_StopSound`.
    Stop { ent: i32, channel: i32, sample: String },
}

/// The open device and the mixer.
pub struct Audio {
    sound: SoundSystem,
    device: Device,
    /// `cl.max_edicts`.
    max_edicts: usize,
    /// The level's sound list went through `CL_PrecacheResources`.
    precached: bool,
    /// `--sound-dump`: the output path and every mixed frame so far.
    dump: Option<(PathBuf, Vec<[i16; 2]>)>,
    /// This update's mixed frames.
    frames: Vec<[i16; 2]>,
    /// The voice codec: Steam's, or without a Steam client `gs-voicecodec`'s (`None` only when
    /// that failed to start; voice then decodes and records nothing).
    codec: Option<Box<dyn VoiceCodec>>,
    /// `CCDAudio` over the device's music stream.
    music: CdAudio,
    /// `realtime` of the last engine frame (for `cd fadeout`).
    realtime: f64,
}

impl Drop for Audio {
    fn drop(&mut self) {
        let Some((path, frames)) = self.dump.take() else { return };
        match std::fs::write(&path, wav_bytes(&frames)) {
            Ok(()) => println!("sound dump {} ({:.1} s)", path.display(), frames.len() as f64 / f64::from(OUTPUT_RATE)),
            Err(e) => log::error!("sound dump {}: {e}", path.display()),
        }
    }
}

/// A 16-bit stereo PCM WAV at the output rate.
fn wav_bytes(frames: &[[i16; 2]]) -> Vec<u8> {
    let data_len = (frames.len() * 4) as u32;
    let mut out = Vec::with_capacity(44 + frames.len() * 4);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&OUTPUT_RATE.to_le_bytes());
    out.extend_from_slice(&(OUTPUT_RATE * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for f in frames {
        out.extend_from_slice(&f[0].to_le_bytes());
        out.extend_from_slice(&f[1].to_le_bytes());
    }
    out
}

impl Audio {
    /// `S_Init`, then `CDAudio_Init` (`MP3Volume` as registered: no config has run yet);
    /// `None` without an output device (the game runs silent).
    pub fn open(fs: &FileSystem, rng: gs_common::rng::EngineRng, dump: Option<PathBuf>, codec: Option<Box<dyn VoiceCodec>>, mp3volume: f32) -> Option<Self> {
        match Device::open() {
            Ok(device) => Some(Self {
                sound: SoundSystem::new(fs.clone(), rng),
                music: CdAudio::new(device.music(), fs.clone(), mp3volume),
                device,
                max_edicts: 1200,
                precached: false,
                dump: dump.map(|p| (p, Vec::new())),
                frames: Vec::new(),
                codec,
                realtime: 0.0,
            }),
            Err(e) => {
                log::warn!("sound disabled: {e}");
                None
            }
        }
    }

    /// A new server connection: `S_StopAllSounds`, and `CL_ReallocateDynamicData`'s
    /// `cl.max_edicts` for `maxclients`; the level's sounds are precached again.
    pub fn new_level(&mut self, maxclients: u8) {
        self.sound.stop_all();
        self.max_edicts = 1200 + 15 * usize::from(maxclients.max(1) - 1);
        self.precached = false;
    }

    /// `S_StopAllSounds`.
    pub fn stop_all(&mut self) {
        self.sound.stop_all();
    }

    /// `CL_PrecacheResources` for the sound list, once per level (the list is complete before
    /// the signon data, whose static sounds already need it).
    pub fn precache(&mut self, sounds: &[Option<String>]) {
        if !self.precached && !sounds.is_empty() {
            self.sound.precache(sounds);
            self.precached = true;
        }
    }

    /// The sfx a server message names.
    fn message_sfx(&mut self, name: SoundName) -> Option<SfxId> {
        match name {
            SoundName::Precache(i) => self.sound.precached(usize::from(i)),
            SoundName::Sentence(i) => self.sound.sentence_sfx(usize::from(i)),
            SoundName::SentenceNumber(n) => self.sound.find_name(&format!("!#{n}")),
        }
    }

    /// `Parse_svc_sound`.
    pub fn server_sound(&mut self, s: &Sound) {
        if usize::from(s.entity) >= self.max_edicts {
            log::debug!("CL_Parse_Sound: ent = {}, cl.max_edicts {}", s.entity, self.max_edicts);
            return;
        }
        let Some(sfx) = self.message_sfx(s.sound) else { return };
        let (ent, channel) = (i32::from(s.entity), i32::from(s.channel));
        let f = (s.flags & !sound_flags::SENTENCE) as i32;
        if channel == chan::STATIC {
            self.sound.start_static(ent, channel, sfx, s.origin, s.volume, s.attenuation, f, i32::from(s.pitch));
        } else {
            self.sound.start_dynamic(ent, channel, sfx, s.origin, s.volume, s.attenuation, f, i32::from(s.pitch));
        }
    }

    /// `CL_ParseStaticSound`.
    pub fn static_sound(&mut self, s: &StaticSound) {
        let Some(sfx) = self.message_sfx(s.sound) else { return };
        let f = i32::from(s.flags) & !(sound_flags::SENTENCE as i32);
        self.sound.start_static(i32::from(s.entity), chan::STATIC, sfx, s.origin, s.volume, s.attenuation, f, i32::from(s.pitch));
    }

    /// `S_StopSound` (`svc_stopsound`).
    pub fn stop(&mut self, ent: i32, channel: i32) {
        self.sound.stop(ent, channel);
    }

    /// `EV_PlaySound` / `EV_StopSound` / the `pfnPlaySoundBy*` family.
    pub fn request(&mut self, r: &SoundRequest) {
        match r {
            SoundRequest::Play { ent, origin, channel, sample, volume, attenuation, flags: f, pitch } => {
                let sfx = if f & sound_flags::SENTENCE as i32 != 0 {
                    sample.get(1..).and_then(|n| n.parse().ok()).and_then(|i| self.sound.sentence_sfx(i))
                } else {
                    self.sound.lookup(sample)
                };
                let Some(sfx) = sfx else { return };
                let f = f & !(sound_flags::SENTENCE as i32);
                if *channel == chan::STATIC {
                    self.sound.start_static(*ent, *channel, sfx, *origin, *volume, *attenuation, f, *pitch);
                } else {
                    self.sound.start_dynamic(*ent, *channel, sfx, *origin, *volume, *attenuation, f, *pitch);
                }
            }
            SoundRequest::Stop { ent, channel, sample } => {
                if let Some(sfx) = self.sound.lookup(sample) {
                    self.sound.start_dynamic(*ent, *channel, sfx, [0.0; 3], 0.0, 0.0, flags::STOP, 100);
                }
            }
        }
    }

    /// `pfnPlaySoundByName` / `…AtPitch` / `PlaySoundVoiceByName`: on the view entity, so full
    /// volume and unpanned.
    pub fn hud_sound(&mut self, channel: i32, name: &str, volume: f32, pitch: i32) {
        if let Some(sfx) = self.sound.precache_sound(name) {
            let (ent, origin) = (self.sound.view_entity(), self.sound.listener_origin());
            self.sound.start_dynamic(ent, channel, sfx, origin, volume.clamp(0.0, 1.0), 1.0, 0, pitch.max(0));
        }
    }

    /// `pfnPlaySoundByNameAtLocation` (studio event 5004): also on the view entity, channel 0.
    pub fn sound_at_location(&mut self, name: &str, volume: f32, origin: [f32; 3]) {
        if let Some(sfx) = self.sound.lookup(name) {
            let ent = self.sound.view_entity();
            self.sound.start_dynamic(ent, chan::AUTO, sfx, origin, volume.clamp(0.0, 1.0), 1.0, 0, 100);
        }
    }

    /// The console's sound commands at the listener (`play`, `playvol`, `speak`, `spk`,
    /// `stopsound`); `false` for other commands.
    pub fn command(&mut self, console: &Console, args: &gs_console::Args) -> bool {
        let name = args.arg(0).to_ascii_lowercase();
        if !COMMANDS.contains(&name.as_str()) {
            return false;
        }
        match name.as_str() {
            "stopsound" => {
                self.stop_all();
                return true;
            }
            "soundlist" => {
                // One Con_Printf per line, as cmd_soundlist prints.
                for line in self.sound.soundlist_text().split_inclusive('\n') {
                    crate::notify::con_printf(line);
                }
                return true;
            }
            // SNDDMA_InitDirect's shm: 2 channels, 16 bits, speed 11025, the 64 KB buffer in
            // samples, submission chunk 1; samplepos is never written (0).
            "soundinfo" => {
                let (samples, buffer) = self.device.dma_info();
                crate::notify::con_printf(&format!(
                    "{:5} stereo\n{samples:5} samples\n{:5} samplepos\n{:5} samplebits\n{:5} submission_chunk\n{:5} speed\n0x{buffer:x} dma buffer\n{:5} total_channels\n",
                    1,
                    0,
                    16,
                    1,
                    gs_sound::wav::BASE_RATE,
                    self.sound.total_channels()
                ));
                return true;
            }
            _ => {}
        }
        if console.cvars.value("speak_enabled") == 0.0 {
            return true;
        }
        let origin = self.sound.listener_origin();
        match name.as_str() {
            // S_Play: each sample on a fresh fake entity at the listener, full volume.
            "play" | "playvol" => {
                let step = if name == "play" { 1 } else { 2 };
                for i in (1..args.argc()).step_by(step) {
                    let mut sample = args.arg(i).to_owned();
                    if !sample.contains('.') {
                        sample.push_str(".wav");
                    }
                    let vol = if step == 2 { args.arg(i + 1).parse().unwrap_or(1.0) } else { 1.0 };
                    let ent = self.sound.next_fake_entity();
                    if let Some(sfx) = self.sound.precache_sound(&sample) {
                        self.sound.start_dynamic(ent, chan::AUTO, sfx, origin, vol, 1.0, 0, 100);
                    }
                }
            }
            // speak / spk: a sentence by name, at the listener.
            _ => {
                let text = args.arg(1);
                let name = if text.starts_with('!') { text.to_owned() } else { format!("!{text}") };
                let Some(sfx) = self.sound.find_name(&name) else { return true };
                let view = self.sound.view_entity();
                if args.arg(0).eq_ignore_ascii_case("spk") {
                    self.sound.start_static(view, chan::STATIC, sfx, origin, 1.0, 0.8, 0, 100);
                } else {
                    self.sound.start_dynamic(view, -1, sfx, origin, 1.0, 0.8, 0, 100);
                }
            }
        }
        true
    }

    /// `S_Update` from the rendered view: spatialize, mix ahead and queue the output.
    /// `timescale` is the engine's internal `sys_timescale`. Returns the room preset the DSP
    /// applied, whose values go into the `room_*` cvars.
    pub fn update(&mut self, console: &Console, listener: Listener, timescale: f32) -> Option<gs_sound::dsp::RoomParams> {
        let cv = &console.cvars;
        let v = |name: &str| cv.value(name);
        self.sound.settings = Settings {
            volume: v("volume"),
            hisound: v("hisound") != 0.0,
            room_type: v("room_type") as i32,
            waterroom_type: v("waterroom_type") as i32,
            room_off: v("room_off") != 0.0,
            room: gs_sound::dsp::RoomParams {
                lp: v("room_lp"),
                modulation: v("room_mod"),
                size: v("room_size"),
                refl: v("room_refl"),
                rvblp: v("room_rvblp"),
                delay: v("room_delay"),
                feedback: v("room_feedback"),
                dlylp: v("room_dlylp"),
                left: v("room_left"),
            },
            mixahead: v("_snd_mixahead"),
            timescale,
            nosound: v("nosound") != 0.0,
            snd_show: v("snd_show") != 0.0,
            precache_timings: v("fs_precache_timings") != 0.0,
        };
        if let Some(listing) = self.sound.update(listener, self.device.consumed(), &mut self.frames) {
            crate::notify::con_printf(&listing);
        }
        // Sounds loaded since the last update (precache or first start): Con_DPrintf.
        let developer = v("developer") != 0.0;
        for line in std::mem::take(&mut self.sound.dprints) {
            if developer {
                crate::notify::con_dprintf(&line);
            }
        }
        self.device.push(&self.frames);
        if let Some((_, all)) = &mut self.dump {
            all.extend_from_slice(&self.frames);
        }
        self.sound.take_room_preset()
    }

    /// `S_PrintChannels`' lines (`s_show`), one per channel slot.
    pub fn channel_lines(&self) -> Vec<Option<String>> {
        self.sound.channel_lines()
    }

    /// The voice cvars, read at the moment of use.
    fn voice_settings(&mut self, cv: &Cvars) {
        self.sound.voice_settings = VoiceSettings {
            enable: cv.value("voice_enable"),
            maxgain: cv.value("voice_maxgain"),
            avggain: cv.value("voice_avggain"),
            scale: cv.value("voice_scale"),
            fadeouttime: cv.value("voice_fadeouttime"),
            overdrive: cv.value("voice_overdrive"),
            overdrivefadetime: cv.value("voice_overdrivefadetime"),
            showchannels: cv.value("voice_showchannels"),
            showincoming: cv.value("voice_showincoming"),
            profile: cv.value("voice_profile"),
        };
    }

    /// `Voice_Init` (`Host_Init` and `svc_voiceinit`).
    pub fn voice_init(&mut self, console: &Console) {
        self.voice_settings(&console.cvars);
        self.sound.voice_init(self.codec.as_deref_mut().map(|c| c as &mut dyn VoiceCodec));
    }

    /// `Voice_Deinit` (`Host_Shutdown`).
    pub fn voice_shutdown(&mut self) {
        self.sound.voice_deinit(self.codec.as_deref_mut().map(|c| c as &mut dyn VoiceCodec));
    }

    /// `Parse_svc_voicedata` once the sender is a valid entity.
    pub fn voice_data(&mut self, console: &Console, ent: i32, local: bool, data: &[u8]) {
        self.voice_settings(&console.cvars);
        self.sound.voice_data(self.codec.as_deref_mut().map(|c| c as &mut dyn VoiceCodec), ent, local, data);
    }

    /// `Voice_Idle((f32)host_frametime)`.
    pub fn voice_idle(&mut self, console: &Console, dt: f32) {
        self.voice_settings(&console.cvars);
        self.sound.voice_idle(dt, self.codec.as_deref_mut().map(|c| c as &mut dyn VoiceCodec));
    }

    /// `Voice_RecordStart` (`+voicerecord`, with `voice_recordtofile`).
    pub fn voice_record_start(&mut self, to_file: bool) {
        self.sound.voice_record_start(self.codec.as_deref_mut().map(|c| c as &mut dyn VoiceCodec), to_file);
    }

    /// `Voice_RecordStop`.
    pub fn voice_record_stop(&mut self) {
        self.sound.voice_record_stop(self.codec.as_deref_mut().map(|c| c as &mut dyn VoiceCodec));
    }

    /// `Voice_IsRecording`.
    pub fn voice_recording(&self) -> bool {
        self.sound.voice.is_recording()
    }

    /// `VoiceTweak_StartVoiceTweakMode` (GameUI's Test Microphone).
    pub fn voice_tweak_start(&mut self) -> bool {
        self.sound.voice_tweak_start(self.codec.as_deref_mut().map(|c| c as &mut dyn VoiceCodec))
    }

    /// `VoiceTweak_EndVoiceTweakMode`.
    pub fn voice_tweak_end(&mut self) {
        self.sound.voice_tweak_end(self.codec.as_deref_mut().map(|c| c as &mut dyn VoiceCodec));
    }

    /// `g_pMixerControls` exists (voice is up) and `VoiceTweak_GetSpeakingVolume`.
    pub fn voice_tweak_state(&self) -> (bool, i32) {
        (self.sound.voice.initted(), self.sound.voice.tweak_speaking_volume)
    }

    /// `Voice_GetCompressedData` for `CL_AddVoiceToDatagram` (only while recording).
    pub fn voice_compressed(&mut self, out: &mut [u8]) -> usize {
        if !self.sound.voice.is_recording() {
            return 0;
        }
        self.sound.voice_compressed(self.codec.as_deref_mut().map(|c| c as &mut dyn VoiceCodec), out)
    }

    /// The queued `HUD_VoiceStatus` calls and voice debug prints.
    pub fn take_voice_output(&mut self) -> (Vec<(i32, bool)>, Vec<String>) {
        (std::mem::take(&mut self.sound.voice.hud_events), std::mem::take(&mut self.sound.voice.prints))
    }

    fn music_input(&self, cv: &Cvars) -> MusicInput {
        MusicInput { bgmvolume: cv.value("bgmvolume"), mp3volume: cv.value("MP3Volume"), fadetime: cv.value("MP3FadeTime"), realtime: self.realtime }
    }

    /// `CCDAudio::Frame` (+ `_Frame`), the first thing of every engine loop iteration; a
    /// changed `bgmvolume` is rewritten to "0.000000" / "1.000000".
    pub fn music_frame(&mut self, cv: &mut Cvars, realtime: f64) {
        self.realtime = realtime;
        let input = self.music_input(cv);
        if let Some(value) = self.music.frame(&input) {
            cv.direct_set("bgmvolume", &value);
        }
    }

    /// The `cd` and `mp3` commands; console output lines.
    pub fn music_command(&mut self, cv: &Cvars, argv: &[&str]) -> Vec<String> {
        if argv.first().is_some_and(|c| c.eq_ignore_ascii_case("mp3")) {
            self.music.mp3_command(argv);
            return Vec::new();
        }
        let input = self.music_input(cv);
        self.music.cd_command(argv, &input)
    }

    /// `CDAudio_Play(track, 1)` (`svc_cdtrack`).
    pub fn music_play(&mut self, track: i32, looped: bool) {
        self.music.play(track, looped);
    }

    /// `CDAudio_Pause` / `CDAudio_Resume` (`svc_setpause`, window focus).
    pub fn music_pause(&mut self, pause: bool) {
        if pause { self.music.pause() } else { self.music.resume() }
    }

    /// `CDAudio_Shutdown`: the music cut off.
    pub fn music_shutdown(&mut self) {
        self.music.shutdown();
    }
}

/// `CL_Move`'s view of the voice engine: `Voice_GetCompressedData` and `Voice_GetLoopback`.
pub struct VoiceSendHost<'a> {
    pub audio: Option<&'a mut Audio>,
    /// `voice_loopback != 0` (NaN counts as set).
    pub loopback: bool,
}

impl gs_client::voice::VoiceSend for VoiceSendHost<'_> {
    fn compressed(&mut self, out: &mut [u8]) -> usize {
        self.audio.as_deref_mut().map_or(0, |a| a.voice_compressed(out))
    }

    fn loopback(&self) -> bool {
        self.loopback
    }
}
