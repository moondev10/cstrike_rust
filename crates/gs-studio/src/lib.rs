//! GoldSrc studio models (MDL v10): parsing, animation decoding and bone setup, following the
//! original renderer (docs/re/render.md Part B). Rendering itself lives in `gs-render`.

pub mod anim;
pub mod model;
pub mod pose;

pub use model::StudioModel;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StudioError {
    #[error("studio model: unexpected end of data at {0}")]
    Truncated(i64),
    #[error("studio model: {0}")]
    BadHeader(String),
}
