//! Bone setup (docs/re/render.md §B.7–B.8): generic `StudioSetupBones`, the CS player variant
//! with 9-way yaw/pitch blending and gait overlay, `StudioMergeBones` for weapon models, and the
//! CS gait/blend estimation.

use crate::anim::{
    AnimRef, Mat3x4, Quat, bone_adj, calc_rotations, concat, estimate_frame, quaternion_matrix, slerp_bones,
};
use crate::model::{SeqDesc, StudioModel};

/// Animation inputs of one entity (`curstate` + `latched`).
#[derive(Debug, Clone, Copy, Default)]
pub struct AnimState {
    pub sequence: i32,
    /// `curstate.frame` (0..256 over the sequence).
    pub frame: f32,
    pub animtime: f32,
    pub framerate: f32,
    pub controller: [u8; 4],
    pub blending: [u8; 2],
    pub mouth: u8,
    pub prev_sequence: i32,
    /// Latched frame of the previous sequence (in frames, not 0..256).
    pub prev_frame: f32,
    pub sequence_time: f32,
    pub prev_animtime: f32,
    pub prev_controller: [u8; 4],
    pub prev_blending: [u8; 2],
    pub prev_seq_blending: [u8; 2],
}

/// Frame-wide parameters.
#[derive(Debug, Clone, Copy)]
pub struct PoseParams {
    /// `cl.time`.
    pub time: f64,
    /// `m_fDoInterp`.
    pub interpolate: bool,
    /// View model drawn right-handed (`cl_righthand > 0`): mirror root bones.
    pub mirror: bool,
}

pub fn anim_ref<'a>(model: &'a StudioModel, seq: &SeqDesc, blend: usize) -> Option<AnimRef<'a>> {
    let data: &[u8] = if seq.seqgroup == 0 { &model.data } else { model.group_data.get(seq.seqgroup as usize)?.as_deref()? };
    let offset = usize::try_from(seq.animindex).ok()? + blend * model.bones.len() * 12;
    Some(AnimRef { data, offset })
}

/// `StudioEstimateInterpolant`.
fn interpolant(state: &AnimState, p: &PoseParams) -> f32 {
    if p.interpolate && state.animtime >= state.prev_animtime + 0.01 {
        (((p.time - f64::from(state.animtime)) / 0.1) as f32).min(2.0)
    } else {
        1.0
    }
}

fn frame_of(seq: &SeqDesc, state: &AnimState, p: &PoseParams) -> f32 {
    let dfdt = if p.interpolate && p.time >= f64::from(state.animtime) {
        ((p.time - f64::from(state.animtime)) as f32) * state.framerate * seq.fps
    } else {
        0.0
    };
    estimate_frame(seq, state.frame, dfdt)
}

struct Pose {
    pos: Vec<[f32; 3]>,
    q: Vec<Quat>,
}

fn rotations(model: &StudioModel, seq: &SeqDesc, blend: usize, f: f32, adj: &[f32; 5]) -> Pose {
    match anim_ref(model, seq, blend) {
        Some(anim) => {
            let (pos, q) = calc_rotations(model, seq, anim, f, adj);
            Pose { pos, q }
        }
        None => Pose {
            pos: model.bones.iter().map(|b| [b.value[0], b.value[1], b.value[2]]).collect(),
            q: vec![[0.0, 0.0, 0.0, 1.0]; model.bones.len()],
        },
    }
}

fn blend(a: &mut Pose, b: &mut Pose, s: f32) {
    slerp_bones(&mut a.q, &mut a.pos, &mut b.q, &b.pos, s);
}

/// Generic 1/2/4-blend pose (no 9-way grid).
fn blended_pose(model: &StudioModel, seq: &SeqDesc, f: f32, adj: &[f32; 5], s0: f32, s1: f32) -> Pose {
    let mut pose = rotations(model, seq, 0, f, adj);
    if seq.numblends > 1 {
        let mut p2 = rotations(model, seq, 1, f, adj);
        blend(&mut pose, &mut p2, s0);
        if seq.numblends == 4 {
            let mut p3 = rotations(model, seq, 2, f, adj);
            let mut p4 = rotations(model, seq, 3, f, adj);
            blend(&mut p3, &mut p4, s0);
            blend(&mut pose, &mut p3, s1);
        }
    }
    pose
}

