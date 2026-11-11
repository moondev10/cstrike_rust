//! A loaded map's spatial data: brush model hulls (`Mod_LoadBrushModel`, `Mod_MakeHull0`) and the
//! render-node surface walks `SurfaceAtPoint` / `RecursiveLightPoint` (docs/re/render.md §B.10.2).

use gs_formats::bsp::{Bsp, decompress_vis};

use crate::hull::{ClipNode, Hull, Plane};

/// `texinfo.flags & TEX_SPECIAL`.
const TEX_SPECIAL: u32 = 1;
const MAX_LIGHTSTYLES: usize = 64;
const CONTENTS_SOLID: i32 = -2;

/// Clip box of BSP hulls 1..3 (hull 0 is the point hull).
const HULL_SIZES: [([f32; 3], [f32; 3]); 4] = [
    ([0.0; 3], [0.0; 3]),
    ([-16.0, -16.0, -36.0], [16.0, 16.0, 36.0]),
    ([-32.0, -32.0, -32.0], [32.0, 32.0, 32.0]),
    ([-16.0, -16.0, -18.0], [16.0, 16.0, 18.0]),
];

#[derive(Debug, Clone, Copy)]
struct HullDef {
    first: i32,
    last: i32,
}

/// One brush model (`model_t` of `*N` / the world).
#[derive(Debug, Clone)]
pub struct BrushModel {
    hulls: [HullDef; 4],
    /// Bounds padded by 1 unit (`Mod_LoadSubmodels`), as returned by `PM_GetModelBounds`.
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    /// First render node (`hulls[0].firstclipnode`).
    pub head_node: i32,
}

#[derive(Debug, Clone, Copy)]
struct SurfNode {
    plane: usize,
    /// Node index (>= 0) or `-1 - leaf`.
    children: [i32; 2],
    first_face: usize,
    num_faces: usize,
}

#[derive(Debug, Clone)]
struct SurfFace {
    vecs: [[f32; 4]; 2],
    texture_mins: [i32; 2],
    extents: [i32; 2],
    texture: Option<usize>,
    /// `SURF_DRAWTILED`: no lightmap sampling.
    tiled: bool,
    /// `SURF_DRAWSKY`.
    sky: bool,
    styles: [u8; 4],
    light_offset: i32,
}

#[derive(Debug, Clone, Copy)]
struct VisLeaf {
    contents: i32,
    vis_offset: i32,
}

/// Map spatial data shared by movement and rendering.
#[derive(Debug, Clone)]
pub struct World {
    planes: Vec<Plane>,
    hull0: Vec<ClipNode>,
    clipnodes: Vec<ClipNode>,
    pub models: Vec<BrushModel>,
    nodes: Vec<SurfNode>,
    faces: Vec<SurfFace>,
    textures: Vec<String>,
    lighting: Vec<u8>,
    leafs: Vec<VisLeaf>,
    visibility: Vec<u8>,
    /// Bytes per decompressed PVS row.
    vis_row: usize,
}

/// `Mod_LoadFaces` surface flags `(SURF_DRAWTILED, SURF_DRAWSKY)`: case-sensitive `sky` and
/// `scroll` prefixes are tiled; liquids (`!`, `laser`, `water`) are only SURF_DRAWTURB and keep
/// their lightmaps; any other TEX_SPECIAL face is tiled.
fn surface_flags(name: &str, special: bool) -> (bool, bool) {
    let starts_ci = |p: &str| name.get(..p.len()).is_some_and(|s| s.eq_ignore_ascii_case(p));
    if name.starts_with("sky") {
        (true, true)
    } else if name.starts_with("scroll") {
        (true, false)
    } else if name.starts_with('!') || starts_ci("laser") || starts_ci("water") {
        (false, false)
    } else {
        (special, false)
    }
}

enum Walk {
    Miss,
    Hit([i32; 3]),
    /// A surface without samples: 0 for this node, a miss for the caller.
    Dark,
}

