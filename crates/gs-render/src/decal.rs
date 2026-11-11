//! Decals (docs/re/render.md §A.2.5, §A.8): the merged `decals.wad` list, `R_DecalShoot` /
//! `R_DecalNode` / `R_DecalCreate` placement on brush surfaces, and the clipped decal polygons
//! drawn after their surfaces with the surface lightmap.

use std::collections::HashMap;

use gs_formats::bsp::{Bsp, TEX_SPECIAL};
use gs_formats::miptex::MipTex;
use gs_formats::wad::{Wad, lump_type};

use crate::gamma::GammaTables;
use crate::lightmap::Lightmaps;
use crate::texture;
use crate::world::WorldVertex;

/// `MAX_DECALS`.
pub const MAX_DECALS: usize = 4096;
/// `MAX_BASE_DECALS`.
const MAX_BASE_DECALS: usize = 512;

/// `FDECAL_*`.
pub mod flags {
    pub const PERMANENT: u16 = 0x01;
    pub const CUSTOM: u16 = 0x04;
    pub const HFLIP: u16 = 0x08;
    pub const VFLIP: u16 = 0x10;
    pub const CLIPTEST: u16 = 0x20;
    pub const NOCLIP: u16 = 0x40;
    /// Static: ignores the `r_decals` limit and is never recycled.
    pub const STATIC: u16 = 0x80;
}

/// One entry of the merged decal list.
#[derive(Debug, Clone)]
pub struct DecalEntry {
    pub name: String,
    pub texture: Option<MipTex>,
}

/// `decals.wad` from the default game, then the mod's merged on top (same names replace in
/// place, new ones are appended), at most 512 entries.
#[derive(Debug, Clone, Default)]
pub struct DecalWad {
    pub entries: Vec<DecalEntry>,
}

impl DecalWad {
    /// Merges `wad` into the list (`Decal_MergeInDecals`).
    pub fn merge(&mut self, wad: &Wad) {
        for lump in wad.lumps().iter().filter(|l| l.kind == lump_type::MIPTEX) {
            let texture = match wad.miptex(&lump.name) {
                Some(Ok(t)) => Some(t),
                Some(Err(e)) => {
                    log::warn!("decal {}: {e}", lump.name);
                    None
                }
                None => None,
            };
            let entry = DecalEntry { name: lump.name.clone(), texture };
            if let Some(i) = self.entries.iter().position(|e| e.name.eq_ignore_ascii_case(&lump.name)) {
                self.entries[i] = entry;
            } else if self.entries.len() < MAX_BASE_DECALS {
                self.entries.push(entry);
            } else {
                log::warn!("Too many decals: {} / {MAX_BASE_DECALS}", self.entries.len() + 1);
            }
        }
    }

    /// `Draw_DecalIndex` / `Draw_DecalIndexFromName`: exact (case-sensitive) lookup; an unknown
    /// name is appended without a texture.
    pub fn index(&mut self, name: &str) -> usize {
        if let Some(i) = self.entries.iter().position(|e| e.name == name) {
            return i;
        }
        self.entries.push(DecalEntry { name: name.to_owned(), texture: None });
        self.entries.len() - 1
    }

    /// Size of entry `i` (`None` without a texture).
    pub fn size(&self, i: usize) -> Option<(u32, u32)> {
        self.entries.get(i)?.texture.as_ref().map(|t| (t.width, t.height))
    }
}

/// `Draw_MiptexTexture` for a decal: palette index 255 pure blue → index 255 transparent
/// (`{`), otherwise an alpha gradient of palette colour 255 (`}`).
pub fn decal_rgba(tex: &MipTex, gamma: &GammaTables) -> Option<Vec<u8>> {
    let data = tex.data.as_ref()?;
    let last = data.palette.get(255).copied().unwrap_or([0, 0, 255]);
    if last == [0, 0, 255] {
        return Some(texture::indexed_rgba(&data.mips[0], &data.palette, tex.width, tex.height, true, gamma));
    }
    let [r, g, b] = gamma.palette_color(last, false);
    let mut rgba: Vec<u8> = data.mips[0].iter().flat_map(|&i| [r, g, b, i]).collect();
    texture::sprite_blend(&mut rgba, tex.width as usize, tex.height as usize);
    Some(rgba)
}

