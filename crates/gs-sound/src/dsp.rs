//! Room DSP `SX_RoomFX` (docs/re/audio.md §10): lowpass/modulation, two-line reverb, mono delay
//! and the left-channel delay on the int32 paint buffer, with the exact integer arithmetic.

/// `rgsxpre`: lp, mod, size, refl, rvblp, delay, feedback, dlylp, left.
const PRESETS: [[f32; 9]; 29] = [
    [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.0],
    [0.0, 0.0, 0.0, 0.0, 1.0, 0.065, 0.1, 0.0, 0.01],
    [0.0, 0.0, 0.0, 0.0, 1.0, 0.02, 0.75, 0.0, 0.01],
    [0.0, 0.0, 0.0, 0.0, 1.0, 0.03, 0.78, 0.0, 0.02],
    [0.0, 0.0, 0.0, 0.0, 1.0, 0.06, 0.77, 0.0, 0.03],
    [0.0, 0.0, 0.05, 0.85, 1.0, 0.008, 0.96, 2.0, 0.01],
    [0.0, 0.0, 0.05, 0.88, 1.0, 0.01, 0.98, 2.0, 0.02],
    [0.0, 0.0, 0.05, 0.92, 1.0, 0.015, 0.995, 2.0, 0.04],
    [0.0, 0.0, 0.05, 0.84, 1.0, 0.0, 0.0, 2.0, 0.012],
    [0.0, 0.0, 0.05, 0.9, 1.0, 0.0, 0.0, 2.0, 0.008],
    [0.0, 0.0, 0.05, 0.95, 1.0, 0.0, 0.0, 2.0, 0.004],
    [0.0, 0.0, 0.05, 0.7, 0.0, 0.0, 0.0, 2.0, 0.012],
    [0.0, 0.0, 0.055, 0.78, 0.0, 0.0, 0.0, 2.0, 0.008],
    [0.0, 0.0, 0.05, 0.86, 0.0, 0.0, 0.0, 2.0, 0.002],
    [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.01],
    [1.0, 0.0, 0.0, 0.0, 1.0, 0.06, 0.85, 2.0, 0.02],
    [1.0, 0.0, 0.0, 0.0, 1.0, 0.2, 0.6, 2.0, 0.05],
    [0.0, 0.0, 0.05, 0.8, 1.0, 0.0, 0.48, 2.0, 0.016],
    [0.0, 0.0, 0.06, 0.9, 1.0, 0.0, 0.52, 2.0, 0.01],
    [0.0, 0.0, 0.07, 0.94, 1.0, 0.3, 0.6, 2.0, 0.008],
    [0.0, 0.0, 0.0, 0.0, 1.0, 0.3, 0.42, 2.0, 0.0],
    [0.0, 0.0, 0.0, 0.0, 1.0, 0.35, 0.48, 2.0, 0.0],
    [0.0, 0.0, 0.0, 0.0, 1.0, 0.38, 0.6, 2.0, 0.0],
    [0.0, 0.0, 0.05, 0.9, 1.0, 0.2, 0.28, 0.0, 0.0],
    [0.0, 0.0, 0.07, 0.9, 1.0, 0.3, 0.4, 0.0, 0.0],
    [0.0, 0.0, 0.09, 0.9, 1.0, 0.35, 0.5, 0.0, 0.0],
    [0.0, 1.0, 0.01, 0.9, 0.0, 0.0, 0.0, 2.0, 0.05],
    [0.0, 0.0, 0.0, 0.0, 1.0, 0.009, 0.999, 2.0, 0.04],
    [0.0, 0.0, 0.001, 0.999, 0.0, 0.2, 0.8, 2.0, 0.05],
];

/// The room cvars (`room_*`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoomParams {
    pub lp: f32,
    pub modulation: f32,
    pub size: f32,
    pub refl: f32,
    pub rvblp: f32,
    pub delay: f32,
    pub feedback: f32,
    pub dlylp: f32,
    pub left: f32,
}

impl Default for RoomParams {
    fn default() -> Self {
        Self { lp: 0.0, modulation: 0.0, size: 0.0, refl: 0.7, rvblp: 1.0, delay: 0.0, feedback: 0.2, dlylp: 1.0, left: 0.0 }
    }
}

