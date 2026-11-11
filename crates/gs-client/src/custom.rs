//! Player customizations (spray logos; docs/re/engine-core.md §5.8, engine-misc.md §5): the
//! local `tempdecal.wad` registered as a custom resource, and the validation that turns a
//! logo WAD into a decal texture (`COM_CreateCustomization` → `CustomDecal_Validate` /
//! `Draw_CustomCacheWadInit` / `Draw_ValidateCustomLogo`).

use gs_formats::miptex::MipTex;
use gs_fs::FileSystem;
use gs_protocol::messages::{Resource, resource_flags};
use md5::{Digest, Md5};

/// `t_decal`.
pub const T_DECAL: u8 = 3;
/// The local logo's file (GAMECONFIG).
pub const TEMPDECAL: &str = "tempdecal.wad";

/// A `customization_t`: the resource, its data and, for a valid logo, the decal texture
/// (`bTranslated`).
#[derive(Debug, Clone)]
pub struct Customization {
    pub resource: Resource,
    pub data: Vec<u8>,
    pub texture: Option<MipTex>,
}

impl Customization {
    /// `COM_CreateCustomization`: a decal with `RES_CUSTOM` is validated into its texture.
    pub fn new(resource: Resource, data: Vec<u8>) -> Self {
        let texture = (resource.kind == T_DECAL && resource.flags & resource_flags::CUSTOM != 0).then(|| decal_texture(&data)).flatten();
        if texture.is_none() && resource.kind == T_DECAL {
            log::debug!("custom decal {} failed validation", resource.name);
        }
        Self { resource, data, texture }
    }
}

/// `CustomDecal_Validate` + `Draw_CustomCacheWadInit` + `Draw_ValidateCustomLogo`: a 1024..20480
/// byte WAD3 with exactly one lump ending at the directory, holding a miptex of 1..256 per side
/// with consistent mip offsets and at most 256 palette entries.
pub fn decal_texture(data: &[u8]) -> Option<MipTex> {
    if !(1024..=20480).contains(&data.len()) || data.get(..4)? != b"WAD3" {
        return None;
    }
    let i32_at = |o: usize| data.get(o..o + 4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if i32_at(4)? != 1 {
        return None;
    }
    let info = usize::try_from(i32_at(8)?).ok()?;
    if info + 32 != data.len() {
        return None;
    }
    let (filepos, disksize, size) = (i32_at(info)?, i32_at(info + 4)?, i32_at(info + 8)?);
    if disksize != size || size <= 0 || filepos < 12 || filepos.checked_add(size)? as usize > info {
        return None;
    }
    let lump = data.get(filepos as usize..(filepos + size) as usize)?;
    let u32_at = |o: usize| lump.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (w, h) = (u32_at(16)?, u32_at(20)?);
    if !(1..=256).contains(&w) || !(1..=256).contains(&h) {
        return None;
    }
    // The mip offsets follow each other exactly.
    let mut expected = 40u32;
    for level in 0..4 {
        if u32_at(24 + level * 4)? != expected {
            return None;
        }
        expected += (w >> level).max(1) * (h >> level).max(1);
    }
    let palette = u16::from_le_bytes(lump.get(expected as usize..expected as usize + 2)?.try_into().ok()?);
    if palette > 256 {
        return None;
    }
    let mut tex = MipTex::parse(lump).ok()?;
    // Draw_CustomCacheWadInit names the cached texture "%03i%02i" (-2, n).
    tex.name = "-0200".to_owned();
    tex.data.is_some().then_some(tex)
}

/// `CL_ClearState`'s logo registration: `tempdecal.wad` (GAMECONFIG) as a `t_decal` custom
/// resource with its size and MD5, when the file has data.
pub fn local_logo(fs: &FileSystem) -> Option<(Resource, Vec<u8>)> {
    let path = fs.resolve_in(TEMPDECAL, Some(gs_fs::PathId::GameConfig)).ok()?;
    let data = std::fs::read(path).ok().filter(|d| !d.is_empty())?;
    let md5: [u8; 16] = Md5::digest(&data).into();
    let resource = Resource {
        kind: T_DECAL,
        name: TEMPDECAL.to_owned(),
        index: 0,
        download_size: data.len() as u32,
        flags: resource_flags::CUSTOM,
        md5: Some(md5),
        reserved: None,
    };
    Some((resource, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GameUI's BMP → WAD output for a `side`² logo of palette colour 255 = (r, g, b).
    pub(crate) fn logo_wad(side: u32) -> Vec<u8> {
        let n = side * side;
        let mut lump = Vec::new();
        let mut name = [0u8; 16];
        name[..4].copy_from_slice(b"LOGO");
        lump.extend_from_slice(&name);
        lump.extend_from_slice(&side.to_le_bytes());
        lump.extend_from_slice(&side.to_le_bytes());
        for off in [40, 40 + n, 40 + n + n / 4, 40 + n + n / 4 + n / 16] {
            lump.extend_from_slice(&off.to_le_bytes());
        }
        for level in 0..4 {
            lump.extend((0..(n >> (2 * level))).map(|i| (i % 256) as u8));
        }
        lump.extend_from_slice(&256u16.to_le_bytes());
        lump.extend(std::iter::repeat_n(0u8, 765));
        lump.extend_from_slice(&[255, 0, 0]);
        while lump.len() % 4 != 0 {
            lump.push(0);
        }
        let mut wad = b"WAD3".to_vec();
        wad.extend_from_slice(&1u32.to_le_bytes());
        wad.extend_from_slice(&(12 + lump.len() as u32).to_le_bytes());
        wad.extend_from_slice(&lump);
        wad.extend_from_slice(&12i32.to_le_bytes());
        wad.extend_from_slice(&(lump.len() as i32).to_le_bytes());
        wad.extend_from_slice(&(lump.len() as i32).to_le_bytes());
        wad.extend_from_slice(&[0x40, 0, 0, 0]);
        wad.extend_from_slice(&name);
        wad
    }

    #[test]
    fn gameui_logos_validate_and_broken_ones_do_not() {
        for (side, size) in [(16, 1196), (32, 2216), (64, 6296)] {
            let wad = logo_wad(side);
            assert_eq!(wad.len(), size, "{side}² logo size");
            let tex = decal_texture(&wad);
            assert_eq!(tex.as_ref().map(|t| (t.width, t.height)), Some((side, side)));
        }
        let mut wad = logo_wad(32);
        wad[4] = 2;
        assert!(decal_texture(&wad).is_none(), "more than one lump");
        let wad = logo_wad(32);
        assert!(decal_texture(&wad[..1000]).is_none(), "under 1024 bytes");
    }
}