/// `decal_t`.
#[derive(Debug, Clone)]
struct Decal {
    /// BSP face.
    surface: usize,
    /// Brush model owning the surface.
    model: usize,
    dx: f32,
    dy: f32,
    /// `1 / scale`.
    scale: f32,
    texture: usize,
    flags: u16,
    /// The decal texture's size (overlap test).
    polygon_size: (f32, f32),
    /// The clipped polygon (model space).
    polygon: Vec<WorldVertex>,
}

/// The decal pool and per-surface lists.
#[derive(Debug, Clone)]
pub struct Decals {
    pool: Vec<Option<Decal>>,
    /// `gDecalCount`.
    count: usize,
    /// `r_decals`.
    pub limit: usize,
    /// Decals per surface, in draw order.
    surfaces: HashMap<usize, Vec<usize>>,
    /// Geometry changed since the last upload.
    pub dirty: bool,
}

impl Default for Decals {
    fn default() -> Self {
        Self { pool: vec![None; MAX_DECALS], count: 0, limit: MAX_DECALS, surfaces: HashMap::new(), dirty: true }
    }
}

/// Inputs of one `R_DecalShoot`.
pub struct DecalShot<'a> {
    pub bsp: &'a Bsp,
    pub lightmaps: &'a Lightmaps,
    /// Brush model index (0 = world).
    pub model: usize,
    pub entity: i32,
    /// Entity origin/angles for brush submodels.
    pub entity_origin: [f32; 3],
    pub entity_angles: [f32; 3],
    pub texture: usize,
    /// Decal texture size and name.
    pub size: (u32, u32),
    pub name: &'a str,
    pub pos: [f32; 3],
    pub flags: u16,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn dot4(p: [f32; 3], v: [f32; 4]) -> f32 {
    p[0] * v[0] + p[1] * v[1] + p[2] * v[2] + v[3]
}

/// `SURF_DRAWTURB | SURF_DRAWTILED` for a face (`Mod_LoadFaces` rules).
fn skip_surface(bsp: &Bsp, face: usize) -> bool {
    let Some(f) = bsp.faces.get(face) else { return true };
    let Some(ti) = bsp.texinfo.get(usize::from(f.texinfo)) else { return true };
    let name = bsp.textures.get(ti.miptex as usize).and_then(|t| t.as_ref()).map_or("", |t| t.name.as_str());
    let starts_ci = |p: &str| name.get(..p.len()).is_some_and(|s| s.eq_ignore_ascii_case(p));
    name.starts_with("sky")
        || name.starts_with("scroll")
        || name.starts_with('!')
        || starts_ci("laser")
        || starts_ci("water")
        || ti.flags & TEX_SPECIAL != 0
}

impl Decals {
    pub fn clear(&mut self) {
        let limit = self.limit;
        *self = Self { limit, ..Self::default() };
    }

    /// Decals by model, then surface order: `(model, texture, polygon)`.
    pub fn polygons(&self) -> impl Iterator<Item = (usize, usize, &[WorldVertex])> {
        let mut faces: Vec<&usize> = self.surfaces.keys().collect();
        faces.sort();
        faces.into_iter().flat_map(move |face| {
            self.surfaces[face].iter().filter_map(|&i| self.pool[i].as_ref()).map(|d| (d.model, d.texture, d.polygon.as_slice()))
        })
    }

    /// `EV_RemoveAllDecals`: `R_DecalRemoveAll(i)` for every texture index `i < r_decals`, i.e.
    /// every non-permanent decal whose texture index is below the limit. Sprays (texture
    /// `~player` in the engine) are never among them.
    pub fn remove_all(&mut self) {
        for i in 0..self.pool.len() {
            if self.pool[i].as_ref().is_some_and(|d| d.texture < self.limit && d.flags & (flags::PERMANENT | flags::CUSTOM) == 0) {
                self.unlink(i);
            }
        }
    }

