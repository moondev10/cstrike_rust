//! Texture conversion and upload mirroring `GL_Upload16`/`GL_Upload32` (docs/re/render.md §A.2.6).

use gs_formats::miptex::MipTex;
use wgpu::util::DeviceExt;

use crate::gamma::GammaTables;

/// `GL_Upload16` texture types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadType {
    /// Opaque: gamma + dither, alpha 255.
    Opaque,
    /// `{` textures: palette index 255 is transparent.
    Alpha,
}

/// Paletted miptex level 0 → RGBA8 exactly as the GL renderer uploads it.
pub fn miptex_rgba(tex: &MipTex, gamma: &GammaTables) -> Option<Vec<u8>> {
    let data = tex.data.as_ref()?;
    Some(indexed_rgba(&data.mips[0], &data.palette, tex.width, tex.height, tex.is_masked(), gamma))
}

/// `GL_Upload16` for 8-bit images: type 1 (`masked`, index 255 transparent) or type 0.
pub fn indexed_rgba(pixels: &[u8], palette: &[[u8; 3]], width: u32, height: u32, masked: bool, gamma: &GammaTables) -> Vec<u8> {
    let mut kind = if masked { UploadType::Alpha } else { UploadType::Opaque };
    let mut dither = true;
    if kind == UploadType::Alpha && !pixels.contains(&255) {
        kind = UploadType::Opaque;
        dither = false;
    }
    let palette: Vec<[u8; 3]> = (0..256)
        .map(|i| {
            let c = palette.get(i).copied().unwrap_or([0, 0, 0]);
            gamma.palette_color(c, kind == UploadType::Opaque && dither)
        })
        .collect();
    let mut rgba = Vec::with_capacity(pixels.len() * 4);
    for &index in pixels {
        if kind == UploadType::Alpha && index == 255 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let [r, g, b] = palette[usize::from(index)];
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    if kind == UploadType::Alpha {
        sprite_blend(&mut rgba, width as usize, height as usize);
    }
    rgba
}

/// `gl_spriteblend`: fully transparent texels take the average colour of the opaque texels in
/// their 3x3 neighbourhood (alpha stays 0), so filtering does not pull in black fringes.
pub fn sprite_blend(rgba: &mut [u8], w: usize, h: usize) {
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            if rgba[i..i + 4] != [0, 0, 0, 0] {
                continue;
            }
            let mut sum = [0u32; 3];
            let mut count = 0u32;
            for ny in y.saturating_sub(1)..(y + 2).min(h) {
                for nx in x.saturating_sub(1)..(x + 2).min(w) {
                    let j = (ny * w + nx) * 4;
                    if rgba[j + 3] != 0 {
                        for c in 0..3 {
                            sum[c] += u32::from(rgba[j + c]);
                        }
                        count += 1;
                    }
                }
            }
            let count = count.max(1);
            for c in 0..3 {
                rgba[i + c] = (sum[c] / count) as u8;
            }
        }
    }
}

/// `GL_Upload32` mip chain: truncating 2x2 average per channel until 1x1.
pub fn mip_chain(width: u32, height: u32, rgba: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let mut levels = vec![(width, height, rgba.to_vec())];
    while let Some(&(w, h, ref prev)) = levels.last() {
        if w == 1 && h == 1 {
            break;
        }
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                let o = ((y * nw + x) * 4) as usize;
                for c in 0..4 {
                    let at = |sx: u32, sy: u32| u32::from(prev[((sy.min(h - 1) * w + sx.min(w - 1)) * 4) as usize + c]);
                    let sum = at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1);
                    next[o + c] = (sum >> 2) as u8;
                }
            }
        }
        levels.push((nw, nh, next));
    }
    levels
}

/// `texels` (`gl_texels`): `w*h` of every uploaded mip level since start.
static TEXELS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The texels uploaded so far (`GL_Upload32`'s running count).
pub fn uploaded_texels() -> u64 {
    TEXELS.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn count_texels(n: u64) {
    TEXELS.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
}

/// An RGBA8 texture with optional mips. A size the device cannot hold (zero, above its 2D limit)
/// or pixels short of it come from a broken or hostile file: wgpu would abort on the validation
/// error, so a 1x1 magenta texture stands in.
pub fn upload_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
    mipmaps: bool,
) -> wgpu::Texture {
    let max = device.limits().max_texture_dimension_2d;
    if width == 0 || height == 0 || width > max || height > max || (rgba.len() as u64) < u64::from(width) * u64::from(height) * 4 {
        log::warn!("{label}: unusable {width}x{height} texture ({} bytes), replaced", rgba.len());
        return upload_rgba(device, queue, label, 1, 1, &[255, 0, 255, 255], false);
    }
    let levels = if mipmaps { mip_chain(width, height, rgba) } else { vec![(width, height, rgba.to_vec())] };
    count_texels(levels.iter().map(|(w, h, _)| u64::from(*w) * u64::from(*h)).sum());
    let data: Vec<u8> = levels.iter().flat_map(|(_, _, px)| px.iter().copied()).collect();
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::MipMajor,
        &data,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_ends_at_1x1_for_non_square_sizes() {
        let levels = mip_chain(8, 2, &[255; 8 * 2 * 4]);
        let sizes: Vec<_> = levels.iter().map(|(w, h, _)| (*w, *h)).collect();
        assert_eq!(sizes, vec![(8, 2), (4, 1), (2, 1), (1, 1)]);
    }

    #[test]
    fn mip_average_truncates() {
        let levels = mip_chain(2, 2, &[1, 0, 0, 255, 2, 0, 0, 255, 2, 0, 0, 255, 2, 0, 0, 254]);
        assert_eq!(levels[1].2, vec![1, 0, 0, 254]);
    }

    #[test]
    fn transparent_texels_take_neighbour_colour_keep_zero_alpha() {
        // 3x1: red | transparent | blue
        let mut px = vec![200, 0, 0, 255, 0, 0, 0, 0, 0, 0, 100, 255];
        sprite_blend(&mut px, 3, 1);
        assert_eq!(&px[4..8], &[100, 0, 50, 0]);
    }
}
