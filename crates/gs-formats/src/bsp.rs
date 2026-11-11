//! BSP version 30 (GoldSrc) maps.

use crate::FormatError;
use crate::miptex::MipTex;
use crate::reader::{Reader, records};

const WHAT: &str = "bsp";
pub const VERSION: i32 = 30;
pub const MAX_MAP_HULLS: usize = 4;

/// Lump indices in the header directory.
mod lump {
    pub const ENTITIES: usize = 0;
    pub const PLANES: usize = 1;
    pub const TEXTURES: usize = 2;
    pub const VERTEXES: usize = 3;
    pub const VISIBILITY: usize = 4;
    pub const NODES: usize = 5;
    pub const TEXINFO: usize = 6;
    pub const FACES: usize = 7;
    pub const LIGHTING: usize = 8;
    pub const CLIPNODES: usize = 9;
    pub const LEAFS: usize = 10;
    pub const MARKSURFACES: usize = 11;
    pub const EDGES: usize = 12;
    pub const SURFEDGES: usize = 13;
    pub const MODELS: usize = 14;
    pub const COUNT: usize = 15;
}

/// Leaf / point contents.
pub mod contents {
    pub const EMPTY: i32 = -1;
    pub const SOLID: i32 = -2;
    pub const WATER: i32 = -3;
    pub const SLIME: i32 = -4;
    pub const LAVA: i32 = -5;
    pub const SKY: i32 = -6;
}

/// `texinfo.flags`: surface is not lightmapped (sky, water).
pub const TEX_SPECIAL: u32 = 1;