    /// `R_DecalRemoveAll(texture)`: every decal of one texture (a player's sprays).
    pub fn remove_texture(&mut self, texture: usize) {
        for i in 0..self.pool.len() {
            if self.pool[i].as_ref().is_some_and(|d| d.texture == texture) {
                self.unlink(i);
            }
        }
    }

    fn unlink(&mut self, i: usize) {
        if let Some(d) = self.pool[i].take()
            && let Some(list) = self.surfaces.get_mut(&d.surface)
        {
            list.retain(|&k| k != i);
            if list.is_empty() {
                self.surfaces.remove(&d.surface);
            }
        }
        self.dirty = true;
    }

    /// `R_DecalShoot` → `R_DecalNode`.
    pub fn shoot(&mut self, shot: &DecalShot, random_float: &mut dyn FnMut(f32, f32) -> f32) {
        let bsp = shot.bsp;
        let Some(model) = bsp.models.get(shot.model) else {
            log::debug!("Decals must hit mod_brush!");
            return;
        };
        let mut pos = shot.pos;
        if shot.entity != 0 && model.first_face != 0 {
            pos = [0, 1, 2].map(|i| pos[i] - shot.entity_origin[i]);
            if shot.entity_angles != [0.0; 3] {
                let (f, r, u) = crate::camera::angle_vectors(glam::Vec3::from(shot.entity_angles));
                pos = [dot(pos, f.to_array()), -dot(pos, r.to_array()), dot(pos, u.to_array())];
            }
        }
        let mut decal_flags = shot.flags;
        if decal_flags & flags::CUSTOM == 0 {
            decal_flags |= flags::CLIPTEST;
        }
        let (w, h) = shot.size;
        let size = (w.max(h) >> 1) as f32;
        let scale = if shot.name.contains("}z_") { random_float(0.1, 0.15) } else { -1.0 };
        let head = model.head_nodes[0];
        self.node(shot, head, pos, size, scale, decal_flags);
    }

    fn node(&mut self, shot: &DecalShot, mut node: i32, pos: [f32; 3], size: f32, scale: f32, decal_flags: u16) {
        let bsp = shot.bsp;
        while let Some(n) = usize::try_from(node).ok().and_then(|n| bsp.nodes.get(n)) {
            let Some(plane) = bsp.planes.get(n.plane as usize) else { return };
            let dist = dot(pos, plane.normal) - plane.dist;
            if dist > size {
                node = i32::from(n.children[0]);
                continue;
            }
            if dist < -size {
                node = i32::from(n.children[1]);
                continue;
            }
            if dist.abs() < 4.0 {
                let first = usize::from(n.first_face);
                for face in first..first + usize::from(n.num_faces) {
                    self.try_surface(shot, face, pos, scale, decal_flags);
                }
            }
            self.node(shot, i32::from(n.children[0]), pos, size, scale, decal_flags);
            node = i32::from(n.children[1]);
        }
    }

    fn try_surface(&mut self, shot: &DecalShot, face: usize, pos: [f32; 3], scale: f32, decal_flags: u16) {
        let bsp = shot.bsp;
        if skip_surface(bsp, face) {
            return;
        }
        let Some(f) = bsp.faces.get(face) else { return };
        let Some(ti) = bsp.texinfo.get(usize::from(f.texinfo)) else { return };
        let sc = if scale == -1.0 { (ti.vecs[0][0] * ti.vecs[0][0] + ti.vecs[0][1] * ti.vecs[0][1] + ti.vecs[0][2] * ti.vecs[0][2]).sqrt() } else { scale };
        if sc == 0.0 {
            return;
        }
        let (tw, th) = shot.size;
        let w = (tw as f32 * sc) as i32 as f32;
        let h = (th as f32 * sc) as i32 as f32;
        let ext = bsp.surface_extents(f);
        let s = dot4(pos, ti.vecs[0]) - ext.texture_mins[0] as f32 - w * 0.5;
        let t = dot4(pos, ti.vecs[1]) - ext.texture_mins[1] as f32 - h * 0.5;
        if !(-w < s && -h < t && s <= w + ext.extents[0] as f32 && t <= h + ext.extents[1] as f32) {
            return;
        }
        let (sw, sh) = bsp.textures.get(ti.miptex as usize).and_then(|t| t.as_ref()).map_or((16, 16), |t| (t.width, t.height));
        let dx = (ext.texture_mins[0] as f32 + s) / sw as f32;
        let dy = (ext.texture_mins[1] as f32 + t) / sh as f32;
        self.create(shot, face, 1.0 / sc, dx, dy, decal_flags);
    }