impl World {
    pub fn from_bsp(bsp: &Bsp) -> Self {
        let planes = bsp
            .planes
            .iter()
            .map(|p| Plane { normal: p.normal, dist: p.dist, kind: u8::try_from(p.kind).unwrap_or(5) })
            .collect();
        let leaf_contents = |leaf: i32| bsp.leafs.get(leaf as usize).map_or(-2, |l| l.contents);
        let hull0 = bsp
            .nodes
            .iter()
            .map(|n| {
                let child = |c: i16| -> i16 {
                    if c >= 0 { c } else { leaf_contents(-1 - i32::from(c)) as i16 }
                };
                ClipNode { plane: n.plane as i32, children: [child(n.children[0]), child(n.children[1])] }
            })
            .collect();
        let clipnodes = bsp.clipnodes.iter().map(|c| ClipNode { plane: c.plane, children: c.children }).collect();
        let (num_nodes, num_clip) = (bsp.nodes.len() as i32, bsp.clipnodes.len() as i32);
        let models = bsp
            .models
            .iter()
            .map(|m| BrushModel {
                hulls: [0, 1, 2, 3].map(|j| HullDef {
                    first: m.head_nodes[j],
                    last: if j == 0 { num_nodes - 1 } else { num_clip - 1 },
                }),
                mins: m.mins.map(|v| v - 1.0),
                maxs: m.maxs.map(|v| v + 1.0),
                head_node: m.head_nodes[0],
            })
            .collect();
        let nodes = bsp
            .nodes
            .iter()
            .map(|n| SurfNode {
                plane: n.plane as usize,
                children: n.children.map(i32::from),
                first_face: usize::from(n.first_face),
                num_faces: usize::from(n.num_faces),
            })
            .collect();
        let textures: Vec<String> = bsp.textures.iter().map(|t| t.as_ref().map(|t| t.name.clone()).unwrap_or_default()).collect();
        let faces = bsp
            .faces
            .iter()
            .map(|f| {
                let ext = bsp.surface_extents(f);
                let ti = bsp.texinfo.get(usize::from(f.texinfo));
                let texture = ti.map(|t| t.miptex as usize).filter(|&t| t < textures.len());
                let name = texture.map_or("", |t| textures[t].as_str());
                let (tiled, sky) = surface_flags(name, ti.is_some_and(|t| t.flags & TEX_SPECIAL != 0));
                SurfFace {
                    vecs: ti.map_or([[0.0; 4]; 2], |t| t.vecs),
                    texture_mins: ext.texture_mins,
                    extents: ext.extents,
                    texture,
                    tiled,
                    sky,
                    styles: f.styles,
                    light_offset: f.light_offset,
                }
            })
            .collect();
        let leafs = bsp.leafs.iter().map(|l| VisLeaf { contents: l.contents, vis_offset: l.vis_offset }).collect();
        let vis_row = bsp.models.first().map_or(0, |m| (m.vis_leafs.max(0) as usize).div_ceil(8));
        Self {
            planes,
            hull0,
            clipnodes,
            models,
            nodes,
            faces,
            textures,
            lighting: bsp.lighting.clone(),
            leafs,
            visibility: bsp.visibility.clone(),
            vis_row,
        }
    }

    /// Collision-only world (no render surfaces): `models[i] = (head nodes of hulls 0..3, mins,
    /// maxs)`. Hull 0 uses `hull0`, hulls 1..3 use `clipnodes`.
    pub fn from_clip_data(
        planes: Vec<Plane>,
        hull0: Vec<ClipNode>,
        clipnodes: Vec<ClipNode>,
        models: Vec<([i32; 4], [f32; 3], [f32; 3])>,
    ) -> Self {
        let (num_nodes, num_clip) = (hull0.len() as i32, clipnodes.len() as i32);
        let models = models
            .into_iter()
            .map(|(heads, mins, maxs)| BrushModel {
                hulls: [0, 1, 2, 3].map(|j| HullDef { first: heads[j], last: if j == 0 { num_nodes - 1 } else { num_clip - 1 } }),
                mins,
                maxs,
                head_node: heads[0],
            })
            .collect();
        Self {
            planes,
            hull0,
            clipnodes,
            models,
            nodes: Vec::new(),
            faces: Vec::new(),
            textures: Vec::new(),
            lighting: Vec::new(),
            leafs: Vec::new(),
            visibility: Vec::new(),
            vis_row: 0,
        }
    }

