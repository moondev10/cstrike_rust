//! Animation decoding and bone math (docs/re/render.md §B.2.3, §B.7). Matrices are the engine's
//! 3x4 row-major `float[3][4]` (rotation columns + translation in column 3).

use crate::model::{Bone, BoneController, SeqDesc, StudioModel};

pub type Mat3x4 = [[f32; 4]; 3];
pub type Quat = [f32; 4];

pub const IDENTITY: Mat3x4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

/// `AngleQuaternion` (radians: roll x, pitch y, yaw z).
pub fn angle_quaternion(a: [f32; 3]) -> Quat {
    let (sr, cr) = (a[0] * 0.5).sin_cos();
    let (sp, cp) = (a[1] * 0.5).sin_cos();
    let (sy, cy) = (a[2] * 0.5).sin_cos();
    [sr * cp * cy - cr * sp * sy, cr * sp * cy + sr * cp * sy, cr * cp * sy - sr * sp * cy, cr * cp * cy + sr * sp * sy]
}

/// `QuaternionSlerp`; flips `q` in place when it is in the opposite hemisphere, like the original.
pub fn quaternion_slerp(p: Quat, q: &mut Quat, t: f32) -> Quat {
    let (mut a, mut b) = (0.0f32, 0.0f32);
    for i in 0..4 {
        a += (p[i] - q[i]) * (p[i] - q[i]);
        b += (p[i] + q[i]) * (p[i] + q[i]);
    }
    if a > b {
        for v in q.iter_mut() {
            *v = -*v;
        }
    }
    let cosom = p[0] * q[0] + p[1] * q[1] + p[2] * q[2] + p[3] * q[3];
    let mut out = [0.0; 4];
    if 1.0 + cosom > 1e-6 {
        let (sclp, sclq) = if 1.0 - cosom > 1e-6 {
            let omega = cosom.acos();
            let sinom = omega.sin();
            (((1.0 - t) * omega).sin() / sinom, (t * omega).sin() / sinom)
        } else {
            (1.0 - t, t)
        };
        for i in 0..4 {
            out[i] = sclp * p[i] + sclq * q[i];
        }
    } else {
        out = [-q[1], q[0], -q[3], q[2]];
        let sclp = ((1.0 - t) * std::f32::consts::FRAC_PI_2).sin();
        let sclq = (t * std::f32::consts::FRAC_PI_2).sin();
        for i in 0..3 {
            out[i] = sclp * p[i] + sclq * out[i];
        }
    }
    out
}

/// `QuaternionMatrix` with translation `pos`.
pub fn quaternion_matrix(q: Quat, pos: [f32; 3]) -> Mat3x4 {
    let [x, y, z, w] = q;
    [
        [1.0 - 2.0 * y * y - 2.0 * z * z, 2.0 * x * y - 2.0 * w * z, 2.0 * x * z + 2.0 * w * y, pos[0]],
        [2.0 * x * y + 2.0 * w * z, 1.0 - 2.0 * x * x - 2.0 * z * z, 2.0 * y * z - 2.0 * w * x, pos[1]],
        [2.0 * x * z - 2.0 * w * y, 2.0 * y * z + 2.0 * w * x, 1.0 - 2.0 * x * x - 2.0 * y * y, pos[2]],
    ]
}

/// `AngleMatrix` (degrees, pitch/yaw/roll) with translation.
pub fn angle_matrix(angles: [f32; 3], origin: [f32; 3]) -> Mat3x4 {
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sy, cy) = angles[1].to_radians().sin_cos();
    let (sr, cr) = angles[2].to_radians().sin_cos();
    [
        [cp * cy, sr * sp * cy - cr * sy, cr * sp * cy + sr * sy, origin[0]],
        [cp * sy, sr * sp * sy + cr * cy, cr * sp * sy - sr * cy, origin[1]],
        [-sp, sr * cp, cr * cp, origin[2]],
    ]
}

