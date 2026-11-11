//! Asset loading: resolves game paths through [`gs_fs::FileSystem`] and turns raw formats into
//! self-contained CPU data (decoded textures, parsed maps) that the renderer and simulation consume.

pub mod image;
pub mod map;
pub mod studio;

pub use image::RgbaImage;
pub use map::{MapAssets, SkyBox};
pub use studio::load_studio;

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error(transparent)]
    Fs(#[from] gs_fs::FsError),
    #[error("{path}: {source}")]
    Format {
        path: String,
        #[source]
        source: gs_formats::FormatError,
    },
    #[error("{path}: {detail}")]
    Decode { path: String, detail: String },
}
