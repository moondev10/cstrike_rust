//! World sprites (docs/re/render.md §C.1–C.2): upload rules per `texFormat`, orientation axes,
//! quad construction and `R_SpriteColor`.

use std::sync::Arc;

use gs_formats::sprite::{Sprite, SpriteFormat, SpriteFrame, SpriteType};

use crate::gamma::GammaTables;
use crate::renderer::RenderMode;
use crate::studio::{AlphaTest, Blend, Geometry, MeshDraw, PipelineKey, StudioVertex, TexEnv};
use crate::texture;

/// Handle of an uploaded sprite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpriteHandle(pub u32);

/// One sprite to draw this frame.
#[derive(Debug, Clone, Copy)]
pub struct SpriteDraw {
    pub sprite: SpriteHandle,
    /// `curstate.frame` (truncated).
    pub frame: f32,
    /// `r_entorigin`.
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    /// `curstate.scale` (≤ 0 means 1).
    pub scale: f32,
    pub rendermode: RenderMode,
    /// `r_blend` (glow attenuation included).
    pub blend: f32,
    pub color: [u8; 3],
}

/// RGBA of one frame as `GL_Upload16` produces it for the sprite's `texFormat`.
pub fn frame_rgba(sprite: &Sprite, frame: &SpriteFrame, gamma: &GammaTables) -> Vec<u8> {
    match sprite.format {
        SpriteFormat::Normal | SpriteFormat::Additive => {
            texture::indexed_rgba(&frame.pixels, &sprite.palette, frame.width, frame.height, false, gamma)
        }
        SpriteFormat::AlphaTest => texture::indexed_rgba(&frame.pixels, &sprite.palette, frame.width, frame.height, true, gamma),
        SpriteFormat::IndexAlpha => {
            // Type 3 (ALPHA_GRADIENT): every texel is palette[255] with the index as alpha.
            let [r, g, b] = gamma.palette_color(sprite.palette.get(255).copied().unwrap_or([0; 3]), false);
            let mut rgba: Vec<u8> = frame.pixels.iter().flat_map(|&i| [r, g, b, i]).collect();
            texture::sprite_blend(&mut rgba, frame.width as usize, frame.height as usize);
            rgba
        }
    }
}

pub(crate) struct SpriteSource {
    pub sprite: Arc<Sprite>,
}

/// View axes for orientation.
pub(crate) struct SpriteView {
    pub origin: [f32; 3],
    pub forward: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l == 0.0 { v } else { v.map(|c| c / l) }
}

/// `R_GetSpriteAxes`: `(right, up)`, or `None` for the degenerate upright cases.
fn axes(kind: SpriteType, angles: [f32; 3], view: &SpriteView) -> Option<([f32; 3], [f32; 3])> {
    let kind = if angles[2] != 0.0 && kind == SpriteType::Parallel { SpriteType::ParallelOriented } else { kind };
    let upright = |t: [f32; 3]| {
        if t[2].abs() > 0.999_848 {
            return None;
        }
        Some((normalize([t[1], -t[0], 0.0]), [0.0, 0.0, 1.0]))
    };
    match kind {
        SpriteType::ParallelUpright => upright(view.forward),
        // `modelorg` is the view origin after the world pass.
        SpriteType::FacingUpright => upright(normalize(view.origin.map(|c| -c))),
        SpriteType::Parallel => Some((view.right, view.up)),
        SpriteType::Oriented => {
            let (_, r, u) = angle_vectors(angles);
            Some((r, u))
        }
        SpriteType::ParallelOriented => {
            let a = angles[2].to_radians();
            let (s, c) = a.sin_cos();
            let right = [0, 1, 2].map(|i| c * view.right[i] + s * view.up[i]);
            let up = [0, 1, 2].map(|i| c * view.up[i] - s * view.right[i]);
            Some((right, up))
        }
    }
}

fn angle_vectors(a: [f32; 3]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let (sy, cy) = a[1].to_radians().sin_cos();
    let (sp, cp) = a[0].to_radians().sin_cos();
    let (sr, cr) = a[2].to_radians().sin_cos();
    (
        [cp * cy, cp * sy, -sp],
        [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp],
        [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp],
    )
}

/// GL state of `R_DrawSpriteModel` per rendermode (gl_spriteblend 1).
pub(crate) fn key(mode: RenderMode) -> PipelineKey {
    let (blend, depth_write, depth_test, texenv) = match mode {
        RenderMode::Normal => (Blend::Alpha, true, true, TexEnv::Modulate),
        // glTexEnvi(GL_ALPHA) is invalid: the previous REPLACE environment stays.
        RenderMode::TransColor => (Blend::Alpha, true, true, TexEnv::Replace),
        RenderMode::TransTexture => (Blend::Alpha, true, true, TexEnv::Modulate),
        RenderMode::Glow => (Blend::Additive, false, false, TexEnv::Modulate),
        RenderMode::TransAlpha => (Blend::Alpha, false, true, TexEnv::Modulate),
        RenderMode::TransAdd => (Blend::Additive, false, true, TexEnv::Modulate),
    };
    PipelineKey { blend, depth_write, depth_test, alpha_test: AlphaTest::NotZero, texenv, cull: true }
}

/// `R_DrawSpriteModel`: one quad (nothing for group frames or degenerate upright axes).
pub(crate) fn build_sprite(src: &SpriteSource, draw: &SpriteDraw, view: &SpriteView, geo: &mut Geometry, meshes: &mut Vec<MeshDraw>) {
    let sprite = &src.sprite;
    let mut index = draw.frame as i32;
    if index < 0 || index as usize >= sprite.frames.len() {
        index = 0;
    }
    let Some(frame) = sprite.frame(index as f32) else { return };
    let Some((right, up)) = axes(sprite.kind, draw.angles, view) else { return };
    let scale = if draw.scale <= 0.0 { 1.0 } else { draw.scale };
    let o = draw.origin;
    let point = |v: f32, h: f32| [0, 1, 2].map(|i| o[i] + up[i] * (scale * v) + right[i] * (scale * h));
    // R_SpriteColor
    let a = (draw.blend * 255.0) as i32;
    let k = if matches!(draw.rendermode, RenderMode::Glow | RenderMode::TransAdd) { a } else { 256 };
    let c = if draw.color != [0, 0, 0] { draw.color.map(|v| (k * i32::from(v)) >> 8) } else { [(255 * k) >> 8; 3] };
    let alpha = match draw.rendermode {
        RenderMode::Normal | RenderMode::Glow | RenderMode::TransAdd => 255,
        _ => i32::from((draw.blend * 255.0) as i32 as u8),
    };
    let color = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, alpha as f32 / 255.0];
    let v = |pos: [f32; 3], uv: [f32; 2]| StudioVertex { pos, uv, color };
    let quad = [
        v(point(frame.down, frame.left), [0.0, 1.0]),
        v(point(frame.up, frame.left), [0.0, 0.0]),
        v(point(frame.up, frame.right), [1.0, 0.0]),
        v(point(frame.down, frame.right), [1.0, 1.0]),
    ];
    let base = geo.vertices.len() as u32;
    let first = geo.index_count();
    geo.vertices.extend(quad);
    geo.indices.extend([0, 1, 2, 0, 2, 3].map(|k| base + k));
    meshes.push(MeshDraw { indices: first..geo.index_count(), texture: index as u32, key: key(draw.rendermode), sprite: None });
}
