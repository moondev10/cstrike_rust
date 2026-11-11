//! `R_GetStudioBounds` / `R_StudioComputeBounds`: the model bounds the client reports for
//! `clc_fileconsistency` (docs/re/studio-bounds.md). Reproduces the engine bit for bit, including
//! its quirks (every bone decodes bone 0's animation stream; per-sequence boxes are re-centred on
//! the origin; part of the decoding happens in f64).

use crate::FormatError;

const WHAT: &str = "studio bounds";
const IDST: i32 = 0x5453_4449;
const BODYPART_SIZE: i64 = 76;
const MODEL_SIZE: i64 = 112;
const BONE_SIZE: i64 = 112;
const SEQ_SIZE: i64 = 176;

struct Buf<'a>(&'a [u8]);

impl Buf<'_> {
    fn bytes<const N: usize>(&self, off: i64) -> Result<[u8; N], FormatError> {
        let start = usize::try_from(off).map_err(|_| FormatError::Truncated { what: WHAT, offset: 0 })?;
        let slice = self.0.get(start..start + N).ok_or(FormatError::Truncated { what: WHAT, offset: start })?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    fn i32(&self, off: i64) -> Result<i32, FormatError> {
        Ok(i32::from_le_bytes(self.bytes(off)?))
    }

    fn f32(&self, off: i64) -> Result<f32, FormatError> {
        Ok(f32::from_le_bytes(self.bytes(off)?))
    }

    fn u16(&self, off: i64) -> Result<u16, FormatError> {
        Ok(u16::from_le_bytes(self.bytes(off)?))
    }

    fn i16(&self, off: i64) -> Result<i16, FormatError> {
        Ok(i16::from_le_bytes(self.bytes(off)?))
    }

    fn u8(&self, off: i64) -> Result<u8, FormatError> {
        Ok(self.bytes::<1>(off)?[0])
    }
}

#[derive(Default)]
struct BoundBox {
    min: [f32; 3],
    max: [f32; 3],
    count: i32,
}

impl BoundBox {
    fn add(&mut self, p: [f32; 3]) {
        if self.count == 0 {
            self.min = p;
            self.max = p;
        } else {
            for (i, &v) in p.iter().enumerate() {
                if self.min[i] > v {
                    self.min[i] = v;
                }
                if v > self.max[i] {
                    self.max[i] = v;
                }
            }
        }
        self.count = self.count.wrapping_add(1);
    }
}

/// `R_StudioCalcBonePosition` with `s = 0` and no controller adjustment.
fn bone_position(buf: &Buf, frame: i32, bone: i64, panim: i64) -> Result<[f32; 3], FormatError> {
    let mut pos = [0.0f32; 3];
    for (j, out) in pos.iter_mut().enumerate() {
        let j64 = j as i64;
        let p = buf.f32(bone + 64 + 4 * j64)?;
        let scale = buf.f32(bone + 88 + 4 * j64)?;
        *out = p;
        let off = buf.u16(panim + 2 * j64)?;
        if off == 0 {
            continue;
        }
        let mut v = panim + i64::from(off);
        let mut valid = i32::from(buf.u8(v)?);
        let mut total = i32::from(buf.u8(v + 1)?);
        let mut k = if total >= valid { frame } else { 0 };
        while total <= k {
            k -= total;
            v += 2 * i64::from(valid) + 2;
            valid = i32::from(buf.u8(v)?);
            total = i32::from(buf.u8(v + 1)?);
            if total < valid {
                k = 0;
            }
        }
        let value = |i: i32| buf.i16(v + 2 * i64::from(i));
        let float = |a: i16| p + f32::from(a) * scale;
        let interp = |a: i16, b: i16| {
            let t1 = f64::from(f32::from(b) * 0.0f32);
            let t2 = (1.0f64 - 0.0f64) * f64::from(a);
            (((t1 + t2) * f64::from(scale)) + f64::from(p)) as f32
        };
        *out = if valid > k {
            if valid > k + 1 { interp(value(k + 1)?, value(k + 2)?) } else { float(value(k + 1)?) }
        } else if total > k + 1 {
            float(value(valid)?)
        } else {
            interp(value(valid)?, value(valid + 2)?)
        };
    }
    Ok(pos)
}

/// `(mins, maxs)`.
pub type Bounds = ([f32; 3], [f32; 3]);

