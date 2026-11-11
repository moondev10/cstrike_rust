//! `cl_entities`: per-entity state history, latched animation vars and render-time
//! interpolation (docs/re/client-sim.md "engine entity pipeline" §1–§6).

use gs_protocol::state::Frame;
use gs_protocol::structs::EntityState;

pub const HISTORY_MAX: usize = 64;
const HISTORY_MASK: usize = HISTORY_MAX - 1;

pub mod movetype {
    pub const NONE: i32 = 0;
    pub const WALK: i32 = 3;
    pub const STEP: i32 = 4;
    pub const FLY: i32 = 5;
    pub const FOLLOW: i32 = 12;
}

pub mod effects {
    pub const BRIGHTLIGHT: i32 = 4;
    pub const DIMLIGHT: i32 = 8;
    pub const NOINTERP: i32 = 0x20;
    pub const NODRAW: i32 = 0x80;
}

const EFLAG_SLERP: i32 = 1;
const ENTITY_NORMAL: i32 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PositionHistory {
    pub animtime: f32,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
}

/// `latchedvars_t`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Latched {
    pub prevanimtime: f32,
    pub sequencetime: f32,
    pub prevseqblending: [u8; 2],
    pub prevorigin: [f32; 3],
    pub prevangles: [f32; 3],
    pub prevsequence: i32,
    pub prevframe: f32,
    pub prevcontroller: [u8; 4],
    pub prevblending: [u8; 2],
}

/// `cl_entity_t` (the parts the engine pipeline maintains).
#[derive(Debug, Clone)]
pub struct ClEntity {
    pub index: i32,
    pub player: bool,
    pub prevstate: EntityState,
    pub curstate: EntityState,
    current_position: usize,
    ph: Box<[PositionHistory; HISTORY_MAX]>,
    pub latched: Latched,
    pub lastmove: f32,
    /// Rendered origin.
    pub origin: [f32; 3],
    /// Rendered angles.
    pub angles: [f32; 3],
    /// `attachment[4]`: reset to `origin` when linked, set by the studio renderer.
    pub attachment: [[f32; 3]; 4],
}

impl Default for ClEntity {
    fn default() -> Self {
        Self {
            index: 0,
            player: false,
            prevstate: EntityState::default(),
            curstate: EntityState::default(),
            current_position: 0,
            ph: Box::new([PositionHistory::default(); HISTORY_MAX]),
            latched: Latched::default(),
            lastmove: 0.0,
            origin: [0.0; 3],
            angles: [0.0; 3],
            attachment: [[0.0; 3]; 4],
        }
    }
}

/// Kind of a linked (visible) entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Player,
    Normal,
    Beam,
}

/// Inputs of `CL_ProcessPacket` besides the frame.
pub struct PacketContext<'a> {
    /// `cl.mtime[0]`.
    pub mtime: f64,
    /// `cl.time`.
    pub time: f64,
    pub maxclients: usize,
    pub playernum: usize,
    pub model_names: &'a [Option<String>],
    /// Local `clientdata.origin` (`HUD_TxferLocalOverrides`).
    pub local_origin: [f32; 3],
    /// `R_StudioEstimateFrame` of the entity's current sequence (0 when not a studio model).
    pub estimate_frame: &'a dyn Fn(&EntityState) -> f32,
}

/// Inputs of `CL_EmitEntities` linking.
pub struct LinkContext<'a> {
    pub time: f64,
    pub ex_interp: f64,
    pub maxclients: usize,
    pub playernum: usize,
    /// `cl.viewentity`.
    pub view_entity: usize,
    /// Predicted local origin (`cl.simorg`).
    pub simorg: [f32; 3],
    /// Draw the local player (third person / chase cam).
    pub third_person: bool,
    pub model_names: &'a [Option<String>],
    /// Remote server (MOVETYPE_STEP is engine-interpolated in CS unless loopback).
    pub loopback: bool,
    pub estimate_frame: &'a dyn Fn(&EntityState) -> f32,
    /// `cl_fixmodelinterpolationartifacts >= 1`: a SLERP reset keeps the server animtime.
    pub fix_interpolation_artifacts: bool,
    /// `r_bmodelinterp != 0`: brush models are interpolated.
    pub bmodel_interp: bool,
    /// `cls.timedemo`.
    pub timedemo: bool,
    /// `cl.onground` and `cl.onground_moving` from the last prediction.
    pub onground: i32,
    pub onground_moving: bool,
}

