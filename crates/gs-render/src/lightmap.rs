//! Lightmaps built on the CPU exactly like `R_BuildLightMap` (docs/re/render.md §A.4, §A.6):
//! style weights, dynamic lights and `lightgammatable` are baked into RGBA8 texels, which the
//! GPU then filters and modulates with the base texture. Faces are re-baked when their style
//! values change or dynamic lights touch them.

use glam::Vec3;
use gs_formats::bsp::{Bsp, Face, TEX_SPECIAL};

use crate::RenderError;
use crate::camera::NUM_LIGHTSTYLES;
use crate::gamma::GammaTables;

pub const ATLAS_WIDTH: u32 = 2048;
pub const ATLAS_MAX_HEIGHT: u32 = 8192;
/// `R_BuildLightMap` refuses larger faces.
const MAX_LUXELS: u32 = 324;
/// Fullbright `blocklights` value when the map has no lighting lump.
const FULLBRIGHT: i64 = 255 * 256;
/// Most separate rectangles one [`Lightmaps::upload_dirty`] hands out; beyond that (a lightstyle
/// step re-bakes hundreds of faces at once) their bounding box goes as one.
const MAX_DIRTY_RECTS: usize = 32;
/// Clean texels between two re-baked faces of a shelf that still go in one rectangle.
const DIRTY_MERGE_GAP: u32 = 64;

/// A live `dlight_t` (`R_PushDlights`: `cl.time <= die && radius != 0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DynamicLight {
    /// `cl_dlights` slot (0..32): the light's bit in `surf.dlightbits`.
    pub slot: usize,
    pub origin: Vec3,
    pub radius: f32,
    pub color: [u8; 3],
    pub minlight: f32,
}

/// Shelf packer. Rectangles are placed left-to-right in rows of the current maximum height.
pub struct AtlasBuilder {
    width: u32,
    max_height: u32,
    cursor: (u32, u32),
    row_height: u32,
}

impl AtlasBuilder {
    pub fn new(width: u32, max_height: u32) -> Self {
        Self { width, max_height, cursor: (0, 0), row_height: 0 }
    }

    /// Top-left texel of a new `w`x`h` rectangle, `None` when full.
    pub fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w > self.width {
            return None;
        }
        if self.cursor.0 + w > self.width {
            self.cursor = (0, self.cursor.1 + self.row_height);
            self.row_height = 0;
        }
        let (x, y) = self.cursor;
        if y + h > self.max_height {
            return None;
        }
        self.row_height = self.row_height.max(h);
        self.cursor.0 += w;
        Some((x, y))
    }

    /// Height needed so far, rounded up to a power of two.
    pub fn height(&self) -> u32 {
        (self.cursor.1 + self.row_height).max(1).next_power_of_two()
    }
}

#[derive(Debug, Clone)]
pub struct FaceLightmap {
    pub texel: (u32, u32),
    pub size: (u32, u32),
    pub texture_mins: [i32; 2],
    styles: [u8; 4],
    /// Byte offset of the first style layer in the lighting lump; `None` = no samples (black).
    samples: Option<usize>,
    /// Style values the texels were last built with (`surf.cached_light`).
    cached: [i32; 4],
    /// Brush model owning the face (its entity origin offsets dynamic lights).
    model: usize,
    /// `surf.dlightframe`: the frame dynamic lights last marked the face.
    dlight_frame: u32,
    /// `surf.dlightbits`: marking lights' slots; never cleared per frame, only masked by the
    /// lights still alive when the face is rebuilt.
    dlight_bits: u32,
    /// `surf.cached_dlight`: live lights the texels were last built with.
    cached_dlight: u32,
}

pub struct Lightmaps {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    /// Indexed by BSP face; `None` for faces drawn without a lightmap (sky, water).
    pub faces: Vec<Option<FaceLightmap>>,
    white: (u32, u32),
    fullbright: bool,
    /// Texel rectangles `[x, y, w, h]` re-baked since the last [`Lightmaps::upload_dirty`].
    dirty: Vec<[u32; 4]>,
    /// `r_dlightframecount`.
    frame: u32,
}

