//! Headless screenshots: load a map, render views offscreen, write a PNG (tiled when several).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use glam::Vec3;
use gs_assets::RgbaImage;
use gs_fs::FileSystem;
use gs_render::{Gpu, Renderer, View, capture};

use crate::lightstyles::LightStyles;
use crate::scene::LocalMap;

/// A camera; missing parts default to the map's first spawn point.
#[derive(Debug, Clone, Copy)]
pub struct ViewSpec {
    pub pos: Option<Vec3>,
    pub angles: Option<Vec3>,
}

impl ViewSpec {
    /// `"x y z pitch yaw roll"`.
    pub fn parse(text: &str) -> Result<Self> {
        let v: Vec<f32> = text
            .split_ascii_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .with_context(|| format!("bad view {text:?}"))?;
        if v.len() != 6 {
            bail!("view {text:?}: expected 6 numbers");
        }
        Ok(Self { pos: Some(Vec3::new(v[0], v[1], v[2])), angles: Some(Vec3::new(v[3], v[4], v[5])) })
    }
}

pub struct ShotRequest {
    pub map: String,
    pub views: Vec<ViewSpec>,
    /// Each view is repeated with yaw rotated in `360 / yaws` steps.
    pub yaws: u32,
    pub fov: f32,
    /// Size of one view.
    pub size: (u32, u32),
    pub time: f64,
    pub out: PathBuf,
    pub fog: gs_render::Fog,
}

pub fn run(fs: &FileSystem, req: &ShotRequest) -> Result<()> {
    let scene = LocalMap::load(fs, &req.map)?;
    let gpu = pollster::block_on(Gpu::new(Gpu::new_instance(), None, None))?;
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, wgpu::TextureFormat::Rgba8Unorm, 4);
    renderer.load_map(&scene.map)?;

    let steps = req.yaws.max(1);
    let mut tiles = Vec::new();
    for spec in &req.views {
        for step in 0..steps {
            let mut view = View::new(spec.pos.unwrap_or(scene.spawn_origin), spec.angles.unwrap_or(scene.spawn_angles));
            view.angles.y += 360.0 * step as f32 / steps as f32;
            view.fov = req.fov;
            view.time = req.time;
            view.fog = req.fog;
            view.light_styles = LightStyles::default().values(req.time);
            let (o, a) = (view.origin, view.angles);
            println!("view {}: {} {} {} {} {} {}", tiles.len(), o.x, o.y, o.z, a.x, a.y, a.z);
            tiles.push(capture::capture(&mut renderer, req.size, &view, &scene.draw, None)?);
        }
    }
    let image = if tiles.len() == 1 { tiles.remove(0) } else { tile(&tiles) };
    save_png(&image, &req.out)?;
    println!("{}", req.out.display());
    Ok(())
}

/// Lays equally sized images out in a near-square grid, row-major.
fn tile(images: &[RgbaImage]) -> RgbaImage {
    let (w, h) = (images[0].width, images[0].height);
    let cols = (images.len() as f32).sqrt().ceil() as u32;
    let rows = (images.len() as u32).div_ceil(cols);
    let mut out = RgbaImage { width: w * cols, height: h * rows, pixels: vec![0; (w * cols * h * rows * 4) as usize] };
    for (i, img) in images.iter().enumerate() {
        let (cx, cy) = (i as u32 % cols, i as u32 / cols);
        for y in 0..h {
            let src = (y * w * 4) as usize;
            let dst = (((cy * h + y) * out.width + cx * w) * 4) as usize;
            out.pixels[dst..dst + (w * 4) as usize].copy_from_slice(&img.pixels[src..src + (w * 4) as usize]);
        }
    }
    out
}

pub fn save_png(image: &RgbaImage, path: &Path) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let buf = image::RgbaImage::from_raw(image.width, image.height, image.pixels.clone())
        .context("capture buffer size mismatch")?;
    buf.save(path).with_context(|| format!("writing {}", path.display()))
}

/// Rows bottom-up as tightly packed B, G, R (`glReadPixels(GL_RGB)` + the byte swap).
fn bottom_up_bgr(image: &RgbaImage) -> Vec<u8> {
    let w = image.width as usize;
    let mut body = Vec::with_capacity(w * image.height as usize * 3);
    for row in image.pixels.chunks_exact(w * 4).rev() {
        for px in row.chunks_exact(4) {
            body.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
    body
}

/// `VID_TakeSnapshot` / `WriteBMP24`: a 24-bit bottom-up BMP with `bfSize = 3wh + 54` (padding
/// not counted) and `biSizeImage` 0; each row is padded to 4 bytes with the first bytes of the
/// next row (zeros after the last, where the original reads past its buffer).
pub fn save_bmp(image: &RgbaImage, path: &Path) -> Result<()> {
    let (w, h) = (image.width as usize, image.height as usize);
    let body = bottom_up_bgr(image);
    let stride = 3 * w + (4 - 3 * w % 4) % 4;
    let mut b = Vec::with_capacity(54 + stride * h);
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&(3u32.wrapping_mul((w * h) as u32 + 18)).to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&(w as i32).to_le_bytes());
    b.extend_from_slice(&(h as i32).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(&[0; 24]);
    for row in 0..h {
        let start = row * 3 * w;
        b.extend((start..start + stride).map(|i| body.get(i).copied().unwrap_or(0)));
    }
    std::fs::write(path, b).with_context(|| format!("writing {}", path.display()))
}

/// `SCR_ScreenShot_f`'s TGA: an 18-byte header (type 2, 24 bpp, bottom-left origin) and the
/// bottom-up BGR pixels without padding or footer.
pub fn save_tga(image: &RgbaImage, path: &Path) -> Result<()> {
    let mut b = vec![0u8; 18];
    b[2] = 2;
    b[12..14].copy_from_slice(&(image.width as u16).to_le_bytes());
    b[14..16].copy_from_slice(&(image.height as u16).to_le_bytes());
    b[16] = 24;
    b.extend_from_slice(&bottom_up_bgr(image));
    std::fs::write(path, b).with_context(|| format!("writing {}", path.display()))
}
