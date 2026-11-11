//! MP3 / "CD audio" music (docs/re/music.md): the `CCDAudio` state machine with its track table,
//! `Frame`/`_Frame`, the `cd` and `mp3` commands and `svc_cdtrack`, over a Miles-like stream
//! player that the output device pulls on its own thread (so music keeps playing while the game
//! thread is busy, and is mixed after the engine's `volume`, like a second OS stream).
//!
//! Deliberate deviations: the worker thread is not modelled (posted items run synchronously, the
//! common immediate-wake case of §3, with `cd off` keeping its original race outcome); Miles'
//! resampling kernel is not reverse-engineered, so a windowed-sinc low-pass stands in for it;
//! the CRT assertion of an invalid `playfile` name or a full table is an open failure (§17 Q1);
//! there is no Red Book CD (§9.4: `m_bIsValid` is always false).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use gs_fs::FileSystem;
use nanomp3_core::{DecodeError, Decoder, MAX_SAMPLES_PER_FRAME};

/// `AIL_open_digital_driver(22050, 16, 2, 1)`.
pub const DRIVER_RATE: u32 = 22050;
/// `g_szMP3Tracks` capacity.
const MAX_TRACKS: usize = 200;
/// The built-in table (§2.1).
const BUILTIN_TRACKS: [&str; 29] = [
    "",
    "",
    "media\\Half-Life01.mp3",
    "media\\Prospero01.mp3",
    "media\\Half-Life12.mp3",
    "media\\Half-Life07.mp3",
    "media\\Half-Life10.mp3",
    "media\\Suspense01.mp3",
    "media\\Suspense03.mp3",
    "media\\Half-Life09.mp3",
    "media\\Half-Life02.mp3",
    "media\\Half-Life13.mp3",
    "media\\Half-Life04.mp3",
    "media\\Half-Life15.mp3",
    "media\\Half-Life14.mp3",
    "media\\Half-Life16.mp3",
    "media\\Suspense02.mp3",
    "media\\Half-Life03.mp3",
    "media\\Half-Life08.mp3",
    "media\\Prospero02.mp3",
    "media\\Half-Life05.mp3",
    "media\\Prospero04.mp3",
    "media\\Half-Life11.mp3",
    "media\\Half-Life06.mp3",
    "media\\Prospero03.mp3",
    "media\\Half-Life17.mp3",
    "media\\Prospero05.mp3",
    "media\\Suspense05.mp3",
    "media\\Suspense07.mp3",
];
/// Zero crossings on each side of the resampling kernel.
const KERNEL_ZEROS: usize = 16;
/// Kernel table steps per input sample.
const KERNEL_RES: usize = 256;
/// Input bytes handed to the decoder at once (several frames, so frames are recognised).
const DECODE_WINDOW: usize = 16 * 1024;

/// `AIL_stream_status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStatus {
    /// 2: all data played (or the loop count ran out).
    Done,
    /// 4.
    Playing,
    /// 8: paused, or opened and never started.
    Stopped,
}

/// The per-channel level of `AIL_set_sample_volume_pan(sample, volume, 0.5)`.
pub fn miles_level(volume: f32) -> f32 {
    (f64::from(volume).powf(f64::from(1.666_666_6_f32)) as f32) * 0.812_252_2_f32
}

/// A windowed-sinc low-pass resampler from the file rate to the driver rate.
#[derive(Debug, Clone)]
struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// Half-width in input samples.
    half: usize,
    /// `h(x)` at `x = k / KERNEL_RES`, `k` in `0..=half·KERNEL_RES + 1`.
    table: Vec<f32>,
    /// Input history; `pos` indexes into it.
    input: VecDeque<[f32; 2]>,
    pos: f64,
}

