//! Clip hulls, point contents and the recursive hull trace (docs/re/movement.md §B.4–B.7).
//! Arithmetic follows the SSE scalar code of hw.dll: f32 everywhere except the split fraction
//! and the backoff step, which are f64.

use crate::contents;

/// `mplane_t` subset: `kind < 3` selects the axial fast path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub normal: [f32; 3],
    pub dist: f32,
    pub kind: u8,
}

impl Plane {
    /// `BoxOnPlaneSide`: bit 1 = some of the box is in front (`>= dist`), bit 2 = some behind.
    pub fn box_on_side(&self, mins: [f32; 3], maxs: [f32; 3]) -> u8 {
        let axis = usize::from(self.kind);
        if axis < 3 {
            return if self.dist <= mins[axis] {
                1
            } else if self.dist >= maxs[axis] {
                2
            } else {
                3
            };
        }
        let mut near = [0.0f32; 3];
        let mut far = [0.0f32; 3];
        for i in 0..3 {
            (far[i], near[i]) = if self.normal[i] < 0.0 { (mins[i], maxs[i]) } else { (maxs[i], mins[i]) };
        }
        let dot = |p: [f32; 3]| self.normal[0] * p[0] + self.normal[1] * p[1] + self.normal[2] * p[2];
        u8::from(dot(far) >= self.dist) | (u8::from(dot(near) < self.dist) << 1)
    }
}

/// `dclipnode_t`: children are node indices (>= 0) or contents (< 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipNode {
    pub plane: i32,
    pub children: [i16; 2],
}

/// `hull_t`.
#[derive(Debug, Clone, Copy)]
pub struct Hull<'a> {
    pub clipnodes: &'a [ClipNode],
    pub planes: &'a [Plane],
    pub first: i32,
    pub last: i32,
    pub clip_mins: [f32; 3],
    pub clip_maxs: [f32; 3],
}

/// `pmtrace_t` / `trace_t` geometry part.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trace {
    pub allsolid: bool,
    pub startsolid: bool,
    pub inopen: bool,
    pub inwater: bool,
    pub fraction: f32,
    pub endpos: [f32; 3],
    pub plane_normal: [f32; 3],
    pub plane_dist: f32,
}

impl Trace {
    /// The state callers set before a hull check: `allsolid = 1`, `fraction = 1`, `endpos = end`.
    pub fn start(end: [f32; 3]) -> Self {
        Self {
            allsolid: true,
            startsolid: false,
            inopen: false,
            inwater: false,
            fraction: 1.0,
            endpos: end,
            plane_normal: [0.0; 3],
            plane_dist: 0.0,
        }
    }
}

/// Which engine copy of the recursive check to emulate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// `PM_RecursiveHullCheck`: empty-hull early out, 0.05 backoff.
    Pm,
    /// `SV_RecursiveHullCheck` (used by `PM_TraceModel`): NaN guard, 0.1 backoff, translucent leaf.
    Sv,
}

const DIST_EPSILON: f64 = 0.03125;

#[inline]
fn plane_dist(plane: &Plane, p: [f32; 3]) -> f32 {
    let d = if plane.kind < 3 {
        p[usize::from(plane.kind)]
    } else {
        (plane.normal[0] * p[0] + plane.normal[1] * p[1]) + plane.normal[2] * p[2]
    };
    d - plane.dist
}