impl RoomParams {
    fn preset(row: &[f32; 9]) -> Self {
        let [lp, modulation, size, refl, rvblp, delay, feedback, dlylp, left] = *row;
        Self { lp, modulation, size, refl, rvblp, delay, feedback, dlylp, left }
    }
}

fn clamp16(v: i32) -> i32 {
    v.clamp(-32768, 32767)
}

/// One `dly_t` line (int16 ring buffer).
#[derive(Debug, Clone, Default)]
struct Delay {
    buf: Vec<i16>,
    lp: i32,
    input: usize,
    out: usize,
    outxf: usize,
    xfade: i32,
    delaysamples: i32,
    feed: i32,
    lp0: i32,
    lp1: i32,
    modulation: i32,
    modcur: i32,
}

impl Delay {
    fn active(&self) -> bool {
        !self.buf.is_empty()
    }

    /// `DLY_Init(maxsec)` keeping `delaysamples`.
    fn init(&mut self, maxsec: f32, shift: u32) {
        let size = (((11025.0 * maxsec.min(0.4)) as i32) << shift) as usize + 1;
        let delaysamples = self.delaysamples;
        *self = Self { buf: vec![0; size], delaysamples, lp: 1, ..Self::default() };
        self.out = wrap_sub(0, delaysamples, size);
    }

    fn free(&mut self) {
        *self = Self::default();
    }

    fn advance(&mut self) {
        let size = self.buf.len();
        self.input = (self.input + 1) % size;
        self.out = (self.out + 1) % size;
    }
}

/// `(a - b) mod size` for ring indices.
fn wrap_sub(a: usize, b: i32, size: usize) -> usize {
    (a as i64 - i64::from(b)).rem_euclid(size as i64) as usize
}

/// `SX_*` state.
#[derive(Debug, Clone)]
pub struct RoomFx {
    lines: [Delay; 4],
    lp: [i32; 10],
    /// The applied room type, -1 before the first.
    last_type: i32,
    /// "last value" caches of the check routines (-1 forces a re-check).
    last_delay: f32,
    last_size: f32,
    last_left: f32,
    last_hires: Option<bool>,
    params: RoomParams,
    /// A preset was applied: `SX_RoomFX` writes its values into the nine `room_*` cvars.
    pub preset_written: Option<RoomParams>,
}

impl Default for RoomFx {
    fn default() -> Self {
        Self {
            lines: Default::default(),
            lp: [0; 10],
            last_type: -1,
            last_delay: -1.0,
            last_size: -1.0,
            last_left: -1.0,
            last_hires: None,
            params: RoomParams::default(),
            preset_written: None,
        }
    }
}

impl RoomFx {
    /// The current parameters (the `room_*` cvars after a preset).
    pub fn params(&self) -> RoomParams {
        self.params
    }

    /// `SX_RoomFX` over `buf` (stereo frames at 11025 Hz, or 22050 Hz with `hires`).
    /// `room_type` is the preset, already switched to `waterroom_type` under water; `cvars` are
    /// the `room_*` values the check routines read (a preset switch overrides them and is
    /// reported in [`Self::preset_written`]).
    pub fn process(&mut self, buf: &mut [[i32; 2]], room_type: i32, hires: bool, room_off: bool, cvars: RoomParams) {
        if room_off {
            return;
        }
        if self.last_hires != Some(hires) {
            self.last_hires = Some(hires);
            self.last_delay = -1.0;
            self.last_size = -1.0;
            self.last_left = -1.0;
            for l in &mut self.lines {
                l.free();
            }
        }
        if room_type == self.last_type {
            if room_type == 0 {
                return;
            }
            self.params = cvars;
        } else {
            self.last_type = room_type;
            match usize::try_from(room_type).ok().and_then(|i| PRESETS.get(i)) {
                Some(row) => {
                    self.params = RoomParams::preset(row);
                    self.preset_written = Some(self.params);
                }
                None => self.params = cvars,
            }
        }
        let shift = u32::from(hires);
        self.check_delay(shift);
        self.check_reverb(shift);
        self.check_stereo_delay(shift);
        let p = self.params;
        if p.lp != 0.0 || p.modulation != 0.0 {
            self.lowpass_mod(buf, p.lp != 0.0, p.modulation != 0.0);
        }
        if self.lines[1].active() {
            self.reverb(buf);
        }
        if self.lines[0].active() {
            self.mono_delay(buf);
        }
        if self.lines[3].active() {
            self.stereo_delay(buf);
        }
    }