    /// `R_DecalCreate`: reuse the closest of 4+ overlapping decals, else the next round-robin
    /// slot; append to the surface's list.
    fn create(&mut self, shot: &DecalShot, face: usize, scale: f32, dx: f32, dy: f32, decal_flags: u16) {
        let bsp = shot.bsp;
        let Some(f) = bsp.faces.get(face) else { return };
        let Some(ti) = bsp.texinfo.get(usize::from(f.texinfo)) else { return };
        let (sw, sh) = bsp.textures.get(ti.miptex as usize).and_then(|t| t.as_ref()).map_or((16.0, 16.0), |t| (t.width as f32, t.height as f32));
        let (nw, nh) = (shot.size.0 as f32, shot.size.1 as f32);
        let mut count = 0;
        let mut closest: Option<(usize, i32)> = None;
        for &i in self.surfaces.get(&face).map_or(&[][..], Vec::as_slice) {
            let Some(old) = self.pool[i].as_ref() else { continue };
            if old.flags & (flags::PERMANENT | flags::STATIC) != 0 {
                continue;
            }
            let (ow, oh) = (old.polygon_size.0, old.polygon_size.1);
            if ow > nw * 1.5 {
                continue;
            }
            let ddx = (sw * dx + nw / 2.0 - (sw * old.dx + ow / 2.0)).abs();
            let ddy = (sh * dy + nh / 2.0 - (sh * old.dy + oh / 2.0)).abs();
            let d = (ddx.max(ddy) + ddx.min(ddy) / 2.0) as i32;
            if (d as f32 * old.scale) < 8.0 {
                count += 1;
                if closest.is_none_or(|(_, cd)| d < cd) {
                    closest = Some((i, d));
                }
            }
        }
        let slot = match closest {
            Some((i, _)) if count >= 4 => i,
            _ => {
                let limit = if decal_flags & flags::STATIC != 0 { MAX_DECALS } else { self.limit.clamp(1, MAX_DECALS) };
                let mut slot = None;
                for _ in 0..limit {
                    self.count = (self.count + 1) % limit;
                    if self.pool[self.count].as_ref().is_none_or(|d| d.flags & (flags::PERMANENT | flags::STATIC) == 0) {
                        slot = Some(self.count);
                        break;
                    }
                }
                let Some(slot) = slot else { return };
                slot
            }
        };
        self.unlink(slot);
        let mut decal = Decal {
            surface: face,
            model: shot.model,
            dx,
            dy,
            scale,
            texture: shot.texture,
            flags: decal_flags,
            polygon: Vec::new(),
            polygon_size: (nw, nh),
        };
        decal.polygon = clip_polygon(bsp, shot.lightmaps, face, &decal, shot.size);
        if decal.polygon.len() < 3 {
            return;
        }
        let list = self.surfaces.entry(face).or_default();
        if decal_flags & flags::CUSTOM != 0 {
            let at = list.iter().position(|&k| self.pool[k].as_ref().is_some_and(|d| d.flags & flags::STATIC != 0)).unwrap_or(list.len());
            list.insert(at, slot);
        } else {
            list.push(slot);
        }
        self.pool[slot] = Some(decal);
        self.dirty = true;
    }
}