/// Faces that carry a lightmap (`GL_BuildLightmaps` skips sky/turbulent/special surfaces).
pub fn face_is_lit(bsp: &Bsp, face: &Face) -> bool {
    bsp.texinfo[usize::from(face.texinfo)].flags & TEX_SPECIAL == 0
}

impl Lightmaps {
    pub fn build(bsp: &Bsp, gamma: &GammaTables, styles: &[i32; NUM_LIGHTSTYLES]) -> Result<Self, RenderError> {
        let mut order: Vec<usize> = (0..bsp.faces.len()).filter(|&i| face_is_lit(bsp, &bsp.faces[i])).collect();
        let extents: Vec<_> = bsp.faces.iter().map(|f| bsp.surface_extents(f)).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(extents[i].lightmap_size().1));
        let mut atlas = AtlasBuilder::new(ATLAS_WIDTH, ATLAS_MAX_HEIGHT);
        let white = atlas.alloc(1, 1).ok_or(RenderError::AtlasFull)?;
        let mut faces: Vec<Option<FaceLightmap>> = vec![None; bsp.faces.len()];
        let mut face_model = vec![0; bsp.faces.len()];
        for (mi, m) in bsp.models.iter().enumerate() {
            let first = usize::try_from(m.first_face).unwrap_or(0);
            let count = usize::try_from(m.num_faces).unwrap_or(0);
            for owner in face_model.iter_mut().skip(first).take(count) {
                *owner = mi;
            }
        }
        for i in order {
            let face = &bsp.faces[i];
            let (w, h) = extents[i].lightmap_size();
            if w * h > MAX_LUXELS {
                log::warn!("face {i}: lightmap {w}x{h} exceeds {MAX_LUXELS} luxels");
                continue;
            }
            let style_count = Bsp::face_style_count(face);
            let samples = usize::try_from(face.light_offset)
                .ok()
                .filter(|&off| off + (w * h * 3) as usize * style_count <= bsp.lighting.len());
            let texel = atlas.alloc(w, h).ok_or(RenderError::AtlasFull)?;
            faces[i] = Some(FaceLightmap {
                texel,
                size: (w, h),
                texture_mins: extents[i].texture_mins,
                styles: face.styles,
                samples,
                cached: [i32::MIN; 4],
                model: face_model[i],
                dlight_frame: 0,
                dlight_bits: 0,
                cached_dlight: 0,
            });
        }
        let (width, height) = (ATLAS_WIDTH, atlas.height());
        let mut lm = Self {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
            faces,
            white,
            fullbright: bsp.lighting.is_empty(),
            dirty: Vec::new(),
            frame: 1,
        };
        let w = ((white.1 * width + white.0) * 4) as usize;
        lm.pixels[w..w + 4].copy_from_slice(&[255; 4]);
        lm.update(bsp, gamma, styles, &[], &[], true);
        lm.dirty.clear();
        Ok(lm)
    }

    /// Normalized coordinate of the reserved white texel (unlit special surfaces).
    pub fn white_coords(&self) -> [f32; 2] {
        [(self.white.0 as f32 + 0.5) / self.width as f32, (self.white.1 as f32 + 0.5) / self.height as f32]
    }

    /// `R_PushDlights`: starts a frame of dynamic light marking.
    pub fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// `R_MarkLights` over brush model `model` placed at `entity_origin` (rotation ignored, as in
    /// `R_DrawBrushModel`).
    pub fn mark_lights(&mut self, bsp: &Bsp, lights: &[DynamicLight], model: usize, entity_origin: Vec3) {
        let Some(head) = bsp.models.get(model).map(|m| m.head_nodes[0]) else { return };
        for light in lights.iter().filter(|l| l.slot < 32) {
            self.mark_node(bsp, light, light.origin - entity_origin, head);
        }
    }

    fn mark_node(&mut self, bsp: &Bsp, light: &DynamicLight, origin: Vec3, mut node: i32) {
        while let Some(n) = usize::try_from(node).ok().and_then(|i| bsp.nodes.get(i)) {
            let Some(plane) = bsp.planes.get(n.plane as usize) else { return };
            let dist = origin.dot(Vec3::from(plane.normal)) - plane.dist;
            if dist > light.radius {
                node = i32::from(n.children[0]);
                continue;
            }
            if dist < -light.radius {
                node = i32::from(n.children[1]);
                continue;
            }
            let rad = light.radius - dist.abs();
            if light.minlight <= rad {
                let maxd = (rad - light.minlight) as i32;
                let first = usize::from(n.first_face);
                for face in first..first + usize::from(n.num_faces) {
                    let (Some(f), Some(Some(lm))) = (bsp.faces.get(face), self.faces.get_mut(face)) else { continue };
                    let Some(ti) = bsp.texinfo.get(usize::from(f.texinfo)) else { continue };
                    let s = (dot4(origin, ti.vecs[0]) - lm.texture_mins[0] as f32) as i32;
                    let t = (dot4(origin, ti.vecs[1]) - lm.texture_mins[1] as f32) as i32;
                    let extents = [(lm.size.0 as i32 - 1) * 16, (lm.size.1 as i32 - 1) * 16];
                    if s > -maxd && t > -maxd && s <= maxd + extents[0] && t <= maxd + extents[1] {
                        lm.dlight_frame = self.frame;
                        lm.dlight_bits |= 1 << light.slot;
                    }
                }
            }
            self.mark_node(bsp, light, origin, i32::from(n.children[0]));
            node = i32::from(n.children[1]);
        }
    }

    /// `R_RenderDynamicLightmaps` + `R_BuildLightMap`: re-bakes faces whose style values changed,
    /// that dynamic lights marked this frame, or that were built with dynamic light last time.
    /// `lights` are this frame's live lights; `origins` the entity origin per brush model.
    pub fn update(
        &mut self,
        bsp: &Bsp,
        gamma: &GammaTables,
        styles: &[i32; NUM_LIGHTSTYLES],
        lights: &[DynamicLight],
        origins: &[Vec3],
        force: bool,
    ) {
        // r_dlightactive
        let active = lights.iter().filter(|l| l.slot < 32).fold(0u32, |m, l| m | 1 << l.slot);
        // `blocklights`: no face holds more than MAX_LUXELS (`build` skips larger ones).
        let mut blocklights = [[0i64; 3]; MAX_LUXELS as usize];
        for (fi, slot) in self.faces.iter_mut().enumerate() {
            let Some(f) = slot.as_mut() else { continue };
            let current = f.styles.map(|s| if s == 255 { 0 } else { styles[usize::from(s) % NUM_LIGHTSTYLES] });
            let lit_now = f.dlight_frame == self.frame;
            if !force && current == f.cached && !lit_now && f.cached_dlight == 0 {
                continue;
            }
            f.cached = current;
            f.cached_dlight = f.dlight_bits & active;
            f.dlight_bits &= active;
            let (w, h) = f.size;
            let luxels = (w * h) as usize;
            let block = &mut blocklights[..luxels];
            if self.fullbright {
                block.fill([FULLBRIGHT; 3]);
            } else {
                block.fill([0; 3]);
                if let Some(base) = f.samples {
                    let layers = f.styles.iter().take_while(|s| **s != 255).count();
                    for (layer, &scale) in current.iter().enumerate().take(layers) {
                        let layer_base = base + layer * luxels * 3;
                        for (i, bl) in block.iter_mut().enumerate() {
                            let s = &bsp.lighting[layer_base + i * 3..layer_base + i * 3 + 3];
                            for c in 0..3 {
                                bl[c] += i64::from(scale) * i64::from(s[c]);
                            }
                        }
                    }
                }
                if lit_now {
                    let origin = origins.get(f.model).copied().unwrap_or(Vec3::ZERO);
                    add_dynamic_lights(bsp, fi, f, lights, origin, block);
                }
            }
            for y in 0..h {
                for x in 0..w {
                    let bl = block[(y * w + x) as usize];
                    let o = (((f.texel.1 + y) * self.width + f.texel.0 + x) * 4) as usize;
                    self.pixels[o] = gamma.lightmap_byte(bl[0]);
                    self.pixels[o + 1] = gamma.lightmap_byte(bl[1]);
                    self.pixels[o + 2] = gamma.lightmap_byte(bl[2]);
                    self.pixels[o + 3] = 255;
                }
            }
            self.dirty.push([f.texel.0, f.texel.1, w, h]);
        }
    }

    /// Hands each texel rectangle `[x, y, w, h]` re-baked since the last call to `upload`, with
    /// the atlas bytes from its first texel on (rows `width * 4` bytes apart). Re-baked faces of a
    /// shelf at most [`DIRTY_MERGE_GAP`] texels apart go as one rectangle; beyond
    /// [`MAX_DIRTY_RECTS`] rectangles their bounding box goes instead.
    pub fn upload_dirty(&mut self, mut upload: impl FnMut([u32; 4], &[u8])) {
        let rects = &mut self.dirty;
        // Shelf faces share their top row, so (y, x) order puts a shelf's faces side by side.
        rects.sort_unstable_by_key(|r| (r[1], r[0]));
        let mut merged = 0usize;
        for i in 0..rects.len() {
            let r = rects[i];
            if let Some(last) = merged.checked_sub(1).map(|m| &mut rects[m])
                && last[1] == r[1]
                && r[0] <= last[0] + last[2] + DIRTY_MERGE_GAP
            {
                // A shelf is as tall as its tallest face, so the taller height stays inside it.
                last[2] = (last[0] + last[2]).max(r[0] + r[2]) - last[0];
                last[3] = last[3].max(r[3]);
                continue;
            }
            rects[merged] = r;
            merged += 1;
        }
        rects.truncate(merged);
        if rects.len() > MAX_DIRTY_RECTS {
            let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
            for r in rects.iter() {
                (x0, y0) = (x0.min(r[0]), y0.min(r[1]));
                (x1, y1) = (x1.max(r[0] + r[2]), y1.max(r[1] + r[3]));
            }
            rects.clear();
            rects.push([x0, y0, x1 - x0, y1 - y0]);
        }
        for &r in rects.iter() {
            let offset = ((r[1] * self.width + r[0]) * 4) as usize;
            upload(r, &self.pixels[offset..]);
        }
        rects.clear();
    }

    /// Normalized lightmap coordinate of texture-space `st` on a lit face.
    pub fn coords(&self, face: usize, st: [f32; 2]) -> Option<[f32; 2]> {
        let f = self.faces.get(face)?.as_ref()?;
        let lx = (st[0] - f.texture_mins[0] as f32 + f.texel.0 as f32 * 16.0 + 8.0) / (self.width as f32 * 16.0);
        let ly = (st[1] - f.texture_mins[1] as f32 + f.texel.1 as f32 * 16.0 + 8.0) / (self.height as f32 * 16.0);
        Some([lx, ly])
    }
}

