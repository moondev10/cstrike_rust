//! Brush geometry: triangulated faces grouped into draw batches per BSP model
//! (docs/re/render.md §A.1.2, §A.7).

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use gs_formats::bsp::{Bsp, Face};

use crate::lightmap::Lightmaps;

/// `GL_SubdivideSurface` grid for turbulent surfaces.
const SUBDIVIDE_SIZE: f32 = 64.0;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct WorldVertex {
    pub pos: [f32; 3],
    /// Lit/sky faces: normalized texture coordinates. Water: raw `(os, ot)` texel coordinates.
    pub uv: [f32; 2],
    pub lm: [f32; 2],
    /// Water: z of the polygon's first vertex (wave sign test). Unused otherwise.
    pub aux: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SurfaceKind {
    Lit,
    Sky,
    Water,
}

impl SurfaceKind {
    /// Face flag assignment (`Mod_LoadFaces`, case-sensitive `sky`, case-insensitive `laser`/`water`).
    pub fn classify(texture_name: &str) -> Self {
        let starts_ci = |p: &str| texture_name.get(..p.len()).is_some_and(|s| s.eq_ignore_ascii_case(p));
        if texture_name.starts_with("sky") {
            Self::Sky
        } else if texture_name.starts_with('!') || starts_ci("laser") || starts_ci("water") {
            Self::Water
        } else {
            Self::Lit
        }
    }
}

#[derive(Debug, Clone)]
pub struct Batch {
    pub kind: SurfaceKind,
    /// Index into `bsp.textures`.
    pub texture: u32,
    pub indices: Range<u32>,
}

#[derive(Debug, Clone)]
pub struct WorldMesh {
    pub vertices: Vec<WorldVertex>,
    pub indices: Vec<u32>,
    /// Batches of each BSP model (model 0 = world), sorted by kind then texture.
    pub models: Vec<Vec<Batch>>,
}

fn dot4(p: [f32; 3], v: [f32; 4]) -> f32 {
    p[0] * v[0] + p[1] * v[1] + p[2] * v[2] + v[3]
}

impl WorldMesh {
    pub fn build(bsp: &Bsp, lightmaps: &Lightmaps) -> Self {
        let mut mesh = Self { vertices: Vec::new(), indices: Vec::new(), models: Vec::with_capacity(bsp.models.len()) };
        for model in &bsp.models {
            let first = model.first_face as usize;
            let mut faces: Vec<(SurfaceKind, u32, usize)> = (first..first + model.num_faces as usize)
                .map(|fi| {
                    let tex = &bsp.texinfo[usize::from(bsp.faces[fi].texinfo)];
                    let name = bsp.textures[tex.miptex as usize].as_ref().map_or("", |t| t.name.as_str());
                    (SurfaceKind::classify(name), tex.miptex, fi)
                })
                .collect();
            faces.sort_by_key(|&(kind, tex, _)| (kind, tex));
            let mut batches: Vec<Batch> = Vec::new();
            for (kind, texture, fi) in faces {
                let start = mesh.indices.len() as u32;
                match kind {
                    SurfaceKind::Water => mesh.push_water(bsp, &bsp.faces[fi]),
                    _ => mesh.push_face(bsp, lightmaps, fi),
                }
                let end = mesh.indices.len() as u32;
                match batches.last_mut() {
                    Some(b) if b.kind == kind && b.texture == texture && b.indices.end == start => b.indices.end = end,
                    _ => batches.push(Batch { kind, texture, indices: start..end }),
                }
            }
            mesh.models.push(batches);
        }
        mesh
    }

    fn push_polygon(&mut self, verts: impl Iterator<Item = WorldVertex>) {
        let base = self.vertices.len() as u32;
        self.vertices.extend(verts);
        let n = self.vertices.len() as u32 - base;
        for k in 1..n.saturating_sub(1) {
            self.indices.extend_from_slice(&[base, base + k, base + k + 1]);
        }
    }

    fn push_face(&mut self, bsp: &Bsp, lightmaps: &Lightmaps, fi: usize) {
        let face = &bsp.faces[fi];
        let tex = &bsp.texinfo[usize::from(face.texinfo)];
        let (tw, th) = bsp.textures[tex.miptex as usize].as_ref().map_or((16, 16), |t| (t.width, t.height));
        let verts: Vec<WorldVertex> = bsp
            .face_vertices(face)
            .map(|p| {
                let st = [dot4(p, tex.vecs[0]), dot4(p, tex.vecs[1])];
                let lm = lightmaps.coords(fi, st).unwrap_or_else(|| lightmaps.white_coords());
                WorldVertex { pos: p, uv: [st[0] / tw as f32, st[1] / th as f32], lm, aux: 0.0 }
            })
            .collect();
        self.push_polygon(verts.into_iter());
    }

    fn push_water(&mut self, bsp: &Bsp, face: &Face) {
        let tex = &bsp.texinfo[usize::from(face.texinfo)];
        let verts: Vec<[f32; 3]> = bsp.face_vertices(face).collect();
        let mut polys = Vec::new();
        subdivide(verts, &mut polys);
        for poly in polys {
            let z0 = poly[0][2];
            self.push_polygon(poly.into_iter().map(|p| {
                let s = p[0] * tex.vecs[0][0] + p[1] * tex.vecs[0][1] + p[2] * tex.vecs[0][2];
                let t = p[0] * tex.vecs[1][0] + p[1] * tex.vecs[1][1] + p[2] * tex.vecs[1][2];
                WorldVertex { pos: p, uv: [s, t], lm: [0.0; 2], aux: z0 }
            }));
        }
    }
}

/// `SubdividePolygon`: recursively split at 64-unit grid lines until no axis spans more than
/// a cell (skipping splits that would leave a sliver under 8 units).
fn subdivide(verts: Vec<[f32; 3]>, out: &mut Vec<Vec<[f32; 3]>>) {
    let mut mins = [f32::MAX; 3];
    let mut maxs = [f32::MIN; 3];
    for v in &verts {
        for i in 0..3 {
            mins[i] = mins[i].min(v[i]);
            maxs[i] = maxs[i].max(v[i]);
        }
    }
    for axis in 0..3 {
        let m = SUBDIVIDE_SIZE * ((mins[axis] + maxs[axis]) * 0.5 / SUBDIVIDE_SIZE + 0.5).floor();
        if maxs[axis] - m < 8.0 || m - mins[axis] < 8.0 {
            continue;
        }
        let n = verts.len();
        let dist: Vec<f32> = verts.iter().map(|v| v[axis] - m).collect();
        let (mut front, mut back) = (Vec::new(), Vec::new());
        for j in 0..n {
            let (v, d) = (verts[j], dist[j]);
            let (next, dn) = (verts[(j + 1) % n], dist[(j + 1) % n]);
            if d >= 0.0 {
                front.push(v);
            }
            if d <= 0.0 {
                back.push(v);
            }
            if d == 0.0 || dn == 0.0 || (d > 0.0) == (dn > 0.0) {
                continue;
            }
            let frac = d / (d - dn);
            let mid = [0, 1, 2].map(|i| v[i] + frac * (next[i] - v[i]));
            front.push(mid);
            back.push(mid);
        }
        subdivide(front, out);
        subdivide(back, out);
        return;
    }
    out.push(verts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_follows_engine_prefix_rules() {
        assert_eq!(SurfaceKind::classify("sky"), SurfaceKind::Sky);
        assert_eq!(SurfaceKind::classify("SKY"), SurfaceKind::Lit, "sky prefix is case-sensitive");
        assert_eq!(SurfaceKind::classify("!waterblue"), SurfaceKind::Water);
        assert_eq!(SurfaceKind::classify("WATER1"), SurfaceKind::Water);
        assert_eq!(SurfaceKind::classify("*lava"), SurfaceKind::Lit, "* is not turbulent in GoldSrc");
    }

    #[test]
    fn subdivision_cuts_on_64_unit_grid() {
        let quad = vec![[0.0, 0.0, 0.0], [128.0, 0.0, 0.0], [128.0, 128.0, 0.0], [0.0, 128.0, 0.0]];
        let mut out = Vec::new();
        subdivide(quad, &mut out);
        assert_eq!(out.len(), 4);
        for poly in &out {
            for axis in 0..2 {
                let (lo, hi) = poly.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v[axis]), b.max(v[axis])));
                assert_eq!(hi - lo, 64.0);
            }
        }
    }
}
