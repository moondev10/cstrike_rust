//! Studio model drawing as the GL engine does it (docs/re/render.md §B.10–B.12): CPU vertex
//! transform and per-normal lighting with the gamma tables, then textured triangles modulated by
//! the vertex colour.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use gs_studio::StudioModel;
use gs_studio::anim::{Mat3x4, irotate};
use gs_studio::model::texture_flags;
use gs_world::World;

use crate::camera::NUM_LIGHTSTYLES;
use crate::gamma::GammaTables;
use crate::renderer::RenderMode;
use crate::sprite::SpriteHandle;

/// `MAXSTUDIOVERTS`: lighting and strips stop at this normal index.
const MAX_STUDIO_VERTS: usize = 2048;
/// `v_lambert1`.
const LAMBERT: f32 = 1.495_324_1;
/// `direct` cvar default.
const DIRECT: f32 = 0.9;

pub const EF_INVLIGHT: i32 = 0x10;
pub const MODEL_NOSHADELIGHT: i32 = 0x100;
pub const MODEL_FORCESKYLIGHT: i32 = 0x400;
pub const RENDERFX_LIGHT_MULTIPLIER: i32 = 21;

/// Result of `StudioDynamicLight`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudioLight {
    pub ambient: i32,
    pub shade: i32,
    /// Max component 1.
    pub color: [f32; 3],
    /// Unit direction the light travels.
    pub dir: [f32; 3],
}

impl StudioLight {
    /// `r_fullbright 1`.
    pub const FULLBRIGHT: Self = Self { ambient: 192, shade: 0, color: [1.0; 3], dir: [0.0, 0.0, -1.0] };
}

/// A `dlight_t`/elight: world position, radius and colour.
#[derive(Debug, Clone, Copy)]
pub struct PointLight {
    pub origin: [f32; 3],
    pub radius: f32,
    pub color: [u8; 3],
}

/// World inputs for model lighting.
pub struct LightEnv<'a> {
    pub world: &'a World,
    pub styles: &'a [i32; NUM_LIGHTSTYLES],
    /// `movevars.skycolor` (sv_skycolor_*).
    pub sky_color: [f32; 3],
    /// `movevars.skyvec`.
    pub sky_vec: [f32; 3],
    /// Active dynamic lights.
    pub dlights: &'a [PointLight],
}

/// Per-entity inputs of `StudioDynamicLight`.
#[derive(Debug, Clone, Copy)]
pub struct LightTarget {
    pub origin: [f32; 3],
    pub effects: i32,
    pub model_flags: i32,
    pub renderfx: i32,
    pub iuser4: i32,
}

fn length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = length(v);
    if l == 0.0 { v } else { v.map(|c| c / l) }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// `StudioDynamicLight` (§B.10.1).