/// CS 9-way yaw/pitch grid (`CGameStudioModelRenderer::StudioSetupBones`).
fn grid_pose(model: &StudioModel, seq: &SeqDesc, f: f32, adj: &[f32; 5], blending: [u8; 2]) -> Pose {
    if seq.numblends != 9 {
        return rotations(model, seq, 0, f, adj);
    }
    let (b0, b1) = (i32::from(blending[0]), i32::from(blending[1]));
    let col = usize::from(b0 > 127);
    let row = usize::from(b1 > 127);
    let s = (if b0 > 127 { (b0 - 127) * 2 } else { b0 * 2 }) as f32 / 255.0;
    let t = (if b1 > 127 { (b1 - 127) * 2 } else { b1 * 2 }) as f32 / 255.0;
    let mut a = rotations(model, seq, 3 * row + col, f, adj);
    let mut b = rotations(model, seq, 3 * row + col + 1, f, adj);
    let mut c = rotations(model, seq, 3 * (row + 1) + col, f, adj);
    let mut d = rotations(model, seq, 3 * (row + 1) + col + 1, f, adj);
    blend(&mut a, &mut b, s);
    blend(&mut c, &mut d, s);
    blend(&mut a, &mut c, t);
    a
}

fn clamp_sequence(model: &StudioModel, seq: i32) -> usize {
    usize::try_from(seq).ok().filter(|&s| s < model.sequences.len()).unwrap_or(0)
}

/// Bone matrices from a pose: root bones get `rotation`, children concatenate their parent.
fn matrices(model: &StudioModel, pose: &Pose, rotation: &Mat3x4, mirror: bool) -> Vec<Mat3x4> {
    let mut out: Vec<Mat3x4> = Vec::with_capacity(model.bones.len());
    for (i, bone) in model.bones.iter().enumerate() {
        let mut m = quaternion_matrix(pose.q[i], pose.pos[i]);
        let parent = usize::try_from(bone.parent).ok().filter(|&p| p < out.len());
        out.push(match parent {
            Some(p) => concat(&out[p], &m),
            None => {
                if mirror {
                    for v in m[1].iter_mut() {
                        *v = -*v;
                    }
                }
                concat(rotation, &m)
            }
        });
    }
    out
}

/// Result of a bone setup: matrices plus the latched frame to store back.
pub struct BoneSetup {
    pub bones: Vec<Mat3x4>,
    /// New `latched.prevframe` (unchanged while a sequence transition runs).
    pub prev_frame: f32,
}

/// Previous-sequence pose for the 0.2 s transition blend, if one is running.
fn transition(
    model: &StudioModel,
    state: &AnimState,
    p: &PoseParams,
    adj: &[f32; 5],
    grid: bool,
) -> Option<(Pose, f32)> {
    let running = p.interpolate
        && state.sequence_time != 0.0
        && f64::from(state.sequence_time) + 0.2 > p.time
        && usize::try_from(state.prev_sequence).is_ok_and(|s| s < model.sequences.len());
    if !running {
        return None;
    }
    let seq = &model.sequences[state.prev_sequence as usize];
    let pose = if grid {
        grid_pose(model, seq, state.prev_frame, adj, state.prev_seq_blending)
    } else {
        let s0 = f32::from(state.prev_seq_blending[0]) / 255.0;
        let s1 = f32::from(state.prev_seq_blending[1]) / 255.0;
        blended_pose(model, seq, state.prev_frame, adj, s0, s1)
    };
    let s = 1.0 - ((p.time - f64::from(state.sequence_time)) / 0.2) as f32;
    Some((pose, s))
}