/// Bounds of a studio model file. `Ok(None)` when it is not an `IDST` model (the engine returns
/// 0); `Err` when the engine's unchecked reads would leave the file (its result is undefined).
pub fn studio_bounds(data: &[u8]) -> Result<Option<Bounds>, FormatError> {
    let buf = Buf(data);
    if data.len() < 4 || buf.i32(0)? != IDST {
        return Ok(None);
    }
    let numbones = buf.i32(0x8C)?;
    let boneindex = i64::from(buf.i32(0x90)?);
    let numseq = buf.i32(0xA4)?;
    let seqindex = i64::from(buf.i32(0xA8)?);
    let numbodyparts = buf.i32(0xCC)?;
    let bodypartindex = i64::from(buf.i32(0xD0)?);

    let mut nmodels: i32 = 0;
    for i in 0..i64::from(numbodyparts.max(0)) {
        nmodels = nmodels.wrapping_add(buf.i32(bodypartindex + BODYPART_SIZE * i + 64)?);
    }
    let models = bodypartindex + BODYPART_SIZE * i64::from(numbodyparts);

    let mut verts = BoundBox::default();
    for m in 0..i64::from(nmodels.max(0)) {
        let model = models + MODEL_SIZE * m;
        let numverts = buf.i32(model + 80)?;
        let vertindex = i64::from(buf.i32(model + 88)?);
        for j in 0..i64::from(numverts.max(0)) {
            let at = vertindex + 12 * j;
            verts.add([buf.f32(at)?, buf.f32(at + 4)?, buf.f32(at + 8)?]);
        }
    }

    for s in 0..i64::from(numseq.max(0)) {
        let seq = seqindex + SEQ_SIZE * s;
        let numframes = buf.i32(seq + 56)?;
        let panim = i64::from(buf.i32(seq + 124)?);
        let mut bones = BoundBox::default();
        for b in 0..i64::from(numbones.max(0)) {
            for f in 0..numframes.max(0) {
                bones.add(bone_position(&buf, f, boneindex + BONE_SIZE * b, panim)?);
            }
        }
        if bones.count > 0 {
            let d = [0, 1, 2].map(|i| bones.max[i] - bones.min[i]);
            verts.add(d.map(|x| 0.5f32 * x));
            verts.add(d.map(|x| -0.5f32 * x));
        }
    }
    Ok(Some((verts.min, verts.max)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_i32(buf: &mut [u8], off: usize, v: i32) {
        buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn put_f32(buf: &mut [u8], off: usize, v: f32) {
        buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// Two bones, one 2-frame sequence whose anim data belongs to bone 0 only, no geometry.
    fn tiny_model() -> Vec<u8> {
        let mut m = vec![0u8; 0x400];
        put_i32(&mut m, 0, IDST);
        put_i32(&mut m, 0x8C, 2);
        put_i32(&mut m, 0x90, 0x100);
        put_i32(&mut m, 0xA4, 1);
        put_i32(&mut m, 0xA8, 0x200);
        put_i32(&mut m, 0xD0, 0x300);
        // bone 0: value x = 1, scale x = 1; bone 1: value x = 10, scale x = 2
        put_f32(&mut m, 0x100 + 64, 1.0);
        put_f32(&mut m, 0x100 + 88, 1.0);
        put_f32(&mut m, 0x170 + 64, 10.0);
        put_f32(&mut m, 0x170 + 88, 2.0);
        // sequence: 2 frames, panim at 0x2C0
        put_i32(&mut m, 0x200 + 56, 2);
        put_i32(&mut m, 0x200 + 124, 0x2C0);
        // bone 0 anim x stream at panim+12: run {valid 2, total 2, values 3, 5}
        m[0x2C0..0x2C2].copy_from_slice(&12u16.to_le_bytes());
        m[0x2CC] = 2;
        m[0x2CD] = 2;
        m[0x2CE..0x2D0].copy_from_slice(&3i16.to_le_bytes());
        m[0x2D0..0x2D2].copy_from_slice(&5i16.to_le_bytes());
        m
    }

    #[test]
    fn bones_reuse_bone_zero_stream_and_box_is_origin_centred() {
        // Bone 0 x: 1+3, 1+5 → 4, 6. Bone 1 decodes the same stream with its own scale: 10+6, 10+10.
        // Spread 4..20 → half extent 8 → x bounds ±8; y/z stay 0.
        let bounds = studio_bounds(&tiny_model()).ok().flatten();
        assert_eq!(bounds, Some(([-8.0, 0.0, 0.0], [8.0, 0.0, 0.0])));
    }

    #[test]
    fn non_idst_is_none_and_truncated_is_error() {
        assert!(matches!(studio_bounds(b"IDSQ0000"), Ok(None)));
        let mut m = tiny_model();
        put_i32(&mut m, 0x200 + 124, 0x3FE);
        assert!(studio_bounds(&m).is_err(), "anim stream past the end of the file");
    }
}