/// `R_ConcatTransforms`.
pub fn concat(a: &Mat3x4, b: &Mat3x4) -> Mat3x4 {
    let mut out = [[0.0; 4]; 3];
    for i in 0..3 {
        for j in 0..4 {
            out[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
        out[i][3] += a[i][3];
    }
    out
}

/// `VectorTransform`.
pub fn transform(m: &Mat3x4, v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| v[0] * m[i][0] + v[1] * m[i][1] + v[2] * m[i][2] + m[i][3])
}

/// `VectorIRotate`: multiply by the transposed rotation.
pub fn irotate(m: &Mat3x4, v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| v[0] * m[0][i] + v[1] * m[1][i] + v[2] * m[2][i])
}

/// Animation values of one channel at frame `k` and `k+1`.
fn channel_values(data: &[u8], stream: usize, frame: i32) -> Option<(i16, i16)> {
    let byte = |at: usize| data.get(at).copied();
    let value = |at: usize| Some(i16::from_le_bytes([byte(at)?, byte(at + 1)?]));
    let mut p = stream;
    let mut k = frame;
    let mut valid = i32::from(byte(p)?);
    let mut total = i32::from(byte(p + 1)?);
    if total < valid {
        k = 0;
    }
    while total <= k {
        k -= total;
        p += 2 * (valid as usize + 1);
        valid = i32::from(byte(p)?);
        total = i32::from(byte(p + 1)?);
        if total < valid {
            k = 0;
        }
    }
    let at = |i: i32| p + 2 * i as usize;
    if valid > k {
        let v0 = value(at(k + 1))?;
        let v1 = if valid > k + 1 {
            value(at(k + 2))?
        } else if total > k + 1 {
            v0
        } else {
            value(at(valid + 2))?
        };
        Some((v0, v1))
    } else {
        let v0 = value(at(valid))?;
        let v1 = if total > k + 1 { v0 } else { value(at(valid + 2))? };
        Some((v0, v1))
    }
}

/// Per-bone animation source: `data[anim_offset + 12 * bone]` is that bone's `mstudioanim_t`.
#[derive(Clone, Copy)]
pub struct AnimRef<'a> {
    pub data: &'a [u8],
    pub offset: usize,
}

impl AnimRef<'_> {
    fn stream(&self, bone: usize, channel: usize) -> Option<usize> {
        let at = self.offset + 12 * bone + 2 * channel;
        let off = u16::from_le_bytes([*self.data.get(at)?, *self.data.get(at + 1)?]);
        (off != 0).then_some(self.offset + 12 * bone + usize::from(off))
    }
}

/// `StudioCalcBoneQuaterion`.
pub fn bone_quaternion(frame: i32, s: f32, bone: &Bone, index: usize, anim: AnimRef, adj: &[f32; 5]) -> Quat {
    let mut a1 = [0.0f32; 3];
    let mut a2 = [0.0f32; 3];
    for j in 0..3 {
        match anim.stream(index, j + 3).and_then(|st| channel_values(anim.data, st, frame)) {
            Some((v0, v1)) => {
                a1[j] = bone.value[j + 3] + f32::from(v0) * bone.scale[j + 3];
                a2[j] = bone.value[j + 3] + f32::from(v1) * bone.scale[j + 3];
            }
            None => {
                a1[j] = bone.value[j + 3];
                a2[j] = bone.value[j + 3];
            }
        }
        if let Some(c) = usize::try_from(bone.controller[j + 3]).ok().and_then(|c| adj.get(c)) {
            a1[j] += c;
            a2[j] += c;
        }
    }
    if a1 == a2 {
        angle_quaternion(a1)
    } else {
        let q1 = angle_quaternion(a1);
        let mut q2 = angle_quaternion(a2);
        quaternion_slerp(q1, &mut q2, s)
    }
}

