//! `V_CalcRefdef` (docs/re/client-sim.md "view" §4–§7, docs/re/view-effects.md §2, §5): the
//! camera and view-model placement for a living player (`V_CalcNormalRefdef`: bob, screen
//! shake, water offset, view roll, idle sway, `scr_ofs*`, the third-person offset, gun angles,
//! punch, stair smoothing), intermission, and the spectator cameras (`V_CalcSpectatorRefdef`:
//! chase, free look, in-eye and the overview cameras, for the main view and the
//! picture-in-picture inset pass).

use gs_protocol::messages::MoveVars;

/// Observer modes (`g_iUser1`).
pub mod obs {
    pub const CHASE_LOCKED: i32 = 1;
    pub const CHASE_FREE: i32 = 2;
    pub const ROAMING: i32 = 3;
    pub const IN_EYE: i32 = 4;
    pub const MAP_FREE: i32 = 5;
    pub const MAP_CHASE: i32 = 6;
}

/// `m_iObserverFlags` director bits.
pub mod drc_flag {
    pub const SIDE: i32 = 0x10;
    pub const DRAMATIC: i32 = 0x20;
    pub const FACEPLAYER: i32 = 0x80;
    pub const FINAL: i32 = 0x200;
}

/// `v_cameraMode`.
const CAM_MODE_RELAX: i32 = 1;
const CAM_MODE_FOCUS: i32 = 2;
/// `SOLID_BSP`.
const SOLID_BSP: i32 = 4;

/// The idle-sway `cvar_t`s the client never registers (`v_iroll_cycle` … `v_iyaw_level`).
const IROLL_CYCLE: f32 = 0.5;
const IPITCH_CYCLE: f32 = 1.0;
const IYAW_CYCLE: f32 = 2.0;
const IROLL_LEVEL: f32 = 0.1;
const IPITCH_LEVEL: f32 = 0.3;
const IYAW_LEVEL: f32 = 0.3;

/// `V_AddIdle` (inlined): roll, pitch, yaw, each `f32(sin(f32(t·cycle))·s·level + angle)` in
/// double.
fn add_idle(angles: &mut [f32; 3], t: f32, s: f32) {
    let s = f64::from(s);
    angles[2] = (f64::from(t * IROLL_CYCLE).sin() * s * f64::from(IROLL_LEVEL) + f64::from(angles[2])) as f32;
    angles[0] = (f64::from(t * IPITCH_CYCLE).sin() * s * f64::from(IPITCH_LEVEL) + f64::from(angles[0])) as f32;
    angles[1] = (f64::from(t * IYAW_CYCLE).sin() * s * f64::from(IYAW_LEVEL) + f64::from(angles[1])) as f32;
}

/// What the spectator camera reads of a `cl_entity_t`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SpecEntity {
    pub index: i32,
    pub player: bool,
    /// Interpolated (rendered) origin and angles.
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    /// `curstate.solid`, `curstate.usehull`.
    pub solid: i32,
    pub usehull: i32,
    /// `curstate` / `prevstate` message times and origins (in-eye velocity estimate).
    pub msg_time: f32,
    pub prev_msg_time: f32,
    pub cur_origin: [f32; 3],
    pub prev_origin: [f32; 3],
    /// `curstate.weaponmodel` (the HLTV in-eye view model).
    pub weaponmodel: i32,
}

/// `PM_TraceLine` result for the chase camera.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LineTrace {
    pub endpos: [f32; 3],
    pub normal: [f32; 3],
    /// Physent index hit (-1 none, 0 the world).
    pub ent: i32,
    /// `PM_GetPhysEntInfo(ent)`: the entity number.
    pub info: i32,
}

/// Engine services of the spectator camera.
pub trait SpecEnv {
    /// `GetEntityByIndex`.
    fn entity(&self, index: i32) -> Option<SpecEntity>;
    /// `PM_TraceLine(start, end, PM_TRACELINE_PHYSENTSONLY, hull 2, ignore)`.
    fn trace_line(&self, start: [f32; 3], end: [f32; 3], ignore: i32) -> LineTrace;
    /// `V_FindViewModelByWeaponModel` + `EV_FindModelIndex`: the `v_` model index of a `p_`
    /// weapon model (0 when there is none).
    fn view_model_for(&self, weaponmodel: i32) -> i32;
}

/// Engine services of `V_CalcNormalRefdef`.
pub trait ViewEngine {
    /// `V_CalcShake`.
    fn calc_shake(&mut self);
    /// `V_ApplyShake(origin, angles, factor)`.
    fn apply_shake(&self, origin: &mut [f32; 3], angles: &mut [f32; 3], factor: f32);
    /// `PM_WaterEntity(p)` + `GetEntityByIndex`: that entity's `curstate.scale` when it is in
    /// `0..max_entities` and has a model.
    fn water_entity_scale(&self, p: [f32; 3]) -> Option<f32>;
    /// `PM_PointContents(p, NULL)`.
    fn point_contents(&self, p: [f32; 3]) -> i32;
}

/// Spectator inputs of `V_CalcSpectatorRefdef`.
pub struct SpecParams<'a> {
    /// `g_iUser1..3`: observer mode, target, second target.
    pub user1: i32,
    pub user2: i32,
    pub user3: i32,
    /// `GetLocalPlayer()->index`.
    pub local_index: i32,
    /// `spec_autodirector_internal`.
    pub autodirector: bool,
    /// `cl_chasedist`.
    pub chase_dist: f32,
    /// Overview `origin` and `zoom` (map modes).
    pub map_origin: [f32; 3],
    pub map_zoom: f32,
    pub env: &'a dyn SpecEnv,
    /// `IsSpectateOnly()`: an HLTV observer.
    pub hltv: bool,
    /// `vJumpOrigin` / `vJumpAngles`.
    pub jump: ([f32; 3], [f32; 3]),
    /// The roaming camera after `CHudSpectator::GetDirectorCamera` (position, angles).
    pub roaming: ([f32; 3], [f32; 3]),
    /// `m_cameraOrigin` / `m_cameraAngles` (the HLTV intermission view).
    pub camera: ([f32; 3], [f32; 3]),
    /// `(int)spec_pip`: the inset mode.
    pub pip: i32,
}