/// `CStudioModelRenderer::StudioSetupBones` for non-player entities.
pub fn setup_bones(model: &StudioModel, state: &AnimState, rotation: &Mat3x4, p: &PoseParams) -> BoneSetup {
    if model.bones.is_empty() || model.sequences.is_empty() {
        return BoneSetup { bones: Vec::new(), prev_frame: state.prev_frame };
    }
    let seq = &model.sequences[clamp_sequence(model, state.sequence)];
    let f = frame_of(seq, state, p);
    let dadt = interpolant(state, p);
    let mut adj = [0.0f32; 5];
    bone_adj(&model.controllers, dadt, state.controller, state.prev_controller, state.mouth, &mut adj);
    let w = 1.0 - dadt;
    let s0 = (f32::from(state.blending[0]) * dadt + f32::from(state.prev_blending[0]) * w) / 255.0;
    let s1 = (f32::from(state.blending[1]) * dadt + f32::from(state.prev_blending[1]) * w) / 255.0;
    let mut pose = blended_pose(model, seq, f, &adj, s0, s1);
    let prev_frame = match transition(model, state, p, &adj, false) {
        Some((mut prev, s)) => {
            blend(&mut pose, &mut prev, s);
            state.prev_frame
        }
        None => f,
    };
    BoneSetup { bones: matrices(model, &pose, rotation, p.mirror), prev_frame }
}

/// Per-player CS animation state (`player_info_t` gait fields).
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerGait {
    pub gaitsequence: i32,
    pub gaitframe: f32,
    pub gaityaw: f32,
    pub prevgaitorigin: [f32; 3],
    /// `ent->baseline.fuser1`: yaw turn speed factor.
    pub turn_rate: f32,
    /// Previous frame's gait sequence (jump detection).
    pub last_gaitsequence: i32,
}

/// `CGameStudioModelRenderer::StudioSetupBones` for players: 9-way grid + gait overlay on the legs.
pub fn setup_player_bones(
    model: &StudioModel,
    state: &AnimState,
    gait: &mut PlayerGait,
    rotation: &Mat3x4,
    p: &PoseParams,
) -> BoneSetup {
    if model.bones.is_empty() || model.sequences.is_empty() {
        return BoneSetup { bones: Vec::new(), prev_frame: state.prev_frame };
    }
    if gait.last_gaitsequence != 6 && gait.gaitsequence == 6 {
        gait.gaitframe = 0.0;
    }
    gait.last_gaitsequence = gait.gaitsequence;
    let sequence = clamp_sequence(model, state.sequence);
    let seq = &model.sequences[sequence];
    let f = frame_of(seq, state, p);
    let mut state = *state;
    if gait.gaitsequence == 3 {
        state.blending[0] = state.blending[0].saturating_sub(26);
        state.prev_seq_blending[0] = state.blending[0];
    }
    let dadt = interpolant(&state, p);
    let mut adj = [0.0f32; 5];
    bone_adj(&model.controllers, dadt, state.controller, state.prev_controller, state.mouth, &mut adj);
    let mut pose = grid_pose(model, seq, f, &adj, state.blending);
    let prev_frame = match transition(model, &state, p, &adj, true) {
        Some((mut prev, s)) => {
            blend(&mut pose, &mut prev, s);
            state.prev_frame
        }
        None => f,
    };
    let swimming = matches!(sequence, 8 | 9) || sequence > 100;
    if !swimming {
        let gseq = &model.sequences[clamp_sequence(model, gait.gaitsequence)];
        let gpose = rotations(model, gseq, 0, gait.gaitframe, &adj);
        let mut copy = true;
        for (i, bone) in model.bones.iter().enumerate() {
            let parent_is_pelvis = usize::try_from(bone.parent)
                .ok()
                .and_then(|pi| model.bones.get(pi))
                .is_some_and(|pb| pb.name == "Bip01 Pelvis");
            if bone.name == "Bip01 Spine" {
                copy = false;
            } else if parent_is_pelvis || copy {
                pose.pos[i] = gpose.pos[i];
                pose.q[i] = gpose.q[i];
                copy = true;
            }
        }
    }
    BoneSetup { bones: matrices(model, &pose, rotation, false), prev_frame }
}

/// `StudioMergeBones`: bones named like cached ones (case-insensitive) take the cached matrix.
pub fn merge_bones(
    model: &StudioModel,
    state: &AnimState,
    cached: &[(String, Mat3x4)],
    rotation: &Mat3x4,
    p: &PoseParams,
) -> Vec<Mat3x4> {
    if model.bones.is_empty() || model.sequences.is_empty() {
        return Vec::new();
    }
    let seq = &model.sequences[clamp_sequence(model, state.sequence)];
    let f = frame_of(seq, state, p);
    let pose = rotations(model, seq, 0, f, &[0.0; 5]);
    let mut out: Vec<Mat3x4> = Vec::with_capacity(model.bones.len());
    for (i, bone) in model.bones.iter().enumerate() {
        if let Some((_, m)) = cached.iter().find(|(n, _)| n.eq_ignore_ascii_case(&bone.name)) {
            out.push(*m);
            continue;
        }
        let m = quaternion_matrix(pose.q[i], pose.pos[i]);
        let parent = usize::try_from(bone.parent).ok().filter(|&p| p < out.len());
        out.push(match parent {
            Some(pi) => concat(&out[pi], &m),
            None => concat(rotation, &m),
        });
    }
    out
}