impl Hull<'_> {
    /// `PM_HullPointContents` (`check_empty`) / `SV_HullPointContents`. A bad node number is a
    /// `Sys_Error` in the engine; here it is logged and treated as empty.
    pub fn point_contents(&self, mut num: i32, p: [f32; 3], check_empty: bool) -> i32 {
        if check_empty && self.first >= self.last {
            return contents::EMPTY;
        }
        while num >= 0 {
            if num < self.first || num > self.last {
                log::error!("HullPointContents: bad node number {num}");
                return contents::EMPTY;
            }
            let Some(node) = self.clipnodes.get(num as usize) else { return contents::EMPTY };
            let Some(plane) = self.planes.get(node.plane as usize) else { return contents::EMPTY };
            let d = plane_dist(plane, p);
            num = i32::from(if 0.0 > d { node.children[1] } else { node.children[0] });
        }
        num
    }

    /// `PM_RecursiveHullCheck` / `SV_RecursiveHullCheck`; returns false once the trace was cut.
    #[allow(clippy::too_many_arguments)]
    pub fn recursive_check(
        &self,
        num: i32,
        p1f: f32,
        p2f: f32,
        p1: [f32; 3],
        p2: [f32; 3],
        trace: &mut Trace,
        variant: Variant,
    ) -> bool {
        if num < 0 {
            if num == contents::SOLID {
                trace.startsolid = true;
            } else {
                trace.allsolid = false;
                if num == contents::EMPTY {
                    trace.inopen = true;
                } else if variant == Variant::Pm || num != contents::TRANSLUCENT {
                    trace.inwater = true;
                }
            }
            return true;
        }
        if variant == Variant::Pm && self.first >= self.last {
            trace.allsolid = false;
            trace.inopen = true;
            return true;
        }
        if variant == Variant::Sv && (num < self.first || num > self.last || self.planes.is_empty()) {
            log::error!("SV_RecursiveHullCheck: bad node number {num}");
            return false;
        }
        let Some(node) = self.clipnodes.get(num as usize) else { return false };
        let Some(plane) = self.planes.get(node.plane as usize) else { return false };
        let t1 = plane_dist(plane, p1);
        let t2 = plane_dist(plane, p2);
        if t1 >= 0.0 && t2 >= 0.0 {
            return self.recursive_check(i32::from(node.children[0]), p1f, p2f, p1, p2, trace, variant);
        }
        if t1 < 0.0 && t2 < 0.0 {
            return self.recursive_check(i32::from(node.children[1]), p1f, p2f, p1, p2, trace, variant);
        }
        let eps = if t1 < 0.0 { DIST_EPSILON } else { -DIST_EPSILON };
        let mut frac = ((f64::from(t1) + eps) / f64::from(t1 - t2)) as f32;
        if variant == Variant::Sv && frac.to_bits() & 0x7F80_0000 == 0x7F80_0000 {
            return false;
        }
        // maxss(0, frac) then minss(1, frac); NaN and -0.0 pass through like `clamp`.
        frac = frac.clamp(0.0, 1.0);
        let dpf = p2f - p1f;
        let mut midf = dpf * frac + p1f;
        let mut mid = [0, 1, 2].map(|k| (p2[k] - p1[k]) * frac + p1[k]);
        let side = usize::from(0.0 > t1);
        if !self.recursive_check(i32::from(node.children[side]), p1f, midf, p1, mid, trace, variant) {
            return false;
        }
        let check_empty = variant == Variant::Pm;
        if self.point_contents(i32::from(node.children[side ^ 1]), mid, check_empty) != contents::SOLID {
            return self.recursive_check(i32::from(node.children[side ^ 1]), midf, p2f, mid, p2, trace, variant);
        }
        if trace.allsolid {
            return false;
        }
        if side == 0 {
            trace.plane_normal = plane.normal;
            trace.plane_dist = plane.dist;
        } else {
            trace.plane_normal = plane.normal.map(|n| 0.0 - n);
            trace.plane_dist = -plane.dist;
        }
        let step = if variant == Variant::Pm { 0.05 } else { 0.1 };
        while self.point_contents(self.first, mid, check_empty) == contents::SOLID {
            frac = (f64::from(frac) - step) as f32;
            // `frac < 0` in the engine: a NaN fraction (a NaN endpoint, from server-sent movevars
            // or positions) never satisfies it and the loop spins forever; here it backs up at once.
            if frac < 0.0 || frac.is_nan() {
                trace.fraction = midf;
                trace.endpos = mid;
                log::debug!("Trace backed up past 0.0.");
                return false;
            }
            midf = dpf * frac + p1f;
            mid = [0, 1, 2].map(|k| (p2[k] - p1[k]) * frac + p1[k]);
        }
        trace.fraction = midf;
        trace.endpos = mid;
        false
    }

    /// Runs a full check from the hull's first clip node (`p1f, p2f = 0, 1`).
    pub fn trace(&self, start: [f32; 3], end: [f32; 3], world_end: [f32; 3], variant: Variant) -> Trace {
        let mut tr = Trace::start(world_end);
        self.recursive_check(self.first, 0.0, 1.0, start, end, &mut tr, variant);
        tr
    }
}

/// `box_clipnodes`: plane i splits EMPTY (-1) from the next plane, the last one from SOLID.
const BOX_CLIPNODES: [ClipNode; 6] = [
    ClipNode { plane: 0, children: [-1, 1] },
    ClipNode { plane: 1, children: [2, -1] },
    ClipNode { plane: 2, children: [-1, 3] },
    ClipNode { plane: 3, children: [4, -1] },
    ClipNode { plane: 4, children: [-1, 5] },
    ClipNode { plane: 5, children: [-2, -1] },
];

/// `PM_HullForBox`: an axis-aligned box as a 6-plane hull.
#[derive(Debug, Clone, Copy)]
pub struct BoxHull {
    planes: [Plane; 6],
}

impl BoxHull {
    pub fn new(mins: [f32; 3], maxs: [f32; 3]) -> Self {
        let plane = |i: usize, dist: f32| {
            let mut normal = [0.0; 3];
            normal[i >> 1] = 1.0;
            Plane { normal, dist, kind: (i >> 1) as u8 }
        };
        Self {
            planes: [
                plane(0, maxs[0]),
                plane(1, mins[0]),
                plane(2, maxs[1]),
                plane(3, mins[1]),
                plane(4, maxs[2]),
                plane(5, mins[2]),
            ],
        }
    }

