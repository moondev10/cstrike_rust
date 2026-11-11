//! Bounds-checked little-endian reader shared by all format parsers.

use crate::FormatError;

#[derive(Clone)]
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    what: &'static str,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8], what: &'static str) -> Self {
        Self { data, pos: 0, what }
    }

    pub(crate) fn at(data: &'a [u8], offset: usize, what: &'static str) -> Result<Self, FormatError> {
        if offset > data.len() {
            return Err(FormatError::Truncated { what, offset });
        }
        Ok(Self { data, pos: offset, what })
    }

    pub(crate) fn bytes(&mut self, len: usize) -> Result<&'a [u8], FormatError> {
        let end = self.pos.checked_add(len).filter(|&e| e <= self.data.len());
        let Some(end) = end else {
            return Err(FormatError::Truncated { what: self.what, offset: self.pos });
        };
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], FormatError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.array::<1>()?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, FormatError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub(crate) fn i16(&mut self) -> Result<i16, FormatError> {
        Ok(i16::from_le_bytes(self.array()?))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, FormatError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub(crate) fn i32(&mut self) -> Result<i32, FormatError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    pub(crate) fn f32(&mut self) -> Result<f32, FormatError> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    pub(crate) fn vec3(&mut self) -> Result<[f32; 3], FormatError> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }

    pub(crate) fn i16x3(&mut self) -> Result<[i16; 3], FormatError> {
        Ok([self.i16()?, self.i16()?, self.i16()?])
    }

    /// Fixed-size, NUL-padded name field (texture/lump names). Bytes are Latin-1.
    pub(crate) fn name(&mut self, len: usize) -> Result<String, FormatError> {
        let raw = self.bytes(len)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        Ok(raw[..end].iter().map(|&b| char::from(b)).collect())
    }
}

/// Parses `data` as a packed array of fixed-size records.
pub(crate) fn records<'a, T>(
    data: &'a [u8],
    size: usize,
    what: &'static str,
    mut parse: impl FnMut(&mut Reader<'a>) -> Result<T, FormatError>,
) -> Result<Vec<T>, FormatError> {
    if !data.len().is_multiple_of(size) {
        return Err(FormatError::Invalid { what, detail: format!("length {} is not a multiple of {size}", data.len()) });
    }
    let mut reader = Reader::new(data, what);
    (0..data.len() / size).map(|_| parse(&mut reader)).collect()
}
