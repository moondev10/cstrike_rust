//! Spatial queries on a loaded BSP map: clip hulls and hull traces (movement, docs/re/movement.md
//! Part B) and render-node surface walks (lighting, texture lookups, docs/re/render.md §B.10.2).

pub mod hull;
mod world;

pub use hull::{BoxHull, ClipNode, Hull, Plane, Trace, Variant};
pub use world::{BrushModel, World};

/// `CONTENTS_*`.
pub mod contents {
    pub const EMPTY: i32 = -1;
    pub const SOLID: i32 = -2;
    pub const WATER: i32 = -3;
    pub const SLIME: i32 = -4;
    pub const LAVA: i32 = -5;
    pub const CURRENT_0: i32 = -9;
    pub const CURRENT_DOWN: i32 = -14;
    pub const TRANSLUCENT: i32 = -15;
    pub const LADDER: i32 = -16;
}