    /// `DLY_CheckNewDelayVal`.
    fn check_delay(&mut self, shift: u32) {
        let p = self.params;
        let line = &mut self.lines[0];
        if p.delay != self.last_delay {
            self.last_delay = p.delay;
            if p.delay == 0.0 {
                line.free();
            } else {
                line.delaysamples = ((11025.0 * p.delay.min(0.4)) as i32) << shift;
                if !line.active() {
                    line.init(0.4, shift);
                }
                let size = line.buf.len();
                line.buf.iter_mut().for_each(|s| *s = 0);
                line.lp0 = 0;
                line.lp1 = 0;
                line.input = 0;
                line.out = wrap_sub(0, line.delaysamples, size);
                if line.delaysamples == 0 {
                    line.free();
                }
            }
        }
        line.lp = p.dlylp as i32;
        line.feed = (p.feedback * 255.0) as i32;
    }

    /// `RVB_CheckNewReverbVal`: two lines with modulated taps.
    fn check_reverb(&mut self, shift: u32) {
        let p = self.params;
        if p.size != self.last_size {
            self.last_size = p.size;
            if p.size == 0.0 {
                self.lines[1].free();
                self.lines[2].free();
            } else {
                for (k, (modulation, scale)) in [(1, (500, 1.0)), (2, (700, 0.71))] {
                    let line = &mut self.lines[k];
                    let d = ((11025.0 * (p.size * scale).min(0.1)) as i32) << shift;
                    if !line.active() {
                        line.delaysamples = d;
                        line.init(0.1, shift);
                    }
                    line.modulation = modulation << shift;
                    line.modcur = line.modulation;
                    if d != line.delaysamples {
                        line.outxf = wrap_sub(line.input, d, line.buf.len());
                        line.xfade = 32;
                    }
                    if line.delaysamples == 0 {
                        line.free();
                    }
                }
            }
        }
        for k in 1..3 {
            self.lines[k].lp = p.rvblp as i32;
            self.lines[k].feed = (p.refl * 255.0) as i32;
        }
    }

    /// `DLY_CheckNewStereoDelayVal`.
    fn check_stereo_delay(&mut self, shift: u32) {
        let p = self.params;
        if p.left == self.last_left {
            return;
        }
        self.last_left = p.left;
        let line = &mut self.lines[3];
        if p.left == 0.0 {
            line.free();
            return;
        }
        let d = ((11025.0 * p.left.min(0.1)) as i32) << shift;
        if !line.active() {
            line.delaysamples = d;
            line.init(0.1, shift);
        }
        if d != line.delaysamples {
            line.outxf = wrap_sub(line.input, d, line.buf.len());
            line.xfade = 128;
        }
        line.modulation = 0;
        line.modcur = 0;
        if line.delaysamples == 0 {
            line.free();
        }
    }

    /// `SX_LowpassMod`: the 5-tap average (whose left history borrows a right sample) and the
    /// modulation, whose targets never move: a constant ×255/256.
    fn lowpass_mod(&mut self, buf: &mut [[i32; 2]], lp: bool, modulation: bool) {
        let h = &mut self.lp;
        for f in buf.iter_mut() {
            let [mut l, mut r] = *f;
            if lp {
                let nl = (l + h[0] + h[1] + h[2] + h[3] + h[4]) >> 2;
                let nr = (r + h[5] + h[6] + h[7] + h[8] + h[9]) >> 2;
                let (old3, old5) = (h[3], h[5]);
                h[0] = h[1];
                h[1] = h[2];
                h[2] = old3;
                h[3] = l;
                h[4] = old5;
                h[5] = h[6];
                h[6] = h[7];
                h[7] = h[8];
                h[8] = r;
                h[9] = r;
                l = nl;
                r = nr;
            }
            if modulation {
                l = (l * 255) >> 8;
                r = (r * 255) >> 8;
            }
            *f = [clamp16(l), clamp16(r)];
        }
    }

