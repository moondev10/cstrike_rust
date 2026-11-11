//! The engine's screen shake and screen fade (docs/re/view-effects.md §1–§3): `gVShake` with
//! `V_ScreenShake` / `V_CalcShake` / `V_ApplyShake`, and `cl.sf` with `V_ScreenFade` /
//! `V_FadeAlpha` and the colour `R_DrawScreenFade` draws. Times are stored as f32 and compared
//! as f64 against `cl.time`, as in the original.

use gs_common::rng::EngineRng;

/// `gVShake` (52 bytes; cleared only by `R_NewMap`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScreenShake {
    /// End time.
    time: f32,
    duration: f32,
    amplitude: f32,
    frequency: f32,
    next_shake: f32,
    offset: [f32; 3],
    angle: f32,
    applied_offset: [f32; 3],
    applied_angle: f32,
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    data.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
}

impl ScreenShake {
    /// `V_ScreenShake`: amplitude and duration in 1/4096 units, frequency in 1/256; the
    /// amplitude is only ever raised.
    pub fn message(&mut self, data: &[u8], cl_time: f64) {
        let a = (f64::from(u16_at(data, 0)) * 0.000_244_140_625) as f32;
        let d = (f64::from(u16_at(data, 2)) * 0.000_244_140_625) as f32;
        let f = (f64::from(u16_at(data, 4)) * 0.003_906_25) as f32;
        self.duration = d;
        self.time = (f64::from(d) + cl_time) as f32;
        if a > self.amplitude {
            self.amplitude = a;
        }
        self.next_shake = 0.0;
        self.frequency = f;
    }

    /// `V_CalcShake`: re-randomises the offsets every `duration / frequency` seconds (four
    /// engine draws), scales them by `sin(freq·t)·fraction²` and decays the amplitude. Expiry
    /// zeroes the applied values but keeps the amplitude.
    pub fn calc(&mut self, cl_time: f64, cl_oldtime: f64, rng: &EngineRng) {
        let t = cl_time;
        if t > f64::from(self.time) || self.duration <= 0.0 || self.amplitude <= 0.0 || self.frequency <= 0.0 {
            if self.time != 0.0 {
                self.time = 0.0;
                self.applied_offset = [0.0; 3];
                self.applied_angle = 0.0;
            }
            return;
        }
        let dt = (cl_time - cl_oldtime) as f32;
        if t > f64::from(self.next_shake) {
            self.next_shake = (f64::from(self.duration / self.frequency) + t) as f32;
            for i in 0..3 {
                self.offset[i] = rng.float(-self.amplitude, self.amplitude);
            }
            let hi = (f64::from(self.amplitude) * 0.25) as f32;
            let lo = (f64::from(-self.amplitude) * 0.25) as f32;
            self.angle = rng.float(lo, hi);
        }
        let fraction = ((t - f64::from(self.time)) / f64::from(self.duration)) as f32;
        let freq = if fraction == 0.0 { 0.0 } else { (self.frequency / fraction) * self.frequency };
        let k = ((f64::from(freq) * t).sin() * f64::from(fraction * fraction)) as f32;
        self.applied_offset = self.offset.map(|o| o * k);
        self.applied_angle = self.angle * k;
        self.amplitude -= (dt / (self.frequency * self.duration)) * self.amplitude;
    }

    /// `V_ApplyShake(origin, angles, factor)`: the offset onto the origin, the angle onto roll.
    pub fn apply(&self, origin: &mut [f32; 3], angles: &mut [f32; 3], factor: f32) {
        for (o, a) in origin.iter_mut().zip(self.applied_offset) {
            *o += factor * a;
        }
        angles[2] += factor * self.applied_angle;
    }
}

/// `screenfade_t` flags.
pub mod ffade {
    pub const OUT: i32 = 1;
    pub const MODULATE: i32 = 2;
    pub const STAYOUT: i32 = 4;
    pub const LONGFADE: i32 = 8;
}

/// `cl.sf` (cleared by `CL_ClearState`; `R_NewMap` resets it too).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScreenFade {
    speed: f32,
    end: f32,
    total_end: f32,
    reset: f32,
    rgba: [u8; 4],
    /// Sign-extended from the wire's i16; flag tests read the low byte.
    flags: i32,
}

impl ScreenFade {
    /// `V_ScreenFade`: a new message always replaces the whole state.
    pub fn message(&mut self, data: &[u8], cl_time: f64) {
        let duration = u16_at(data, 0);
        let hold = u16_at(data, 2);
        let flags = i32::from(u16_at(data, 4) as i16);
        let byte = |i: usize| data.get(i).copied().unwrap_or(0);
        let scale = if flags & ffade::LONGFADE != 0 { 256.0f32 } else { 4096.0 };
        let inv = 1.0 / f64::from(scale);
        let dur = (f64::from(duration) * inv) as f32;
        let hold = (f64::from(hold) * inv) as f32;
        self.end = dur;
        self.reset = hold;
        self.rgba = [byte(6), byte(7), byte(8), byte(9)];
        self.flags = flags;
        self.speed = 0.0;
        if duration == 0 {
            return;
        }
        let alpha = f32::from(self.rgba[3]);
        if flags & ffade::OUT == 0 {
            if dur != 0.0 {
                self.speed = alpha / dur;
            }
            self.reset = (f64::from(hold) + cl_time) as f32;
            self.end = self.reset + dur;
        } else {
            if dur != 0.0 {
                self.speed = -alpha / dur;
            }
            self.end = (f64::from(dur) + cl_time) as f32;
            self.total_end = self.end;
            self.reset = self.end + hold;
        }
    }