/// `R_DecalVertsClip`: the face polygon in decal texture space, clipped to the decal.
fn clip_polygon(bsp: &Bsp, lightmaps: &Lightmaps, face: usize, decal: &Decal, size: (u32, u32)) -> Vec<WorldVertex> {
    let Some(f) = bsp.faces.get(face) else { return Vec::new() };
    let Some(ti) = bsp.texinfo.get(usize::from(f.texinfo)) else { return Vec::new() };
    let (sw, sh) = bsp.textures.get(ti.miptex as usize).and_then(|t| t.as_ref()).map_or((16.0, 16.0), |t| (t.width as f32, t.height as f32));
    let (dw, dh) = (size.0.max(1) as f32, size.1.max(1) as f32);
    // (position, decal s, decal t)
    let mut poly: Vec<([f32; 3], f32, f32)> = bsp
        .face_vertices(f)
        .map(|p| {
            let vs = dot4(p, ti.vecs[0]) / sw;
            let vt = dot4(p, ti.vecs[1]) / sh;
            let mut s = (vs - decal.dx) * (sw * decal.scale / dw);
            let mut t = (vt - decal.dy) * (sh * decal.scale / dh);
            if decal.flags & flags::HFLIP != 0 {
                s = 1.0 - s;
            }
            if decal.flags & flags::VFLIP != 0 {
                t = 1.0 - t;
            }
            (p, s, t)
        })
        .collect();
    for (axis, keep_below) in [(0, false), (0, true), (1, false), (1, true)] {
        poly = clip_edge(&poly, axis, keep_below);
        if poly.is_empty() {
            return Vec::new();
        }
    }
    poly.into_iter()
        .map(|(p, s, t)| {
            let st = [dot4(p, ti.vecs[0]), dot4(p, ti.vecs[1])];
            let lm = lightmaps.coords(face, st).unwrap_or_else(|| lightmaps.white_coords());
            WorldVertex { pos: p, uv: [s, t], lm, aux: 0.0 }
        })
        .collect()
}