fn wrap180(a: f32) -> f32 {
    let mut a = a % 360.0;
    if a > 180.0 {
        a -= 360.0;
    } else if a < -180.0 {
        a += 360.0;
    }
    a
}

/// Output of the CS player blend/gait estimation applied before bone setup.
#[derive(Debug, Clone, Copy)]
pub struct PlayerBlend {
    pub blending: [u8; 2],
    /// Model angles to render with (pitch 0, yaw = gait yaw).
    pub angles: [f32; 3],
    /// Movement this frame (`m_flGaitMovement`).
    pub movement: f32,
}

/// `CalculatePitchBlend` + `CalculateYawBlend` (with `StudioEstimateGait`); `first_this_frame`
/// is false when the player was already set up this render frame.
pub fn player_blend(
    gait: &mut PlayerGait,
    origin: [f32; 3],
    angles: [f32; 3],
    sequence: i32,
    dt: f64,
    first_this_frame: bool,
) -> PlayerBlend {
    // CalculatePitchBlend
    let pitch = (3.0 * angles[0]) as i32;
    let pitch_blend = if pitch <= -45 {
        255
    } else if pitch >= 45 {
        0
    } else {
        ((45 - pitch) * 255 / 90) as u8
    };

    // CalculateYawBlend / StudioEstimateGait
    let dt = dt.clamp(0.0, 1.0) as f32;
    let mut movement = 0.0;
    if dt != 0.0 && first_this_frame {
        let mut est = [origin[0] - gait.prevgaitorigin[0], origin[1] - gait.prevgaitorigin[1], origin[2] - gait.prevgaitorigin[2]];
        gait.prevgaitorigin = origin;
        movement = (est[0] * est[0] + est[1] * est[1] + est[2] * est[2]).sqrt();
        if dt <= 0.0 || movement / dt < 5.0 {
            movement = 0.0;
            est[0] = 0.0;
            est[1] = 0.0;
        }
        if est[0] == 0.0 && est[1] == 0.0 && sequence <= 100 {
            let d = wrap180(angles[1] - gait.gaityaw);
            if d.abs() < 5.0 {
                gait.turn_rate = 0.05;
            }
            if d.abs() > 90.0 {
                gait.turn_rate = 3.5;
            }
            let mut dt2 = dt;
            if dt2 < 0.25 {
                dt2 *= gait.turn_rate;
            }
            let mut step = d * dt2;
            if (step as i32).abs() == 0 {
                step = 0.0;
            }
            gait.gaityaw += step;
            gait.gaityaw -= 360.0 * (gait.gaityaw / 360.0) as i32 as f32;
            movement = 0.0;
        } else if sequence > 100 {
            gait.gaityaw = angles[1];
        } else {
            gait.gaityaw = est[1].atan2(est[0]).to_degrees().clamp(-180.0, 180.0);
        }
    }
    let mut y = wrap180((angles[1] - gait.gaityaw) % 360.0);
    if movement != 0.0 {
        if y > 120.0 {
            gait.gaityaw -= 180.0;
            movement = -movement;
            y -= 180.0;
        } else if y < -120.0 {
            gait.gaityaw += 180.0;
            movement = -movement;
            y += 180.0;
        }
    }
    let v = y / 90.0 * 128.0 + 127.0;
    let yaw_blend = if v >= 255.0 {
        0
    } else if v <= 0.0 {
        255
    } else {
        (255.0 - v) as u8
    };
    let mut yaw = gait.gaityaw;
    if yaw < 0.0 {
        yaw += 360.0;
    }
    PlayerBlend { blending: [yaw_blend, pitch_blend], angles: [0.0, yaw, angles[2]], movement }
}