impl Resampler {
    fn new(in_rate: u32) -> Self {
        let step = f64::from(in_rate) / f64::from(DRIVER_RATE);
        let cut = (1.0 / step).min(1.0);
        let half = (KERNEL_ZEROS as f64 / cut).ceil() as usize;
        let table = (0..=half * KERNEL_RES + 1)
            .map(|k| {
                let x = k as f64 / KERNEL_RES as f64;
                if x >= half as f64 {
                    return 0.0;
                }
                let sinc = if x == 0.0 { 1.0 } else { (std::f64::consts::PI * cut * x).sin() / (std::f64::consts::PI * cut * x) };
                let w = x / half as f64;
                let blackman = 0.42 + 0.5 * (std::f64::consts::PI * w).cos() + 0.08 * (2.0 * std::f64::consts::PI * w).cos();
                (cut * sinc * blackman) as f32
            })
            .collect();
        // Leading silence so the first output lines up with the first input sample.
        let input = std::iter::repeat_n([0.0; 2], half).collect();
        Self { step, half, table, input, pos: half as f64 }
    }

    /// Input samples still needed before the next output can be computed.
    fn needs(&self) -> usize {
        (self.pos.floor() as usize + self.half + 1).saturating_sub(self.input.len())
    }

    fn kernel(&self, x: f64) -> f32 {
        let f = x.abs() * KERNEL_RES as f64;
        let k = f as usize;
        let (a, b) = (self.table.get(k).copied().unwrap_or(0.0), self.table.get(k + 1).copied().unwrap_or(0.0));
        a + (b - a) * (f - k as f64) as f32
    }

    /// The next output frame (call when [`Self::needs`] is 0).
    fn output(&mut self) -> [f32; 2] {
        let center = self.pos.floor() as usize;
        let lo = (center + 1).saturating_sub(self.half);
        let mut acc = [0.0f32; 2];
        for i in lo..(center + self.half + 1).min(self.input.len()) {
            let h = self.kernel(self.pos - i as f64);
            let s = self.input[i];
            acc[0] += s[0] * h;
            acc[1] += s[1] * h;
        }
        self.pos += self.step;
        // Drop history the kernel no longer reaches.
        while self.pos - self.half as f64 > 1.0 && !self.input.is_empty() {
            self.input.pop_front();
            self.pos -= 1.0;
        }
        acc
    }
}

/// An open Miles stream over a whole MP3 file.
struct Stream {
    data: Vec<u8>,
    /// First byte after an ID3v2 tag (the loop point).
    start: usize,
    pos: usize,
    decoder: Box<Decoder>,
    pcm: Vec<f32>,
    /// Miles loop count: 0 forever, else plays left including the current one.
    loops: u32,
    started: bool,
    paused: bool,
    /// The decoder hit the end and the tail was queued.
    drained: bool,
    done: bool,
    resampler: Resampler,
}

/// The size of a leading ID3v2 tag.
fn id3v2_len(d: &[u8]) -> usize {
    if d.len() >= 10 && &d[..3] == b"ID3" {
        let size = d[6..10].iter().fold(0usize, |a, &b| (a << 7) | usize::from(b & 0x7F));
        let footer = if d[5] & 0x10 != 0 { 10 } else { 0 };
        (10 + size + footer).min(d.len())
    } else {
        0
    }
}

impl Stream {
    /// `AIL_open_stream`: Layer III only; `None` when no decodable frame is found.
    fn open(data: Vec<u8>) -> Option<Self> {
        let start = id3v2_len(&data);
        let mut probe = Box::new(Decoder::new());
        let mut pcm = vec![0f32; MAX_SAMPLES_PER_FRAME];
        let mut pos = start;
        let rate = loop {
            let (used, r) = probe.decode(&data[pos..(pos + DECODE_WINDOW).min(data.len())], &mut pcm);
            pos += used;
            match r {
                Ok(info) => break info.sample_rate,
                Err(DecodeError::ReservoirUnavailable(info) | DecodeError::Corrupt(info)) => break info.sample_rate,
                Err(DecodeError::UnsupportedLayer(_)) => return None,
                Err(_) if used == 0 || pos >= data.len() => return None,
                Err(_) => {}
            }
        };
        Some(Self {
            data,
            start,
            pos: start,
            decoder: Box::new(Decoder::new()),
            pcm,
            loops: 1,
            started: false,
            paused: false,
            drained: false,
            done: false,
            resampler: Resampler::new(rate),
        })
    }

    fn status(&self) -> StreamStatus {
        if self.done {
            StreamStatus::Done
        } else if self.started && !self.paused {
            StreamStatus::Playing
        } else {
            StreamStatus::Stopped
        }
    }