    /// `V_FadeAlpha` (once per drawn frame; the STAYOUT path moves `fadeEnd` as a side effect).
    pub fn alpha(&mut self, cl_time: f64) -> i32 {
        let t = cl_time;
        let a = i32::from(self.rgba[3]);
        let fl = self.flags & 0xFF;
        if t > f64::from(self.reset) && t > f64::from(self.end) && fl & ffade::STAYOUT == 0 {
            return 0;
        }
        let v = if fl & ffade::STAYOUT != 0 {
            if fl & ffade::OUT != 0 && f64::from(self.total_end) > t {
                (f64::from(self.total_end) - t) * f64::from(self.speed) + f64::from(a)
            } else {
                self.end = (t + 0.1) as f32;
                return a;
            }
        } else {
            let v = (f64::from(self.end) - t) * f64::from(self.speed);
            if fl & ffade::OUT != 0 { v + f64::from(a) } else { v }
        };
        // cvttsd2si: truncation; out of range gives INT_MIN.
        let iv = if v.is_finite() && v > f64::from(i32::MIN) - 1.0 && v < f64::from(i32::MAX) + 1.0 { v as i32 } else { i32::MIN };
        if iv > a { a } else { iv.max(0) }
    }

    /// `R_DrawScreenFade`'s colour for alpha `a`: `(rgba, modulate)`. Modulate darkens by
    /// `(A·(c−255) + 65025) >> 8` per channel with alpha 255.
    pub fn draw_color(&self, a: i32) -> ([u8; 4], bool) {
        if self.flags & 0xFF & ffade::MODULATE != 0 {
            let m = |c: u8| ((a * (i32::from(c) - 255) + 65025) >> 8) as u8;
            ([m(self.rgba[0]), m(self.rgba[1]), m(self.rgba[2]), 255], true)
        } else {
            ([self.rgba[0], self.rgba[1], self.rgba[2], a as u8], false)
        }
    }

    /// `V_ResetShakeFade`'s fade part (`R_NewMap`): cleared, or with `v_dark` a 5 s black
    /// hold fading out over 5 s. `fadeTotalEnd` is left as it was.
    pub fn new_map(&mut self, v_dark: bool, cl_time: f64) {
        self.flags = 0;
        if v_dark {
            self.rgba = [0, 0, 0, 255];
            self.speed = 51.0;
            self.reset = (cl_time + 5.0) as f32;
            self.end = self.reset + 5.0;
        } else {
            self.speed = 0.0;
            self.end = 0.0;
            self.reset = 0.0;
            self.rgba = [0; 4];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fade(duration_s: f32, hold_s: f32, flags: i16, alpha: u8, t: f64) -> ScreenFade {
        let mut f = ScreenFade::default();
        let mut data = Vec::new();
        data.extend_from_slice(&((duration_s * 4096.0) as u16).to_le_bytes());
        data.extend_from_slice(&((hold_s * 4096.0) as u16).to_le_bytes());
        data.extend_from_slice(&flags.to_le_bytes());
        data.extend_from_slice(&[255, 255, 255, alpha]);
        f.message(&data, t);
        f
    }

    #[test]
    fn fade_in_holds_then_falls_linearly() {
        let mut f = fade(2.0, 1.0, 0, 200, 100.0);
        assert_eq!(f.alpha(100.5), 200, "held");
        assert_eq!(f.alpha(102.0), 100, "halfway down");
        assert_eq!(f.alpha(103.01), 0, "over");
    }

    #[test]
    fn fade_out_rises_holds_then_vanishes_unless_stayout() {
        let mut f = fade(2.0, 1.0, ffade::OUT as i16, 200, 100.0);
        assert_eq!(f.alpha(101.0), 100);
        assert_eq!(f.alpha(102.5), 200);
        assert_eq!(f.alpha(103.01), 0, "drops at once after the hold");
        let mut s = fade(2.0, 1.0, (ffade::OUT | ffade::STAYOUT) as i16, 200, 100.0);
        assert_eq!(s.alpha(1000.0), 200, "stays out forever");
    }

    #[test]
    fn modulate_colour_has_the_254_ceiling() {
        let f = fade(1.0, 0.0, ffade::MODULATE as i16, 255, 0.0);
        assert_eq!(f.draw_color(255), ([254, 254, 254, 255], true));
    }

    #[test]
    fn shake_amplitude_only_rises_and_survives_expiry() {
        let rng = EngineRng::new(1);
        let mut s = ScreenShake::default();
        let msg = |a: f32, d: f32, f: f32| -> Vec<u8> {
            [(a * 4096.0) as u16, (d * 4096.0) as u16, (f * 256.0) as u16].iter().flat_map(|v| v.to_le_bytes()).collect()
        };
        s.message(&msg(10.0, 1.0, 100.0), 0.0);
        s.calc(0.5, 0.49, &rng);
        assert!(s.applied_offset != [0.0; 3]);
        s.message(&msg(2.0, 1.0, 100.0), 0.6);
        assert!(s.amplitude > 9.0, "a weaker shake keeps the larger amplitude");
        s.calc(5.0, 4.99, &rng);
        assert_eq!((s.applied_offset, s.applied_angle), ([0.0; 3], 0.0), "expired");
        assert!(s.amplitude > 9.0, "the amplitude is not reset at expiry");
    }
}