pub fn dynamic_light(env: &LightEnv, t: &LightTarget) -> StudioLight {
    let up = if t.effects & EF_INVLIGHT != 0 { 1.0 } else { -1.0 };
    let mut lightvec = [0.0, 0.0, up];
    let start = [t.origin[0], t.origin[1], t.origin[2] - 8.0 * up];
    let mut color = [0.0f32; 3];
    let sky = env.sky_color;
    if sky[0] + sky[1] + sky[2] != 0.0 {
        let end = [0, 1, 2].map(|i| start[i] - env.sky_vec[i] * 8192.0);
        let sky_hit = env.world.surface_at(0, start, end).is_some_and(|f| env.world.face_is_sky(f));
        if t.model_flags & MODEL_FORCESKYLIGHT != 0 || sky_hit {
            color = sky.map(|c| (c as i32) as f32);
            lightvec = env.sky_vec;
        }
    }
    if color[0] + color[1] + color[2] == 0.0 {
        let end = [0, 1, 2].map(|i| start[i] + lightvec[i] * 2048.0);
        let c = env.world.light_vec(env.styles, start, end);
        color = c.map(|v| v as f32);
        let mut samples = [0.0f32; 4];
        for (k, (dx, dy)) in [(-16.0, -16.0), (16.0, -16.0), (16.0, 16.0), (-16.0, 16.0)].into_iter().enumerate() {
            let s = [start[0] + dx, start[1] + dy, start[2]];
            let e = [end[0] + dx, end[1] + dy, end[2]];
            let c = env.world.light_vec(env.styles, s, e);
            samples[k] = (c[0] + c[1] + c[2]) as f32 / 768.0;
        }
        let [a, b, c, d] = samples;
        lightvec[0] = a - b - c + d;
        lightvec[1] = a + b - c - d;
        lightvec = normalize(lightvec);
    }
    if t.renderfx == RENDERFX_LIGHT_MULTIPLIER && t.iuser4 != 10 {
        color = color.map(|c| ((c * t.iuser4 as f32 / 10.0) as i32) as f32);
    }
    let mut total = color[0].max(color[1]).max(color[2]);
    if total == 0.0 {
        total = 1.0;
    }
    lightvec = lightvec.map(|v| v * total);
    for dl in env.dlights {
        let dir = [0, 1, 2].map(|i| t.origin[i] - dl.origin[i]);
        let d = length(dir);
        let add = dl.radius - d;
        if add > 0.0 {
            total += add;
            let k = if d > 1.0 { add / d } else { add };
            for i in 0..3 {
                lightvec[i] += dir[i] * k;
                color[i] += f32::from(dl.color[i]) * add / 256.0;
            }
        }
    }
    let direct = if t.model_flags & MODEL_NOSHADELIGHT != 0 { 0.6 } else { DIRECT.clamp(0.75, 1.0) };
    lightvec = lightvec.map(|v| v * direct);
    let mut shade = length(lightvec) as i32;
    let mut ambient = (total - shade as f32) as i32;
    let max = color[0].max(color[1]).max(color[2]);
    color = if max == 0.0 { [1.0; 3] } else { color.map(|c| c * (1.0 / max)) };
    ambient = ambient.min(128);
    if shade + ambient > 255 {
        shade = 255 - ambient;
    }
    StudioLight { ambient, shade, color, dir: normalize(lightvec) }
}

/// A selected local light (`StudioEntityLight`, at most 3).
#[derive(Debug, Clone, Copy)]
pub struct LocalLight {
    pub origin: [f32; 3],
    pub radius2: f32,
    /// `lineargammatable[c·4]` per channel.
    pub linear: [i32; 3],
}

/// `StudioEntityLight`: pick up to 3 elights by strength at `origin`. `minstrength` is
/// initialised once for the whole loop, as in the original, so a replaced slot lowers the bar
/// for every later light.
pub fn select_local_lights(elights: &[PointLight], origin: [f32; 3], gamma: &GammaTables) -> Vec<LocalLight> {
    let mut out: Vec<(LocalLight, f32)> = Vec::with_capacity(3);
    let mut minstrength = 1.0e6f32;
    for el in elights.iter().filter(|e| e.radius > 0.0) {
        let r2 = el.radius * el.radius;
        let d = [0, 1, 2].map(|i| origin[i] - el.origin[i]);
        let d2 = dot(d, d);
        let s = if d2 <= r2 { 1.0 } else { r2 / d2 };
        if s <= 0.004 {
            continue;
        }
        let slot = if out.len() < 3 {
            Some(out.len())
        } else {
            let mut best: Option<usize> = None;
            for (i, (_, strength)) in out.iter().enumerate() {
                if minstrength > *strength && s > *strength {
                    minstrength = *strength;
                    best = Some(i);
                }
            }
            best
        };
        if let Some(slot) = slot {
            let linear = el.color.map(|c| i32::from(gamma.linear[usize::from(c) * 4]));
            let light = (LocalLight { origin: el.origin, radius2: r2, linear }, s);
            if slot == out.len() {
                out.push(light);
            } else {
                out[slot] = light;
            }
        }
    }
    out.into_iter().map(|(l, _)| l).collect()
}

