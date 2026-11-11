//! Parsers for GoldSrc on-disk formats. All parsers are pure: bytes in, owned data out.

pub mod bsp;
pub mod entities;
pub mod keyvalues;
pub mod miptex;
mod reader;
pub mod sprite;
pub mod studio_bounds;
pub mod wad;

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("{what}: unexpected end of data at offset {offset}")]
    Truncated { what: &'static str, offset: usize },
    #[error("{what}: unsupported header {found}")]
    BadHeader { what: &'static str, found: String },
    #[error("{what}: {detail}")]
    Invalid { what: &'static str, detail: String },
}
