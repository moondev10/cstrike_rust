//! Engine pmove services (docs/re/movement.md §B.2–B.10): physents, `_PM_PlayerTrace`,
//! `_PM_TestPlayerPosition`, `PM_PointContents`, `PM_TraceModel` and `PM_TraceTexture`.

use gs_world::{BoxHull, Hull, Variant, World, contents};

use crate::math::{Vec3, angle_vectors, angle_vectors_transpose, dot, sub};
use crate::studio_hull::{StudioPhys, studio_hulls};

/// `player_mins` / `player_maxs` (CS never overrides them).
pub const PLAYER_MINS: [Vec3; 4] = [[-16.0, -16.0, -36.0], [-16.0, -16.0, -18.0], [0.0; 3], [-32.0, -32.0, -32.0]];
pub const PLAYER_MAXS: [Vec3; 4] = [[16.0, 16.0, 36.0], [16.0, 16.0, 18.0], [0.0; 3], [32.0, 32.0, 32.0]];

pub const SOLID_BSP: i32 = 4;

/// `_PM_PlayerTrace` flags.
pub mod trace_flags {
    pub const NORMAL: i32 = 0;
    pub const STUDIO_IGNORE: i32 = 1;
    pub const STUDIO_BOX: i32 = 2;
    pub const GLASS_IGNORE: i32 = 4;
    pub const WORLD_ONLY: i32 = 8;
}

/// `physent_t` (fields movement reads).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Physent {
    /// Player number (1-based) for player boxes, else 0.
    pub player: i32,
    pub origin: Vec3,
    /// Brush model index in [`World::models`].
    pub model: Option<usize>,
    /// `studiomodel`: a BBOX entity whose model traces hitboxes, or a non-BSP, non-BBOX studio
    /// entity (movement.md §B.12).
    pub studiomodel: Option<StudioPhys>,
    pub mins: Vec3,
    pub maxs: Vec3,
    /// Entity number.
    pub info: i32,
    pub angles: Vec3,
    pub solid: i32,
    /// Contents for brush entities (`-16` ladder, `-3` water...).
    pub skin: i32,
    pub rendermode: i32,
    pub movetype: i32,
    pub team: i32,
    /// `entity_state.playerclass` (`CL_AddStateToEntlist`); CS breakables use 1.
    pub classnumber: i32,
}

/// `pmtrace_t`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PmTrace {
    pub allsolid: bool,
    pub startsolid: bool,
    pub inopen: bool,
    pub inwater: bool,
    pub fraction: f32,
    pub endpos: Vec3,
    pub plane_normal: Vec3,
    pub plane_dist: f32,
    /// Physent index (-1 = none).
    pub ent: i32,
    pub deltavelocity: Vec3,
    pub hitgroup: i32,
}

impl PmTrace {
    fn empty(end: Vec3) -> Self {
        Self {
            allsolid: false,
            startsolid: false,
            inopen: false,
            inwater: false,
            fraction: 1.0,
            endpos: end,
            plane_normal: [0.0; 3],
            plane_dist: 0.0,
            ent: -1,
            deltavelocity: [0.0; 3],
            hitgroup: 0,
        }
    }
}

/// `usehull` → BSP hull number (`PM_HullForBsp` switch).
fn bsp_hull_index(usehull: i32) -> usize {
    match usehull {
        1 => 3,
        2 => 0,
        3 => 2,
        _ => 1,
    }
}

fn hull_slot(usehull: i32) -> usize {
    usize::try_from(usehull).ok().filter(|&h| h < 4).unwrap_or(0)
}

/// Collision shape of one physent for the current hull.
enum Shape<'a> {
    Bsp(Hull<'a>),
    Box(BoxHull),
}

impl Shape<'_> {
    fn hull(&self) -> Hull<'_> {
        match self {
            Shape::Bsp(h) => *h,
            Shape::Box(b) => b.hull(),
        }
    }
}

/// `PM_HullForBsp` + inlined `PM_HullForBox`: hull and world offset of `pe`.
fn shape<'a>(world: &'a World, pe: &Physent, usehull: i32) -> Option<(Shape<'a>, Vec3)> {
    let slot = hull_slot(usehull);
    match pe.model {
        Some(model) => {
            let hull = world.hull(model, bsp_hull_index(usehull))?;
            let offset = [0, 1, 2].map(|k| (hull.clip_mins[k] - PLAYER_MINS[slot][k]) + pe.origin[k]);
            Some((Shape::Bsp(hull), offset))
        }
        None => {
            let mins = sub(pe.mins, PLAYER_MAXS[slot]);
            let maxs = sub(pe.maxs, PLAYER_MINS[slot]);
            Some((Shape::Box(BoxHull::new(mins, maxs)), pe.origin))
        }
    }
}

fn rotated(pe: &Physent) -> bool {
    pe.solid == SOLID_BSP && pe.angles.iter().any(|&a| a != 0.0)
}