/// `StudioCalcBonePosition`.
pub fn bone_position(frame: i32, s: f32, bone: &Bone, index: usize, anim: AnimRef, adj: &[f32; 5]) -> [f32; 3] {
    let mut pos = [0.0f32; 3];
    for (j, p) in pos.iter_mut().enumerate() {
        *p = bone.value[j];
        if let Some((v0, v1)) = anim.stream(index, j).and_then(|st| channel_values(anim.data, st, frame)) {
            *p += if v0 != v1 {
                (f32::from(v0) * (1.0 - s) + s * f32::from(v1)) * bone.scale[j]
            } else {
                f32::from(v0) * bone.scale[j]
            };
        }
        if let Some(c) = usize::try_from(bone.controller[j]).ok().and_then(|c| adj.get(c)) {
            *p += c;
        }
    }
    pos
}

/// `StudioEstimateFrame` for a sequence at entity frame `frame` (0..255) advanced by `dfdt`.
pub fn estimate_frame(seq: &SeqDesc, frame: f32, dfdt: f32) -> f32 {
    let n = seq.numframes;
    let mut f = if n <= 1 { 0.0 } else { frame * (n - 1) as f32 / 256.0 };
    f += dfdt;
    if seq.flags & crate::model::STUDIO_LOOPING != 0 {
        if n > 1 {
            f -= ((f / (n - 1) as f32) as i32 * (n - 1)) as f32;
        }
        if f < 0.0 {
            f += (n - 1) as f32;
        }
    } else {
        f = f.clamp(0.0, (n as f32 - 1.001).max(0.0));
    }
    f
}

pub mod controller_types {
    pub const X: i32 = 1;
    pub const Y: i32 = 2;
    pub const Z: i32 = 4;
    pub const XR: i32 = 8;
    pub const YR: i32 = 16;
    pub const ZR: i32 = 32;
    pub const RLOOP: i32 = 0x8000;
}

/// `StudioCalcBoneAdj`: controller adjustments (units / radians), `adj` indexed by controller.
pub fn bone_adj(controllers: &[BoneController], dadt: f32, c1: [u8; 4], c2: [u8; 4], mouthopen: u8, adj: &mut [f32; 5]) {
    use controller_types as T;
    for (j, ctl) in controllers.iter().enumerate().take(5) {
        let index = ctl.index;
        let value = if (0..=3).contains(&index) {
            let i = index as usize;
            if ctl.kind & T::RLOOP != 0 {
                if (i32::from(c1[i]) - i32::from(c2[i])).abs() <= 128 {
                    (f32::from(c2[i]) * (1.0 - dadt) + f32::from(c1[i]) * dadt) * (360.0 / 256.0) + ctl.start
                } else {
                    let jj = j.min(3);
                    let a = ((u16::from(c2[jj]) + 128) & 255) as f32;
                    let b = ((u16::from(c1[jj]) + 128) & 255) as f32;
                    (a * (1.0 - dadt) + b * dadt - 128.0) * 1.40625 + ctl.start
                }
            } else {
                let t = ((f32::from(c2[i]) * (1.0 - dadt) + f32::from(c1[i]) * dadt) / 255.0).clamp(0.0, 1.0);
                (1.0 - t) * ctl.start + t * ctl.end
            }
        } else if index == 4 {
            let t = (f32::from(mouthopen) / 64.0).min(1.0);
            (1.0 - t) * ctl.start + t * ctl.end
        } else {
            continue;
        };
        match ctl.kind & 0x7FFF {
            T::X | T::Y | T::Z => adj[j] = value,
            T::XR | T::YR | T::ZR => adj[j] = value * (std::f32::consts::PI / 180.0),
            _ => {}
        }
    }
}

