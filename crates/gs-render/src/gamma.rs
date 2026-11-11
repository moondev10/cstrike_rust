//! `BuildGammaTable` (docs/re/render.md §A.3): the CPU-side gamma tables the GL renderer bakes
//! into textures (`texgammatable`) and lightmaps (`lightgammatable`). No hardware gamma is used.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GammaSettings {
    pub gamma: f32,
    pub lightgamma: f32,
    pub texgamma: f32,
    pub brightness: f32,
}

impl Default for GammaSettings {
    fn default() -> Self {
        Self { gamma: 2.2, lightgamma: 2.5, texgamma: 2.0, brightness: 0.0 }
    }
}

impl GammaSettings {
    /// `V_CheckGamma` clamps (multiplayer limits brightness to 2).
    pub fn clamped(self, multiplayer: bool) -> Self {
        let brightness = if multiplayer { self.brightness.min(2.0) } else { self.brightness };
        Self {
            gamma: self.gamma.clamp(1.8, 3.0),
            lightgamma: self.lightgamma.clamp(1.8, 3.0),
            texgamma: self.texgamma.clamp(1.8, 3.0),
            brightness: brightness.clamp(0.0, 100.0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GammaTables {
    pub settings: GammaSettings,
    pub tex: [u8; 256],
    pub light: [u16; 1024],
    /// `lineargammatable` (studio elights): `pow(i/1023, gamma)·1023`.
    pub linear: [u16; 1024],
    /// `screengammatable` (studio elights): `pow(i/1023, 1/gamma)·1023`.
    pub screen: [u16; 1024],
    /// `R_BuildLightMap` scale: `(int)(pow(2, 1/lightgamma) * 256 + 0.5)`.
    pub lightmap_scale: u32,
}

impl GammaTables {
    pub fn new(settings: GammaSettings) -> Self {
        let g = if settings.gamma == 0.0 { 2.5 } else { settings.gamma };
        let g1 = (1.0 / f64::from(g)) as f32;
        let g3 = g1 * settings.texgamma;
        let b = settings.brightness;
        let gb: f32 = if b <= 0.0 {
            0.125
        } else if b > 1.0 {
            0.05
        } else {
            (0.125 - f64::from(b) * f64::from(b) * 0.075) as f32
        };
        let mut tex = [0u8; 256];
        for (i, t) in tex.iter_mut().enumerate() {
            let v = ((i as f64 / 255.0).powf(f64::from(g3)) * 255.0) as i32;
            *t = v.clamp(0, 255) as u8;
        }
        let mut light = [0u16; 1024];
        for (j, l) in light.iter_mut().enumerate() {
            let mut f = (j as f64 / 1023.0).powf(f64::from(settings.lightgamma)) as f32;
            if b > 1.0 {
                f *= b;
            }
            let f2: f64 = if f <= gb {
                f64::from(f / gb) * 0.125
            } else {
                f64::from(f - gb) / f64::from(1.0 - gb) * 0.875 + 0.125
            };
            let v = (f64::from(f2 as f32).powf(f64::from(g1)) * 1023.0) as i32;
            *l = v.clamp(0, 1023) as u16;
        }
        let lightmap_scale = ((2.0f64).powf(1.0 / f64::from(settings.lightgamma)) * 256.0 + 0.5) as u32;
        let mut linear = [0u16; 1024];
        let mut screen = [0u16; 1024];
        for i in 0..1024 {
            let x = i as f64 / 1023.0;
            linear[i] = (x.powf(f64::from(g)) * 1023.0) as u16;
            screen[i] = (x.powf(1.0 / f64::from(g)) * 1023.0) as u16;
        }
        Self { settings, tex, light, linear, screen, lightmap_scale }
    }

    /// Palette entry after `GL_Upload16` gamma and `gl_dither` (`c | c >> 6`) for opaque textures.
    pub fn palette_color(&self, c: [u8; 3], dither: bool) -> [u8; 3] {
        c.map(|v| {
            let g = self.tex[usize::from(v)];
            if dither { g | (g >> 6) } else { g }
        })
    }

    /// Final lightmap byte for an accumulated `blocklights` value (`R_BuildLightMap`).
    pub fn lightmap_byte(&self, block: i64) -> u8 {
        let v = ((u64::from(self.lightmap_scale) as i64 * block) as u32 >> 14).min(1023);
        (self.light[v as usize] >> 2) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tables_match_engine_check_values() {
        let t = GammaTables::new(GammaSettings::default());
        let tex: Vec<u8> = (0..8).map(|i| t.tex[i * 32]).collect();
        assert_eq!(tex, [0, 38, 72, 104, 136, 166, 197, 226]);
        let light: Vec<u16> = (0..16).map(|i| t.light[i * 64]).collect();
        assert_eq!(light, [0, 43, 96, 152, 211, 273, 335, 400, 465, 532, 600, 669, 738, 808, 879, 951]);
        assert_eq!(t.light[1023], 1023);
        assert_eq!(t.lightmap_scale, 338);
    }

    #[test]
    fn lightmap_overflow_wraps_to_white_like_the_engine() {
        let t = GammaTables::new(GammaSettings::default());
        assert_eq!(t.lightmap_byte(-1), 255, "negative sums wrap as unsigned → clamp 1023");
        assert_eq!(t.lightmap_byte(0), 0);
    }
}
