//! Hand-built BSP fixtures for unit tests.

use gs_formats::bsp::{Bsp, Face, Model, Node, Plane, TexInfo};
use gs_formats::miptex::MipTex;

/// Lighting sample value of every luxel of [`floor`].
pub const FLOOR_LIGHT: u8 = 64;

/// A 128×128 floor at z = 0 (one face, one node) with a 64×64 texture whose axes are world
/// x/y, lit by one style-0 layer of 9×9 luxels all [`FLOOR_LIGHT`].
pub fn floor() -> Bsp {
    Bsp {
        entities: String::new(),
        planes: vec![Plane { normal: [0.0, 0.0, 1.0], dist: 0.0, kind: 2 }],
        textures: vec![Some(MipTex { name: "floor".into(), width: 64, height: 64, data: None })],
        vertices: vec![[0.0, 0.0, 0.0], [128.0, 0.0, 0.0], [128.0, 128.0, 0.0], [0.0, 128.0, 0.0]],
        visibility: Vec::new(),
        nodes: vec![Node { plane: 0, children: [-1, -1], mins: [0; 3], maxs: [128, 128, 0], first_face: 0, num_faces: 1 }],
        texinfo: vec![TexInfo { vecs: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]], miptex: 0, flags: 0 }],
        faces: vec![Face { plane: 0, side: 0, first_edge: 0, num_edges: 4, texinfo: 0, styles: [0, 255, 255, 255], light_offset: 0 }],
        lighting: vec![FLOOR_LIGHT; 9 * 9 * 3],
        clipnodes: Vec::new(),
        leafs: Vec::new(),
        mark_surfaces: Vec::new(),
        edges: vec![[0, 0], [0, 1], [1, 2], [2, 3], [3, 0]],
        surfedges: vec![1, 2, 3, 4],
        models: vec![Model {
            mins: [0.0; 3],
            maxs: [128.0, 128.0, 0.0],
            origin: [0.0; 3],
            head_nodes: [0; 4],
            vis_leafs: 0,
            first_face: 0,
            num_faces: 1,
        }],
    }
}
