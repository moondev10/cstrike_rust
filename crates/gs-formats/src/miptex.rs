//! `miptex_t`: 8-bit paletted texture with 4 mip levels, used inside BSP texture lumps and WAD3 files.
//!
//! Layout: `name[16]`, `width u32`, `height u32`, `offsets[4] u32` (relative to the miptex start),
//! then mip pixels; after the smallest mip: `u16 palette_count` and `palette_count * RGB`.
//! A miptex with `offsets[0] == 0` has no pixels and is resolved by name from WAD files.

use crate::FormatError;
use crate::reader::Reader;

const WHAT: &str = "miptex";
pub const NAME_LEN: usize = 16;
pub const MIP_LEVELS: usize = 4;
/// Largest dimension accepted; the engine rejects anything absurd long before this.
const MAX_DIMENSION: u32 = 4096;

#[derive(Debug, Clone)]
pub struct MipTex {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// `None` when the texture lives in an external WAD.
    pub data: Option<MipData>,
}

#[derive(Debug, Clone)]
pub struct MipData {
    pub mips: [Vec<u8>; MIP_LEVELS],
    pub palette: Vec<[u8; 3]>,
}

impl MipTex {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data, WHAT);
        let name = r.name(NAME_LEN)?;
        let width = r.u32()?;
        let height = r.u32()?;
        let offsets = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(FormatError::Invalid { what: WHAT, detail: format!("{name}: bad size {width}x{height}") });
        }
        if offsets[0] == 0 {
            return Ok(Self { name, width, height, data: None });
        }
        let mut mips: [Vec<u8>; MIP_LEVELS] = Default::default();
        for (level, mip) in mips.iter_mut().enumerate() {
            let len = mip_len(width, height, level);
            *mip = Reader::at(data, offsets[level] as usize, WHAT)?.bytes(len)?.to_vec();
        }
        let palette_at = offsets[MIP_LEVELS - 1] as usize + mip_len(width, height, MIP_LEVELS - 1);
        let mut r = Reader::at(data, palette_at, WHAT)?;
        let count = usize::from(r.u16()?);
        let raw = r.bytes(count * 3)?;
        let palette = raw.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        Ok(Self { name, width, height, data: Some(MipData { mips, palette }) })
    }

    pub fn level_size(&self, level: usize) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }

    /// `{name`: palette index 255 is transparent.
    pub fn is_masked(&self) -> bool {
        self.name.starts_with('{')
    }

    /// RGBA8 pixels of one mip level; `None` for external textures.
    pub fn rgba(&self, level: usize) -> Option<Vec<u8>> {
        let data = self.data.as_ref()?;
        let masked = self.is_masked();
        let mut out = Vec::with_capacity(data.mips[level].len() * 4);
        for &index in &data.mips[level] {
            if masked && index == 255 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let [r, g, b] = data.palette.get(usize::from(index)).copied().unwrap_or([0, 0, 0]);
                out.extend_from_slice(&[r, g, b, 255]);
            }
        }
        Some(out)
    }
}

fn mip_len(width: u32, height: u32, level: usize) -> usize {
    ((width >> level).max(1) * (height >> level).max(1)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(name: &str, w: u32, h: u32, palette_len: u16) -> Vec<u8> {
        let mut out = Vec::new();
        let mut n = [0u8; 16];
        n[..name.len()].copy_from_slice(name.as_bytes());
        out.extend_from_slice(&n);
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        let mut offset = 40u32;
        let mut offsets = Vec::new();
        for level in 0..4 {
            offsets.push(offset);
            offset += mip_len(w, h, level) as u32;
        }
        for o in &offsets {
            out.extend_from_slice(&o.to_le_bytes());
        }
        for level in 0..4 {
            out.extend((0..mip_len(w, h, level)).map(|i| (i % 256) as u8));
        }
        out.extend_from_slice(&palette_len.to_le_bytes());
        out.extend((0..palette_len as usize * 3).map(|i| (i % 251) as u8));
        out
    }

    #[test]
    fn masked_index_255_is_transparent() {
        let mut raw = build("{fence", 16, 16, 256);
        raw[40] = 255;
        let tex = MipTex::parse(&raw).ok();
        let rgba = tex.as_ref().and_then(|t| t.rgba(0)).unwrap_or_default();
        assert_eq!(&rgba[..4], &[0, 0, 0, 0]);
        assert_eq!(rgba[7], 255, "other indices stay opaque");
    }

    #[test]
    fn truncated_palette_is_an_error() {
        let mut raw = build("wall", 16, 16, 256);
        raw.truncate(raw.len() - 1);
        assert!(matches!(MipTex::parse(&raw), Err(FormatError::Truncated { .. })));
    }

    #[test]
    fn external_texture_has_no_pixels() {
        let mut raw = build("wall", 32, 16, 256);
        raw[24..40].fill(0);
        let tex = MipTex::parse(&raw).ok();
        assert!(tex.is_some_and(|t| t.data.is_none() && t.width == 32 && t.height == 16));
    }
}