#[derive(Debug, Clone, Copy)]
pub struct Plane {
    pub normal: [f32; 3],
    pub dist: f32,
    /// 0..2 axial (x, y, z), 3..5 nearest axis.
    pub kind: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct Node {
    pub plane: u32,
    /// `>= 0`: node index; `< 0`: leaf `-(child + 1)`.
    pub children: [i16; 2],
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub first_face: u16,
    pub num_faces: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct TexInfo {
    /// `[s; t]`, each `xyz + offset`.
    pub vecs: [[f32; 4]; 2],
    pub miptex: u32,
    pub flags: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Face {
    pub plane: u16,
    pub side: u16,
    pub first_edge: u32,
    pub num_edges: u16,
    pub texinfo: u16,
    /// Light style per lightmap layer; 255 terminates.
    pub styles: [u8; 4],
    /// Byte offset into the lighting lump, `-1` = unlit.
    pub light_offset: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct ClipNode {
    pub plane: i32,
    /// `>= 0`: clipnode index; `< 0`: contents.
    pub children: [i16; 2],
}

#[derive(Debug, Clone, Copy)]
pub struct Leaf {
    pub contents: i32,
    /// Offset into the visibility lump, `-1` = no vis data (see everything).
    pub vis_offset: i32,
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub first_mark_surface: u16,
    pub num_mark_surfaces: u16,
    pub ambient_level: [u8; 4],
}

#[derive(Debug, Clone, Copy)]
pub struct Model {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub origin: [f32; 3],
    /// Hull 0 head node is a BSP node; hulls 1..3 are clipnode indices.
    pub head_nodes: [i32; MAX_MAP_HULLS],
    pub vis_leafs: i32,
    pub first_face: i32,
    pub num_faces: i32,
}

#[derive(Debug, Clone)]
pub struct Bsp {
    pub entities: String,
    pub planes: Vec<Plane>,
    /// `None` for directory entries marked missing (offset -1).
    pub textures: Vec<Option<MipTex>>,
    pub vertices: Vec<[f32; 3]>,
    pub visibility: Vec<u8>,
    pub nodes: Vec<Node>,
    pub texinfo: Vec<TexInfo>,
    pub faces: Vec<Face>,
    /// RGB8 luxels.
    pub lighting: Vec<u8>,
    pub clipnodes: Vec<ClipNode>,
    pub leafs: Vec<Leaf>,
    pub mark_surfaces: Vec<u16>,
    pub edges: Vec<[u16; 2]>,
    pub surfedges: Vec<i32>,
    pub models: Vec<Model>,
}

impl Bsp {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data, WHAT);
        let version = r.i32()?;
        if version != VERSION {
            return Err(FormatError::BadHeader { what: WHAT, found: format!("version {version}") });
        }
        let mut lumps = [&data[0..0]; lump::COUNT];
        for slot in &mut lumps {
            let offset = r.i32()?;
            let len = r.i32()?;
            let (Ok(offset), Ok(len)) = (usize::try_from(offset), usize::try_from(len)) else {
                return Err(FormatError::Invalid { what: WHAT, detail: format!("negative lump {offset}+{len}") });
            };
            *slot = Reader::at(data, offset, WHAT)?.bytes(len)?;
        }

        let entities_raw = lumps[lump::ENTITIES];
        let end = entities_raw.iter().position(|&b| b == 0).unwrap_or(entities_raw.len());
        let entities = entities_raw[..end].iter().map(|&b| char::from(b)).collect();

        let bsp = Self {
            entities,
            planes: records(lumps[lump::PLANES], 20, "bsp planes", |r| {
                Ok(Plane { normal: r.vec3()?, dist: r.f32()?, kind: r.i32()? })
            })?,
            textures: parse_textures(lumps[lump::TEXTURES])?,
            vertices: records(lumps[lump::VERTEXES], 12, "bsp vertices", Reader::vec3)?,
            visibility: lumps[lump::VISIBILITY].to_vec(),
            nodes: records(lumps[lump::NODES], 24, "bsp nodes", |r| {
                Ok(Node {
                    plane: r.u32()?,
                    children: [r.i16()?, r.i16()?],
                    mins: r.i16x3()?,
                    maxs: r.i16x3()?,
                    first_face: r.u16()?,
                    num_faces: r.u16()?,
                })
            })?,
            texinfo: records(lumps[lump::TEXINFO], 40, "bsp texinfo", |r| {
                let s = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
                let t = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
                Ok(TexInfo { vecs: [s, t], miptex: r.u32()?, flags: r.u32()? })
            })?,
            faces: records(lumps[lump::FACES], 20, "bsp faces", |r| {
                Ok(Face {
                    plane: r.u16()?,
                    side: r.u16()?,
                    first_edge: r.u32()?,
                    num_edges: r.u16()?,
                    texinfo: r.u16()?,
                    styles: [r.u8()?, r.u8()?, r.u8()?, r.u8()?],
                    light_offset: r.i32()?,
                })
            })?,
            lighting: lumps[lump::LIGHTING].to_vec(),
            clipnodes: records(lumps[lump::CLIPNODES], 8, "bsp clipnodes", |r| {
                Ok(ClipNode { plane: r.i32()?, children: [r.i16()?, r.i16()?] })
            })?,
            leafs: records(lumps[lump::LEAFS], 28, "bsp leafs", |r| {
                Ok(Leaf {
                    contents: r.i32()?,
                    vis_offset: r.i32()?,
                    mins: r.i16x3()?,
                    maxs: r.i16x3()?,
                    first_mark_surface: r.u16()?,
                    num_mark_surfaces: r.u16()?,
                    ambient_level: [r.u8()?, r.u8()?, r.u8()?, r.u8()?],
                })
            })?,
            mark_surfaces: records(lumps[lump::MARKSURFACES], 2, "bsp marksurfaces", Reader::u16)?,
            edges: records(lumps[lump::EDGES], 4, "bsp edges", |r| Ok([r.u16()?, r.u16()?]))?,
            surfedges: records(lumps[lump::SURFEDGES], 4, "bsp surfedges", Reader::i32)?,
            models: records(lumps[lump::MODELS], 64, "bsp models", |r| {
                Ok(Model {
                    mins: r.vec3()?,
                    maxs: r.vec3()?,
                    origin: r.vec3()?,
                    head_nodes: [r.i32()?, r.i32()?, r.i32()?, r.i32()?],
                    vis_leafs: r.i32()?,
                    first_face: r.i32()?,
                    num_faces: r.i32()?,
                })
            })?,
        };
        bsp.validate()?;
        Ok(bsp)
    }

    /// Cross-references are checked once here so consumers may index without re-validating.
    fn validate(&self) -> Result<(), FormatError> {
        let bad = |detail: String| Err(FormatError::Invalid { what: WHAT, detail });
        for (i, e) in self.edges.iter().enumerate() {
            if e.iter().any(|&v| usize::from(v) >= self.vertices.len()) {
                return bad(format!("edge {i} references missing vertex"));
            }
        }
        for (i, &se) in self.surfedges.iter().enumerate() {
            if se.unsigned_abs() as usize >= self.edges.len() {
                return bad(format!("surfedge {i} references missing edge"));
            }
        }
        for (i, f) in self.faces.iter().enumerate() {
            let end = f.first_edge as usize + usize::from(f.num_edges);
            if f.num_edges < 3 || end > self.surfedges.len() {
                return bad(format!("face {i} edge range {}+{}", f.first_edge, f.num_edges));
            }
            if usize::from(f.texinfo) >= self.texinfo.len() || usize::from(f.plane) >= self.planes.len() {
                return bad(format!("face {i} references missing texinfo/plane"));
            }
        }
        for (i, t) in self.texinfo.iter().enumerate() {
            if t.miptex as usize >= self.textures.len() {
                return bad(format!("texinfo {i} references missing miptex {}", t.miptex));
            }
        }
        for (i, m) in self.models.iter().enumerate() {
            let (Ok(first), Ok(count)) = (usize::try_from(m.first_face), usize::try_from(m.num_faces)) else {
                return bad(format!("model {i} negative face range"));
            };
            if first + count > self.faces.len() {
                return bad(format!("model {i} face range {first}+{count}"));
            }
        }
        for (i, &ms) in self.mark_surfaces.iter().enumerate() {
            if usize::from(ms) >= self.faces.len() {
                return bad(format!("marksurface {i} references missing face"));
            }
        }
        for (i, n) in self.nodes.iter().enumerate() {
            if n.plane as usize >= self.planes.len() {
                return bad(format!("node {i} references missing plane"));
            }
        }
        // Hull 0 puts a leaf's contents where a node child goes (`Mod_MakeHull0`), as an i16:
        // only a negative one ends the walk, anything else would be followed as a node index.
        for (i, l) in self.leafs.iter().enumerate() {
            if !(i32::from(i16::MIN)..0).contains(&l.contents) {
                return bad(format!("leaf {i} contents {}", l.contents));
            }
        }
        // The tree walks (point leaf, hull traces, light and decal walks) follow children
        // blindly: a child leading back to an ancestor would loop or recurse forever, and a
        // degenerate chain would exhaust the stack.
        for (what, depth) in [("node", tree_depth(self.nodes.len(), |i| self.nodes[i].children)), ("clipnode", tree_depth(self.clipnodes.len(), |i| self.clipnodes[i].children))] {
            match depth {
                None => return bad(format!("{what} tree has a cycle")),
                Some(d) if d > MAX_TREE_DEPTH => return bad(format!("{what} tree {d} deep")),
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// Polygon vertices of a face in winding order.
    pub fn face_vertices(&self, face: &Face) -> impl Iterator<Item = [f32; 3]> + '_ {
        let range = face.first_edge as usize..face.first_edge as usize + usize::from(face.num_edges);
        self.surfedges[range].iter().map(|&se| {
            let edge = self.edges[se.unsigned_abs() as usize];
            let v = if se >= 0 { edge[0] } else { edge[1] };
            self.vertices[usize::from(v)]
        })
    }

    /// Number of lightmap layers (styles) stored for a face.
    pub fn face_style_count(face: &Face) -> usize {
        face.styles.iter().take_while(|&&s| s != 255).count()
    }

    /// Lightmap rectangle of a face (`CalcSurfaceExtents`): each projection is evaluated in f64
    /// (y term first, like the engine) and rounded to f32; min/max are tracked in f32.
    pub fn surface_extents(&self, face: &Face) -> SurfaceExtents {
        let tex = &self.texinfo[usize::from(face.texinfo)];
        let mut mins = [999_999.0f32; 2];
        let mut maxs = [-99_999.0f32; 2];
        for v in self.face_vertices(face) {
            for axis in 0..2 {
                let vec = tex.vecs[axis];
                let val = (f64::from(vec[1]) * f64::from(v[1])
                    + f64::from(vec[0]) * f64::from(v[0])
                    + f64::from(vec[2]) * f64::from(v[2])
                    + f64::from(vec[3])) as f32;
                if val < mins[axis] {
                    mins[axis] = val;
                }
                if val > maxs[axis] {
                    maxs[axis] = val;
                }
            }
        }
        let mut out = SurfaceExtents { texture_mins: [0; 2], extents: [0; 2] };
        for axis in 0..2 {
            let bmin = f64::from(mins[axis] * 0.0625).floor();
            let bmax = f64::from(maxs[axis] * 0.0625).ceil();
            out.texture_mins[axis] = i32::from((bmin * 16.0) as i16);
            out.extents[axis] = i32::from(((bmax - bmin) * 16.0) as i16);
        }
        out
    }

    /// Decompressed PVS row for a leaf: one bit per leaf, leaf 1 at bit 0. Leafs without vis data see all.
    pub fn leaf_pvs(&self, leaf: usize) -> Vec<u8> {
        let row = self.models.first().map_or(0, |m| (m.vis_leafs.max(0) as usize).div_ceil(8));
        let vis_offset = self.leafs.get(leaf).map_or(-1, |l| l.vis_offset);
        match usize::try_from(vis_offset) {
            Ok(offset) if offset < self.visibility.len() => decompress_vis(&self.visibility[offset..], row),
            _ => vec![0xFF; row],
        }
    }
}

/// The deepest path a node tree may have. Compiled maps stay far below it; the recursive walks
/// over the tree must fit the stack.
const MAX_TREE_DEPTH: usize = 1024;

/// The longest root-to-leaf path through the graph whose node `i` has `children(i)` (a negative
/// or out-of-range child is a leaf); `None` when a child leads back to a node on the current
/// path. Shared subtrees are fine.
fn tree_depth(count: usize, children: impl Fn(usize) -> [i16; 2]) -> Option<usize> {
    const ON_PATH: usize = usize::MAX;
    let node = |c: i16| usize::try_from(c).ok().filter(|&c| c < count);
    // 0: not seen; ON_PATH: on the walk's stack; else 1 + the deepest child.
    let mut depth = vec![0usize; count];
    let mut deepest = 0;
    let mut stack: Vec<(usize, u8)> = Vec::new();
    for root in 0..count {
        if depth[root] != 0 {
            continue;
        }
        depth[root] = ON_PATH;
        stack.push((root, 0));
        while let Some(top) = stack.last_mut() {
            let (n, k) = *top;
            if k < 2 {
                top.1 += 1;
                if let Some(c) = node(children(n)[usize::from(k)]) {
                    match depth[c] {
                        0 => {
                            depth[c] = ON_PATH;
                            stack.push((c, 0));
                        }
                        ON_PATH => return None,
                        _ => {}
                    }
                }
            } else {
                let d = 1 + children(n).into_iter().filter_map(node).map(|c| depth[c]).max().unwrap_or(0);
                depth[n] = d;
                deepest = deepest.max(d);
                stack.pop();
            }
        }
    }
    Some(deepest)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceExtents {
    /// Texture-space origin of the lightmap, multiple of 16.
    pub texture_mins: [i32; 2],
    /// Texture-space size, multiple of 16.
    pub extents: [i32; 2],
}

impl SurfaceExtents {
    /// Lightmap size in luxels.
    pub fn lightmap_size(&self) -> (u32, u32) {
        ((self.extents[0] / 16 + 1) as u32, (self.extents[1] / 16 + 1) as u32)
    }
}

/// `CRC_MapFile`: CRC-32 over every lump except the entity lump, in lump order, **without** the
/// final inversion (the engine compares the raw register; verified against HLDS 10210). Servers
/// send this value in `svc_serverinfo` and expect it back in `spawn`.
pub fn map_crc(data: &[u8]) -> Result<u32, FormatError> {
    let mut r = Reader::new(data, WHAT);
    let version = r.i32()?;
    if version != VERSION && version != 29 {
        return Err(FormatError::BadHeader { what: WHAT, found: format!("version {version}") });
    }
    let mut crc = gs_common::crc::Crc32::default();
    for index in 0..lump::COUNT {
        let offset = r.i32()?;
        let len = r.i32()?;
        if index == lump::ENTITIES || len <= 0 {
            continue;
        }
        let offset = usize::try_from(offset)
            .map_err(|_| FormatError::Invalid { what: WHAT, detail: format!("lump {index} offset {offset}") })?;
        crc.update(Reader::at(data, offset, WHAT)?.bytes(len as usize)?);
    }
    Ok(!crc.finish())
}

/// Run-length decoding of a vis row: a zero byte is followed by a count of zero bytes.
pub fn decompress_vis(src: &[u8], row: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(row);
    let mut it = src.iter();
    while out.len() < row {
        let Some(&b) = it.next() else { break };
        if b != 0 {
            out.push(b);
            continue;
        }
        let run = it.next().map_or(0, |&c| usize::from(c));
        out.extend(std::iter::repeat_n(0, run.min(row - out.len())));
    }
    out.resize(row, 0);
    out
}

fn parse_textures(data: &[u8]) -> Result<Vec<Option<MipTex>>, FormatError> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mut r = Reader::new(data, "bsp textures");
    let count = usize::try_from(r.i32()?).map_err(|_| FormatError::Invalid {
        what: "bsp textures",
        detail: "negative texture count".to_owned(),
    })?;
    let offsets: Vec<i32> = (0..count).map(|_| r.i32()).collect::<Result<_, _>>()?;
    offsets
        .into_iter()
        .map(|offset| match usize::try_from(offset) {
            Ok(offset) => Reader::at(data, offset, "bsp textures").and_then(|_| MipTex::parse(&data[offset..])).map(Some),
            Err(_) => Ok(None),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vis_rle_expands_zero_runs_and_clamps_to_row() {
        assert_eq!(decompress_vis(&[0xFF, 0x00, 0x03, 0x81], 5), vec![0xFF, 0, 0, 0, 0x81]);
        assert_eq!(decompress_vis(&[0x00, 0xFF], 4), vec![0; 4], "run longer than the row is clamped");
        assert_eq!(decompress_vis(&[0x01], 3), vec![0x01, 0, 0], "short source is zero-padded");
    }

    #[test]
    fn rejects_other_versions() {
        let mut raw = vec![0u8; 4 + 15 * 8];
        raw[..4].copy_from_slice(&29i32.to_le_bytes());
        assert!(matches!(Bsp::parse(&raw), Err(FormatError::BadHeader { .. })));
    }

    /// A downloaded map whose node children lead back up the tree hung or overflowed the stack.
    #[test]
    fn tree_depth_rejects_cycles_and_measures_shared_subtrees() {
        let chain = [[1, -1], [2, -1], [-1, -2]];
        assert_eq!(tree_depth(3, |i| chain[i]), Some(3));
        let shared = [[1, 2], [2, -1], [-1, -1]];
        assert_eq!(tree_depth(3, |i| shared[i]), Some(3), "node 2 under both 0 and 1");
        let cycle = [[1, -1], [2, -1], [-1, 0]];
        assert_eq!(tree_depth(3, |i| cycle[i]), None);
        assert_eq!(tree_depth(1, |_| [0, -1]), None, "a node that is its own child");
        assert_eq!(tree_depth(1, |_| [7, -1]), Some(1), "out-of-range children end the walk");
    }
}