/// `StudioProcessGait` frame advance for the gait sequence.
pub fn advance_gait(model: &StudioModel, gait: &mut PlayerGait, movement: f32, dt: f64, framerate: f32) {
    let Some(g) = model.sequences.get(clamp_sequence(model, gait.gaitsequence)) else { return };
    let n = g.numframes as f32;
    if g.linearmovement[0] > 0.0 {
        gait.gaitframe += movement / g.linearmovement[0] * n;
    } else {
        gait.gaitframe += dt.clamp(0.0, 1.0) as f32 * g.fps * framerate;
    }
    if n > 0.0 {
        gait.gaitframe -= n * (gait.gaitframe / n) as i32 as f32;
        if gait.gaitframe < 0.0 {
            gait.gaitframe += n;
        }
    }
}

/// Bone matrix list as `(name, matrix)` for `merge_bones` (`StudioSaveBones`).
pub fn save_bones(model: &StudioModel, bones: &[Mat3x4]) -> Vec<(String, Mat3x4)> {
    model.bones.iter().zip(bones).map(|(b, m)| (b.name.clone(), *m)).collect()
}

/// Root transform for entity angles (`StudioSetUpTransform`: studio pitch is inverted).
pub fn entity_rotation(origin: [f32; 3], angles: [f32; 3]) -> Mat3x4 {
    crate::anim::angle_matrix([-angles[0], angles[1], angles[2]], origin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bone, SeqDesc};

    /// A root bone turned `root_yaw` radians about z and a child 10 units along the root's x
    /// axis, posed from the bone values alone (every animation offset is zero).
    fn chain(root_yaw: f32) -> StudioModel {
        let bone = |name: &str, parent: i32, value: [f32; 6]| Bone { name: name.into(), parent, controller: [-1; 6], value, scale: [0.0; 6] };
        let seq = SeqDesc {
            label: "idle".into(),
            fps: 30.0,
            flags: 0,
            activity: 0,
            events: Vec::new(),
            numframes: 1,
            motiontype: 0,
            motionbone: 0,
            linearmovement: [0.0; 3],
            bbmin: [0.0; 3],
            bbmax: [0.0; 3],
            numblends: 1,
            animindex: 0,
            seqgroup: 0,
        };
        StudioModel {
            name: "chain.mdl".into(),
            eyeposition: [0.0; 3],
            min: [0.0; 3],
            max: [0.0; 3],
            bbmin: [0.0; 3],
            bbmax: [0.0; 3],
            flags: 0,
            bones: vec![bone("root", -1, [0.0, 0.0, 0.0, 0.0, 0.0, root_yaw]), bone("tip", 0, [10.0, 0.0, 0.0, 0.0, 0.0, 0.0])],
            controllers: Vec::new(),
            hitboxes: Vec::new(),
            sequences: vec![seq],
            seqgroups: Vec::new(),
            bodyparts: Vec::new(),
            attachments: Vec::new(),
            textures: Vec::new(),
            skins: Vec::new(),
            data: std::sync::Arc::from(vec![0u8; 24]),
            group_data: Vec::new(),
        }
    }

    #[test]
    fn child_bones_follow_the_parent_rotation_and_the_entity_yaw() {
        let p = PoseParams { time: 0.0, interpolate: false, mirror: false };
        let tip = |root_yaw: f32, yaw: f32| {
            let rotation = entity_rotation([100.0, 0.0, 0.0], [0.0, yaw, 0.0]);
            let bones = setup_bones(&chain(root_yaw), &AnimState::default(), &rotation, &p).bones;
            [bones[1][0][3], bones[1][1][3], bones[1][2][3]]
        };
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-4);
        let quarter = std::f32::consts::FRAC_PI_2;
        assert!(close(tip(0.0, 0.0), [110.0, 0.0, 0.0]), "{:?}", tip(0.0, 0.0));
        assert!(close(tip(0.0, 90.0), [100.0, 10.0, 0.0]), "the entity yaw turns the chain: {:?}", tip(0.0, 90.0));
        assert!(close(tip(quarter, 0.0), [100.0, 10.0, 0.0]), "the root's rotation carries the child: {:?}", tip(quarter, 0.0));
        assert!(close(tip(quarter, 90.0), [90.0, 0.0, 0.0]), "both turns add up: {:?}", tip(quarter, 90.0));
    }
}
