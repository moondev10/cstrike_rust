//! `host_basepal16` (docs/re/render.md §D.3): the Quake palette loaded with sign extension, so
//! components ≥ 128 read as `0xFF00 | c` through the 16-bit view.

/// Particle ramps (§D.8.3).
pub const RAMP1: [i16; 8] = [0x6f, 0x6d, 0x6b, 0x69, 0x67, 0x65, 0x63, 0x61];
pub const RAMP2: [i16; 8] = [0x6f, 0x6e, 0x6d, 0x6c, 0x6b, 0x6a, 0x68, 0x66];
pub const RAMP3: [i16; 8] = [0x6d, 0x6b, 6, 5, 4, 3, 0, 0];
pub const SPARK_RAMP: [i16; 9] = [0xfe, 0xfd, 0xfc, 0x6f, 0x6e, 0x6d, 0x6c, 0x67, 0x60];

#[derive(Debug, Clone)]
pub struct Palette {
    rgb: [[u8; 3]; 256],
}

impl Palette {
    /// From `gfx/palette.lmp` (768 bytes; shorter data leaves the rest black).
    pub fn new(lmp: &[u8]) -> Self {
        let mut rgb = [[0u8; 3]; 256];
        for (dst, src) in rgb.iter_mut().zip(lmp.chunks_exact(3)) {
            *dst = [src[0], src[1], src[2]];
        }
        Self { rgb }
    }

    /// The low bytes (correct 0..255 colour) of entry `index & 255`.
    pub fn color(&self, index: i32) -> [u8; 3] {
        self.rgb[(index & 0xFF) as usize]
    }

    /// The uint16 view: `c` below 128, else `c + 65280`.
    fn wide(c: u8) -> f32 {
        if c >= 128 { f32::from(c) + 65280.0 } else { f32::from(c) }
    }

    /// `R_LookupColor(r, g, b)`: nearest entry by the weighted distance over the sign-extended
    /// values; the first strict minimum wins.
    pub fn lookup(&self, r: u8, g: u8, b: u8) -> i16 {
        let mut best = 0i16;
        let mut best_d = 999_999.0f32;
        for (i, c) in self.rgb.iter().enumerate() {
            let dr = f32::from(r) - Self::wide(c[0]);
            let dg = f32::from(g) - Self::wide(c[1]);
            let db = f32::from(b) - Self::wide(c[2]);
            let d = dg * 0.5 * dg + dr * 0.3 * dr + db * 0.2 * db;
            if d < best_d {
                best_d = d;
                best = i as i16;
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bright_components_miss_bright_entries() {
        let mut lmp = vec![0u8; 768];
        lmp[3..6].copy_from_slice(&[255, 255, 255]);
        lmp[6..9].copy_from_slice(&[100, 100, 100]);
        let pal = Palette::new(&lmp);
        // The white entry reads as 65535 per channel, so mid grey wins for white.
        assert_eq!(pal.lookup(255, 255, 255), 2);
        assert_eq!(pal.lookup(0, 0, 0), 0);
        assert_eq!(pal.color(257), [255, 255, 255]);
    }
}