    /// Decodes until one more frame is in the resampler, looping as the count allows.
    fn feed(&mut self) {
        while !self.drained {
            if self.pos >= self.data.len() {
                if self.loops != 1 {
                    // Loop: from the first frame after the tag, with fresh decoder state.
                    self.loops = self.loops.saturating_sub(1);
                    self.pos = self.start;
                    *self.decoder = Decoder::new();
                    continue;
                }
                // The tail: the kernel's reach of silence lets the last samples out.
                self.drained = true;
                self.resampler.input.extend(std::iter::repeat_n([0.0; 2], self.resampler.half + 1));
                return;
            }
            let end = (self.pos + DECODE_WINDOW).min(self.data.len());
            let (used, r) = self.decoder.decode(&self.data[self.pos..end], &mut self.pcm);
            self.pos = if used == 0 { end } else { self.pos + used };
            if let Ok(info) = r {
                let n = info.samples_produced;
                if info.channels.num() == 1 {
                    self.resampler.input.extend(self.pcm[..n].iter().map(|&s| [s, s]));
                } else {
                    self.resampler.input.extend(self.pcm[..n * 2].chunks_exact(2).map(|c| [c[0], c[1]]));
                }
                if n > 0 {
                    return;
                }
            }
        }
    }

    /// One driver-rate frame at `level`, or `None` once the data has all played.
    fn next_frame(&mut self, level: f32) -> Option<[i16; 2]> {
        while self.resampler.needs() > 0 {
            if self.drained {
                self.done = true;
                return None;
            }
            self.feed();
        }
        let s = self.resampler.output();
        Some(s.map(|v| (v * level * 32768.0).round().clamp(-32768.0, 32767.0) as i16))
    }
}

/// The Miles side: the one open stream and its level, pulled by the output device.
#[derive(Default)]
pub struct MusicOutput {
    stream: Option<Stream>,
    level: f32,
}

impl MusicOutput {
    /// The next 22050 Hz frame of the driver (silence unless a stream is playing).
    pub fn next_frame(&mut self) -> [i16; 2] {
        let level = self.level;
        match &mut self.stream {
            Some(s) if s.status() == StreamStatus::Playing => s.next_frame(level).unwrap_or([0; 2]),
            _ => [0; 2],
        }
    }
}

/// The handle the device pulls from.
pub type MusicHandle = Arc<Mutex<MusicOutput>>;

/// The music cvars (`bgmvolume`, `MP3Volume`, `MP3FadeTime`) and `realtime`.
#[derive(Debug, Clone, Copy)]
pub struct MusicInput {
    pub bgmvolume: f32,
    pub mp3volume: f32,
    pub fadetime: f32,
    pub realtime: f64,
}

/// `CCDAudio` (MP3 path).
pub struct CdAudio {
    out: MusicHandle,
    fs: FileSystem,
    /// `g_szMP3Tracks[0..g_iMP3NumTracks]`.
    tracks: Vec<String>,
    /// `m_dFadeOutTime`.
    fade_out_time: f64,
    is_playing: bool,
    was_playing: bool,
    initialized: bool,
    enabled: bool,
    is_looping: bool,
    /// `m_flVolume` (the bgmvolume mirror, 0 or 1) and `m_flMP3Volume`.
    volume: f32,
    mp3_volume: f32,
    play_track: i32,
    pending_file: String,
    mp3_active: bool,
    title: String,
    mp3_track: i32,
    mp3_loop: bool,
    /// `m_flAppliedVolume`.
    applied_volume: f32,
}

impl CdAudio {
    /// `CCDAudio::Init` with the driver open (`MP3Volume` as registered: config not run yet).
    pub fn new(out: MusicHandle, fs: FileSystem, mp3volume: f32) -> Self {
        let mut cd = Self {
            out,
            fs,
            tracks: BUILTIN_TRACKS.iter().map(|s| (*s).to_owned()).collect(),
            fade_out_time: 0.0,
            is_playing: false,
            was_playing: false,
            initialized: true,
            enabled: true,
            is_looping: false,
            volume: 1.0,
            mp3_volume: 1.0,
            play_track: 0,
            pending_file: String::new(),
            mp3_active: false,
            title: String::new(),
            mp3_track: 0,
            mp3_loop: false,
            applied_volume: 100.0,
        };
        let c = clamp_volume(mp3volume);
        if cd.applied_volume != c {
            cd.applied_volume = c;
            cd.output().level = miles_level(c);
        }
        cd
    }