/// Sutherland–Hodgman against `s >= 0` / `s <= 1` (`axis` 0) or `t` (`axis` 1).
fn clip_edge(poly: &[([f32; 3], f32, f32)], axis: usize, keep_below: bool) -> Vec<([f32; 3], f32, f32)> {
    let coord = |v: &([f32; 3], f32, f32)| if axis == 0 { v.1 } else { v.2 };
    let inside = |v: &([f32; 3], f32, f32)| if keep_below { coord(v) <= 1.0 } else { coord(v) >= 0.0 };
    let edge = if keep_below { 1.0 } else { 0.0 };
    let mut out = Vec::with_capacity(poly.len() + 2);
    for (i, cur) in poly.iter().enumerate() {
        let next = &poly[(i + 1) % poly.len()];
        if inside(cur) {
            out.push(*cur);
        }
        if inside(cur) != inside(next) {
            let f = (edge - coord(cur)) / (coord(next) - coord(cur));
            let lerp = |a: f32, b: f32| a + (b - a) * f;
            out.push(([0, 1, 2].map(|k| lerp(cur.0[k], next.0[k])), lerp(cur.1, next.1), lerp(cur.2, next.2)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::NUM_LIGHTSTYLES;
    use crate::gamma::GammaSettings;
    use crate::test_bsp::floor;

    struct World {
        bsp: Bsp,
        lightmaps: Lightmaps,
    }

    impl World {
        fn new() -> Self {
            let bsp = floor();
            let gamma = GammaTables::new(GammaSettings::default());
            let lightmaps = Lightmaps::build(&bsp, &gamma, &[0; NUM_LIGHTSTYLES]).expect("lightmaps");
            Self { bsp, lightmaps }
        }

        /// A 16×16 decal of `texture` at `(x, y)` on the floor.
        fn shoot(&self, decals: &mut Decals, texture: usize, x: f32, y: f32, flags: u16) {
            let shot = DecalShot {
                bsp: &self.bsp,
                lightmaps: &self.lightmaps,
                model: 0,
                entity: 0,
                entity_origin: [0.0; 3],
                entity_angles: [0.0; 3],
                texture,
                size: (16, 16),
                name: "{shot1",
                pos: [x, y, 0.0],
                flags,
            };
            decals.shoot(&shot, &mut |lo, _| lo);
        }
    }

    /// `(texture, min xy, max xy)` of every decal.
    fn placed(decals: &Decals) -> Vec<(usize, [f32; 2], [f32; 2])> {
        decals
            .polygons()
            .map(|(_, texture, poly)| {
                let fold = |f: fn(f32, f32) -> f32, init: f32| {
                    poly.iter().fold([init; 2], |acc, v| [f(acc[0], v.pos[0]), f(acc[1], v.pos[1])])
                };
                (texture, fold(f32::min, f32::INFINITY), fold(f32::max, f32::NEG_INFINITY))
            })
            .collect()
    }

    #[test]
    fn decal_is_texture_sized_and_centred_on_the_impact() {
        let world = World::new();
        let mut decals = Decals::default();
        world.shoot(&mut decals, 0, 64.0, 40.0, 0);
        assert_eq!(placed(&decals), vec![(0, [56.0, 32.0], [72.0, 48.0])]);
        let uvs: Vec<[f32; 2]> = decals.polygons().flat_map(|(_, _, p)| p.iter().map(|v| v.uv)).collect();
        assert!(uvs.iter().all(|uv| uv.iter().all(|c| (0.0..=1.0).contains(c))), "{uvs:?}");
    }

    #[test]
    fn decal_overhanging_the_face_is_clipped_to_it() {
        let world = World::new();
        let mut decals = Decals::default();
        world.shoot(&mut decals, 0, 2.0, 64.0, 0);
        assert_eq!(placed(&decals), vec![(0, [0.0, 56.0], [10.0, 72.0])]);
        let poly: Vec<WorldVertex> = decals.polygons().flat_map(|(_, _, p)| p.to_vec()).collect();
        // The face edge x = 0 cuts the decal 6 of its 16 texels in.
        let at_edge: Vec<f32> = poly.iter().filter(|v| v.pos[0] == 0.0).map(|v| v.uv[0]).collect();
        assert!(at_edge.iter().all(|&s| (s - 0.375).abs() < 1e-5), "{at_edge:?}");
    }

    #[test]
    fn recycling_honours_the_limit_and_never_takes_permanent_decals() {
        let world = World::new();
        let mut decals = Decals { limit: 3, ..Decals::default() };
        world.shoot(&mut decals, 9, 100.0, 100.0, flags::PERMANENT);
        for (i, x) in [20.0, 40.0, 60.0, 80.0].into_iter().enumerate() {
            world.shoot(&mut decals, i, x, 20.0, 0);
        }
        let mut textures: Vec<usize> = placed(&decals).into_iter().map(|(t, ..)| t).collect();
        textures.sort_unstable();
        assert_eq!(textures, vec![2, 3, 9], "the two oldest normal decals are recycled");
    }

    #[test]
    fn fifth_overlapping_decal_replaces_the_closest() {
        let world = World::new();
        let mut decals = Decals::default();
        for (i, x) in [64.0, 65.0, 66.0, 67.0].into_iter().enumerate() {
            world.shoot(&mut decals, i + 1, x, 64.0, 0);
        }
        world.shoot(&mut decals, 9, 67.2, 64.0, 0);
        let mut textures: Vec<usize> = placed(&decals).into_iter().map(|(t, ..)| t).collect();
        textures.sort_unstable();
        assert_eq!(textures, vec![1, 2, 3, 9]);
    }

    #[test]
    fn remove_all_keeps_permanent_decals() {
        let world = World::new();
        let mut decals = Decals { limit: 300, ..Decals::default() };
        world.shoot(&mut decals, 1, 30.0, 30.0, flags::PERMANENT);
        world.shoot(&mut decals, 2, 90.0, 90.0, 0);
        decals.remove_all();
        assert_eq!(placed(&decals).into_iter().map(|(t, ..)| t).collect::<Vec<_>>(), vec![1]);
    }

    /// A `w`×`h` miptex lump filled with `index`, palette entry 255 = `last`.
    fn miptex(name: &str, w: u32, h: u32, index: u8, last: [u8; 3]) -> Vec<u8> {
        let mut out = vec![0u8; 16];
        out[..name.len()].copy_from_slice(name.as_bytes());
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        let mut offset = 40u32;
        for mip in 0..4 {
            out.extend_from_slice(&offset.to_le_bytes());
            offset += (w >> mip) * (h >> mip);
        }
        for mip in 0..4 {
            out.extend(std::iter::repeat_n(index, ((w >> mip) * (h >> mip)) as usize));
        }
        out.extend_from_slice(&256u16.to_le_bytes());
        for i in 0..256 {
            out.extend_from_slice(&if i == 255 { last } else { [i as u8; 3] });
        }
        out.extend_from_slice(&[0, 0]);
        out
    }

    fn wad(lumps: &[Vec<u8>]) -> Wad {
        let mut data = b"WAD3".to_vec();
        let table = 12 + lumps.iter().map(Vec::len).sum::<usize>();
        data.extend_from_slice(&(lumps.len() as i32).to_le_bytes());
        data.extend_from_slice(&(table as i32).to_le_bytes());
        let mut offsets = Vec::new();
        for lump in lumps {
            offsets.push(data.len());
            data.extend_from_slice(lump);
        }
        for (lump, offset) in lumps.iter().zip(offsets) {
            data.extend_from_slice(&(offset as i32).to_le_bytes());
            data.extend_from_slice(&(lump.len() as i32).to_le_bytes());
            data.extend_from_slice(&(lump.len() as i32).to_le_bytes());
            data.extend_from_slice(&[lump_type::MIPTEX, 0, 0, 0]);
            data.extend_from_slice(&lump[..16]);
        }
        Wad::parse(data).expect("wad")
    }

    #[test]
    fn mod_decals_replace_base_ones_and_lookup_is_case_sensitive() {
        let blue = [0, 0, 255];
        let mut list = DecalWad::default();
        list.merge(&wad(&[miptex("{AMMO", 16, 16, 1, blue), miptex("REFLECT1", 16, 16, 1, blue)]));
        list.merge(&wad(&[miptex("{ammo", 32, 16, 1, blue), miptex("{shot1", 16, 16, 1, blue)]));
        let names: Vec<&str> = list.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["{ammo", "REFLECT1", "{shot1"]);
        assert_eq!(list.index("{ammo"), 0);
        assert_eq!(list.size(0), Some((32, 16)), "the mod's texture wins");
        let upper = list.index("{AMMO");
        assert_eq!(upper, 3, "an exact-case miss appends a new entry");
        assert_eq!(list.size(upper), None);
    }

    #[test]
    fn blue_index_255_is_a_cutout_otherwise_an_alpha_gradient() {
        let gamma = GammaTables::new(GammaSettings::default());
        let cutout = MipTex::parse(&miptex("{hole", 16, 16, 255, [0, 0, 255])).expect("miptex");
        let rgba = decal_rgba(&cutout, &gamma).expect("rgba");
        assert!(rgba.chunks(4).all(|p| p[3] == 0), "index 255 is transparent");
        let solid = MipTex::parse(&miptex("{hole", 16, 16, 7, [0, 0, 255])).expect("miptex");
        assert!(decal_rgba(&solid, &gamma).expect("rgba").chunks(4).all(|p| p[3] == 255));

        let tint = [200, 30, 10];
        let gradient = MipTex::parse(&miptex("}blood", 16, 16, 90, tint)).expect("miptex");
        let rgba = decal_rgba(&gradient, &gamma).expect("rgba");
        let colour = gamma.palette_color(tint, false);
        assert!(rgba.chunks(4).all(|p| p[..3] == colour && p[3] == 90), "{:?}", &rgba[..4]);
    }
}