/// Into the entity's rotated frame: `(dot(v,f), -dot(v,r), dot(v,u))`.
fn rotate(v: Vec3, (f, r, u): (Vec3, Vec3, Vec3)) -> Vec3 {
    [dot(v, f), -dot(v, r), dot(v, u)]
}

/// The studio hulls `_PM_PlayerTrace` uses for `pe` (§B.8 step 5), `None` for a box or BSP.
fn trace_hulls(pe: &Physent, usehull: i32, flags: i32) -> Option<Vec<(BoxHull, i32)>> {
    let sp = pe.studiomodel.as_ref().filter(|_| pe.model.is_none())?;
    (sp.trace_hitbox() || (usehull == 2 && flags & trace_flags::STUDIO_BOX == 0)).then(|| studio_hulls(sp, pe.angles, pe.origin, usehull))
}

/// One physent's hull check from `start_l` to `end_l` (§B.8 step 8): a single hull, or every
/// hitbox hull keeping the closest (an all/start-solid test always replaces), with the hit
/// group of the kept one.
fn hull_trace(world: &World, pe: &Physent, usehull: i32, hulls: Option<&[(BoxHull, i32)]>, start_l: Vec3, end_l: Vec3, end: Vec3) -> Option<(gs_world::Trace, i32)> {
    let Some(hulls) = hulls else {
        let (shape, _) = shape(world, pe, usehull)?;
        return Some((shape.hull().trace(start_l, end_l, end, Variant::Pm), 0));
    };
    match hulls {
        [] => Some((gs_world::Trace { allsolid: false, startsolid: false, inopen: false, inwater: false, fraction: 1.0, endpos: end, plane_normal: [0.0; 3], plane_dist: 0.0 }, 0)),
        [(hull, _)] => Some((hull.hull().trace(start_l, end_l, end, Variant::Pm), 0)),
        _ => {
            let mut best: Option<gs_world::Trace> = None;
            let mut closest = 0;
            for (j, (hull, _)) in hulls.iter().enumerate() {
                let test = hull.hull().trace(start_l, end_l, end, Variant::Pm);
                match &mut best {
                    None => best = Some(test),
                    Some(t) if test.allsolid || test.startsolid || test.fraction < t.fraction => {
                        let keep = t.startsolid;
                        *t = test;
                        t.startsolid |= keep;
                        closest = j;
                    }
                    Some(_) => {}
                }
            }
            Some((best?, hulls[closest].1))
        }
    }
}

/// `_PM_PlayerTrace`: trace the current hull against every physent; the strictly smallest
/// fraction wins and `ent` is the physent index.
pub fn player_trace(world: &World, physents: &[Physent], usehull: i32, start: Vec3, end: Vec3, flags: i32, ignore_pe: i32) -> PmTrace {
    let mut total = PmTrace::empty(end);
    for (i, pe) in physents.iter().enumerate() {
        if i > 0 && flags & trace_flags::WORLD_ONLY != 0 {
            break;
        }
        if ignore_pe != -1 && i as i32 == ignore_pe {
            continue;
        }
        if pe.model.is_some() && pe.solid == 0 && pe.skin != 0 {
            continue;
        }
        if flags & trace_flags::GLASS_IGNORE != 0 && pe.rendermode != 0 {
            continue;
        }
        if pe.model.is_none() && pe.studiomodel.is_some() && flags & trace_flags::STUDIO_IGNORE != 0 {
            continue;
        }
        let hulls = trace_hulls(pe, usehull, flags);
        let offset = match hulls {
            Some(_) => [0.0; 3],
            None => match shape(world, pe, usehull) {
                Some((_, offset)) => offset,
                None => continue,
            },
        };
        let mut start_l = sub(start, offset);
        let mut end_l = sub(end, offset);
        let rot = rotated(pe);
        if rot {
            let basis = angle_vectors(pe.angles);
            start_l = rotate(start_l, basis);
            end_l = rotate(end_l, basis);
        }
        let Some((t, hitgroup)) = hull_trace(world, pe, usehull, hulls.as_deref(), start_l, end_l, end) else { continue };
        let mut tr = PmTrace {
            allsolid: t.allsolid,
            startsolid: t.startsolid,
            inopen: t.inopen,
            inwater: t.inwater,
            fraction: t.fraction,
            endpos: t.endpos,
            plane_normal: t.plane_normal,
            plane_dist: t.plane_dist,
            ent: -1,
            deltavelocity: [0.0; 3],
            hitgroup,
        };
        if tr.allsolid {
            tr.startsolid = true;
        }
        if tr.startsolid {
            tr.fraction = 0.0;
        }
        if tr.fraction != 1.0 {
            if rot {
                let (f, r, u) = angle_vectors_transpose(pe.angles);
                let n = tr.plane_normal;
                tr.plane_normal = [dot(f, n), dot(r, n), dot(u, n)];
            }
            tr.endpos = [0, 1, 2].map(|k| (end[k] - start[k]) * tr.fraction + start[k]);
        }
        if tr.fraction < total.fraction {
            total = tr;
            total.ent = i as i32;
        }
    }
    total
}