    fn output(&self) -> MutexGuard<'_, MusicOutput> {
        self.out.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn stream_status(&self) -> Option<StreamStatus> {
        self.output().stream.as_ref().map(Stream::status)
    }

    /// `CCDAudio::Frame` then `_Frame`: the bgmvolume toggle (the cvar's new string when it is
    /// rewritten), `MP3Volume` unless fading, and the fade / end-of-stream check.
    pub fn frame(&mut self, input: &MusicInput) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let mut rewrite = None;
        if self.volume != input.bgmvolume {
            if self.volume == 0.0 {
                self.volume = 1.0;
                self.resume();
            } else {
                self.volume = 0.0;
                self.pause();
            }
            rewrite = Some(format!("{:.6}", self.volume));
        }
        if self.fade_out_time == 0.0 && self.mp3_volume != input.mp3volume {
            self.mp3_volume = self.set_volume(input.mp3volume);
        }
        self.worker_frame(input);
        rewrite
    }

    /// `_Frame`.
    fn worker_frame(&mut self, input: &MusicInput) {
        if !self.is_playing {
            return;
        }
        let vol = f64::from(input.mp3volume);
        if self.fade_out_time != 0.0 {
            let now = input.realtime;
            let v = if now >= self.fade_out_time {
                self.stop();
                self.fade_out_time = 0.0;
                vol
            } else {
                (self.fade_out_time - now) / f64::from(input.fadetime) * f64::from(input.mp3volume)
            };
            self.set_volume(v as f32);
        }
        if self.mp3_active {
            self.is_playing = self.stream_status() == Some(StreamStatus::Playing);
            if !self.is_playing {
                self.pending_file.clear();
            }
        }
    }

    /// `MP3_SetVolume`: clamped to 0..1 (NaN 0), applied when it changed.
    fn set_volume(&mut self, v: f32) -> f32 {
        let c = clamp_volume(v);
        if self.applied_volume == c {
            return self.applied_volume;
        }
        self.applied_volume = c;
        self.output().level = miles_level(c);
        c
    }

    /// `MP3_GetTrackFromName`: the table index of `name` (appended when new), 0 if rejected.
    fn track_from_name(&mut self, name: &str) -> usize {
        if name.contains(':') || name.contains("..") {
            return 0;
        }
        let mut buf = if name.contains(".mp3") { name.to_owned() } else { format!("{name}.mp3") };
        gs_common::bytestr::truncate_encoded_in_place(&mut buf, 259);
        let buf = buf.replace('/', "\\");
        for (i, t) in self.tracks.iter().enumerate() {
            if *t == buf {
                return i;
            }
            if i + 1 == MAX_TRACKS {
                return 0;
            }
        }
        self.tracks.push(buf);
        self.tracks.len() - 1
    }

    /// `MP3_InitStream`: the current music stopped, then the track opened at the applied
    /// volume (looping forever with `looped`).
    fn init_stream(&mut self, track: usize, looped: bool) -> bool {
        if !(1 < track && track < self.tracks.len()) {
            log::debug!("MP3_InitStream: assertion 1 < trackNum && trackNum < g_iMP3NumTracks failed ({track})");
            return false;
        }
        let path = self.tracks[track].replace('\\', "/");
        let data = self.fs.read(&path).ok();
        self.stop();
        self.fade_out_time = 0.0;
        let Some(mut stream) = data.and_then(Stream::open) else {
            log::debug!("MP3_InitStream: couldn't open {path}");
            return false;
        };
        if looped {
            stream.loops = 0;
        }
        let level = miles_level(self.applied_volume);
        {
            let mut out = self.output();
            out.stream = Some(stream);
            out.level = level;
        }
        self.title.clone_from(&self.tracks[track]);
        true
    }

    /// `MP3_StartStream`: playing, or paused at once with `MP3Volume` 0.
    fn start_stream(&mut self, track: i32, looped: bool) {
        if let Some(s) = &mut self.output().stream {
            s.started = true;
        }
        self.mp3_active = true;
        self.mp3_track = track;
        self.mp3_loop = looped;
        self.is_looping = looped;
        self.play_track = track;
        self.is_playing = true;
        if self.mp3_volume == 0.0 && self.enabled {
            if let Some(s) = &mut self.output().stream {
                s.paused = true;
            }
            self.was_playing = self.is_playing;
            self.is_playing = false;
            self.pending_file.clear();
        }
    }

    /// `MP3_PlayTrack`.
    fn mp3_play_track(&mut self, track: i32, looped: bool) -> bool {
        let Ok(t) = usize::try_from(track) else { return false };
        if t < 2 || t >= self.tracks.len() || !self.init_stream(t, looped) {
            return false;
        }
        self.start_stream(track, looped);
        true
    }

    /// `MP3_PlayFile`: started as track 0.
    fn mp3_play_file(&mut self, name: &str, looped: bool) -> bool {
        if name.is_empty() {
            return false;
        }
        let t = self.track_from_name(name);
        if !self.init_stream(t, looped) {
            return false;
        }
        self.start_stream(0, looped);
        true
    }

    /// `_Stop`: only a playing or paused track (one that ended by itself keeps its handle).
    fn stop(&mut self) {
        if self.enabled && (self.is_playing || self.was_playing) {
            self.is_playing = false;
            self.was_playing = false;
            self.pending_file.clear();
            self.output().stream = None;
            self.mp3_active = false;
            self.title.clear();
            self.mp3_track = 0;
            self.mp3_loop = false;
            self.applied_volume = self.mp3_volume;
        }
    }

    /// `_Pause`.
    fn pause_now(&mut self) {
        if self.enabled && self.is_playing {
            if self.mp3_active
                && let Some(s) = &mut self.output().stream
            {
                s.paused = true;
            }
            self.was_playing = self.is_playing;
            self.is_playing = false;
            self.pending_file.clear();
        }
    }

    /// `_Resume`: a never-started stream starts, a finished one stays done; without an MP3 the
    /// MCI replay fails (no CD).
    fn resume_now(&mut self) {
        if self.enabled && self.was_playing {
            if !self.mp3_active {
                return;
            }
            if let Some(s) = &mut self.output().stream {
                s.paused = false;
                s.started = true;
            }
            self.is_playing = true;
        }
    }

    /// vt `Pause` (`svc_setpause`, focus loss, bgmvolume, `cd pause`).
    pub fn pause(&mut self) {
        if self.initialized {
            self.pause_now();
        }
    }

    /// vt `Resume`.
    pub fn resume(&mut self) {
        if self.initialized {
            self.resume_now();
        }
    }

    /// vt `Play` (`svc_cdtrack` loops; `cd play|loop`): any running fade is cancelled.
    pub fn play(&mut self, track: i32, looped: bool) {
        self.fade_out_time = 0.0;
        self.worker_play(track, looped);
    }

    /// vt `PlayFile`.
    fn play_file(&mut self, name: &str, looped: bool) {
        self.fade_out_time = 0.0;
        self.pending_file = gs_common::bytestr::truncate_encoded(name, 511).into_owned();
        self.worker_play(0, looped);
    }

    /// `_Play`: a pending file, else the table track.
    fn worker_play(&mut self, track: i32, looped: bool) {
        if !self.enabled {
            return;
        }
        if track == 0 && !self.pending_file.is_empty() {
            let name = std::mem::take(&mut self.pending_file);
            self.mp3_play_file(&name, looped);
            self.pending_file.clear();
            return;
        }
        self.pending_file.clear();
        self.mp3_play_track(track, looped);
    }

    /// `cmd_cd`: console output lines.
    pub fn cd_command(&mut self, args: &[&str], input: &MusicInput) -> Vec<String> {
        let mut out = Vec::new();
        let Some(&cmd) = args.get(1) else { return out };
        let arg = |i: usize| args.get(i).copied().unwrap_or("");
        match cmd.to_ascii_lowercase().as_str() {
            "on" => self.enabled = true,
            // The posted _Stop runs after m_bEnabled is cleared and does nothing.
            "off" => self.enabled = false,
            "reset" => {
                self.enabled = true;
                if self.is_playing {
                    self.stop();
                }
            }
            "remap" | "close" => {}
            "mp3info" => {
                out.push(format!("Current MP3 Title: {}\n", self.title));
                out.push(format!("Current MP3 Track: {}\n", self.mp3_track));
                // %i of a double: its low dword.
                out.push(format!("Current MP3 Volume: {}\n", f64::from(self.applied_volume).to_bits() as u32 as i32));
            }
            "mp3track" => {
                if args.len() > 2 {
                    let t = atoi(arg(2)).wrapping_add(1);
                    if self.mp3_play_track(t, false) {
                        self.play_track = t;
                        self.is_looping = false;
                        self.is_playing = true;
                    }
                }
            }
            "play" => self.play(atoi(arg(2)), false),
            "playfile" => self.play_file(arg(2), false),
            "loop" => self.play(atoi(arg(2)), true),
            "loopfile" => self.play_file(arg(2), true),
            "stop" => self.stop(),
            "fadeout" => {
                if self.is_playing {
                    self.fade_out_time = f64::from(input.fadetime) + input.realtime;
                    self.worker_frame(input);
                }
            }
            "pause" => self.pause(),
            "resume" => self.resume(),
            "eject" => {
                if self.is_playing {
                    self.stop();
                }
            }
            "info" => {
                out.push(format!("{} tracks\n", self.tracks.len() as u32 - 1));
                let mode = if self.is_looping { "looping" } else { "playing" };
                if self.is_playing {
                    out.push(format!("Currently {mode} track {}\n", self.play_track as u32));
                } else if self.was_playing {
                    out.push(format!("Paused {mode} track {}\n", self.play_track as u32));
                }
                out.push(format!("Volume is {:.6}\n", self.volume));
            }
            _ => {}
        }
        out
    }

    /// `cmd_mp3`.
    pub fn mp3_command(&mut self, args: &[&str]) {
        let Some(&cmd) = args.get(1) else { return };
        let name = args.get(2).copied().unwrap_or("");
        let looped = match cmd.to_ascii_lowercase().as_str() {
            "stop" => {
                self.stop();
                return;
            }
            "play" | "playfile" => false,
            "loop" | "loopfile" => true,
            _ => return,
        };
        self.stop();
        if cmd.eq_ignore_ascii_case("play") || cmd.eq_ignore_ascii_case("loop") {
            let t = self.track_from_name(name);
            self.fade_out_time = 0.0;
            self.worker_play(t as i32, looped);
        } else {
            self.fade_out_time = 0.0;
            self.pending_file = gs_common::bytestr::truncate_encoded(name, 511).into_owned();
            self.worker_play(0, looped);
        }
    }

    /// `CCDAudio::Shutdown`: closed abruptly, no fade.
    pub fn shutdown(&mut self) {
        self.output().stream = None;
        self.stop();
    }
}