/// Per-frame inputs (`ref_params_t` fields that are used).
pub struct RefParams<'a> {
    /// `cl.time`.
    pub time: f32,
    pub frametime: f32,
    pub simorg: [f32; 3],
    pub simvel: [f32; 3],
    /// `clientdata.view_ofs`.
    pub viewheight: [f32; 3],
    pub cl_viewangles: [f32; 3],
    pub punchangle: [f32; 3],
    pub health: f32,
    pub onground: bool,
    pub movevars: &'a MoveVars,
    /// `rp.viewentity` (`cl.viewentity`) and the linked origin and angles of that entity
    /// (`GetEntityByIndex`): view roll, and the camera of a non-player view entity.
    pub viewentity: i32,
    pub viewentity_pose: Option<([f32; 3], [f32; 3])>,
    /// `cl.intermission`.
    pub intermission: bool,
    /// `cl.paused`.
    pub paused: bool,
    /// `rp.waterlevel`.
    pub waterlevel: i32,
    /// `cl.crosshairangle` (`svc_crosshairangle`).
    pub crosshair_angle: [f32; 3],
    /// `v_idlescale` (`CHud::m_iConcussionEffect`).
    pub idle_scale: f32,
    /// `cl_waterdist`.
    pub waterdist: f32,
    /// Set while observing (`g_iUser1 != 0`) or on an HLTV connection (`rp.spectator`).
    pub spectator: Option<SpecParams<'a>>,
    /// `CL_CameraOffset` while `cam_thirdperson` (`CL_IsThirdPerson` outside observing).
    pub cam_ofs: Option<[f32; 3]>,
    /// `cl_bob`, `cl_bobcycle`, `cl_bobup`.
    pub bob: [f32; 3],
    /// `scr_ofsx`, `scr_ofsy`, `scr_ofsz` (used only with `maxclients <= 1`).
    pub scr_ofs: [f32; 3],
    /// `rp.maxclients`.
    pub maxclients: i32,
}

/// Camera and view model placement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefDef {
    pub vieworg: [f32; 3],
    pub viewangles: [f32; 3],
    pub gun_origin: [f32; 3],
    /// Studio angles of the view model (pitch pre-inverted).
    pub gun_angles: [f32; 3],
    /// `false` in intermission (`GetViewModel()->model = NULL`).
    pub view_model: bool,
    /// HLTV in-eye: the view model taken from the target's weapon (0 = none), and whether it
    /// changed this frame (`pfnWeaponAnim(0, 0)`).
    pub view_model_index: Option<i32>,
    pub weapon_anim_reset: bool,
    /// `rp.onlyClientDraw`: a map view; the engine draws only the client's triangles.
    pub only_client_draw: bool,
    /// `rp.nextView`: the PIP inset pass follows.
    pub next_view: bool,
    /// `v_angles` (the weapons' `pev->v_angle`): the view angles with the punch, taken before
    /// the third-person camera replaces them; a non-player view entity's angles.
    pub v_angles: [f32; 3],
}

impl RefDef {
    /// A spectator camera: the view model follows the view, no inset flags.
    const fn camera(origin: [f32; 3], angles: [f32; 3]) -> Self {
        Self { vieworg: origin, viewangles: angles, gun_origin: origin, gun_angles: angles, view_model: true, view_model_index: None, weapon_anim_reset: false, only_client_draw: false, next_view: false, v_angles: angles }
    }
}

/// The in-eye prelude's results (`rp.cl_viewangles`, `rp.simvel`, `rp.simorg`, `rp.health`
/// as `V_CalcSpectatorRefdef` rewrites them) and the HLTV view model.
struct InEye {
    angles: [f32; 3],
    vel: [f32; 3],
    org: [f32; 3],
    health: f32,
    view_model_index: Option<i32>,
    weapon_anim_reset: bool,
}

/// `V_DriftPitch`'s state `pd` (all zero at load, so drift starts enabled with no speed).
#[derive(Debug, Clone, Copy, Default)]
pub struct PitchDrift {
    pitchvel: f32,
    nodrift: bool,
    driftmove: f32,
    laststop: f64,
}

/// What `V_DriftPitch` reads besides the view angles.
#[derive(Debug, Clone, Copy)]
pub struct DriftParams {
    /// `GetClientTime()`.
    pub time: f64,
    pub frametime: f32,
    /// `IsNoClipping()`, `rp.onground`, `rp.demoplayback`, `rp.spectator`.
    pub noclip: bool,
    pub onground: bool,
    pub demoplayback: bool,
    pub spectator: bool,
    /// `rp.cmd->forwardmove` and `cl_forwardspeed`.
    pub forwardmove: f32,
    pub forwardspeed: f32,
    /// `v_centermove`, `v_centerspeed`.
    pub centermove: f32,
    pub centerspeed: f32,
    /// `rp.idealpitch` (`cl.idealpitch`).
    pub idealpitch: f32,
}

impl PitchDrift {
    /// `V_StopPitchDrift`.
    pub fn stop(&mut self, time: f64) {
        self.laststop = time;
        self.nodrift = true;
        self.pitchvel = 0.0;
    }

    /// `V_StartPitchDrift` (not in the same client frame as a stop).
    pub fn start(&mut self, time: f64, centerspeed: f32) {
        if self.laststop != time && (self.nodrift || self.pitchvel == 0.0) {
            self.pitchvel = centerspeed;
            self.nodrift = false;
            self.driftmove = 0.0;
        }
    }

    /// `V_DriftPitch`: while drifting, the pitch moves toward the ideal pitch at a speed growing
    /// by `v_centerspeed` per second; while not, running forward at full speed for longer
    /// than `v_centermove` starts it. The engine keeps the changed `cl_viewangles`.
    pub fn drift(&mut self, p: &DriftParams, cl_viewangles: &mut [f32; 3]) {
        if p.noclip || !p.onground || p.demoplayback || p.spectator {
            self.driftmove = 0.0;
            self.pitchvel = 0.0;
            return;
        }
        if self.nodrift {
            self.driftmove = if p.forwardmove.abs() >= p.forwardspeed { self.driftmove + p.frametime } else { 0.0 };
            if self.driftmove > p.centermove {
                self.start(p.time, p.centerspeed);
            }
            return;
        }
        let delta = p.idealpitch - cl_viewangles[0];
        if delta == 0.0 {
            self.pitchvel = 0.0;
            return;
        }
        let mut step = p.frametime * self.pitchvel;
        self.pitchvel += p.frametime * p.centerspeed;
        if delta > 0.0 {
            if step > delta {
                self.pitchvel = 0.0;
                step = delta;
            }
            cl_viewangles[0] += step;
        } else {
            if step > -delta {
                self.pitchvel = 0.0;
                step = -delta;
            }
            cl_viewangles[0] -= step;
        }
    }
}

