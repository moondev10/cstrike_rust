//! SPR v2 sprites (`Mod_LoadSpriteModel`, docs/re/render.md §C.1).

use crate::FormatError;
use crate::reader::Reader;

const WHAT: &str = "sprite";

/// Sprite orientation (`type` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteType {
    ParallelUpright,
    FacingUpright,
    Parallel,
    Oriented,
    ParallelOriented,
}

/// Texel interpretation (`texFormat` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteFormat {
    Normal,
    Additive,
    IndexAlpha,
    AlphaTest,
}

/// `mspriteframe_t` plus its pixels.
#[derive(Debug, Clone)]
pub struct SpriteFrame {
    pub width: u32,
    pub height: u32,
    /// `origin_y`.
    pub up: f32,
    /// `origin_y - height`.
    pub down: f32,
    /// `origin_x`.
    pub left: f32,
    /// `origin_x + width`.
    pub right: f32,
    /// 8-bit palette indices, top row first.
    pub pixels: Vec<u8>,
}

/// A frame slot: single frames draw; group frames never do (`R_GetSpriteFrame` returns NULL).
#[derive(Debug, Clone)]
pub enum SpriteFrameSlot {
    Single(SpriteFrame),
    Group(Vec<(f32, SpriteFrame)>),
}

#[derive(Debug, Clone)]
pub struct Sprite {
    pub kind: SpriteType,
    pub format: SpriteFormat,
    pub max_width: i32,
    pub max_height: i32,
    pub beam_length: f32,
    pub sync_type: i32,
    /// 256 entries (missing ones are black).
    pub palette: Vec<[u8; 3]>,
    pub frames: Vec<SpriteFrameSlot>,
}

fn frame(r: &mut Reader) -> Result<SpriteFrame, FormatError> {
    let origin_x = r.i32()?;
    let origin_y = r.i32()?;
    let width = r.i32()?;
    let height = r.i32()?;
    let (Ok(w), Ok(h)) = (u32::try_from(width), u32::try_from(height)) else {
        return Err(FormatError::Invalid { what: WHAT, detail: format!("frame size {width}x{height}") });
    };
    let pixels = r.bytes((w as usize) * (h as usize))?.to_vec();
    Ok(SpriteFrame {
        width: w,
        height: h,
        up: origin_y as f32,
        down: (origin_y - height) as f32,
        left: origin_x as f32,
        right: (origin_x + width) as f32,
        pixels,
    })
}

impl Sprite {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data, WHAT);
        let ident = r.bytes(4)?;
        if ident != b"IDSP" {
            return Err(FormatError::BadHeader { what: WHAT, found: String::from_utf8_lossy(ident).into_owned() });
        }
        let version = r.i32()?;
        if version != 2 {
            return Err(FormatError::Invalid { what: WHAT, detail: format!("wrong version number ({version} should be 2)") });
        }
        let kind = match r.i32()? {
            0 => SpriteType::ParallelUpright,
            1 => SpriteType::FacingUpright,
            2 => SpriteType::Parallel,
            3 => SpriteType::Oriented,
            4 => SpriteType::ParallelOriented,
            other => return Err(FormatError::Invalid { what: WHAT, detail: format!("Bad sprite type {other}") }),
        };
        let format = match r.i32()? {
            1 => SpriteFormat::Additive,
            2 => SpriteFormat::IndexAlpha,
            3 => SpriteFormat::AlphaTest,
            _ => SpriteFormat::Normal,
        };
        let _bounding_radius = r.f32()?;
        let max_width = r.i32()?;
        let max_height = r.i32()?;
        let numframes = r.i32()?;
        if numframes < 1 {
            return Err(FormatError::Invalid { what: WHAT, detail: format!("Invalid # of frames: {numframes}") });
        }
        let beam_length = r.f32()?;
        let sync_type = r.i32()?;
        let colors = usize::from(r.u16()?);
        let raw = r.bytes(colors * 3)?;
        let mut palette: Vec<[u8; 3]> = raw.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        palette.resize(256, [0, 0, 0]);
        let mut frames = Vec::with_capacity(numframes as usize);
        for _ in 0..numframes {
            if r.i32()? == 0 {
                frames.push(SpriteFrameSlot::Single(frame(&mut r)?));
            } else {
                let count = r.i32()?.max(0) as usize;
                let intervals = (0..count).map(|_| r.f32()).collect::<Result<Vec<_>, _>>()?;
                if intervals.iter().any(|&i| i <= 0.0) {
                    return Err(FormatError::Invalid { what: WHAT, detail: "Mod_LoadSpriteGroup: interval<=0".into() });
                }
                let group = intervals.into_iter().map(|i| Ok((i, frame(&mut r)?))).collect::<Result<Vec<_>, FormatError>>()?;
                frames.push(SpriteFrameSlot::Group(group));
            }
        }
        Ok(Self { kind, format, max_width, max_height, beam_length, sync_type, palette, frames })
    }

    /// `R_GetSpriteFrame`: `(int)frame`, out of range → 0; group slots → `None`.
    pub fn frame(&self, frame: f32) -> Option<&SpriteFrame> {
        let mut f = frame as i32;
        if f < 0 || f as usize >= self.frames.len() {
            f = 0;
        }
        match self.frames.get(f as usize)? {
            SpriteFrameSlot::Single(s) => Some(s),
            SpriteFrameSlot::Group(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sprite_bytes(kind: i32, format: i32) -> Vec<u8> {
        let mut b = b"IDSP".to_vec();
        for v in [2, kind, format] {
            b.extend_from_slice(&i32::to_le_bytes(v));
        }
        b.extend_from_slice(&f32::to_le_bytes(4.0));
        for v in [2, 2, 1] {
            b.extend_from_slice(&i32::to_le_bytes(v));
        }
        b.extend_from_slice(&f32::to_le_bytes(0.0));
        b.extend_from_slice(&i32::to_le_bytes(1));
        b.extend_from_slice(&u16::to_le_bytes(256));
        b.extend((0..256u32).flat_map(|i| [i as u8, 0, 0]));
        b.extend_from_slice(&i32::to_le_bytes(0));
        for v in [-1, 1, 2, 2] {
            b.extend_from_slice(&i32::to_le_bytes(v));
        }
        b.extend_from_slice(&[1, 2, 3, 255]);
        b
    }

    #[test]
    fn parses_single_frame_with_origin_box() {
        let s = Sprite::parse(&sprite_bytes(2, 3)).unwrap();
        assert_eq!(s.kind, SpriteType::Parallel);
        assert_eq!(s.format, SpriteFormat::AlphaTest);
        let f = s.frame(0.9).unwrap();
        assert_eq!((f.left, f.right, f.up, f.down), (-1.0, 1.0, 1.0, -1.0));
        assert_eq!(f.pixels, [1, 2, 3, 255]);
        assert!(s.frame(7.0).is_some(), "out-of-range frames fall back to 0");
    }

    #[test]
    fn rejects_bad_versions() {
        let mut b = sprite_bytes(2, 0);
        b[4] = 1;
        assert!(Sprite::parse(&b).is_err());
    }
}