/// `MP3_SetVolume`'s clamp: `v >= 0 ? fminf(1, v) : 0` (NaN → 0).
fn clamp_volume(v: f32) -> f32 {
    if v >= 0.0 { v.min(1.0) } else { 0.0 }
}

/// `atoi`: leading whitespace, sign and digits; 0 otherwise.
fn atoi(s: &str) -> i32 {
    let s = s.trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let v = digits.bytes().take_while(u8::is_ascii_digit).fold(0i32, |a, d| a.wrapping_mul(10).wrapping_add(i32::from(d - b'0')));
    if neg { v.wrapping_neg() } else { v }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gs_fs::{PathId, SearchPath};

    /// `n` silent MPEG-1 Layer III frames, 44100 Hz stereo 128 kbit/s (417 bytes each).
    fn silent_mp3(n: usize) -> Vec<u8> {
        let mut frame = vec![0u8; 417];
        frame[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        frame.repeat(n)
    }

    fn cd(name: &str, mp3: &[u8]) -> (CdAudio, MusicHandle) {
        let dir = std::env::temp_dir().join(format!("gs-music-{name}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("media")).unwrap();
        std::fs::write(dir.join("media/Half-Life01.mp3"), mp3).unwrap();
        let out = MusicHandle::default();
        let fs = FileSystem::new(vec![SearchPath { dir, id: PathId::Game, writable: false }]);
        (CdAudio::new(out.clone(), fs, 0.8), out)
    }

    fn input() -> MusicInput {
        MusicInput { bgmvolume: 1.0, mp3volume: 0.8, fadetime: 2.0, realtime: 10.0 }
    }

    #[test]
    fn the_miles_level_law() {
        assert_eq!(miles_level(1.0), 0.812_252_2);
        assert!((miles_level(0.8) - 0.5600).abs() < 1e-4);
    }

    #[test]
    fn names_are_appended_once_with_mp3_and_backslashes() {
        let (mut cd, _) = cd("names", &silent_mp3(4));
        assert_eq!(cd.track_from_name("media/Half-Life01"), 2);
        assert_eq!(cd.track_from_name("media/half-life01"), 29, "case-sensitive: a new entry");
        assert_eq!(cd.track_from_name("FOO.MP3"), 30);
        assert_eq!(cd.tracks[30], "FOO.MP3.mp3");
        assert_eq!(cd.track_from_name("c:/x"), 0);
        assert_eq!(cd.track_from_name("../x"), 0);
        assert_eq!(cd.cd_command(&["cd", "info"], &input())[0], "30 tracks\n");
    }

    #[test]
    fn a_track_plays_once_ends_by_itself_and_keeps_its_title() {
        let (mut cd, out) = cd("play", &silent_mp3(8));
        cd.cd_command(&["cd", "play", "2"], &input());
        assert_eq!(cd.cd_command(&["cd", "info"], &input())[1], "Currently playing track 2\n");
        // 8 frames × 1152 samples at 44100 Hz → 4608 frames at 22050 Hz, then done.
        let mut played = 0;
        while played < 10_000 {
            let mut o = out.lock().unwrap();
            if o.stream.as_ref().map(Stream::status) != Some(StreamStatus::Playing) {
                break;
            }
            o.next_frame();
            played += 1;
        }
        assert!((4608..4608 + 64).contains(&played), "{played}");
        assert!(cd.frame(&input()).is_none());
        let info = cd.cd_command(&["cd", "info"], &input());
        assert_eq!(info, vec!["28 tracks\n".to_owned(), "Volume is 1.000000\n".to_owned()]);
        assert_eq!(cd.cd_command(&["cd", "mp3info"], &input())[0], "Current MP3 Title: media\\Half-Life01.mp3\n");
        assert_eq!(cd.cd_command(&["cd", "mp3info"], &input())[2], "Current MP3 Volume: -1610612736\n", "0.8's low dword");
    }

    #[test]
    fn bgmvolume_toggles_pause_and_is_rewritten() {
        let (mut cd, out) = cd("bgm", &silent_mp3(64));
        cd.play(2, true);
        let mut i = input();
        i.bgmvolume = 0.5;
        assert_eq!(cd.frame(&i).as_deref(), Some("0.000000"));
        assert_eq!(out.lock().unwrap().stream.as_ref().map(Stream::status), Some(StreamStatus::Stopped));
        assert_eq!(cd.cd_command(&["cd", "info"], &i)[1], "Paused looping track 2\n");
        i.bgmvolume = 0.3;
        assert_eq!(cd.frame(&i).as_deref(), Some("1.000000"));
        assert_eq!(out.lock().unwrap().stream.as_ref().map(Stream::status), Some(StreamStatus::Playing));
    }

    #[test]
    fn a_fadeout_ramps_the_level_then_stops_and_restores_it() {
        let (mut cd, out) = cd("fade", &silent_mp3(64));
        cd.play(2, true);
        let mut i = input();
        cd.cd_command(&["cd", "fadeout"], &i);
        i.realtime = 11.0;
        cd.frame(&i);
        assert_eq!(cd.applied_volume, 0.4, "halfway: (12 - 11) / 2 · 0.8");
        i.realtime = 12.0;
        cd.frame(&i);
        assert!(out.lock().unwrap().stream.is_none(), "stopped at the end");
        assert_eq!(cd.applied_volume, 0.8);
    }

    #[test]
    fn cd_off_while_playing_leaves_the_music_running() {
        let (mut cd, out) = cd("off", &silent_mp3(64));
        cd.play(2, true);
        cd.cd_command(&["cd", "off"], &input());
        cd.cd_command(&["cd", "stop"], &input());
        assert_eq!(out.lock().unwrap().stream.as_ref().map(Stream::status), Some(StreamStatus::Playing));
    }
}