/// `StudioCalcRotations` for one blend: positions and quaternions of every bone at frame `f`.
pub fn calc_rotations(model: &StudioModel, seq: &SeqDesc, anim: AnimRef, f: f32, adj: &[f32; 5]) -> (Vec<[f32; 3]>, Vec<Quat>) {
    let mut f = f;
    if f > (seq.numframes - 1) as f32 {
        f = 0.0;
    } else if f < -0.01 {
        f = -0.01;
    }
    let frame = f as i32;
    let s = f - frame as f32;
    let mut pos = Vec::with_capacity(model.bones.len());
    let mut q = Vec::with_capacity(model.bones.len());
    for (i, bone) in model.bones.iter().enumerate() {
        q.push(bone_quaternion(frame, s, bone, i, anim, adj));
        pos.push(bone_position(frame, s, bone, i, anim, adj));
    }
    if let Some(p) = usize::try_from(seq.motionbone).ok().and_then(|b| pos.get_mut(b)) {
        if seq.motiontype & controller_types::X != 0 {
            p[0] = 0.0;
        }
        if seq.motiontype & controller_types::Y != 0 {
            p[1] = 0.0;
        }
        if seq.motiontype & controller_types::Z != 0 {
            p[2] = 0.0;
        }
    }
    (pos, q)
}

/// `StudioSlerpBones`: blend `(q1, pos1)` toward `(q2, pos2)` by `s` (clamped to 0..1).
pub fn slerp_bones(q1: &mut [Quat], pos1: &mut [[f32; 3]], q2: &mut [Quat], pos2: &[[f32; 3]], s: f32) {
    let s = s.clamp(0.0, 1.0);
    let s1 = 1.0 - s;
    for i in 0..q1.len() {
        q1[i] = quaternion_slerp(q1[i], &mut q2[i], s);
        for j in 0..3 {
            pos1[i][j] = pos1[i][j] * s1 + pos2[i][j] * s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_length_values_follow_engine_rules() {
        // run 1: valid 2, total 3 → frames 0,1 explicit, frame 2 repeats the last value
        // run 2: valid 1, total 2
        let data: Vec<u8> = [2u8, 3, 10, 0, 20, 0, 1, 2, 30, 0].to_vec();
        assert_eq!(channel_values(&data, 0, 0), Some((10, 20)));
        assert_eq!(channel_values(&data, 0, 1), Some((20, 20)), "next frame is the repeated tail");
        assert_eq!(channel_values(&data, 0, 2), Some((20, 30)), "last frame of a run blends into the next run");
        assert_eq!(channel_values(&data, 0, 3), Some((30, 30)));
    }

    #[test]
    fn slerp_endpoints_and_hemisphere_flip() {
        let p = angle_quaternion([0.0, 0.0, 0.0]);
        let mut q = angle_quaternion([0.0, 0.0, 1.0]);
        let mid = quaternion_slerp(p, &mut q, 0.0);
        assert!(mid.iter().zip(p.iter()).all(|(a, b)| (a - b).abs() < 1e-6));
        let mut neg = q.map(|v| -v);
        let r = quaternion_slerp(p, &mut neg, 1.0);
        assert!(r.iter().zip(q.iter()).all(|(a, b)| (a - b).abs() < 1e-5), "flipped input lands on the same rotation");
        assert!(neg.iter().zip(q.iter()).all(|(a, b)| (a - b).abs() < 1e-6), "the caller's quaternion is negated in place");
    }

    #[test]
    fn looping_frames_wrap_and_non_looping_clamp() {
        let mut seq = SeqDesc {
            label: String::new(),
            fps: 30.0,
            flags: 1,
            activity: 0,
            events: vec![],
            numframes: 11,
            motiontype: 0,
            motionbone: 0,
            linearmovement: [0.0; 3],
            bbmin: [0.0; 3],
            bbmax: [0.0; 3],
            numblends: 1,
            animindex: 0,
            seqgroup: 0,
        };
        assert!((estimate_frame(&seq, 0.0, 12.5) - 2.5).abs() < 1e-5);
        seq.flags = 0;
        assert!((estimate_frame(&seq, 0.0, 12.5) - 9.999).abs() < 1e-4);
    }
}
