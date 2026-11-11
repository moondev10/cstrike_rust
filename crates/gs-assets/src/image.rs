//! Decoded RGBA8 images.

use gs_fs::FileSystem;

use crate::AssetError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// Row-major RGBA8, top row first.
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    /// Loads a TGA/BMP/PNG through the game filesystem.
    pub fn load(fs: &FileSystem, path: &str) -> Result<Self, AssetError> {
        let bytes = fs.read(path)?;
        let decode_err = |e: ::image::ImageError| AssetError::Decode { path: path.to_owned(), detail: e.to_string() };
        // TGA has no magic number, so the format must come from the extension.
        let format = ::image::ImageFormat::from_path(path).map_err(decode_err)?;
        let decoded = ::image::load_from_memory_with_format(&bytes, format).map_err(decode_err)?.into_rgba8();
        Ok(Self { width: decoded.width(), height: decoded.height(), pixels: decoded.into_raw() })
    }
}