fn wrap_once(a: f32) -> f32 {
    if a > 180.0 {
        a - 360.0
    } else if a < -180.0 {
        a + 360.0
    } else {
        a
    }
}

/// `NormalizeAngles`: `fmod` keeps the sign, so huge values are not fully normalised.
fn normalize_angles(a: [f32; 3]) -> [f32; 3] {
    a.map(|v| {
        if v > 180.0 {
            v % 360.0 - 360.0
        } else if v < -180.0 {
            v % 360.0 + 360.0
        } else {
            v
        }
    })
}

/// `VectorAngles`.
fn vector_angles(v: [f32; 3]) -> [f32; 3] {
    if v[1] == 0.0 && v[0] == 0.0 {
        return [if v[2] > 0.0 { 90.0 } else { 270.0 }, 0.0, 0.0];
    }
    let mut yaw = (v[1].atan2(v[0]).to_degrees() as i32) as f32;
    if yaw < 0.0 {
        yaw += 360.0;
    }
    let forward = (v[0] * v[0] + v[1] * v[1]).sqrt();
    let mut pitch = (v[2].atan2(forward).to_degrees() as i32) as f32;
    if pitch < 0.0 {
        pitch += 360.0;
    }
    [pitch, yaw, 0.0]
}

fn is_brush(names: &[Option<String>], modelindex: i32) -> bool {
    usize::try_from(modelindex)
        .ok()
        .and_then(|i| names.get(i))
        .and_then(|n| n.as_deref())
        .is_some_and(|n| n.starts_with('*'))
}

fn is_studio(names: &[Option<String>], modelindex: i32) -> bool {
    usize::try_from(modelindex)
        .ok()
        .and_then(|i| names.get(i))
        .and_then(|n| n.as_deref())
        .is_some_and(|n| n.to_ascii_lowercase().ends_with(".mdl"))
}

impl ClEntity {
    /// `R_UpdateLatchedVars`.
    fn update_latched(&mut self) {
        let (prev, cur) = (&self.prevstate, &self.curstate);
        self.latched.prevorigin = prev.origin();
        self.latched.prevangles = prev.angles();
        self.latched.prevanimtime = prev.animtime();
        if cur.sequence() != prev.sequence() {
            self.latched.sequencetime = cur.animtime();
            self.latched.prevsequence = prev.sequence();
            self.latched.prevseqblending = prev.blending();
        }
        self.latched.prevcontroller = prev.controller();
        self.latched.prevblending = prev.blending();
    }

    /// `R_ResetLatched`.
    fn reset_latched(&mut self, full: bool, mtime: f64, estimate_frame: &dyn Fn(&EntityState) -> f32) {
        if full {
            self.latched.prevcontroller = self.curstate.controller();
            self.latched.prevblending = self.curstate.blending();
            self.latched.sequencetime = self.curstate.animtime();
            self.latched.prevframe = estimate_frame(&self.curstate);
            self.prevstate = self.curstate.clone();
        }
        self.latched.prevsequence = self.curstate.sequence();
        self.latched.prevorigin = self.curstate.origin();
        self.latched.prevangles = self.curstate.angles();
        self.curstate.set_animtime(mtime as f32);
        self.latched.prevanimtime = mtime as f32;
    }

    /// `CL_UpdatePositions`.
    fn push_position(&mut self) {
        self.current_position = (self.current_position + 1) & HISTORY_MASK;
        self.ph[self.current_position] =
            PositionHistory { animtime: self.curstate.animtime(), origin: self.curstate.origin(), angles: self.curstate.angles() };
    }

    /// `CL_ResetPositions`.
    fn reset_positions(&mut self) {
        let keep = self.ph[self.current_position];
        self.ph.fill(PositionHistory::default());
        self.current_position = 1;
        self.ph[0] = keep;
        self.ph[1] = keep;
    }

    /// `CL_EntityTeleported`: signed comparison, as in the original.
    fn teleported(&self) -> bool {
        let (c, p) = (self.curstate.origin(), self.prevstate.origin());
        (0..3).any(|i| c[i] - p[i] > 128.0)
    }

