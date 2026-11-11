//! The output device (the `SNDDMA_*` side): a cpal stream fed from a queue of 22050 Hz stereo
//! 16-bit frames, linearly resampled to the device rate. The callback counts the 22050 Hz
//! frames it consumed; that count is `soundtime` for [`crate::SoundSystem::update`]. The music
//! driver ([`crate::music`]) is pulled here too, frame for frame, as an independent stream: it
//! keeps playing when the engine queue runs dry, and the two are summed and clipped only at the
//! end, like the OS mixing two device streams.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::mixer::OUTPUT_RATE;
use crate::music::{MusicHandle, MusicOutput};

#[derive(Default)]
struct Shared {
    queue: Mutex<VecDeque<[i16; 2]>>,
    /// 22050 Hz frames consumed.
    consumed: AtomicU64,
    music: MusicHandle,
}

/// Linear resampling state of the callback.
struct Resampler {
    shared: Arc<Shared>,
    /// 22050 / device rate.
    step: f64,
    frac: f64,
    /// The music frames around the current position.
    music: [[i16; 2]; 2],
}

impl Resampler {
    /// The next output frame as -1..1 floats.
    fn next(&mut self, q: &mut VecDeque<[i16; 2]>, music: &mut MusicOutput) -> [f32; 2] {
        let t = self.frac as f32;
        let lerp = |a: [i16; 2], b: [i16; 2]| [0, 1].map(|c| f32::from(a[c]) + (f32::from(b[c]) - f32::from(a[c])) * t);
        let engine = match q.front() {
            Some(&a) => lerp(a, q.get(1).copied().unwrap_or(a)),
            None => [0.0; 2],
        };
        let m = lerp(self.music[0], self.music[1]);
        let out = [0, 1].map(|c| ((engine[c] + m[c]) / 32768.0).clamp(-1.0, 1.0));
        self.frac += self.step;
        while self.frac >= 1.0 {
            self.frac -= 1.0;
            self.music = [self.music[1], music.next_frame()];
            if q.pop_front().is_some() {
                self.shared.consumed.fetch_add(1, Ordering::Relaxed);
            }
        }
        out
    }

    fn fill<T: Copy>(&mut self, data: &mut [T], channels: usize, convert: impl Fn(f32) -> T) {
        let shared = self.shared.clone();
        let Ok(mut q) = shared.queue.lock() else { return };
        let mut music = shared.music.lock().unwrap_or_else(PoisonError::into_inner);
        for frame in data.chunks_mut(channels.max(1)) {
            let s = self.next(&mut q, &mut music);
            for (i, out) in frame.iter_mut().enumerate() {
                *out = convert(s[i.min(1)]);
            }
        }
    }
}

/// An open output stream.
pub struct Device {
    _stream: cpal::Stream,
    shared: Arc<Shared>,
}

impl Device {
    /// The default output device at its default rate.
    pub fn open() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no audio output device")?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let channels = usize::from(config.channels());
        let rate = config.sample_rate();
        let shared = Arc::new(Shared::default());
        let mut rs = Resampler { shared: shared.clone(), step: f64::from(OUTPUT_RATE) / f64::from(rate), frac: 0.0, music: [[0; 2]; 2] };
        let err = |e: cpal::StreamError| log::warn!("audio stream: {e}");
        let stream_config: cpal::StreamConfig = config.config();
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(&stream_config, move |d: &mut [f32], _| rs.fill(d, channels, |s| s), err, None),
            cpal::SampleFormat::I16 => device.build_output_stream(&stream_config, move |d: &mut [i16], _| rs.fill(d, channels, |s| (s * 32767.0) as i16), err, None),
            cpal::SampleFormat::U16 => {
                device.build_output_stream(&stream_config, move |d: &mut [u16], _| rs.fill(d, channels, |s| ((s * 32767.0) as i32 + 32768) as u16), err, None)
            }
            other => return Err(format!("unsupported sample format {other}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        log::info!("audio: {rate} Hz, {channels} channels");
        Ok(Self { _stream: stream, shared })
    }

    /// `soundtime`: 22050 Hz frames played so far.
    pub fn consumed(&self) -> u64 {
        self.shared.consumed.load(Ordering::Relaxed)
    }

    /// `shm`'s ring as `soundinfo` prints it: the DirectSound buffer's size in 16-bit samples
    /// (64 KB) and a non-zero buffer id.
    pub fn dma_info(&self) -> (u32, u32) {
        (0x10000 / 2, Arc::as_ptr(&self.shared) as usize as u32)
    }

    /// The music driver this device plays.
    pub fn music(&self) -> MusicHandle {
        self.shared.music.clone()
    }

    /// Queues freshly painted frames.
    pub fn push(&self, frames: &[[i16; 2]]) {
        if let Ok(mut q) = self.shared.queue.lock() {
            q.extend(frames);
        }
    }
}
