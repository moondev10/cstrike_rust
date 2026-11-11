//! Streaming linear resampling, box-filtered first when decimating (enough for the voice band).

/// One mono stream from `from` Hz to `to` Hz.
#[derive(Debug, Clone)]
pub struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// The next output position past `last`, in input samples.
    pos: f64,
    /// The previous filtered input sample.
    last: f32,
    /// The box filter's recent inputs (as wide as one output sample spans), the next slot to
    /// overwrite, and their sum.
    taps: Vec<f32>,
    tap: usize,
    sum: f32,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        let step = f64::from(from) / f64::from(to.max(1));
        let width = if step > 1.0 { step.ceil() as usize } else { 1 };
        Self { step, pos: 0.0, last: 0.0, taps: vec![0.0; width], tap: 0, sum: 0.0 }
    }

    /// Feeds `input`, calling `emit` with each output sample due.
    pub fn process(&mut self, input: impl IntoIterator<Item = f32>, mut emit: impl FnMut(f32)) {
        let width = self.taps.len() as f32;
        for x in input {
            self.sum += x - self.taps[self.tap];
            self.taps[self.tap] = x;
            self.tap = (self.tap + 1) % self.taps.len();
            let y = self.sum / width;
            while self.pos < 1.0 {
                emit(self.last + (y - self.last) * self.pos as f32);
                self.pos += self.step;
            }
            self.pos -= 1.0;
            self.last = y;
        }
    }
}

/// A -1..1-scaled sample as 16-bit, clipped.
pub fn to_i16(s: f32) -> i16 {
    s.round().clamp(-32768.0, 32767.0) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_count_follows_the_rate_ratio_across_calls() {
        let mut r = Resampler::new(12000, 11025);
        let mut n = 0usize;
        for _ in 0..50 {
            r.process(std::iter::repeat_n(1000.0, 240), |_| n += 1);
        }
        // 1 s at 12 kHz in 20 ms pieces: 11025 samples, give or take the boundary sample.
        assert!((11024..=11026).contains(&n), "{n}");
    }

    #[test]
    fn decimating_a_tone_at_the_target_nyquist_cancels_it() {
        // 24 kHz at 48 kHz alternates sign every sample: it aliases to DC unless filtered.
        let mut r = Resampler::new(48000, 24000);
        let mut out = Vec::new();
        r.process((0..960).map(|i| if i % 2 == 0 { 10000.0 } else { -10000.0 }), |y| out.push(y));
        assert!(out[4..].iter().all(|y| y.abs() < 1.0), "{:?}", &out[..8]);
    }
}
