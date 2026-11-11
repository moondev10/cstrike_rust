//! The microphone: the default input device through cpal, mixed to mono at the device rate,
//! scaled to 16-bit range.

use std::sync::{Arc, Mutex, PoisonError};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// An open input stream.
pub struct Capture {
    _stream: cpal::Stream,
    /// Captured samples not yet taken, at most a second of them (a stalled game loses the
    /// oldest).
    shared: Arc<Mutex<Vec<f32>>>,
    pub rate: u32,
}

/// Appends one callback's frames, mixed to mono, dropping the oldest beyond `cap`.
fn push<T: Copy>(shared: &Mutex<Vec<f32>>, data: &[T], channels: usize, cap: usize, convert: impl Fn(T) -> f32) {
    let mut q = shared.lock().unwrap_or_else(PoisonError::into_inner);
    for frame in data.chunks(channels) {
        q.push(frame.iter().map(|&s| convert(s)).sum::<f32>() / frame.len() as f32);
    }
    if q.len() > cap {
        let excess = q.len() - cap;
        q.drain(..excess);
    }
}

impl Capture {
    /// The default input device at its default configuration, recording.
    pub fn open() -> Result<Self, String> {
        let device = cpal::default_host().default_input_device().ok_or("no audio input device")?;
        let config = device.default_input_config().map_err(|e| e.to_string())?;
        let channels = usize::from(config.channels()).max(1);
        let rate = config.sample_rate();
        let cap = rate as usize;
        let shared = Arc::new(Mutex::new(Vec::with_capacity(cap)));
        let q = shared.clone();
        let err = |e: cpal::StreamError| log::warn!("voice capture: {e}");
        let stream_config: cpal::StreamConfig = config.config();
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(&stream_config, move |d: &[f32], _| push(&q, d, channels, cap, |s| s * 32768.0), err, None),
            cpal::SampleFormat::I16 => device.build_input_stream(&stream_config, move |d: &[i16], _| push(&q, d, channels, cap, f32::from), err, None),
            cpal::SampleFormat::I32 => {
                device.build_input_stream(&stream_config, move |d: &[i32], _| push(&q, d, channels, cap, |s| (f64::from(s) / 65536.0) as f32), err, None)
            }
            cpal::SampleFormat::U16 => device.build_input_stream(&stream_config, move |d: &[u16], _| push(&q, d, channels, cap, |s| f32::from(s) - 32768.0), err, None),
            other => return Err(format!("unsupported sample format {other}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self { _stream: stream, shared, rate })
    }

    /// Moves everything captured since the last call onto `out`.
    pub fn take(&self, out: &mut Vec<f32>) {
        out.append(&mut self.shared.lock().unwrap_or_else(PoisonError::into_inner));
    }
}