    /// The box clipnode layout over six given planes (`R_StudioHull`'s oriented hitbox hulls:
    /// planes `2c`/`2c+1` share axis `c`'s normal, `type = 5`).
    pub const fn from_planes(planes: [Plane; 6]) -> Self {
        Self { planes }
    }

    pub fn hull(&self) -> Hull<'_> {
        Hull { clipnodes: &BOX_CLIPNODES, planes: &self.planes, first: 0, last: 5, clip_mins: [0.0; 3], clip_maxs: [0.0; 3] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_hull_contents_and_trace() {
        let b = BoxHull::new([-10.0, -10.0, -10.0], [10.0, 10.0, 10.0]);
        let h = b.hull();
        assert_eq!(h.point_contents(0, [0.0; 3], true), contents::SOLID);
        assert_eq!(h.point_contents(0, [20.0, 0.0, 0.0], true), contents::EMPTY);
        let tr = h.trace([-30.0, 0.0, 0.0], [30.0, 0.0, 0.0], [30.0, 0.0, 0.0], Variant::Pm);
        assert!(!tr.startsolid && !tr.allsolid);
        assert_eq!(tr.plane_normal, [-1.0, 0.0, 0.0]);
        assert_eq!(tr.plane_dist, 10.0, "dist of the negated plane: -(-10)");
        // The hit stops DIST_EPSILON short of the face, ~(20 - 1/32) / 60 of the way (the exact
        // float depends on the nested split through the +x plane first).
        assert!((tr.fraction - (19.96875 / 60.0)).abs() < 1e-6, "{}", tr.fraction);
        assert!((tr.endpos[0] + 10.03125).abs() < 1e-4, "{:?}", tr.endpos);
    }

    #[test]
    fn start_inside_is_startsolid() {
        let b = BoxHull::new([-10.0; 3], [10.0; 3]);
        let tr = b.hull().trace([0.0; 3], [5.0, 0.0, 0.0], [5.0, 0.0, 0.0], Variant::Pm);
        assert!(tr.startsolid && tr.allsolid);
    }

    /// A NaN endpoint (server movevars can be NaN) made the backoff loop spin forever.
    #[test]
    fn a_nan_endpoint_backs_up_instead_of_looping() {
        let planes = [Plane { normal: [1.0, 0.0, 0.0], dist: 0.0, kind: 0 }];
        let solid_front = ClipNode { plane: 0, children: [contents::SOLID as i16, contents::EMPTY as i16] };
        let clipnodes = [solid_front, solid_front];
        let hull = Hull { clipnodes: &clipnodes, planes: &planes, first: 0, last: 1, clip_mins: [0.0; 3], clip_maxs: [0.0; 3] };
        let end = [f32::NAN, 0.0, 0.0];
        let tr = hull.trace([-1.0, 0.0, 0.0], end, end, Variant::Pm);
        assert!(!tr.allsolid && tr.fraction.is_nan());
    }

    /// `BoxOnPlaneSide`: the axial shortcut and the corner test agree away from the plane; a box
    /// whose far face lies on an axial plane is behind on the shortcut but on both sides for the
    /// corner test (the engine's own split). A slanted plane picks the corners by normal sign.
    #[test]
    fn box_on_plane_side_shortcut_and_corner_test() {
        let axial = Plane { normal: [1.0, 0.0, 0.0], dist: 8.0, kind: 0 };
        let general = Plane { kind: 3, ..axial };
        for p in [axial, general] {
            assert_eq!(p.box_on_side([9.0, -1.0, -1.0], [12.0, 1.0, 1.0]), 1);
            assert_eq!(p.box_on_side([0.0, -1.0, -1.0], [7.0, 1.0, 1.0]), 2);
            assert_eq!(p.box_on_side([0.0, -1.0, -1.0], [12.0, 1.0, 1.0]), 3);
            assert_eq!(p.box_on_side([8.0, -1.0, -1.0], [12.0, 1.0, 1.0]), 1, "touching from the front");
        }
        assert_eq!(axial.box_on_side([0.0, -1.0, -1.0], [8.0, 1.0, 1.0]), 2);
        assert_eq!(general.box_on_side([0.0, -1.0, -1.0], [8.0, 1.0, 1.0]), 3);
        let slanted = Plane { normal: [-0.6, 0.8, 0.0], dist: 2.0, kind: 3 };
        assert_eq!(slanted.box_on_side([0.0, 0.0, 0.0], [5.0, 5.0, 1.0]), 3);
        assert_eq!(slanted.box_on_side([-10.0, 5.0, 0.0], [-5.0, 10.0, 1.0]), 1);
        assert_eq!(slanted.box_on_side([5.0, -10.0, 0.0], [10.0, -5.0, 1.0]), 2);
    }
}