/// `_PM_TestPlayerPosition`: index of the first physent whose hull contains `pos`, or -1.
/// The trace it fills is run from `origin` (the pmove origin), not `pos`, like the original.
/// Studio hulls apply for hitbox models and the point hull, without the trace's flags.
pub fn test_player_position(world: &World, physents: &[Physent], usehull: i32, origin: Vec3, pos: Vec3) -> (i32, PmTrace) {
    let tr = player_trace(world, physents, usehull, origin, origin, trace_flags::NORMAL, -1);
    for (i, pe) in physents.iter().enumerate() {
        if pe.model.is_some() && pe.solid == 0 && pe.skin != 0 {
            continue;
        }
        let studio = pe.studiomodel.as_ref().filter(|sp| pe.model.is_none() && (sp.trace_hitbox() || usehull == 2));
        if let Some(sp) = studio {
            let hulls = studio_hulls(sp, pe.angles, pe.origin, usehull);
            if hulls.iter().any(|(h, _)| {
                let hull = h.hull();
                hull.point_contents(hull.first, pos, true) == contents::SOLID
            }) {
                return (i as i32, tr);
            }
            continue;
        }
        let Some((shape, offset)) = shape(world, pe, usehull) else { continue };
        let mut test = sub(pos, offset);
        if rotated(pe) {
            test = rotate(test, angle_vectors(pe.angles));
        }
        let hull = shape.hull();
        if hull.point_contents(hull.first, test, true) == contents::SOLID {
            return (i as i32, tr);
        }
    }
    (-1, tr)
}

/// `PM_LinkContents`: contents (`skin`) and entity of the first non-solid brush containing `p`.
fn link_contents(world: &World, physents: &[Physent], p: Vec3) -> Option<(i32, i32)> {
    physents.iter().skip(1).find_map(|pe| {
        let model = pe.model.filter(|_| pe.solid == 0)?;
        let hull = world.hull(model, 0)?;
        let test = sub(p, pe.origin);
        (hull.point_contents(hull.first, test, true) != contents::EMPTY).then_some((pe.skin, pe.info))
    })
}

/// `PM_PointContents(p, &truecontents)`: world contents with currents folded to water, overridden
/// by a containing contents brush. Returns `(contents, truecontents)`.
pub fn point_contents(world: &World, physents: &[Physent], p: Vec3) -> (i32, i32) {
    let Some(hull) = world.hull(0, 0) else { return (contents::EMPTY, contents::EMPTY) };
    let truec = hull.point_contents(hull.first, p, true);
    let mut c = truec;
    if (contents::CURRENT_DOWN..=contents::CURRENT_0).contains(&c) {
        c = contents::WATER;
    }
    if c == contents::SOLID {
        return (c, truec);
    }
    match link_contents(world, physents, p) {
        Some((e, _)) if e != contents::EMPTY => (e, truec),
        _ => (c, truec),
    }
}

/// `PM_WaterEntity`: -1 solid/none, 0 world liquid, else the entity number of the first liquid
/// brush containing `p`.
pub fn water_entity(world: &World, physents: &[Physent], p: Vec3) -> i32 {
    let Some(hull) = world.hull(0, 0) else { return -1 };
    let c = hull.point_contents(hull.first, p, true);
    if c == contents::SOLID {
        return -1;
    }
    let base = if c < contents::SOLID { 0 } else { -1 };
    link_contents(world, physents, p).map_or(base, |(_, info)| info)
}

/// `PM_TraceModel(pe, start, end)` with the point hull and `SV_RecursiveHullCheck`; no rotation.
pub fn trace_model(world: &World, pe: &Physent, start: Vec3, end: Vec3) -> Option<gs_world::Trace> {
    let (shape, offset) = shape(world, pe, 2)?;
    let hull = shape.hull();
    Some(hull.trace(sub(start, offset), sub(end, offset), end, Variant::Sv))
}

/// `PM_TraceTexture(ground, start, end)` (`EV_TraceTexture`): texture name of the surface the
/// segment crosses on physent `ground`'s brush model.
pub fn trace_texture<'a>(world: &'a World, physents: &[Physent], ground: i32, start: Vec3, end: Vec3) -> Option<&'a str> {
    let pe = physents.get(usize::try_from(ground).ok()?)?;
    let model = pe.model?;
    let (mut s, mut e) = (start, end);
    let head = world.models.get(model)?.head_node;
    if ground >= 1 {
        let hull = world.hull(model, 0)?;
        let offset = [0, 1, 2].map(|k| hull.clip_mins[k] + pe.origin[k]);
        s = sub(s, offset);
        e = sub(e, offset);
        if pe.angles.iter().any(|&a| a != 0.0) {
            let basis = angle_vectors(pe.angles);
            s = rotate(s, basis);
            e = rotate(e, basis);
        }
    }
    let face = world.surface_at(head, s, e)?;
    Some(world.face_texture(face))
}