    /// `RVB_DoReverb`.
    fn reverb(&mut self, buf: &mut [[i32; 2]]) {
        for f in buf.iter_mut() {
            let [l, r] = *f;
            let mono = (l + r) >> 1;
            let mut acc = 0;
            for k in 1..3 {
                let line = &mut self.lines[k];
                if !line.active() {
                    continue;
                }
                line.modcur = if line.modcur - 1 < 0 { line.modulation } else { line.modcur - 1 };
                let size = line.buf.len();
                let mut y = i32::from(line.buf[line.out]);
                if line.xfade > 0 {
                    y = i32::from((((line.xfade * y) >> 5) + (((32 - line.xfade) * i32::from(line.buf[line.outxf])) >> 5)) as i16);
                    line.outxf = (line.outxf + 1) % size;
                    if line.xfade == 1 {
                        line.out = line.outxf;
                    }
                    line.xfade -= 1;
                } else if y == 0 && l == 0 && r == 0 {
                    line.lp0 = 0;
                    line.buf[line.input] = 0;
                    line.advance();
                    continue;
                }
                let v = if y != 0 { clamp16(mono + ((line.feed * y) >> 8)) } else { mono };
                let w = if line.lp != 0 {
                    let w = (line.lp0 + v) >> 1;
                    line.lp0 = v;
                    w
                } else {
                    v
                };
                line.buf[line.input] = w as i16;
                acc += w;
                line.advance();
            }
            let o = (11 * acc) >> 6;
            *f = [clamp16(l + o), clamp16(r + o)];
        }
    }

    fn mono_delay(&mut self, buf: &mut [[i32; 2]]) {
        let line = &mut self.lines[0];
        for f in buf.iter_mut() {
            let [l, r] = *f;
            let y = i32::from(line.buf[line.out]);
            if y != 0 || l != 0 || r != 0 {
                let v = clamp16(((l + r) >> 1) + ((line.feed * y) >> 8));
                let w = if line.lp != 0 {
                    let w = (line.lp0 + line.lp1 + 2 * v) >> 2;
                    line.lp0 = line.lp1;
                    line.lp1 = v;
                    w
                } else {
                    v
                };
                line.buf[line.input] = w as i16;
                *f = [clamp16(l + (w >> 2)), clamp16(r + (w >> 2))];
            } else {
                line.lp0 = 0;
                line.lp1 = 0;
                line.buf[line.input] = 0;
            }
            line.advance();
        }
    }

    /// The left-channel delay: left is replaced by its delayed copy.
    fn stereo_delay(&mut self, buf: &mut [[i32; 2]]) {
        let line = &mut self.lines[3];
        let size = line.buf.len();
        for f in buf.iter_mut() {
            let l = f[0];
            let mut y = i32::from(line.buf[line.out]);
            if line.xfade > 0 {
                y = ((line.xfade * y) >> 7) + (((128 - line.xfade) * i32::from(line.buf[line.outxf])) >> 7);
                line.outxf = (line.outxf + 1) % size;
                if line.xfade == 1 {
                    line.out = line.outxf;
                }
                line.xfade -= 1;
                line.buf[line.input] = clamp16(l) as i16;
                f[0] = y;
            } else if y == 0 && l == 0 {
                line.buf[line.input] = 0;
            } else {
                line.buf[line.input] = clamp16(l) as i16;
                f[0] = y;
            }
            line.advance();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_type_zero_is_dry_and_a_delay_preset_echoes() {
        let mut fx = RoomFx::default();
        let mut buf = vec![[1000, 1000]; 4];
        fx.process(&mut buf, 0, false, false, RoomParams::default());
        fx.process(&mut buf, 0, false, false, RoomParams::default());
        assert_eq!(buf, vec![[1000, 1000]; 4]);
        // Preset 2: a 20 ms mono delay (220 samples at 11025) with 0.75 feedback, no lowpass.
        let mut fx = RoomFx::default();
        let mut buf = vec![[0, 0]; 300];
        buf[0] = [4000, 4000];
        fx.process(&mut buf, 2, false, false, RoomParams::default());
        assert_eq!(fx.preset_written, Some(RoomParams::preset(&PRESETS[2])), "a preset switch is written to the room cvars");
        assert_eq!(fx.params().delay, 0.02);
        // The dry impulse plus a quarter of itself; the left channel is delayed by 110 samples.
        assert_eq!(buf[0], [0, 5000]);
        assert_eq!(buf[110][0], 5000);
        // The echo after 220 samples: feedback (0.75·255 = 191) × 4000 >> 8, quartered.
        assert_eq!(buf[220][1], ((191 * 4000) >> 8) >> 2);
    }
}