/// Statics of `V_CalcBob`, the stair smoothing and the spectator cameras.
#[derive(Debug, Clone, Copy)]
pub struct ViewState {
    bobtime: f64,
    bob: f32,
    bob_lasttime: f32,
    oldz: f32,
    step_lasttime: f32,
    /// `v_lastDistance`, `v_resetCamera`, `v_lastAngles`, `v_cameraMode`, `v_frametime`.
    last_distance: f32,
    reset_camera: bool,
    last_angles: [f32; 3],
    camera_mode: i32,
    frametime: f32,
    /// The in-eye velocity estimate (a static in `V_CalcSpectatorRefdef`).
    spec_velocity: [f32; 3],
    /// `m_iObserverFlags`.
    pub observer_flags: i32,
    /// The last computed view: kept while paused (`V_CalcNormalRefdef` does not run).
    last: Option<RefDef>,
    /// `lastWeaponModelIndex` / `lastViewModelIndex` of the HLTV in-eye view model.
    last_weapon_model: i32,
    last_view_model: i32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            bobtime: 0.0,
            bob: 0.0,
            bob_lasttime: 0.0,
            oldz: 0.0,
            step_lasttime: 0.0,
            last_distance: 0.0,
            reset_camera: true,
            last_angles: [0.0; 3],
            camera_mode: CAM_MODE_FOCUS,
            frametime: 0.0,
            spec_velocity: [0.0; 3],
            observer_flags: 0,
            last: None,
            last_weapon_model: 0,
            last_view_model: 0,
        }
    }
}

