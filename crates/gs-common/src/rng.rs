//! The engine RNG behind `RandomLong` / `RandomFloat` (docs/re/render.md §D.2): Numerical
//! Recipes `ran1` (Park–Miller with a Bays–Durham shuffle). One generator serves the engine
//! and the client DLL (`pfnRandomLong` / `pfnRandomFloat`), so every consumer holds a clone
//! of the same [`EngineRng`].

use std::sync::{Arc, Mutex, PoisonError};

const IA: i32 = 16807;
const IM: i32 = 0x7FFF_FFFF;
const IQ: i32 = 127_773;
const NTAB: usize = 32;
const NDIV: i32 = 1 + (IM - 1) / NTAB as i32;

#[derive(Debug, Clone)]
pub struct Rng {
    idum: i32,
    iy: i32,
    iv: [i32; NTAB],
}

impl Rng {
    /// `SeedRandomNumberGenerator`: `idum = -seed` (the engine seeds with `time(NULL)`).
    pub fn new(seed: i32) -> Self {
        Self { idum: seed.wrapping_neg(), iy: 0, iv: [0; NTAB] }
    }

    /// Schrage step `idum = IA*idum mod IM` in wrapping int32 arithmetic.
    fn step(&mut self) {
        let k = self.idum / IQ;
        self.idum = IA.wrapping_mul(self.idum).wrapping_sub(IM.wrapping_mul(k));
        if self.idum < 0 {
            self.idum = self.idum.wrapping_add(IM);
        }
    }

    fn ran1(&mut self) -> i32 {
        if self.idum <= 0 || self.iy == 0 {
            self.idum = self.idum.wrapping_neg().max(1);
            for j in (0..NTAB + 8).rev() {
                self.step();
                if j < NTAB {
                    self.iv[j] = self.idum;
                }
            }
            self.iy = self.iv[0];
        }
        self.step();
        let j = usize::try_from(self.iy / NDIV).unwrap_or(0).min(NTAB - 1);
        self.iy = self.iv[j];
        self.iv[j] = self.idum;
        self.iy
    }

    /// `RandomFloat(lo, hi)`: `[lo, hi)`, computed in float.
    pub fn float(&mut self, lo: f32, hi: f32) -> f32 {
        let f = (f64::from(self.ran1()) * 4.656_612_875_245_797e-10).min(0.999_999_88) as f32;
        (hi - lo) * f + lo
    }

    /// `RandomLong(lo, hi)`: inclusive; returns `lo` for an empty or oversized range.
    pub fn long(&mut self, lo: i32, hi: i32) -> i32 {
        let span = hi.wrapping_sub(lo) as u32;
        let range = span.wrapping_add(1);
        if range == 0 || span > 0x7FFF_FFFF {
            return lo;
        }
        let max_acceptable = 0x7FFF_FFFFu32 - (0x8000_0000u32 % range);
        loop {
            let n = self.ran1() as u32;
            if n <= max_acceptable {
                return ((n % range) as i32).wrapping_add(lo);
            }
        }
    }
}

/// The process-wide generator, shared by the effects, studio, prediction, weapon and sound code.
#[derive(Debug, Clone)]
pub struct EngineRng(Arc<Mutex<Rng>>);

impl EngineRng {
    /// `SeedRandomNumberGenerator`.
    pub fn new(seed: i32) -> Self {
        Self(Arc::new(Mutex::new(Rng::new(seed))))
    }

    /// Seeded from the wall clock like the engine's `time(NULL)`.
    pub fn from_time() -> Self {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_secs());
        Self::new(secs as i32)
    }

    /// `RandomLong(lo, hi)`.
    pub fn long(&self, lo: i32, hi: i32) -> i32 {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).long(lo, hi)
    }

    /// `RandomFloat(lo, hi)`.
    pub fn float(&self, lo: f32, hi: f32) -> f32 {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).float(lo, hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_inclusive_and_bounded() {
        let mut rng = Rng::new(12345);
        let (mut saw_lo, mut saw_hi) = (false, false);
        for _ in 0..10_000 {
            let v = rng.long(-3, 3);
            assert!((-3..=3).contains(&v));
            saw_lo |= v == -3;
            saw_hi |= v == 3;
            let f = rng.float(-0.5, 0.5);
            assert!((-0.5..0.5).contains(&f));
        }
        assert!(saw_lo && saw_hi);
        assert_eq!(rng.long(5, 4), 5, "inverted range");
        assert_eq!(rng.long(i32::MIN, i32::MAX), i32::MIN, "oversized range");
    }

    #[test]
    fn minimal_standard_sequence() {
        // Park–Miller from seed 1: the 10000th value of x = 16807·x mod (2^31−1) is 1043618065.
        let mut rng = Rng { idum: 1, iy: 1, iv: [0; NTAB] };
        for _ in 0..10_000 {
            rng.step();
        }
        assert_eq!(rng.idum, 1_043_618_065);
    }
}
