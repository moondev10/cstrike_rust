//! WAD3 texture archives (`halflife.wad`, `cstrike.wad`, `decals.wad`, `gfx.wad`, ...).

use crate::FormatError;
use crate::miptex::MipTex;
use crate::reader::Reader;

const WHAT: &str = "wad";

pub mod lump_type {
    pub const QPIC: u8 = 0x42;
    pub const MIPTEX: u8 = 0x43;
    pub const FONT: u8 = 0x46;
}

#[derive(Debug, Clone)]
pub struct LumpInfo {
    pub name: String,
    pub kind: u8,
    pub compression: u8,
    offset: usize,
    len: usize,
}

#[derive(Debug, Clone)]
pub struct Wad {
    data: Vec<u8>,
    lumps: Vec<LumpInfo>,
}

impl Wad {
    pub fn parse(data: Vec<u8>) -> Result<Self, FormatError> {
        let mut r = Reader::new(&data, WHAT);
        let magic = r.bytes(4)?;
        if magic != b"WAD3" && magic != b"WAD2" {
            return Err(FormatError::BadHeader { what: WHAT, found: String::from_utf8_lossy(magic).into_owned() });
        }
        let count = r.i32()?;
        let table = r.i32()?;
        let (Ok(count), Ok(table)) = (usize::try_from(count), usize::try_from(table)) else {
            return Err(FormatError::Invalid { what: WHAT, detail: format!("bad directory {count}@{table}") });
        };
        let mut r = Reader::at(&data, table, WHAT)?;
        let mut lumps = Vec::with_capacity(count);
        for _ in 0..count {
            let offset = r.i32()?;
            let disk_size = r.i32()?;
            let _size = r.i32()?;
            let kind = r.u8()?;
            let compression = r.u8()?;
            r.bytes(2)?;
            let name = r.name(16)?;
            let (Ok(offset), Ok(len)) = (usize::try_from(offset), usize::try_from(disk_size)) else {
                return Err(FormatError::Invalid { what: WHAT, detail: format!("lump {name}: negative range") });
            };
            Reader::at(&data, offset, WHAT)?.bytes(len)?;
            lumps.push(LumpInfo { name, kind, compression, offset, len });
        }
        Ok(Self { data, lumps })
    }

    pub fn lumps(&self) -> &[LumpInfo] {
        &self.lumps
    }

    pub fn find(&self, name: &str) -> Option<&LumpInfo> {
        self.lumps.iter().find(|l| l.name.eq_ignore_ascii_case(name))
    }

    pub fn lump_data(&self, lump: &LumpInfo) -> &[u8] {
        &self.data[lump.offset..lump.offset + lump.len]
    }

    /// Texture by name; `None` if absent or not a miptex lump.
    pub fn miptex(&self, name: &str) -> Option<Result<MipTex, FormatError>> {
        let lump = self.find(name).filter(|l| l.kind == lump_type::MIPTEX)?;
        if lump.compression != 0 {
            return Some(Err(FormatError::Invalid { what: WHAT, detail: format!("{name}: compressed lump") }));
        }
        Some(MipTex::parse(self.lump_data(lump)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_is_case_insensitive_and_type_checked() {
        let mut data = b"WAD3".to_vec();
        data.extend_from_slice(&1i32.to_le_bytes());
        data.extend_from_slice(&12i32.to_le_bytes());
        data.extend_from_slice(&44i32.to_le_bytes());
        data.extend_from_slice(&0i32.to_le_bytes());
        data.extend_from_slice(&0i32.to_le_bytes());
        data.extend_from_slice(&[lump_type::QPIC, 0, 0, 0]);
        let mut name = [0u8; 16];
        name[..7].copy_from_slice(b"CONBACK");
        data.extend_from_slice(&name);
        let wad = Wad::parse(data).ok();
        let wad = wad.as_ref();
        assert!(wad.and_then(|w| w.find("conback")).is_some());
        assert!(wad.and_then(|w| w.miptex("conback")).is_none(), "qpic is not a miptex");
    }

    #[test]
    fn lump_past_end_is_rejected() {
        let mut data = b"WAD3".to_vec();
        data.extend_from_slice(&1i32.to_le_bytes());
        data.extend_from_slice(&12i32.to_le_bytes());
        data.extend_from_slice(&40i32.to_le_bytes());
        data.extend_from_slice(&100i32.to_le_bytes());
        data.extend_from_slice(&[0u8; 24]);
        assert!(matches!(Wad::parse(data), Err(FormatError::Truncated { .. })));
    }
}