pub fn angle_vectors(a: [f32; 3]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let (sy, cy) = a[1].to_radians().sin_cos();
    let (sp, cp) = a[0].to_radians().sin_cos();
    let (sr, cr) = a[2].to_radians().sin_cos();
    let forward = [cp * cy, cp * sy, -sp];
    let right = [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp];
    let up = [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp];
    (forward, right, up)
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(v: [f32; 3], s: f32) -> [f32; 3] {
    v.map(|c| c * s)
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = sub(a, b);
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len == 0.0 { v } else { scale(v, 1.0 / len) }
}

/// `NormalizeAngles`: one wrap step into [-180, 180].
fn normalize_angles(a: [f32; 3]) -> [f32; 3] {
    a.map(|v| {
        if v > 180.0 {
            v - 360.0
        } else if v < -180.0 {
            v + 360.0
        } else {
            v
        }
    })
}

/// `VectorAngles`: pitch and yaw in 0..360.
fn vector_angles(f: [f32; 3]) -> [f32; 3] {
    if f[0] == 0.0 && f[1] == 0.0 {
        return [if f[2] > 0.0 { 90.0 } else { 270.0 }, 0.0, 0.0];
    }
    let mut yaw = f[1].atan2(f[0]).to_degrees();
    if yaw < 0.0 {
        yaw += 360.0;
    }
    let mut pitch = f[2].atan2((f[0] * f[0] + f[1] * f[1]).sqrt()).to_degrees();
    if pitch < 0.0 {
        pitch += 360.0;
    }
    [pitch, yaw, 0.0]
}

/// Wraps `d` once into [-180, 180].
fn wrap(d: f32) -> f32 {
    if d > 180.0 {
        d - 360.0
    } else if d < -180.0 {
        d + 360.0
    } else {
        d
    }
}

/// `MaxAngleBetweenAngles`.
fn max_angle_between(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (a, b) = (normalize_angles(a), normalize_angles(b));
    (0..3).map(|i| wrap(b[i] - a[i]).abs()).fold(0.0, f32::max)
}

impl ViewState {
    /// `V_CalcBob`.
    fn calc_bob(&mut self, rp: &RefParams, simvel: [f32; 3]) -> f32 {
        if rp.time == self.bob_lasttime {
            return self.bob;
        }
        let [cl_bob, cl_bobcycle, cl_bobup] = rp.bob;
        let cyc = cl_bobcycle.max(0.1);
        self.bob_lasttime = rp.time;
        self.bobtime += f64::from(rp.frametime);
        let mut c = (self.bobtime - f64::from((self.bobtime / f64::from(cyc)) as i32) * f64::from(cyc)) as f32;
        c /= cyc;
        c = if c < cl_bobup {
            std::f32::consts::PI * c / cl_bobup
        } else {
            std::f32::consts::PI + std::f32::consts::PI * (c - cl_bobup) / (1.0 - cl_bobup)
        };
        let mut b = (simvel[0] * simvel[0] + simvel[1] * simvel[1]).sqrt() * cl_bob;
        b = b * 0.3 + b * 0.7 * c.sin();
        self.bob = b.clamp(-7.0, 4.0);
        self.bob
    }

    /// `V_CalcRefdef`: intermission, then the spectator cameras while observing, else the
    /// first-person view unless paused (the previous frame's view stays).
    pub fn calc_refdef(&mut self, rp: &RefParams, eng: &mut dyn ViewEngine) -> RefDef {
        let r = if rp.intermission {
            Self::intermission(rp)
        } else {
            match &rp.spectator {
                Some(spec) => self.calc_spectator(rp, spec, eng),
                None => match self.last.filter(|_| rp.paused) {
                    Some(last) => last,
                    None => self.calc_normal(rp, rp.cl_viewangles, rp.simvel, rp.simorg, rp.health, eng),
                },
            }
        };
        self.last = Some(r);
        r
    }

    /// The inlined `V_CalcIntermissionRefdef`: the player origin without view height, the
    /// idle sway at scale 1, no view model; an HLTV observer sees the map's start camera.
    fn intermission(rp: &RefParams) -> RefDef {
        let mut viewangles = rp.cl_viewangles;
        add_idle(&mut viewangles, rp.time, 1.0);
        let mut vieworg = rp.simorg;
        if let Some(spec) = rp.spectator.as_ref().filter(|s| s.hltv) {
            (vieworg, viewangles) = spec.camera;
        }
        RefDef { view_model: false, ..RefDef::camera(vieworg, viewangles) }
    }

    /// The water offset of `V_CalcNormalRefdef`: with the eyes at the surface (waterlevel 2)
    /// the camera rises out of the liquid, fully under (3) it sinks into it, by at most
    /// `cl_waterdist` plus the water entity's wave height.
    fn water_offset(rp: &RefParams, simorg: [f32; 3], vieworg: [f32; 3], eng: &dyn ViewEngine) -> f32 {
        const CONTENTS_WATER: i32 = -3;
        if rp.waterlevel < 2 {
            return 0.0;
        }
        let mut dist = rp.waterdist as i32;
        if let Some(scale) = eng.water_entity_scale(simorg) {
            dist = (scale * 16.0 + dist as f32) as i32;
        }
        let d = dist as f32;
        let mut p = vieworg;
        if rp.waterlevel == 2 {
            p[2] = vieworg[2] - d;
            for _ in 0..dist.max(0) {
                if eng.point_contents(p) > CONTENTS_WATER {
                    break;
                }
                p[2] += 1.0;
            }
            (p[2] + d) - vieworg[2]
        } else {
            p[2] = vieworg[2] + d;
            for _ in 0..dist.max(0) {
                if eng.point_contents(p) <= CONTENTS_WATER {
                    break;
                }
                p[2] -= 1.0;
            }
            (p[2] - d) - vieworg[2]
        }
    }

    /// `V_CalcNormalRefdef` with the given input angles, velocity, origin and health (in-eye
    /// spectating substitutes the target's), in the client's operation order (client-sim.md
    /// Part F §4).
    fn calc_normal(
        &mut self,
        rp: &RefParams,
        cl_viewangles: [f32; 3],
        simvel: [f32; 3],
        simorg: [f32; 3],
        health: f32,
        eng: &mut dyn ViewEngine,
    ) -> RefDef {
        let bob = self.calc_bob(rp, simvel);
        let mut viewangles = cl_viewangles;
        let mut vieworg = [simorg[0] + rp.viewheight[0], simorg[1] + rp.viewheight[1], (bob + simorg[2]) + rp.viewheight[2]];
        eng.calc_shake();
        eng.apply_shake(&mut vieworg, &mut viewangles, 1.0);
        // "Don't sit on a plane": +1/32 on each axis, in double.
        for v in &mut vieworg {
            *v = (f64::from(*v) + 0.031_25) as f32;
        }
        let water_offset = Self::water_offset(rp, simorg, vieworg, eng);
        vieworg[2] += water_offset;
        // V_CalcViewRoll
        if let Some((_, ent_angles)) = rp.viewentity_pose {
            let (_, right, _) = angle_vectors(ent_angles);
            let side = simvel[0] * right[0] + simvel[1] * right[1] + simvel[2] * right[2];
            let sign = if side >= 0.0 { 1.0 } else { -1.0 };
            let side = side.abs();
            let (angle, speed) = (rp.movevars.rollangle, rp.movevars.rollspeed);
            let side = if side < speed { side * angle / speed } else { angle };
            viewangles[2] += sign * side;
            if health <= 0.0 && rp.viewheight[2] != 0.0 {
                viewangles[2] = 80.0;
            }
        }
        add_idle(&mut viewangles, rp.time, rp.idle_scale);
        let (forward, right, up) = angle_vectors(cl_viewangles);
        // "Don't allow cheats in multiplayer."
        if rp.maxclients <= 1 {
            for i in 0..3 {
                vieworg[i] += rp.scr_ofs[0] * forward[i] + rp.scr_ofs[1] * right[i] + rp.scr_ofs[2] * up[i];
            }
        }
        // Third person: the camera backs off along the camera angles' forward.
        let cam_angles = rp.cam_ofs.map(|ofs| [ofs[0], ofs[1], 0.0]);
        if let (Some(ofs), Some(angles)) = (rp.cam_ofs, cam_angles) {
            let (cam_forward, _, _) = angle_vectors(angles);
            for i in 0..3 {
                vieworg[i] += cam_forward[i] * -ofs[2];
            }
        }
        // V_CalcGunAngle: toward the crosshair angle (full yaw, a quarter of the pitch), studio
        // pitch inverted, minus the idle sway.
        let mut gun_angles = cl_viewangles;
        gun_angles[1] = rp.crosshair_angle[1] + viewangles[1];
        gun_angles[0] = (f64::from(rp.crosshair_angle[0]) * 0.25 - f64::from(viewangles[0])) as f32;
        let (t, s) = (rp.time, f64::from(rp.idle_scale));
        gun_angles[2] = (f64::from(gun_angles[2]) - f64::from(t * IROLL_CYCLE).sin() * s * f64::from(IROLL_LEVEL)) as f32;
        gun_angles[0] = (f64::from(gun_angles[0]) - f64::from(t * IPITCH_CYCLE).sin() * s * (f64::from(IPITCH_LEVEL) * 0.5)) as f32;
        gun_angles[1] = (f64::from(gun_angles[1]) - f64::from(t * IYAW_CYCLE).sin() * s * f64::from(IYAW_LEVEL)) as f32;
        let mut gun_origin = [simorg[0] + rp.viewheight[0], simorg[1] + rp.viewheight[1], (water_offset + simorg[2]) + rp.viewheight[2]];
        eng.apply_shake(&mut gun_origin, &mut gun_angles, 0.9);
        let b = f64::from(bob);
        for i in 0..3 {
            gun_origin[i] = (f64::from(gun_origin[i]) + b * 0.4 * f64::from(forward[i])) as f32;
        }
        gun_origin[2] += bob;
        gun_angles[1] = (f64::from(gun_angles[1]) - b * 0.5) as f32;
        gun_angles[2] -= bob;
        gun_angles[0] = (f64::from(gun_angles[0]) - b * 0.3) as f32;
        gun_origin[2] -= 1.0;
        for (v, p) in viewangles.iter_mut().zip(rp.punchangle) {
            *v += p;
        }
        // Stair smoothing (`rp.smoothing` is 0 in CS).
        if !rp.onground || simorg[2] - self.oldz <= 0.0 {
            self.oldz = simorg[2];
        } else {
            let steptime = (rp.time - self.step_lasttime).max(0.0);
            self.oldz += steptime * 150.0;
            if self.oldz > simorg[2] {
                self.oldz = simorg[2];
            }
            if simorg[2] - self.oldz > 18.0 {
                self.oldz = simorg[2] - 18.0;
            }
            vieworg[2] += self.oldz - simorg[2];
            gun_origin[2] += self.oldz - simorg[2];
        }
        self.step_lasttime = rp.time;
        // v_angles = rp.viewangles before the third-person camera angles replace the punch, roll
        // and idle sway (there is no view model then).
        let mut v_angles = viewangles;
        if let Some(angles) = cam_angles {
            viewangles = angles;
        }
        // svc_setview to a non-player (trigger_camera): the camera is the entity, without the eye
        // offset and the punch.
        if rp.viewentity > rp.maxclients
            && let Some((origin, angles)) = rp.viewentity_pose
        {
            vieworg = origin;
            viewangles = angles;
            v_angles = angles;
        }
        let view_model = rp.cam_ofs.is_none();
        RefDef { vieworg, viewangles, gun_origin, gun_angles, view_model, view_model_index: None, weapon_anim_reset: false, only_client_draw: false, next_view: false, v_angles }
    }

    /// `V_ResetChaseCam`.
    pub fn reset_chase_cam(&mut self) {
        self.reset_camera = true;
    }

    /// `CHudSpectator::DeathMessage` for the local player: dramatic final shot.
    pub fn local_death(&mut self) {
        self.observer_flags = drc_flag::DRAMATIC | drc_flag::FINAL;
        self.reset_chase_cam();
    }

    /// `V_CalcSpectatorRefdef`'s prelude (every pass): with an in-eye main view or inset and a
    /// target, the velocity estimate, then for a normal client in in-eye only the target's
    /// angles; for HLTV or the in-eye inset the target's eyes, health 1 and its view model.
    fn spectator_prelude(&mut self, rp: &RefParams, spec: &SpecParams) -> InEye {
        let mut r = InEye { angles: rp.cl_viewangles, vel: rp.simvel, org: rp.simorg, health: rp.health, view_model_index: None, weapon_anim_reset: false };
        if (spec.user1 == obs::IN_EYE || spec.pip == 2)
            && let Some(ent) = spec.env.entity(spec.user2)
        {
            let dt = ent.msg_time - ent.prev_msg_time;
            if dt > 0.0 {
                let d = scale(sub(ent.prev_origin, ent.cur_origin), 1.0 / dt);
                self.spec_velocity = [0, 1, 2].map(|i| 0.9 * self.spec_velocity[i] + 0.1 * d[i]);
            }
            r.vel = self.spec_velocity;
            if !spec.hltv && spec.pip != 2 {
                // A normal client takes origin, view height and view model from the server's
                // clientdata of the target; only the angles come from the entity.
                r.angles = [ent.angles[0] * -3.0, ent.angles[1], ent.angles[2]];
            } else {
                if spec.user2 != 0 {
                    r.org = ent.origin;
                    r.angles = [ent.angles[0] * -3.0, ent.angles[1], ent.angles[2]];
                    if ent.solid != 0 {
                        r.org[2] += if ent.usehull == 1 { 12.0 } else { 17.0 };
                    } else {
                        r.org[2] -= 8.0;
                        r.angles[2] = 80.0;
                    }
                } else {
                    (r.org, r.angles) = spec.jump;
                }
                r.health = 1.0;
                if ent.weaponmodel != self.last_weapon_model {
                    self.last_weapon_model = ent.weaponmodel;
                    self.last_view_model = spec.env.view_model_for(ent.weaponmodel);
                    if self.last_view_model == 0 {
                        self.last_weapon_model = 0;
                    } else {
                        r.weapon_anim_reset = true;
                    }
                }
                r.view_model_index = Some(self.last_view_model);
            }
        }
        self.frametime = rp.frametime;
        r
    }

    /// `V_CalcSpectatorRefdef`, main pass: the mode's camera; map modes draw only the client's
    /// triangles; the inset pass follows with PIP on unless we watch ourselves.
    fn calc_spectator(&mut self, rp: &RefParams, spec: &SpecParams, eng: &mut dyn ViewEngine) -> RefDef {
        let eye = self.spectator_prelude(rp, spec);
        let cl_angles = rp.cl_viewangles;
        let mut r = match spec.user1 {
            obs::CHASE_LOCKED => {
                let (o, a) = self.chase_pos(spec, None);
                RefDef::camera(o, a)
            }
            obs::CHASE_FREE => {
                let (o, a) = self.chase_pos(spec, Some(cl_angles));
                RefDef::camera(o, a)
            }
            obs::ROAMING => RefDef::camera(spec.roaming.0, spec.roaming.1),
            obs::IN_EYE => self.in_eye(rp, eye, eng),
            obs::MAP_FREE => {
                let (o, a) = map_free(cl_angles, spec.map_origin, spec.map_zoom);
                RefDef { only_client_draw: true, ..RefDef::camera(o, a) }
            }
            obs::MAP_CHASE => {
                let (o, a) = self.map_chase(spec, cl_angles, rp.vieworg_input());
                RefDef { only_client_draw: true, ..RefDef::camera(o, a) }
            }
            _ => RefDef::camera(rp.simorg, cl_angles),
        };
        r.next_view = spec.pip != 0 && spec.local_index != spec.user2;
        r
    }

    /// `V_CalcNormalRefdef` from the in-eye prelude's values.
    fn in_eye(&mut self, rp: &RefParams, eye: InEye, eng: &mut dyn ViewEngine) -> RefDef {
        let mut r = self.calc_normal(rp, eye.angles, eye.vel, eye.org, eye.health, eng);
        r.view_model_index = eye.view_model_index;
        r.weapon_anim_reset = eye.weapon_anim_reset;
        r
    }

    /// `V_CalcSpectatorRefdef`, inset pass (`rp.nextView`), after `main` filled `rp`: by
    /// `spec_pip` the free chase camera, in-eye, or a map camera (client triangles only).
    pub fn calc_spectator_inset(&mut self, rp: &RefParams, main: &RefDef, eng: &mut dyn ViewEngine) -> Option<RefDef> {
        let spec = rp.spectator.as_ref()?;
        let eye = self.spectator_prelude(rp, spec);
        let cl_angles = rp.cl_viewangles;
        let r = match spec.pip {
            1 => {
                let (o, a) = self.chase_pos(spec, Some(cl_angles));
                RefDef::camera(o, a)
            }
            2 => self.in_eye(rp, eye, eng),
            3 => {
                let (o, a) = map_free(cl_angles, spec.map_origin, spec.map_zoom);
                RefDef { only_client_draw: true, ..RefDef::camera(o, a) }
            }
            4 if spec.user1 == obs::ROAMING => {
                // The map formula over the main view's origin.
                let pitch = 51.25 + 38.75 * (cl_angles[0] / 90.0);
                let angles = [pitch, cl_angles[1], 0.0];
                let mut origin = main.vieworg;
                origin[2] *= (90.0 - pitch) / 90.0;
                let (forward, _, _) = angle_vectors(angles);
                RefDef { only_client_draw: true, ..RefDef::camera(sub(origin, scale(forward, 1536.0)), angles) }
            }
            4 => {
                let (o, a) = self.map_chase(spec, cl_angles, main.vieworg);
                RefDef { only_client_draw: true, ..RefDef::camera(o, a) }
            }
            _ => return None,
        };
        Some(r)
    }

    /// The camera `DrawOverviewEntities` draws the view cone for (the world view's):
    /// in-eye, free chase, the roaming origin, else the locked chase camera. The chase calls
    /// advance the chase smoothing again, as in the original.
    pub fn overview_cone(&mut self, rp: &RefParams) -> Option<([f32; 3], [f32; 3])> {
        let spec = rp.spectator.as_ref()?;
        let cl_angles = rp.cl_viewangles;
        Some(if spec.pip == 2 || spec.user1 == obs::IN_EYE {
            // V_GetInEyePos.
            match (spec.user2, spec.env.entity(spec.user2)) {
                (0, _) => spec.jump,
                (_, None) => (rp.simorg, cl_angles),
                (_, Some(e)) => {
                    let mut origin = e.origin;
                    let mut angles = [e.angles[0] * -3.0, e.angles[1], e.angles[2]];
                    if e.solid != 0 {
                        origin[2] += if e.usehull == 1 { 12.0 } else { 17.0 };
                    } else {
                        angles[2] = 80.0;
                        origin[2] -= 8.0;
                    }
                    (origin, angles)
                }
            }
        } else if spec.pip == 1 || spec.user1 == obs::CHASE_FREE {
            self.chase_pos(spec, Some(cl_angles))
        } else if spec.user1 == obs::ROAMING {
            (rp.simorg, cl_angles)
        } else {
            self.chase_pos(spec, None)
        })
    }

    /// `V_GetChasePos(target, cl_angles)`.
    fn chase_pos(&mut self, spec: &SpecParams, cl_angles: Option<[f32; 3]>) -> ([f32; 3], [f32; 3]) {
        let Some(ent) = spec.env.entity(spec.user2).filter(|_| spec.user2 != 0) else {
            // No target: the jump point (the map start, or where the last target was).
            return spec.jump;
        };
        let out = if ent.index == spec.local_index {
            // The own corpse.
            match spec.env.entity(spec.user3).filter(|_| spec.user3 != 0) {
                Some(killer) => self.death_cam(spec, &ent, Some(&killer)),
                None => self.death_cam(spec, &ent, None),
            }
        } else if !spec.autodirector {
            let angles = cl_angles.unwrap_or([-ent.angles[0], ent.angles[1], ent.angles[2]]);
            let origin = add(ent.origin, [0.0, 0.0, 17.0]);
            (self.chase_origin(spec, angles, origin, spec.chase_dist), angles)
        } else if spec.user3 != 0 {
            let second = spec.env.entity(spec.user3);
            self.directed_chase(spec, &ent, second.as_ref())
        } else {
            if self.reset_camera {
                self.last_distance = 4096.0;
            }
            let out = self.single_target_cam(spec, &ent);
            self.last_angles = out.1;
            out
        };
        self.reset_camera = false;
        out
    }

    /// `V_GetChaseOrigin`: back off from `origin` along `-forward(angles)` until world or a
    /// solid brush entity, passing through players and point entities.
    fn chase_origin(&mut self, spec: &SpecParams, angles: [f32; 3], origin: [f32; 3], dist: f32) -> [f32; 3] {
        let (forward, _, _) = angle_vectors(angles);
        let end = add(origin, scale(forward, -dist));
        let mut start = origin;
        let mut ignore = -1;
        let mut tr = LineTrace::default();
        for _ in 0..8 {
            tr = spec.env.trace_line(start, end, ignore);
            if tr.ent <= 0 {
                break;
            }
            let Some(e) = spec.env.entity(tr.info) else { break };
            if (e.solid == SOLID_BSP && !e.player) || distance(tr.endpos, end) < 1.0 {
                break;
            }
            start = tr.endpos;
            ignore = tr.ent;
        }
        self.last_distance = distance(tr.endpos, origin);
        add(tr.endpos, scale(tr.normal, 4.0))
    }

    /// `V_SmoothInterpolateAngles(start, end, degrees per second)`.
    fn smooth_angles(&self, start: [f32; 3], end: [f32; 3], dps: f32) -> [f32; 3] {
        let (start, end) = (normalize_angles(start), normalize_angles(end));
        let out = [0, 1, 2].map(|i| {
            let d = wrap(end[i] - start[i]);
            let a = d.abs();
            if a <= 0.01 {
                return end[i];
            }
            let mut step = self.frametime * dps;
            if 0.25 * dps > a {
                let f = a / (0.25 * dps);
                step *= f * f;
            }
            if step > a { end[i] } else if d > 0.0 { start[i] + step } else { start[i] - step }
        });
        normalize_angles(out)
    }

    /// `V_GetDeathCam`: circle back from the own corpse, facing the killer (or down).
    fn death_cam(&mut self, spec: &SpecParams, me: &SpecEntity, killer: Option<&SpecEntity>) -> ([f32; 3], [f32; 3]) {
        self.last_distance += 96.0 * self.frametime;
        let dist = if self.reset_camera {
            self.last_distance = 64.0;
            64.0
        } else {
            self.last_distance.min(168.0)
        };
        let org = add(me.origin, [0.0, 0.0, if me.player { 17.0 } else { 0.0 }]);
        let new_angle = match killer {
            Some(k) => {
                let a = vector_angles(sub(k.origin, me.origin));
                [-a[0], a[1], 0.0]
            }
            None => [90.0, 0.0, 0.0],
        };
        let angle = self.smooth_angles(self.last_angles, new_angle, 120.0);
        let origin = self.chase_origin(spec, angle, org, dist);
        self.last_angles = angle;
        (origin, angle)
    }

    /// `V_GetSingleTargetCam`.
    fn single_target_cam(&mut self, spec: &SpecParams, ent: &SpecEntity) -> ([f32; 3], [f32; 3]) {
        let flags = self.observer_flags;
        let dead = ent.player && ent.solid == 0;
        let dramatic = flags & drc_flag::DRAMATIC != 0;
        let (normal, dead_dist, final_dist, pitch_add) = if dramatic { (96.0, 144.0, 192.0, -12.5) } else { (128.0, 192.0, 256.0, 12.5) };
        let max_dist = if flags & drc_flag::FINAL != 0 {
            final_dist
        } else if dead {
            dead_dist
        } else {
            normal
        };
        self.last_distance += 32.0 * self.frametime;
        let dist = f32::min(max_dist, self.last_distance);
        let lift = if ent.player { if dead { 2.0 } else { 17.0 } } else { 8.0 };
        let org = add(ent.origin, [0.0, 0.0, lift]);
        let mut new_angle = ent.angles;
        if flags & drc_flag::FACEPLAYER != 0 {
            new_angle[1] += 180.0;
        }
        new_angle[0] = if flags & drc_flag::FINAL != 0 { 22.5 } else { ent.angles[0] + pitch_add };
        new_angle[1] += if flags & drc_flag::SIDE != 0 { 22.5 } else { -22.5 };
        let angle = self.smooth_angles(self.last_angles, new_angle, 120.0);
        (self.chase_origin(spec, angle, org, dist), angle)
    }

    /// `V_GetDoubleTargetsCam`: both targets in view.
    fn double_targets_cam(&mut self, spec: &SpecParams, ent1: &SpecEntity, ent2: &SpecEntity) -> ([f32; 3], [f32; 3]) {
        let flags = self.observer_flags;
        let dramatic = flags & drc_flag::DRAMATIC != 0;
        let max_dist = if flags & drc_flag::FINAL != 0 {
            if dramatic { 192.0 } else { 256.0 }
        } else if dramatic {
            96.0
        } else {
            128.0
        };
        self.last_distance += 32.0 * self.frametime;
        let dist = f32::min(max_dist, self.last_distance);
        let org = add(ent1.origin, [0.0, 0.0, if ent1.player { 17.0 } else { 8.0 }]);
        let mut new_angle = vector_angles(sub(ent2.origin, ent1.origin));
        new_angle[0] = (if dramatic { -12.5 } else { 12.5 }) - new_angle[0];
        new_angle[1] += if flags & drc_flag::SIDE != 0 { 22.5 } else { -22.5 };
        let last = normalize_angles(self.last_angles);
        let new_angle = normalize_angles(new_angle);
        let d = max_angle_between(last, new_angle);
        let angle = if d < 35.0 && self.camera_mode == CAM_MODE_RELAX {
            last
        } else if d < 5.0 && self.camera_mode == CAM_MODE_FOCUS {
            self.camera_mode = CAM_MODE_RELAX;
            new_angle
        } else {
            self.camera_mode = CAM_MODE_FOCUS;
            if self.reset_camera { new_angle } else { self.smooth_angles(last, new_angle, 180.0) }
        };
        let mut origin = self.chase_origin(spec, new_angle, org, dist);
        if self.last_distance < 64.0 {
            origin = add(origin, scale(normalize(sub(ent2.origin, ent1.origin)), 64.0 - self.last_distance));
        }
        (origin, angle)
    }

    /// `V_GetDirectedChasePosition`: the target alone, with its second object, or a point.
    fn directed_chase(&mut self, spec: &SpecParams, ent1: &SpecEntity, ent2: Option<&SpecEntity>) -> ([f32; 3], [f32; 3]) {
        if self.reset_camera {
            self.last_distance = 4096.0;
        }
        let out = match ent2 {
            _ if ent1.player && ent1.solid == 0 => self.single_target_cam(spec, ent1),
            Some(e2) => self.double_targets_cam(spec, ent1, e2),
            None => {
                let flags = self.observer_flags;
                let dramatic = flags & drc_flag::DRAMATIC != 0;
                let max_dist = if flags & drc_flag::FINAL != 0 {
                    if dramatic { 192.0 } else { 256.0 }
                } else if dramatic {
                    96.0
                } else {
                    128.0
                };
                self.last_distance += 32.0 * self.frametime;
                let dist = f32::min(max_dist, self.last_distance);
                let org = add(ent1.origin, [0.0, 0.0, if ent1.player { 17.0 } else { 8.0 }]);
                let angle = self.last_angles;
                (self.chase_origin(spec, angle, org, dist), angle)
            }
        };
        self.last_angles = out.1;
        out
    }

    /// `V_GetMapChasePosition`.
    fn map_chase(&mut self, spec: &SpecParams, cl_angles: [f32; 3], vieworg: [f32; 3]) -> ([f32; 3], [f32; 3]) {
        let (mut origin, mut angles, k) = match spec.env.entity(spec.user2).filter(|_| spec.user2 != 0) {
            Some(ent) if spec.autodirector => {
                let (_, mut angles) = self.chase_pos(spec, Some(cl_angles));
                angles[0] = 45.0;
                (ent.origin, angles, 0.5)
            }
            Some(ent) => {
                let pitch = 51.25 + 38.75 * (cl_angles[0] / 90.0);
                ([0, 1, 2].map(|i| ent.origin[i]), [pitch, cl_angles[1], cl_angles[2]], (90.0 - pitch) / 90.0)
            }
            None => {
                let pitch = 51.25 + 38.75 * (cl_angles[0] / 90.0);
                (vieworg, [pitch, cl_angles[1], cl_angles[2]], (90.0 - pitch) / 90.0)
            }
        };
        origin[2] *= k;
        angles[2] = 0.0;
        let (forward, _, _) = angle_vectors(angles);
        (sub(origin, scale(normalize(forward), 1536.0)), angles)
    }
}

/// The map-free camera (`V_GetMapFreePosition`): over the overview origin, pitched by the view.
fn map_free(cl_angles: [f32; 3], map_origin: [f32; 3], zoom: f32) -> ([f32; 3], [f32; 3]) {
    let pitch = 51.25 + 38.75 * (cl_angles[0] / 90.0);
    let angles = [pitch, cl_angles[1], cl_angles[2]];
    let target = [map_origin[0], map_origin[1], (90.0 - pitch) / 90.0 * map_origin[2]];
    let (forward, _, _) = angle_vectors(angles);
    (sub(target, scale(normalize(forward), 4096.0 / zoom)), angles)
}

impl RefParams<'_> {
    /// `rp.vieworg` as the engine passes it in (the predicted origin plus view height).
    fn vieworg_input(&self) -> [f32; 3] {
        add(self.simorg, self.viewheight)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No shake; water below `water_top` (none when it is `f32::MIN`).
    struct Dry {
        water_top: f32,
    }

    impl ViewEngine for Dry {
        fn calc_shake(&mut self) {}
        fn apply_shake(&self, _origin: &mut [f32; 3], _angles: &mut [f32; 3], _factor: f32) {}
        fn water_entity_scale(&self, _p: [f32; 3]) -> Option<f32> {
            None
        }
        fn point_contents(&self, p: [f32; 3]) -> i32 {
            if p[2] < self.water_top { -3 } else { -1 }
        }
    }

    fn dry() -> Dry {
        Dry { water_top: f32::MIN }
    }

    fn params(mv: &MoveVars) -> RefParams<'_> {
        RefParams {
            time: 1.0,
            frametime: 0.01,
            simorg: [0.0, 0.0, 100.0],
            simvel: [0.0; 3],
            viewheight: [0.0, 0.0, 17.0],
            cl_viewangles: [10.0, 90.0, 0.0],
            punchangle: [-2.0, 0.0, 0.0],
            health: 100.0,
            onground: true,
            movevars: mv,
            viewentity: 1,
            viewentity_pose: None,
            intermission: false,
            paused: false,
            waterlevel: 0,
            crosshair_angle: [0.0; 3],
            idle_scale: 0.0,
            waterdist: 4.0,
            spectator: None,
            cam_ofs: None,
            bob: [0.01, 0.8, 0.5],
            scr_ofs: [0.0; 3],
            maxclients: 32,
        }
    }

    #[test]
    fn standing_view_has_epsilon_punch_and_inverted_gun_pitch() {
        let mv = MoveVars::default();
        let mut v = ViewState::default();
        let r = v.calc_refdef(&params(&mv), &mut dry());
        assert_eq!(r.vieworg, [0.03125, 0.03125, 117.03125]);
        assert_eq!(r.viewangles, [8.0, 90.0, 0.0], "punch moves the camera only");
        assert_eq!(r.gun_angles, [-10.0, 90.0, 0.0]);
        assert_eq!(r.gun_origin, [0.0, 0.0, 116.0]);
    }

    #[test]
    fn stepping_up_is_smoothed_at_150_units_per_second() {
        let mv = MoveVars::default();
        let mut v = ViewState::default();
        let mut p = params(&mv);
        v.calc_refdef(&p, &mut dry());
        p.time = 1.02;
        p.simorg[2] = 116.0;
        let r = v.calc_refdef(&p, &mut dry());
        assert!((r.vieworg[2] - (100.0 + 3.0 + 17.03125)).abs() < 1e-3, "oldz rose by 0.02·150 = 3 units: {}", r.vieworg[2]);
    }

    #[test]
    fn eyes_at_the_water_surface_rise_out_of_it() {
        let mv = MoveVars::default();
        let mut p = params(&mv);
        p.waterlevel = 2;
        let r = ViewState::default().calc_refdef(&p, &mut Dry { water_top: 115.0 });
        assert_eq!(r.vieworg[2], 119.03125, "two units up, to the first point above the water");
        assert_eq!(r.gun_origin[2], 118.0, "the gun moves with it");
    }

    /// An open world: traces never hit; entity 2 stands at the origin facing +y.
    struct Open;

    impl SpecEnv for Open {
        fn entity(&self, index: i32) -> Option<SpecEntity> {
            (index == 2).then_some(SpecEntity { index: 2, player: true, origin: [0.0; 3], angles: [0.0, 90.0, 0.0], solid: 3, ..SpecEntity::default() })
        }
        fn trace_line(&self, _start: [f32; 3], end: [f32; 3], _ignore: i32) -> LineTrace {
            LineTrace { endpos: end, normal: [0.0; 3], ent: -1, info: 0 }
        }
        fn view_model_for(&self, _weaponmodel: i32) -> i32 {
            0
        }
    }

    #[test]
    fn free_chase_sits_cl_chasedist_behind_the_target_eye() {
        let mv = MoveVars::default();
        let mut v = ViewState::default();
        let mut p = params(&mv);
        p.cl_viewangles = [0.0, 0.0, 0.0];
        p.spectator = Some(SpecParams {
            user1: obs::CHASE_FREE,
            user2: 2,
            user3: 0,
            local_index: 1,
            autodirector: false,
            chase_dist: 112.0,
            map_origin: [0.0; 3],
            map_zoom: 1.0,
            env: &Open,
            hltv: false,
            jump: ([0.0; 3], [0.0; 3]),
            roaming: ([0.0; 3], [0.0; 3]),
            camera: ([0.0; 3], [0.0; 3]),
            pip: 0,
        });
        let r = v.calc_refdef(&p, &mut dry());
        assert_eq!(r.viewangles, [0.0, 0.0, 0.0]);
        assert!((r.vieworg[0] + 112.0).abs() < 1e-3 && r.vieworg[1].abs() < 1e-3 && (r.vieworg[2] - 17.0).abs() < 1e-3, "{:?}", r.vieworg);
    }

    #[test]
    fn intermission_uses_the_origin_without_view_height_and_hides_the_gun() {
        let mv = MoveVars::default();
        let mut p = params(&mv);
        p.intermission = true;
        p.time = 0.0;
        let r = ViewState::default().calc_refdef(&p, &mut dry());
        assert_eq!((r.vieworg, r.viewangles, r.view_model), ([0.0, 0.0, 100.0], [10.0, 90.0, 0.0], false));
    }
}