/// One studio model to draw this frame.
#[derive(Debug, Clone)]
pub struct StudioDraw {
    pub model: StudioHandle,
    /// Bone matrices from `gs_studio::pose`.
    pub bones: Vec<Mat3x4>,
    pub light: StudioLight,
    pub local_lights: Vec<LocalLight>,
    /// Entity origin, for translucent sorting.
    pub origin: [f32; 3],
    pub body: i32,
    pub skin: i32,
    pub rendermode: RenderMode,
    /// `r_blend` (0..1).
    pub blend: f32,
    /// Drawn in the view-model depth range.
    pub view_model: bool,
    /// Right-handed view model: no face culling and mirrored normals for lighting.
    pub mirrored: bool,
    /// `kRenderFxGlowShell`: a second, additive chrome pass.
    pub shell: Option<GlowShell>,
}

/// The glow shell pass (docs/re/render.md §B.11.4).
#[derive(Debug, Clone, Copy)]
pub struct GlowShell {
    /// The engine chrome sprite (`sprites/shellchrome.spr`), frame 0.
    pub sprite: SpriteHandle,
    /// `rendercolor`.
    pub color: [u8; 3],
    /// `renderamt`: vertices move out along their normal by `renderamt · 0.05`.
    pub renderamt: i32,
    /// `cl.time`.
    pub time: f64,
    /// `r_glowshellfreq`.
    pub freq: f64,
}

/// Handle of an uploaded studio model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StudioHandle(pub u32);

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct StudioVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Blend {
    Opaque,
    Alpha,
    Additive,
}

/// `glAlphaFunc` state of a draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlphaTest {
    None,
    /// `GL_GREATER 0.5` (studio MASKED textures).
    Greater05,
    /// `GL_NOTEQUAL 0` (the global default; sprites).
    NotZero,
}

/// `GL_TEXTURE_ENV_MODE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TexEnv {
    Modulate,
    /// Texture colour only (vertex colour ignored).
    Replace,
}

/// GL state of one mesh draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    pub blend: Blend,
    pub depth_write: bool,
    pub depth_test: bool,
    pub alpha_test: AlphaTest,
    pub texenv: TexEnv,
    pub cull: bool,
}

/// `R_StudioDrawPoints`' GL state for a mesh with texture `flags`.
pub(crate) fn mesh_key(rendermode: RenderMode, flags: i32, mirrored: bool) -> PipelineKey {
    let masked = flags & texture_flags::MASKED != 0;
    let additive = flags & texture_flags::ADDITIVE != 0 && rendermode == RenderMode::Normal;
    PipelineKey {
        blend: match rendermode {
            RenderMode::Normal if additive => Blend::Additive,
            RenderMode::Normal => Blend::Opaque,
            RenderMode::TransAdd => Blend::Additive,
            _ => Blend::Alpha,
        },
        // MASKED forces DepthMask(1), then ADDITIVE (mode 0) turns it off again.
        depth_write: !additive && (masked || rendermode != RenderMode::TransAdd),
        depth_test: true,
        alpha_test: if masked { AlphaTest::Greater05 } else { AlphaTest::None },
        texenv: TexEnv::Modulate,
        cull: !mirrored,
    }
}

/// The GL state of a chrome mesh's additive pass (`flags` without CHROME added).
pub(crate) fn chrome_key(flags: i32, mirrored: bool) -> PipelineKey {
    let masked = flags & texture_flags::MASKED != 0;
    PipelineKey {
        blend: Blend::Additive,
        depth_write: masked,
        depth_test: true,
        alpha_test: if masked { AlphaTest::Greater05 } else { AlphaTest::None },
        texenv: TexEnv::Modulate,
        cull: !mirrored,
    }
}

/// A contiguous run of triangles with one texture and GL state.
#[derive(Debug, Clone)]
pub struct MeshDraw {
    pub vertices: std::ops::Range<u32>,
    /// Studio texture index, or the frame of `sprite`.
    pub texture: u32,
    pub key: PipelineKey,
    /// A sprite texture bound instead of the model's own (glow shell chrome).
    pub sprite: Option<SpriteHandle>,
}