fn dot4(p: Vec3, v: [f32; 4]) -> f32 {
    p.x * v[0] + p.y * v[1] + p.z * v[2] + v[3]
}

/// `R_AddDynamicLights`: every live light in the face's `dlightbits`, measured from the face
/// plane without projecting onto it, falls off linearly per luxel with an octagonal distance.
fn add_dynamic_lights(bsp: &Bsp, face: usize, f: &FaceLightmap, lights: &[DynamicLight], entity_origin: Vec3, block: &mut [[i64; 3]]) {
    let Some(bf) = bsp.faces.get(face) else { return };
    let (Some(plane), Some(ti)) = (bsp.planes.get(usize::from(bf.plane)), bsp.texinfo.get(usize::from(bf.texinfo))) else { return };
    let (w, h) = f.size;
    for light in lights.iter().filter(|l| l.slot < 32 && f.dlight_bits & (1 << l.slot) != 0) {
        let local = light.origin - entity_origin;
        let dist = local.dot(Vec3::from(plane.normal)) - plane.dist;
        let rad = light.radius - dist.abs();
        if light.minlight > rad {
            continue;
        }
        let minl = rad - light.minlight;
        let ls = dot4(local, ti.vecs[0]) - f.texture_mins[0] as f32;
        let lt = dot4(local, ti.vecs[1]) - f.texture_mins[1] as f32;
        for t in 0..h {
            let td = ((lt - (t * 16) as f32) as i32).abs();
            for s in 0..w {
                let sd = ((ls - (s * 16) as f32) as i32).abs();
                let d = if sd > td { sd + (td >> 1) } else { td + (sd >> 1) };
                if (d as f32) < minl {
                    let b = ((rad - d as f32) * 256.0) as i64;
                    let bl = &mut block[(t * w + s) as usize];
                    for (channel, &color) in bl.iter_mut().zip(&light.color) {
                        *channel += (b * i64::from(color)) >> 8;
                    }
                }
            }
        }
    }
}