    /// BSP hull `index` (0 point, 1 stand, 2 large, 3 duck) of brush model `model`.
    pub fn hull(&self, model: usize, index: usize) -> Option<Hull<'_>> {
        let m = self.models.get(model)?;
        let def = m.hulls.get(index)?;
        let (mins, maxs) = HULL_SIZES[index];
        Some(Hull {
            clipnodes: if index == 0 { &self.hull0 } else { &self.clipnodes },
            planes: &self.planes,
            first: def.first,
            last: def.last,
            clip_mins: mins,
            clip_maxs: maxs,
        })
    }

    /// Texture name of face `face` (`""` when missing).
    pub fn face_texture(&self, face: usize) -> &str {
        self.faces.get(face).and_then(|f| f.texture).map_or("", |t| self.textures[t].as_str())
    }

    pub fn face_is_sky(&self, face: usize) -> bool {
        self.faces.get(face).is_some_and(|f| f.sky)
    }

    /// `Mod_PointInLeaf`: render leaf index containing `p` (0 without render nodes).
    pub fn point_leaf(&self, p: [f32; 3]) -> usize {
        let mut node = self.models.first().map_or(0, |m| m.head_node);
        while let Some(n) = usize::try_from(node).ok().and_then(|n| self.nodes.get(n)) {
            let Some(plane) = self.planes.get(n.plane) else { return 0 };
            let d = (plane.normal[0] * p[0] + plane.normal[1] * p[1]) + plane.normal[2] * p[2] - plane.dist;
            node = n.children[usize::from(d <= 0.0)];
        }
        usize::try_from(-1 - node).unwrap_or(0)
    }

    /// `Mod_LeafPVS`: decompressed visibility row of `leaf`; leaf 0 (outside) and maps without
    /// visibility see everything.
    pub fn leaf_pvs(&self, leaf: usize) -> Vec<u8> {
        let offset = self.leafs.get(leaf).filter(|_| leaf != 0).and_then(|l| usize::try_from(l.vis_offset).ok());
        match offset {
            Some(o) if o < self.visibility.len() => decompress_vis(&self.visibility[o..], self.vis_row),
            _ => vec![0xFF; self.vis_row],
        }
    }

    /// `PVSNode(world nodes, mins, maxs) != NULL`: some non-solid leaf touched by the box is in
    /// `pvs` (a row from [`Self::leaf_pvs`]).
    pub fn box_in_pvs(&self, pvs: &[u8], mins: [f32; 3], maxs: [f32; 3]) -> bool {
        let head = self.models.first().map_or(0, |m| m.head_node);
        self.pvs_node(pvs, head, mins, maxs)
    }

    fn pvs_node(&self, pvs: &[u8], node: i32, mins: [f32; 3], maxs: [f32; 3]) -> bool {
        let Ok(n) = usize::try_from(node) else {
            let leaf = usize::try_from(-1 - node).unwrap_or(0);
            let Some(l) = self.leafs.get(leaf) else { return false };
            let bit = leaf.wrapping_sub(1);
            let visible = leaf != 0 && pvs.get(bit >> 3).is_some_and(|b| b & (1 << (bit & 7)) != 0);
            return visible && l.contents != CONTENTS_SOLID;
        };
        let Some(n) = self.nodes.get(n) else { return false };
        let Some(plane) = self.planes.get(n.plane) else { return false };
        let sides = plane.box_on_side(mins, maxs);
        (sides & 1 != 0 && self.pvs_node(pvs, n.children[0], mins, maxs))
            || (sides & 2 != 0 && self.pvs_node(pvs, n.children[1], mins, maxs))
    }

    fn split(&self, node: i32, start: [f32; 3], end: [f32; 3]) -> Option<Split> {
        let n = usize::try_from(node).ok()?;
        let node_ref = self.nodes.get(n)?;
        let plane = self.planes.get(node_ref.plane)?;
        let dot = |p: [f32; 3]| (plane.normal[0] * p[0] + plane.normal[1] * p[1]) + plane.normal[2] * p[2] - plane.dist;
        let (front, back) = (dot(start), dot(end));
        let side = usize::from(front < 0.0);
        if (back < 0.0) == (front < 0.0) {
            return Some(Split::Child(node_ref.children[side]));
        }
        let frac = front / (front - back);
        let mid = [0, 1, 2].map(|i| (end[i] - start[i]) * frac + start[i]);
        Some(Split::Cross { node: n, mid, side })
    }

    /// Texel coordinates relative to the face's texture mins when `p` is within its extents.
    fn face_coords(&self, face: usize, p: [f32; 3]) -> Option<(i32, i32)> {
        let f = self.faces.get(face)?;
        let ds = ((p[1] * f.vecs[0][1] + p[0] * f.vecs[0][0]) + p[2] * f.vecs[0][2] + f.vecs[0][3]) as i32;
        let dt = ((p[1] * f.vecs[1][1] + p[0] * f.vecs[1][0]) + p[2] * f.vecs[1][2] + f.vecs[1][3]) as i32;
        if ds < f.texture_mins[0] || dt < f.texture_mins[1] {
            return None;
        }
        // A hostile map's extreme vertices or texinfo would overflow the subtraction.
        let (ds, dt) = (ds.checked_sub(f.texture_mins[0])?, dt.checked_sub(f.texture_mins[1])?);
        (ds <= f.extents[0] && dt <= f.extents[1]).then_some((ds, dt))
    }

    /// `SurfaceAtPoint(model, node, start, end)`: first face the segment crosses within its
    /// texture extents, walking from render node `node`.
    pub fn surface_at(&self, node: i32, start: [f32; 3], end: [f32; 3]) -> Option<usize> {
        match self.split(node, start, end)? {
            Split::Child(child) => self.surface_at(child, start, end),
            Split::Cross { node, mid, side } => {
                let n = self.nodes[node];
                if let Some(f) = self.surface_at(n.children[side], start, mid) {
                    return Some(f);
                }
                if let Some(f) = (n.first_face..n.first_face + n.num_faces).find(|&f| self.face_coords(f, mid).is_some()) {
                    return Some(f);
                }
                self.surface_at(n.children[side ^ 1], mid, end)
            }
        }
    }

    /// `R_LightVec(start, end)` from the world's first node: the lightmap colour where the segment
    /// first hits a lit surface (`(255,255,255)` without light data, 0 on a miss).
    pub fn light_vec(&self, styles: &[i32; MAX_LIGHTSTYLES], start: [f32; 3], end: [f32; 3]) -> [i32; 3] {
        if self.lighting.is_empty() {
            return [255; 3];
        }
        let head = self.models.first().map_or(0, |m| m.head_node);
        match self.light_walk(styles, head, start, end) {
            Walk::Hit(c) => c.map(|v| v.min(255)),
            Walk::Miss | Walk::Dark => [0; 3],
        }
    }

    fn light_walk(&self, styles: &[i32; MAX_LIGHTSTYLES], node: i32, start: [f32; 3], end: [f32; 3]) -> Walk {
        let Some(split) = self.split(node, start, end) else { return Walk::Miss };
        let (node, mid, side) = match split {
            Split::Child(child) => return self.light_walk(styles, child, start, end),
            Split::Cross { node, mid, side } => (node, mid, side),
        };
        let n = self.nodes[node];
        if let hit @ Walk::Hit(_) = self.light_walk(styles, n.children[side], start, mid) {
            return hit;
        }
        for fi in n.first_face..n.first_face + n.num_faces {
            let Some(f) = self.faces.get(fi) else { continue };
            if f.tiled {
                continue;
            }
            let Some((ds, dt)) = self.face_coords(fi, mid) else { continue };
            let Ok(offset) = usize::try_from(f.light_offset) else { return Walk::Dark };
            let width = (f.extents[0] >> 4) as usize + 1;
            let size = width * ((f.extents[1] >> 4) as usize + 1);
            let mut at = offset + 3 * ((dt >> 4) as usize * width + (ds >> 4) as usize);
            let mut c = [0i32; 3];
            for &style in f.styles.iter().take_while(|&&s| s != 255) {
                let scale = styles[usize::from(style) % MAX_LIGHTSTYLES];
                for (k, v) in c.iter_mut().enumerate() {
                    *v += scale * i32::from(self.lighting.get(at + k).copied().unwrap_or(0));
                }
                at += size * 3;
            }
            let mut c = c.map(|v| v >> 8);
            if c[0] == 0 {
                c[0] = 1;
            }
            return Walk::Hit(c);
        }
        self.light_walk(styles, n.children[side ^ 1], mid, end)
    }
}

enum Split {
    /// Both endpoints on one side: continue in this child.
    Child(i32),
    /// The segment crosses `node`'s plane at `mid`; `side` is the start's side.
    Cross { node: usize, mid: [f32; 3], side: usize },
}