/// View inputs for chrome.
pub struct ChromeView {
    pub origin: [f32; 3],
    pub right: [f32; 3],
}

/// CPU geometry for one model.
pub(crate) struct StudioSource {
    pub model: Arc<StudioModel>,
    /// `(width, height, flags)` per texture.
    pub textures: Vec<(u32, u32, i32)>,
}

/// `R_StudioDrawPoints` for every bodypart of `draw`, appending triangles to `out`.
pub(crate) fn build_draw(
    src: &StudioSource,
    draw: &StudioDraw,
    gamma: &GammaTables,
    chrome_view: &ChromeView,
    out: &mut Vec<StudioVertex>,
    meshes: &mut Vec<MeshDraw>,
) {
    let model = &src.model;
    let bones = &draw.bones;
    if bones.len() < model.bones.len() {
        return;
    }
    let skins = &model.skins;
    let family = usize::try_from(draw.skin).ok().filter(|&s| s != 0 && s < skins.len()).unwrap_or(0);
    let Some(skinref) = skins.get(family) else { return };

    let blightvec: Vec<[f32; 3]> = bones.iter().map(|m| irotate(m, draw.light.dir)).collect();
    let (chrome_up, chrome_right) = chrome_vectors(bones, chrome_view.origin, chrome_view.right);
    // Per-bone local light positions in bone space.
    let light_bone_pos: Vec<Vec<[f32; 3]>> = bones
        .iter()
        .map(|m| {
            draw.local_lights
                .iter()
                .map(|l| irotate(m, [l.origin[0] - m[0][3], l.origin[1] - m[1][3], l.origin[2] - m[2][3]]))
                .collect()
        })
        .collect();

    let trans_add = draw.rendermode == RenderMode::TransAdd;
    let mut flat = false;
    for part in 0..model.bodyparts.len() {
        let Some(sub) = model.submodel(part, draw.body) else { continue };
        let xform: Vec<[f32; 3]> = sub
            .verts
            .iter()
            .zip(&sub.vert_bones)
            .map(|(v, &b)| gs_studio::anim::transform(&bones[usize::from(b).min(bones.len() - 1)], *v))
            .collect();

        // Lighting per normal, mesh by mesh (normals are consumed sequentially).
        let mut light_values = vec![[0.0f32; 3]; sub.norms.len().min(MAX_STUDIO_VERTS)];
        let mut chrome = vec![[0i32; 2]; light_values.len()];
        let mut n = 0usize;
        for mesh in &sub.meshes {
            let flags = mesh_flags(src, skinref, mesh.skinref);
            for _ in 0..mesh.numnorms.max(0) {
                if n >= light_values.len() {
                    break;
                }
                let bone = usize::from(sub.norm_bones[n]).min(bones.len() - 1);
                let mut normal = sub.norms[n];
                if draw.mirrored {
                    normal[1] = -normal[1];
                }
                light_values[n] = if trans_add {
                    [draw.blend; 3]
                } else {
                    let lv = studio_lighting(gamma, &draw.light, flags, dot(normal, blightvec[bone]));
                    draw.light.color.map(|c| lv * c)
                };
                if flags & texture_flags::CHROME != 0 {
                    chrome[n] = [
                        ((dot(normal, chrome_right[bone]) + 1.0) * 32.0 * 1024.0) as i32,
                        ((dot(normal, chrome_up[bone]) + 1.0) * 32.0 * 1024.0) as i32,
                    ];
                }
                n += 1;
            }
        }

        for mesh in &sub.meshes {
            let flags = mesh_flags(src, skinref, mesh.skinref);
            let tex_index = skinref.get(usize::try_from(mesh.skinref).unwrap_or(usize::MAX)).copied().unwrap_or(0);
            let Some(&(tw, th, _)) = src.textures.get(usize::try_from(tex_index).unwrap_or(usize::MAX)) else { continue };
            let (s_scale, t_scale) = (1.0 / tw.max(1) as f32, 1.0 / th.max(1) as f32);
            let additive = flags & texture_flags::ADDITIVE != 0 && draw.rendermode == RenderMode::Normal;
            let key = mesh_key(draw.rendermode, flags, draw.mirrored);
            let first = out.len() as u32;
            let vertex = |vi: usize, ni: usize, s: i16, t: i16| -> StudioVertex {
                let uv = if flags & texture_flags::CHROME != 0 {
                    let c = chrome[ni];
                    [c[0] as f32 * s_scale / 1024.0, c[1] as f32 * t_scale / 1024.0]
                } else {
                    [f32::from(s) * s_scale, f32::from(t) * t_scale]
                };
                let lit = light_values[ni];
                let rgb = if draw.local_lights.is_empty() || trans_add {
                    lit
                } else {
                    let mut normal = sub.norms[ni];
                    if draw.mirrored {
                        normal[1] = -normal[1];
                    }
                    let bone = usize::from(sub.vert_bones[vi]).min(bones.len() - 1);
                    light_lambert(gamma, &draw.local_lights, &light_bone_pos[bone], sub.verts[vi], normal, lit)
                };
                StudioVertex { pos: xform[vi], uv, color: [rgb[0], rgb[1], rgb[2], draw.blend] }
            };
            for cmd in &mesh.cmds {
                let mut strip: Vec<StudioVertex> = Vec::with_capacity(cmd.verts.len());
                for v in &cmd.verts {
                    let (vi, ni) = (usize::try_from(v[0]).unwrap_or(usize::MAX), usize::try_from(v[1]).unwrap_or(usize::MAX));
                    if ni >= light_values.len() {
                        break;
                    }
                    if vi >= xform.len() {
                        break;
                    }
                    strip.push(vertex(vi, ni, v[2], v[3]));
                }
                triangulate(&strip, cmd.fan, flat, out);
            }
            if out.len() as u32 > first {
                meshes.push(MeshDraw { vertices: first..out.len() as u32, texture: tex_index as u32, key, sprite: None });
            }
            if additive {
                // Original bug: ShadeModel(GL_FLAT) after an additive mesh sticks for the rest of the model.
                flat = true;
            }
        }
    }
    if let Some(shell) = &draw.shell {
        build_shell(src, draw, shell, skinref, chrome_view.right, out, meshes);
    }
}