    /// `CL_ParametricMove`.
    fn parametric_move(&mut self, time: f64) -> bool {
        let (start, impact) = (self.curstate.starttime(), self.curstate.impacttime());
        if start == 0.0 || impact == 0.0 {
            return false;
        }
        let (sp, ep) = (self.curstate.startpos(), self.curstate.endpos());
        let dir = [ep[0] - sp[0], ep[1] - sp[1], ep[2] - sp[2]];
        if impact - start != 0.0 {
            let tt = (time as f32).max(self.lastmove);
            let f = ((tt - start) / (impact - start)).clamp(0.0, 1.0);
            self.curstate.set_origin([sp[0] + f * dir[0], sp[1] + f * dir[1], sp[2] + f * dir[2]]);
            self.lastmove = tt;
        }
        self.origin = self.curstate.origin();
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        if len > 0.0 {
            self.curstate.set_angles(vector_angles(dir.map(|v| v / len)));
        }
        true
    }

    /// `CL_FindInterpolationUpdates`: `(newer, older)` history samples around `t`.
    fn interpolation_pair(&self, t: f32) -> (usize, usize) {
        let cp = self.current_position;
        let (mut newer, mut older) = (cp & HISTORY_MASK, cp.wrapping_sub(1) & HISTORY_MASK);
        if self.ph[newer].animtime >= t {
            for i in 1..HISTORY_MAX - 1 {
                let k = cp.wrapping_sub(i) & HISTORY_MASK;
                if self.ph[k].animtime == 0.0 {
                    break;
                }
                if self.ph[k].animtime < t {
                    newer = (k + 1) & HISTORY_MASK;
                    older = k;
                    break;
                }
            }
        }
        (newer, older)
    }

    /// `CL_InterpolateModel`; false means "do not draw this frame" for non-brush entities.
    /// `current` (timedemo, single player, or the moving ground the local player stands on)
    /// keeps the current state without interpolating.
    fn interpolate_model(&mut self, t: f32, current: bool) -> bool {
        self.origin = self.curstate.origin();
        self.angles = self.curstate.angles();
        if current {
            return true;
        }
        let (n, o) = self.interpolation_pair(t);
        let (n, o) = (self.ph[n], self.ph[o]);
        if t - o.animtime < 0.0 {
            return false;
        }
        if o.animtime == 0.0 || (o.origin == [0.0; 3] && n.origin != [0.0; 3]) {
            self.origin = n.origin;
            self.angles = n.angles;
            return false;
        }
        if n.animtime == o.animtime {
            self.origin = n.origin;
            self.angles = n.angles;
            return true;
        }
        let f = (t - o.animtime) / (n.animtime - o.animtime);
        if f < 0.0 {
            return false;
        }
        let f = f.min(1.0);
        for i in 0..3 {
            self.origin[i] = o.origin[i] + f * (n.origin[i] - o.origin[i]);
            let d = wrap_once(n.angles[i] - o.angles[i]);
            self.angles[i] = o.angles[i] + d * f;
        }
        self.angles = normalize_angles(self.angles);
        true
    }

    /// `CL_ComputePlayerOrigin` / `CL_PureOrigin`: up to 20% extrapolation.
    fn compute_player_origin(&mut self, t: f32) {
        let (ni, oi) = self.interpolation_pair(t);
        let (n, o) = (self.ph[ni], self.ph[oi]);
        if n.animtime == 0.0 {
            self.origin = o.origin;
            self.angles = o.angles;
            return;
        }
        let f = if n.animtime == o.animtime { 1.0 } else { (t - o.animtime) / (n.animtime - o.animtime) };
        let f = f.clamp(0.0, 1.2);
        // InterpolateAngles normalises both inputs in place (mutating the history).
        let a = o.angles.map(wrap_once);
        let b = n.angles.map(wrap_once);
        self.ph[oi].angles = a;
        self.ph[ni].angles = b;
        for i in 0..3 {
            self.origin[i] = o.origin[i] + f * (n.origin[i] - o.origin[i]);
            self.angles[i] = wrap_once(a[i] + wrap_once(b[i] - a[i]) * f);
        }
    }

    pub fn history(&self) -> &[PositionHistory; HISTORY_MAX] {
        &self.ph
    }
}

/// `cl_entities` plus the linking pass.
#[derive(Debug, Default)]
pub struct Entities {
    ents: Vec<ClEntity>,
}

impl Entities {
    pub fn clear(&mut self) {
        self.ents.clear();
    }

    pub fn get(&self, index: usize) -> Option<&ClEntity> {
        self.ents.get(index)
    }

