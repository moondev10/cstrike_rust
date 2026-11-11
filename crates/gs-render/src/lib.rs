//! wgpu renderer for GoldSrc content.
//!
//! The renderer draws into any `Rgba8Unorm`/`Bgra8Unorm` color target. It does no color-space
//! conversion: like the original GL renderer, all math happens on gamma-encoded values.

pub mod camera;
pub mod capture;
pub mod decal;
pub mod gamma;
pub mod gpu;
pub mod lightmap;
mod overlay;
pub mod present;
mod renderer;
pub mod sprite;
pub mod studio;
pub mod texture;
pub mod world;
#[cfg(test)]
mod test_bsp;

pub use camera::{Fog, View};
pub use gpu::Gpu;
pub use lightmap::DynamicLight;
pub use renderer::{BrushInstance, EffectBatch, EffectBlend, RenderMode, Renderer, Scene, TextureMode};
pub use sprite::{SpriteDraw, SpriteHandle};
pub use studio::{StudioDraw, StudioHandle, StudioLight};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("GPU initialization: {0}")]
    Init(String),
    #[error("lightmap atlas is full")]
    AtlasFull,
    #[error("skybox {0}: faces differ in size or exceed the texture size limit")]
    SkySize(String),
    #[error("screen capture: {0}")]
    Capture(String),
}