/// `R_AnimateLight`: `22 * (letter - 'a')` per style at 10 Hz, 256 for empty patterns.
pub fn animate_styles(patterns: &[String], time: f64) -> [i32; NUM_LIGHTSTYLES] {
    let frame = (time * 10.0) as i64;
    let mut out = [256; NUM_LIGHTSTYLES];
    for (slot, p) in out.iter_mut().zip(patterns) {
        let bytes = p.as_bytes();
        if !bytes.is_empty() {
            let c = bytes[(frame.rem_euclid(bytes.len() as i64)) as usize];
            *slot = 22 * (i32::from(c) - i32::from(b'a'));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamma::GammaSettings;
    use crate::test_bsp::floor;

    /// RGBA of luxel `(s, t)` of face 0.
    fn luxel(lm: &Lightmaps, s: u32, t: u32) -> [u8; 4] {
        let f = lm.faces[0].as_ref().expect("lit face");
        let o = (((f.texel.1 + t) * lm.width + f.texel.0 + s) * 4) as usize;
        lm.pixels[o..o + 4].try_into().expect("rgba")
    }

    /// One frame of `R_PushDlights` + rebuild with `lights`, the floor placed at `origin`.
    fn frame(lm: &mut Lightmaps, bsp: &Bsp, lights: &[DynamicLight], origin: Vec3) {
        let gamma = GammaTables::new(GammaSettings::default());
        lm.begin_frame();
        lm.mark_lights(bsp, lights, 0, origin);
        lm.update(bsp, &gamma, &[264; NUM_LIGHTSTYLES], lights, &[origin], false);
    }

    fn red_light(origin: Vec3) -> DynamicLight {
        DynamicLight { slot: 3, origin, radius: 100.0, color: [255, 0, 0], minlight: 0.0 }
    }

    /// Copies `upload_dirty`'s rectangles into `gpu` the way `write_texture` reads them;
    /// returns how many there were.
    fn upload_into(lm: &mut Lightmaps, gpu: &mut [u8]) -> usize {
        let stride = lm.width as usize * 4;
        let mut count = 0;
        lm.upload_dirty(|[x, y, w, h], data| {
            count += 1;
            for row in 0..h as usize {
                let src = &data[row * stride..row * stride + w as usize * 4];
                let dst = (y as usize + row) * stride + x as usize * 4;
                gpu[dst..dst + src.len()].copy_from_slice(src);
            }
        });
        count
    }

    #[test]
    fn uploads_carry_every_rebaked_texel_in_few_rectangles() {
        let bsp = floor();
        let gamma = GammaTables::new(GammaSettings::default());
        let mut lm = Lightmaps::build(&bsp, &gamma, &[264; NUM_LIGHTSTYLES]).expect("lightmaps");
        let mut gpu = lm.pixels.clone();
        frame(&mut lm, &bsp, &[red_light(Vec3::new(64.0, 64.0, 16.0))], Vec3::ZERO);
        assert_ne!(gpu, lm.pixels, "the light re-baked the face");
        assert_eq!(upload_into(&mut lm, &mut gpu), 1);
        assert_eq!(gpu, lm.pixels);
        assert_eq!(upload_into(&mut lm, &mut gpu), 0, "nothing is sent twice");

        // Groups of two faces 10 texels apart on shelves 4 rows tall, 200 texels between groups:
        // each group goes as one rectangle, and past MAX_DIRTY_RECTS all as their bounding box.
        for (groups, rects) in [(10u32, 10), (40, 1)] {
            for g in 0..groups {
                let (x, y) = ((g % 10) * 200, (g / 10) * 4);
                for face in [[x, y, 9, 3], [x + 19, y, 9, 2]] {
                    for row in face[1]..face[1] + face[3] {
                        let o = ((row * lm.width + face[0]) * 4) as usize;
                        lm.pixels[o..o + face[2] as usize * 4].fill(g as u8 + 1);
                    }
                    lm.dirty.push(face);
                }
            }
            assert_eq!(upload_into(&mut lm, &mut gpu), rects, "{groups} groups");
            assert_eq!(gpu, lm.pixels, "{groups} groups");
        }
    }

    #[test]
    fn dynamic_light_tints_nearby_luxels_and_leaves_no_trace_once_gone() {
        let bsp = floor();
        let gamma = GammaTables::new(GammaSettings::default());
        let mut lm = Lightmaps::build(&bsp, &gamma, &[264; NUM_LIGHTSTYLES]).expect("lightmaps");
        let base = luxel(&lm, 4, 4);
        assert_eq!(luxel(&lm, 0, 0), base);

        frame(&mut lm, &bsp, &[red_light(Vec3::new(64.0, 64.0, 16.0))], Vec3::ZERO);
        let lit = luxel(&lm, 4, 4);
        assert!(lit[0] > base[0], "{lit:?} vs {base:?}");
        assert_eq!(lit[1..], base[1..], "only the light's colour channel brightens");
        assert_eq!(luxel(&lm, 0, 0), base, "the corner is beyond the light's reach");

        frame(&mut lm, &bsp, &[], Vec3::ZERO);
        assert_eq!(luxel(&lm, 4, 4), base, "a vanished light is rebuilt away");
    }

    #[test]
    fn brush_entity_lights_are_measured_from_the_entity_origin() {
        let bsp = floor();
        let gamma = GammaTables::new(GammaSettings::default());
        let mut world = Lightmaps::build(&bsp, &gamma, &[264; NUM_LIGHTSTYLES]).expect("lightmaps");
        let mut moved = Lightmaps::build(&bsp, &gamma, &[264; NUM_LIGHTSTYLES]).expect("lightmaps");
        frame(&mut world, &bsp, &[red_light(Vec3::new(64.0, 64.0, 16.0))], Vec3::ZERO);
        let offset = Vec3::new(300.0, -50.0, 8.0);
        frame(&mut moved, &bsp, &[red_light(Vec3::new(64.0, 64.0, 16.0) + offset)], offset);
        assert_eq!(world.pixels, moved.pixels);
    }

    #[test]
    fn shelf_packer_wraps_rows_and_reports_full() {
        let mut atlas = AtlasBuilder::new(4, 4);
        assert_eq!(atlas.alloc(3, 2), Some((0, 0)));
        assert_eq!(atlas.alloc(3, 2), Some((0, 2)), "second block wraps to the next row");
        assert_eq!(atlas.alloc(3, 2), None, "no vertical space left");
        assert_eq!(atlas.height(), 4);
    }

    #[test]
    fn style_letters_map_to_22_per_step() {
        let patterns = vec!["m".to_owned(), "az".to_owned(), String::new(), "A".to_owned()];
        let v = animate_styles(&patterns, 0.1);
        assert_eq!(&v[..4], &[264, 550, 256, 22 * (65 - 97)], "uppercase gives negative values like the original");
        assert_eq!(v[4], 256);
    }
}
