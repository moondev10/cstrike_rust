//! `PM_HullForStudioModel` → `R_StudioHull` (docs/re/movement.md §B.10): one oriented six-plane
//! hull per hitbox, posed by the engine's default blending interface
//! `SV_StudioSetupBones@0x101f1310` (a client has no game DLL to replace it).

use std::fmt;
use std::sync::Arc;

use gs_studio::StudioModel;
use gs_studio::anim::{Mat3x4, Quat, bone_adj, bone_position, bone_quaternion, concat, slerp_bones};
use gs_studio::pose::anim_ref;
use gs_world::{BoxHull, Plane};

use crate::math::{Vec3, angle_vectors_transpose};
use crate::trace::{PLAYER_MAXS, PLAYER_MINS};

/// `studiohdr_t.flags`: trace against hitboxes instead of the bounding box.
pub const STUDIO_TRACE_HITBOX: i32 = 0x200;

/// `physent_t.studiomodel` with the pose fields `R_StudioHull` reads.
#[derive(Clone)]
pub struct StudioPhys {
    pub model: Arc<StudioModel>,
    pub frame: f32,
    pub sequence: i32,
    pub controller: [u8; 4],
    pub blending: [u8; 2],
}

impl StudioPhys {
    /// The model's `STUDIO_TRACE_HITBOX` flag.
    pub fn trace_hitbox(&self) -> bool {
        self.model.flags & STUDIO_TRACE_HITBOX != 0
    }
}

impl PartialEq for StudioPhys {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.model, &other.model)
            && self.frame == other.frame
            && self.sequence == other.sequence
            && self.controller == other.controller
            && self.blending == other.blending
    }
}

impl fmt::Debug for StudioPhys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StudioPhys")
            .field("model", &self.model.name)
            .field("frame", &self.frame)
            .field("sequence", &self.sequence)
            .field("controller", &self.controller)
            .field("blending", &self.blending)
            .finish()
    }
}

/// The inlined `QuaternionMatrix` of `SV_StudioSetupBones`: products and sums in double, each
/// element rounded to float, the position in column 3.
fn quaternion_matrix(q: Quat, pos: [f32; 3]) -> Mat3x4 {
    let [x, y, z, w] = q.map(f64::from);
    let (y2, w2, x2) = (y + y, w + w, x + x);
    let zz = (z + z) * z;
    let yy = y * y2;
    let (xy, zw, xz, yw) = (y * x2, z * w2, z * x2, y * w2);
    let xx = 1.0 - x * x2;
    let (xw, yz) = (x * w2, z * y2);
    [
        [(1.0 - yy - zz) as f32, (xy - zw) as f32, (yw + xz) as f32, pos[0]],
        [(xy + zw) as f32, (xx - zz) as f32, (yz - xw) as f32, pos[1]],
        [(xz - yw) as f32, (xw + yz) as f32, (xx - yy) as f32, pos[2]],
    ]
}

/// `SV_StudioSetupBones(model, frame, sequence, angles, origin, controller, blending, -1)`:
/// every bone's transform, parents first. No interpolation, no motion-bone zeroing; a second
/// blend is slerped in by `blending[0] / 255`.
pub fn sv_setup_bones(model: &StudioModel, frame: f32, sequence: i32, angles: Vec3, origin: Vec3, controller: [u8; 4], blending: [u8; 2]) -> Vec<Mat3x4> {
    let numseq = model.sequences.len();
    let index = match usize::try_from(sequence) {
        Ok(s) if s < numseq => s,
        _ => {
            gs_common::conprint::dprintf(&format!("SV_StudioSetupBones:  sequence {sequence}/{numseq} out of range for model {}\n", model.name));
            0
        }
    };
    let Some(seq) = model.sequences.get(index) else { return Vec::new() };
    let Some(anim) = anim_ref(model, seq, 0) else { return Vec::new() };
    let f = if seq.numframes > 1 { ((seq.numframes - 1) as f32 * frame) * 0.003_906_25 } else { 0.0 };
    let iframe = f as i32;
    let s = f - iframe as f32;
    let mut adj = [0.0f32; 5];
    bone_adj(&model.controllers, 0.0, controller, controller, 0, &mut adj);
    let pose = |anim| -> (Vec<Quat>, Vec<[f32; 3]>) {
        model.bones.iter().enumerate().map(|(i, bone)| (bone_quaternion(iframe, s, bone, i, anim, &adj), bone_position(iframe, s, bone, i, anim, &adj))).unzip()
    };
    let (mut q, mut pos) = pose(anim);
    if seq.numblends > 1
        && let Some(anim2) = anim_ref(model, seq, 1)
    {
        let (mut q2, pos2) = pose(anim2);
        let weight = (f64::from(blending[0]) / 255.0) as f32;
        slerp_bones(&mut q, &mut pos, &mut q2, &pos2, weight);
    }
    // AngleMatrix's rows are AngleVectorsTranspose's vectors.
    let (r0, r1, r2) = angle_vectors_transpose(angles);
    let rotation: Mat3x4 = [[r0[0], r0[1], r0[2], origin[0]], [r1[0], r1[1], r1[2], origin[1]], [r2[0], r2[1], r2[2], origin[2]]];
    let mut out: Vec<Mat3x4> = Vec::with_capacity(model.bones.len());
    for (i, bone) in model.bones.iter().enumerate() {
        let local = quaternion_matrix(q[i], pos[i]);
        let parent = usize::try_from(bone.parent).ok().and_then(|p| out.get(p)).copied().unwrap_or(rotation);
        out.push(concat(&parent, &local));
    }
    out
}

/// `PM_HullForStudioModel` → `R_StudioHull(model, frame, sequence, angles, origin, size, …)`:
/// each hitbox as a hull in world space (the trace offset is zero) with its hit group. The
/// box is grown by half the player hull of `usehull`; the bones see the pitch negated.
pub fn studio_hulls(sp: &StudioPhys, angles: Vec3, origin: Vec3, usehull: i32) -> Vec<(BoxHull, i32)> {
    let slot = usize::try_from(usehull).ok().filter(|&h| h < 4).unwrap_or(0);
    let size: [f32; 3] = std::array::from_fn(|k| (PLAYER_MAXS[slot][k] - PLAYER_MINS[slot][k]) * 0.5);
    let bones = sv_setup_bones(&sp.model, sp.frame, sp.sequence, [-angles[0], angles[1], angles[2]], origin, sp.controller, sp.blending);
    sp.model
        .hitboxes
        .iter()
        .filter_map(|hb| {
            let m = bones.get(usize::try_from(hb.bone).ok()?)?;
            let planes: [Plane; 6] = std::array::from_fn(|i| {
                let c = i >> 1;
                let normal = [m[0][c], m[1][c], m[2][c]];
                let base = (m[1][c] * m[1][3] + m[0][c] * m[0][3]) + m[2][c] * m[2][3];
                let e = f64::from((m[1][c] * size[1]).abs()) + f64::from((m[0][c] * size[0]).abs()) + f64::from((m[2][c] * size[2]).abs());
                let dist = if i & 1 == 0 { f64::from(base + hb.maxs[c]) + e } else { f64::from(base + hb.mins[c]) - e };
                Plane { normal, dist: dist as f32, kind: 5 }
            });
            Some((BoxHull::from_planes(planes), hb.group))
        })
        .collect()
}