    /// `cl.num_entities`: one past the highest entity number allocated this level.
    pub fn num_entities(&self) -> usize {
        self.ents.len()
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut ClEntity> {
        self.ents.get_mut(index)
    }

    /// `CL_ComputePlayerOrigin` at `t` for player entity `index` (prediction physents), without
    /// touching the rendered state.
    pub fn player_position(&self, index: usize, t: f32) -> Option<([f32; 3], [f32; 3])> {
        let mut e = self.ents.get(index)?.clone();
        e.origin = e.curstate.origin();
        e.angles = e.curstate.angles();
        e.compute_player_origin(t);
        Some((e.origin, e.angles))
    }

    fn slot(&mut self, index: usize) -> &mut ClEntity {
        if self.ents.len() <= index {
            self.ents.resize_with(index + 1, ClEntity::default);
        }
        &mut self.ents[index]
    }

    /// `CL_ProcessPacket` for a freshly parsed frame.
    pub fn process_packet(&mut self, frame: &Frame, ctx: &PacketContext) {
        for (i, state) in frame.entities.iter().enumerate() {
            let Ok(number) = usize::try_from(state.number()) else { continue };
            let mut state = state.clone();
            state.set_msg_time(ctx.mtime as f32);
            let player = (1..=ctx.maxclients).contains(&number);
            if player && number - 1 == ctx.playernum {
                state.set_origin(ctx.local_origin);
            }
            let ent = self.slot(number);
            ent.player = player;
            ent.prevstate = std::mem::replace(&mut ent.curstate, state);
            ent.index = number as i32;
            ent.curstate.set_angles(normalize_angles(ent.curstate.angles()));
            let mt = ent.curstate.movetype();
            if is_brush(ctx.model_names, ent.curstate.modelindex())
                || (![movetype::STEP, movetype::NONE, movetype::WALK, movetype::FLY].contains(&mt)
                    && ent.curstate.starttime() == 0.0
                    && ent.curstate.impacttime() == 0.0)
            {
                ent.curstate.set_animtime(ent.curstate.msg_time());
            }
            // (int)(animtime * -1000.0): the float times a double constant, truncated.
            let ms = |t: f32| (f64::from(t) * -1000.0) as i32;
            if (ms(ent.prevstate.animtime()) - ms(ent.curstate.animtime())).abs() > 1 || mt == movetype::NONE {
                ent.update_latched();
                ent.push_position();
            }
            if player {
                let mut a = ent.curstate.angles();
                a[0] /= -3.0;
                ent.curstate.set_angles(a);
            }
            let (origin, angles) = (ent.curstate.origin(), ent.curstate.angles());
            ent.origin = origin;
            ent.parametric_move(ctx.time);
            ent.angles = ent.curstate.angles();
            if frame.new_entity.get(i).copied().unwrap_or(false) {
                ent.latched.prevorigin = origin;
                ent.latched.prevangles = angles;
                ent.reset_latched(true, ctx.mtime, ctx.estimate_frame);
                ent.reset_positions();
                if ent.curstate.starttime() != 0.0 && ent.curstate.impacttime() != 0.0 {
                    ent.lastmove = ctx.time as f32;
                }
            }
        }
    }

    /// `CL_LinkPlayers` + `CL_LinkPacketEntities` + MOVETYPE_FOLLOW fixup: the render list for
    /// this frame, in engine order.
    pub fn link(&mut self, frame: &Frame, ctx: &LinkContext) -> Vec<(usize, LinkKind)> {
        let t = (ctx.time - ctx.ex_interp) as f32;
        let mut out = Vec::new();
        let mtime = frame.time;

        // Players.
        for state in &frame.entities {
            let Ok(number) = usize::try_from(state.number()) else { continue };
            if !(1..=ctx.maxclients).contains(&number) {
                continue;
            }
            let i = number - 1;
            if !(ctx.third_person || i + 1 != ctx.view_entity) {
                continue;
            }
            if state.modelindex() == 0 || state.effects() & effects::NODRAW != 0 {
                continue;
            }
            let ent = self.slot(number);
            ent.index = number as i32;
            if i == ctx.playernum {
                let o = state.origin();
                ent.origin = o;
                ent.angles = ent.curstate.angles();
                ent.curstate.set_origin(o);
                ent.prevstate.set_origin(o);
            }
            if ent.curstate.effects() & effects::NOINTERP != 0 {
                ent.reset_latched(false, mtime, ctx.estimate_frame);
            }
            if ent.teleported() {
                ent.latched.prevorigin = ent.curstate.origin();
                ent.latched.prevangles = ent.curstate.angles();
                ent.reset_positions();
            }
            if i == ctx.playernum {
                ent.origin = ctx.simorg;
            } else {
                ent.origin = ent.curstate.origin();
                ent.angles = ent.curstate.angles();
                ent.compute_player_origin(t);
            }
            ent.attachment = [ent.origin; 4];
            out.push((number, LinkKind::Player));
        }

        // Packet entities.
        for state in &frame.entities {
            let Ok(n) = usize::try_from(state.number()) else { continue };
            if (1..=ctx.maxclients).contains(&n) || state.modelindex() == 0 || state.effects() & effects::NODRAW != 0 {
                continue;
            }
            let brush = is_brush(ctx.model_names, state.modelindex());
            let studio = is_studio(ctx.model_names, state.modelindex());
            let ent = self.slot(n);
            ent.curstate = state.clone();
            let parametric = ent.curstate.impacttime() != 0.0 && ent.curstate.starttime() != 0.0;
            if !parametric {
                if ent.curstate.animtime() == ent.prevstate.animtime() && ent.curstate.origin() != ent.prevstate.origin() {
                    ent.lastmove = (ctx.time + 0.2) as f32;
                }
                if ent.curstate.eflags() & EFLAG_SLERP != 0 && ent.curstate.animtime() != 0.0 && studio {
                    if ctx.time as f32 <= ent.lastmove {
                        ent.curstate.set_movetype(movetype::NONE);
                        let anim = ent.curstate.animtime();
                        ent.reset_latched(true, mtime, ctx.estimate_frame);
                        if ctx.fix_interpolation_artifacts {
                            ent.curstate.set_animtime(anim);
                            ent.latched.prevanimtime = anim;
                        }
                        ent.latched.prevorigin = ent.curstate.origin();
                        ent.latched.prevangles = ent.curstate.angles();
                    } else {
                        ent.curstate.set_movetype(movetype::STEP);
                    }
                }
            }
            let visible = if brush {
                if ctx.bmodel_interp {
                    let current = ctx.timedemo || ctx.maxclients == 1 || (ctx.onground_moving && ctx.onground == ent.index);
                    ent.interpolate_model(t, current);
                } else {
                    ent.origin = ent.curstate.origin();
                    ent.angles = ent.curstate.angles();
                }
                true
            } else if parametric {
                ent.parametric_move(ctx.time);
                ent.origin = ent.curstate.origin();
                ent.angles = ent.curstate.angles();
                true
            } else {
                let mt = ent.curstate.movetype();
                let no_lerp = [movetype::NONE, movetype::WALK, movetype::FLY, movetype::STEP].contains(&mt)
                    && (mt != movetype::STEP || ctx.loopback);
                if no_lerp {
                    ent.origin = ent.curstate.origin();
                    ent.angles = ent.curstate.angles();
                    true
                } else {
                    let current = ctx.timedemo || ctx.maxclients == 1 || (ctx.onground_moving && ctx.onground == ent.index);
                    ent.interpolate_model(t, current)
                }
            };
            if !visible {
                continue;
            }
            if state.entity_type() & ENTITY_NORMAL != 0 {
                if ent.curstate.aiment() != 0 {
                    ent.curstate.set_movetype(movetype::FOLLOW);
                }
                if ent.curstate.effects() & effects::NOINTERP != 0 {
                    ent.reset_latched(false, mtime, ctx.estimate_frame);
                }
                if ent.teleported() {
                    ent.latched.prevorigin = ent.curstate.origin();
                    ent.latched.prevangles = ent.curstate.angles();
                    ent.reset_positions();
                }
                ent.attachment = [ent.origin; 4];
                out.push((n, LinkKind::Normal));
            } else {
                ent.latched.prevsequence = ent.curstate.sequence();
                ent.prevstate = ent.curstate.clone();
                ent.latched.prevorigin = ent.origin;
                ent.latched.prevangles = ent.angles;
                out.push((n, LinkKind::Beam));
            }
        }

        // MOVETYPE_FOLLOW: origin only, after the parent was interpolated.
        for &(n, _) in &out {
            let (aiment, mt) = (self.ents[n].curstate.aiment(), self.ents[n].curstate.movetype());
            if aiment != 0
                && mt == movetype::FOLLOW
                && let Some(parent) = usize::try_from(aiment).ok().and_then(|a| self.ents.get(a))
            {
                let o = parent.origin;
                self.ents[n].origin = o;
            }
        }
        out
    }
}

/// `CL_FxBlend` (docs/re/render.md §C.3.1): the entity's `r_blend`·255 for this frame. Fade and
/// solid effects write `curstate.renderamt` back, like the original. `random(lo, hi)` is the
/// engine's shared `RandomLong`.
pub fn fx_blend(
    ent: &mut ClEntity,
    time: f64,
    view_origin: [f32; 3],
    view_forward: [f32; 3],
    random: &mut dyn FnMut(i32, i32) -> i32,
) -> i32 {
    let s = &mut ent.curstate;
    let amt = s.renderamt();
    let offset = f64::from((s.number() as f32) * 363.0);
    let sin = |k: f64| (k * time + offset).sin();
    let blend = match s.renderfx() {
        1 => amt + (16.0 * sin(2.0)) as i32,
        2 => amt + (16.0 * sin(8.0)) as i32,
        3 => amt + (64.0 * sin(2.0)) as i32,
        4 => amt + (64.0 * sin(8.0)) as i32,
        5 => {
            s.set_renderamt((amt - 1).max(0));
            s.renderamt()
        }
        6 => {
            s.set_renderamt(if amt > 3 { amt - 4 } else { 0 });
            s.renderamt()
        }
        7 => {
            s.set_renderamt(if amt >= 255 { 255 } else { amt + 1 });
            s.renderamt()
        }
        8 => {
            s.set_renderamt(if amt >= 252 { 255 } else { amt + 4 });
            s.renderamt()
        }
        9..=11 => {
            let k = match s.renderfx() {
                9 => 4.0,
                10 => 16.0,
                _ => 36.0,
            };
            if ((20.0 * sin(k)) as i32) < 0 { 0 } else { amt }
        }
        12 => {
            if ((20.0 * (sin(17.0) + (2.0 * time).sin())) as i32) < 0 { 0 } else { amt }
        }
        13 => {
            if ((20.0 * (sin(23.0) + (16.0 * time).sin())) as i32) < 0 { 0 } else { amt }
        }
        15 => {
            s.set_renderamt(180);
            180 + random(-32, 31)
        }
        16 => {
            let o = ent.origin;
            let d: f32 = (0..3).map(|i| (o[i] - view_origin[i]) * view_forward[i]).sum();
            if d <= 0.0 {
                return 0;
            }
            s.set_renderamt(180);
            if d > 100.0 { ((1.0 - (d - 100.0) * 0.0025) * 180.0) as i32 + random(-32, 31) } else { 180 + random(-32, 31) }
        }
        _ => amt,
    };
    blend.clamp(0, 255)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent_with_history(samples: &[(f32, f32)]) -> ClEntity {
        let mut e = ClEntity::default();
        for &(t, x) in samples {
            e.curstate.set_animtime(t);
            e.curstate.set_origin([x, 0.0, 0.0]);
            e.push_position();
        }
        e
    }

    #[test]
    fn model_interpolation_brackets_and_never_extrapolates() {
        let mut e = ent_with_history(&[(1.0, 0.0), (1.1, 10.0), (1.2, 20.0)]);
        assert!(e.interpolate_model(1.15, false));
        assert!((e.origin[0] - 15.0).abs() < 1e-3);
        assert!(e.interpolate_model(1.5, false));
        assert_eq!(e.origin[0], 20.0, "clamped to the newest sample");
        assert!(!e.interpolate_model(0.5, false), "older than every sample: not drawn");
    }

    #[test]
    fn players_extrapolate_up_to_twenty_percent() {
        let mut e = ent_with_history(&[(1.0, 0.0), (1.1, 10.0)]);
        e.compute_player_origin(1.5);
        assert!((e.origin[0] - 12.0).abs() < 1e-3);
    }

    #[test]
    fn reset_positions_duplicates_the_newest_sample() {
        let mut e = ent_with_history(&[(1.0, 0.0), (1.1, 10.0)]);
        e.reset_positions();
        assert!(!e.interpolate_model(1.05, false), "a fresh history hides the entity until t reaches it");
        assert!(e.interpolate_model(1.2, false));
        assert_eq!(e.origin[0], 10.0);
    }

    #[test]
    fn teleport_test_is_signed() {
        let mut e = ClEntity::default();
        e.prevstate.set_origin([0.0, 0.0, 0.0]);
        e.curstate.set_origin([-500.0, 0.0, 0.0]);
        assert!(!e.teleported());
        e.curstate.set_origin([500.0, 0.0, 0.0]);
        assert!(e.teleported());
    }
}