/// `R_StudioChrome` per bone: (chromeup, chromeright) in bone space for `origin`.
fn chrome_vectors(bones: &[Mat3x4], origin: [f32; 3], vright: [f32; 3]) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    bones
        .iter()
        .map(|m| {
            let dir = normalize([m[0][3] - origin[0], m[1][3] - origin[1], m[2][3] - origin[2]]);
            let up = normalize(cross(dir, vright));
            let right = normalize(cross(up, dir));
            (irotate(m, up), irotate(m, right))
        })
        .unzip()
}

/// Triangles of one tricmd strip or fan; GL_FLAT gives each triangle its last vertex colour.
fn triangulate(strip: &[StudioVertex], fan: bool, flat: bool, out: &mut Vec<StudioVertex>) {
    for i in 2..strip.len() {
        let mut tri = if fan {
            [strip[0], strip[i - 1], strip[i]]
        } else if i % 2 == 0 {
            [strip[i - 2], strip[i - 1], strip[i]]
        } else {
            [strip[i - 1], strip[i - 2], strip[i]]
        };
        if flat {
            let c = tri[2].color;
            for v in &mut tri {
                v.color = c;
            }
        }
        out.extend(tri);
    }
}

/// The glow shell pass of `R_StudioDrawPoints` (renderfx 19): vertices pushed out along their
/// first normal, forced chrome UVs from the orbiting shell origin, constant `rendercolor`,
/// additive without depth write, chrome sprite texture.
fn build_shell(
    src: &StudioSource,
    draw: &StudioDraw,
    shell: &GlowShell,
    skinref: &[i16],
    vright: [f32; 3],
    out: &mut Vec<StudioVertex>,
    meshes: &mut Vec<MeshDraw>,
) {
    let model = &src.model;
    let bones = &draw.bones;
    let t = shell.freq * shell.time;
    let origin = [(t.cos() * 4000.0) as f32, (t.sin() * 4000.0) as f32, ((0.33 * t).cos() * 4000.0) as f32];
    let (chrome_up, chrome_right) = chrome_vectors(bones, origin, vright);
    let scale = shell.renderamt as f32 * 0.05;
    let color = [f32::from(shell.color[0]) / 255.0, f32::from(shell.color[1]) / 255.0, f32::from(shell.color[2]) / 255.0, 1.0];
    for part in 0..model.bodyparts.len() {
        let Some(sub) = model.submodel(part, draw.body) else { continue };
        // The first normal each vertex is drawn with.
        let mut vert_normal: Vec<Option<usize>> = vec![None; sub.verts.len()];
        for v in sub.meshes.iter().flat_map(|m| &m.cmds).flat_map(|c| &c.verts) {
            if let (Ok(vi), Ok(ni)) = (usize::try_from(v[0]), usize::try_from(v[1]))
                && let Some(slot @ None) = vert_normal.get_mut(vi)
            {
                *slot = Some(ni);
            }
        }
        let xform: Vec<[f32; 3]> = sub
            .verts
            .iter()
            .zip(&sub.vert_bones)
            .zip(&vert_normal)
            .map(|((v, &b), n)| {
                let push = n.and_then(|n| sub.norms.get(n)).map_or([0.0; 3], |n| n.map(|c| c * scale));
                let p = [v[0] + push[0], v[1] + push[1], v[2] + push[2]];
                gs_studio::anim::transform(&bones[usize::from(b).min(bones.len() - 1)], p)
            })
            .collect();
        let chrome: Vec<[i32; 2]> = sub
            .norms
            .iter()
            .zip(&sub.norm_bones)
            .take(MAX_STUDIO_VERTS)
            .map(|(n, &b)| {
                let bone = usize::from(b).min(bones.len() - 1);
                [
                    ((dot(*n, chrome_right[bone]) + 1.0) * 32.0 * 1024.0) as i32,
                    ((dot(*n, chrome_up[bone]) + 1.0) * 32.0 * 1024.0) as i32,
                ]
            })
            .collect();
        for mesh in &sub.meshes {
            let flags = mesh_flags(src, skinref, mesh.skinref) | texture_flags::CHROME;
            let tex_index = skinref.get(usize::try_from(mesh.skinref).unwrap_or(usize::MAX)).copied().unwrap_or(0);
            let Some(&(tw, th, _)) = src.textures.get(usize::try_from(tex_index).unwrap_or(usize::MAX)) else { continue };
            let (s_scale, t_scale) = (1.0 / tw.max(1) as f32, 1.0 / th.max(1) as f32);
            let key = chrome_key(flags, draw.mirrored);
            let first = out.len() as u32;
            for cmd in &mesh.cmds {
                let mut strip: Vec<StudioVertex> = Vec::with_capacity(cmd.verts.len());
                for v in &cmd.verts {
                    let (Ok(vi), Ok(ni)) = (usize::try_from(v[0]), usize::try_from(v[1])) else { break };
                    if ni >= chrome.len() || vi >= xform.len() {
                        break;
                    }
                    let c = vert_normal[vi].and_then(|n| chrome.get(n)).copied().unwrap_or([0; 2]);
                    let uv = [c[0] as f32 * s_scale / 1024.0, c[1] as f32 * t_scale / 1024.0];
                    strip.push(StudioVertex { pos: xform[vi], uv, color });
                }
                triangulate(&strip, cmd.fan, false, out);
            }
            if out.len() as u32 > first {
                meshes.push(MeshDraw { vertices: first..out.len() as u32, texture: 0, key, sprite: Some(shell.sprite) });
            }
        }
    }
}

fn mesh_flags(src: &StudioSource, skinref: &[i16], mesh_skinref: i32) -> i32 {
    usize::try_from(mesh_skinref)
        .ok()
        .and_then(|r| skinref.get(r))
        .and_then(|&t| usize::try_from(t).ok())
        .and_then(|t| src.textures.get(t))
        .map_or(0, |t| t.2)
}

/// `R_StudioLighting` for one normal: `lightcos = normal · bone light vector`.
fn studio_lighting(gamma: &GammaTables, light: &StudioLight, flags: i32, lightcos: f32) -> f32 {
    let mut illum = light.ambient as f32;
    let shade = light.shade as f32;
    if flags & texture_flags::FLATSHADE != 0 {
        illum += shade * 0.8;
    } else {
        let lightcos = lightcos.min(1.0);
        illum += shade;
        let r = (lightcos + LAMBERT - 1.0) / LAMBERT;
        if r > 0.0 {
            illum -= shade * r;
        }
        illum = illum.max(0.0);
    }
    f32::from(gamma.light[((illum.min(255.0) * 4.0) as usize).min(1023)]) * (1.0 / 1023.0)
}

/// `R_LightLambert`: add local elights to a vertex colour.
fn light_lambert(
    gamma: &GammaTables,
    lights: &[LocalLight],
    bone_pos: &[[f32; 3]],
    vert: [f32; 3],
    normal: [f32; 3],
    lit: [f32; 3],
) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    for (j, l) in lights.iter().enumerate() {
        let lp = [vert[0] - bone_pos[j][0], vert[1] - bone_pos[j][1], vert[2] - bone_pos[j][2]];
        let d = -dot(lp, normal);
        if d > 0.0 {
            let l2 = dot(lp, lp);
            let w = if l2 <= 0.0 { 1.0 } else { l.radius2 / (l2.sqrt() * l2) };
            for (s, &c) in sum.iter_mut().zip(&l.linear) {
                *s += c as f32 * (w * d);
            }
        }
    }
    if sum == [0.0; 3] {
        return lit;
    }
    [0, 1, 2].map(|k| {
        let x = f32::from(gamma.linear[((lit[k] * 1023.0) as usize).min(1023)]) + sum[k];
        let xi = x as i32;
        if xi > 1023 { 1.0 } else { f32::from(gamma.screen[xi.max(0) as usize]) / 1023.0 }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamma::GammaSettings;

    #[test]
    fn lambert_shading_spans_ambient_to_full() {
        let g = GammaTables::new(GammaSettings::default());
        let light = StudioLight { ambient: 40, shade: 100, color: [1.0; 3], dir: [0.0, 0.0, -1.0] };
        let facing = studio_lighting(&g, &light, 0, -1.0);
        let away = studio_lighting(&g, &light, 0, 1.0);
        assert_eq!(facing, f32::from(g.light[140 * 4]) / 1023.0, "facing the light gets ambient + shade");
        assert_eq!(away, f32::from(g.light[40 * 4]) / 1023.0, "facing away gets ambient only");
        let flat = studio_lighting(&g, &light, texture_flags::FLATSHADE, 1.0);
        assert_eq!(flat, f32::from(g.light[120 * 4]) / 1023.0, "flat shading adds 0.8 of the shade light");
    }

    #[test]
    fn local_light_replacement_keeps_the_original_minstrength_quirk() {
        let g = GammaTables::new(GammaSettings::default());
        let at = |x: f32| PointLight { origin: [x, 0.0, 0.0], radius: 100.0, color: [255, 0, 0] };
        // strengths r²/d²: 0.04, 0.0625, 0.111, then 1.0 replaces the weakest (0.04) and leaves
        // minstrength at 0.04; 0.08 would beat 0.0625 but no slot is below the stale minimum.
        let lights = [at(500.0), at(400.0), at(300.0), at(0.0), at(353.6), at(2000.0)];
        let picked: Vec<f32> = select_local_lights(&lights, [0.0; 3], &g).iter().map(|l| l.origin[0]).collect();
        assert_eq!(picked, [0.0, 400.0, 300.0]);
    }
}
