//! CS `pm_shared` (docs/re/movement.md Part A), matching the Windows SSE2 build: f32 arithmetic
//! with the original association, `[d]` expressions in f64 rounded once.

use gs_protocol::messages::MoveVars;
use gs_protocol::structs::UserCmd;
use gs_world::{World, contents};

use crate::math::{Vec3, angle_vectors, cross, dot, length, ma, normalize, scale};
use crate::textures::TextureTypes;
use crate::trace::{PLAYER_MAXS, PLAYER_MINS, Physent, PmTrace, player_trace, point_contents, test_player_position, trace_model, trace_texture, trace_flags};

pub mod buttons {
    pub const ATTACK: i32 = 1;
    pub const JUMP: i32 = 2;
    pub const DUCK: i32 = 4;
    pub const FORWARD: i32 = 8;
    pub const BACK: i32 = 0x10;
    pub const MOVELEFT: i32 = 0x200;
    pub const MOVERIGHT: i32 = 0x400;
}

pub mod flags {
    pub const ONGROUND: i32 = 0x200;
    pub const WATERJUMP: i32 = 0x800;
    pub const FROZEN: i32 = 0x1000;
    pub const DUCKING: i32 = 0x4000;
    pub const ONTRAIN: i32 = 0x0100_0000;
}

pub mod movetype {
    pub const NONE: i32 = 0;
    pub const WALK: i32 = 3;
    pub const FLY: i32 = 5;
    pub const TOSS: i32 = 6;
    pub const NOCLIP: i32 = 8;
    pub const FLYMISSILE: i32 = 9;
    pub const BOUNCE: i32 = 10;
    pub const BOUNCEMISSILE: i32 = 11;
}

const STOP_EPSILON: f64 = 0.1;
const MAX_CLIP_PLANES: usize = 5;
const MAX_TOUCH: usize = 600;
const OBS_ROAMING: i32 = 3;
const STUCK_TABLE_SIZE: usize = 54;

/// Appends one line to the move trace (`PlayerMove::log`) when one is kept; the arguments are
/// formatted only then, so the move itself is the same either way.
macro_rules! note {
    ($pm:expr, $($arg:tt)*) => {
        if $pm.p.log.is_some() {
            let line = format!($($arg)*);
            if let Some(log) = $pm.p.log.as_mut() {
                log.push_str("      ");
                log.push_str(&line);
                log.push('\n');
            }
        }
    };
}

/// A vector in the move trace: three decimals.
struct V3(Vec3);

impl std::fmt::Display for V3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({:.3} {:.3} {:.3})", self.0[0], self.0[1], self.0[2])
    }
}

/// Horizontal speed, for the trace.
fn hspeed(v: Vec3) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

/// `playermove_t` (the parts `pm_shared` reads and writes).
#[derive(Debug, Clone, Default)]
pub struct PlayerMove {
    pub player_index: i32,
    pub server: bool,
    pub multiplayer: bool,
    pub time: f32,
    pub frametime: f32,
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub origin: Vec3,
    pub angles: Vec3,
    pub oldangles: Vec3,
    pub velocity: Vec3,
    pub movedir: Vec3,
    pub basevelocity: Vec3,
    pub view_ofs: Vec3,
    pub duck_time: f32,
    pub in_duck: bool,
    pub time_step_sound: i32,
    pub step_left: i32,
    pub fall_velocity: f32,
    pub punchangle: Vec3,
    pub swim_time: f32,
    pub next_primary_attack: f32,
    pub effects: i32,
    pub flags: i32,
    pub usehull: i32,
    pub gravity: f32,
    pub friction: f32,
    pub oldbuttons: i32,
    pub waterjumptime: f32,
    pub dead: bool,
    pub deadflag: i32,
    pub spectator: i32,
    /// The client DLL's `iJumpSpectator` with `vJumpOrigin` / `vJumpAngles` (client pmove
    /// only); taken when roaming consumes it.
    pub jump_spectator: Option<(Vec3, Vec3)>,
    pub movetype: i32,
    pub onground: i32,
    pub waterlevel: i32,
    pub watertype: i32,
    pub oldwaterlevel: i32,
    pub texture_name: String,
    pub texture_type: u8,
    pub maxspeed: f32,
    pub clientmaxspeed: f32,
    pub iuser: [i32; 4],
    pub fuser: [f32; 4],
    pub vuser: [Vec3; 4],
    pub physents: Vec<Physent>,
    pub moveents: Vec<Physent>,
    pub cmd: UserCmd,
    pub touch: Vec<PmTrace>,
    pub movevars: MoveVars,
    pub runfuncs: bool,
    /// The step-by-step trace `cl_predlog` keeps: a line per step while `Some` (nothing is
    /// formatted otherwise).
    pub log: Option<String>,
}

/// A `PM_PlaySound` request.
#[derive(Debug, Clone, PartialEq)]
pub struct PmSound {
    pub channel: i32,
    pub sample: String,
    pub volume: f32,
    pub attenuation: f32,
}

/// `rgStuckLast` / `rgStuckCheckTime` (shared by every pmove of a side).
#[derive(Debug, Clone)]
pub struct StuckState {
    last: [[i32; 2]; 32],
    check_time: [[f32; 2]; 32],
    /// `iSkipStep` of `PM_PlayStepSound` (one static for all players).
    skip_step: i32,
}

impl Default for StuckState {
    fn default() -> Self {
        Self { last: [[0; 2]; 32], check_time: [[0.0; 2]; 32], skip_step: 0 }
    }
}

/// Everything outside `playermove_t` that `PM_Move` uses.
pub struct PmContext<'a> {
    pub world: &'a World,
    pub textures: &'a TextureTypes,
    pub stuck: &'a mut StuckState,
    /// Engine `RandomLong(lo, hi)` (sound selection only).
    pub random: &'a mut dyn FnMut(i32, i32) -> i32,
    /// `Sys_FloatTime()` (stuck re-test interval).
    pub float_time: f64,
    pub sounds: Vec<PmSound>,
}

/// `PM_CreateStuckTable`.
fn stuck_table() -> [Vec3; STUCK_TABLE_SIZE] {
    let mut t = [[0.0f32; 3]; STUCK_TABLE_SIZE];
    let mut idx = 0;
    let z_off = [-0.125, 0.0, 0.125];
    for z in z_off {
        t[idx] = [0.0, 0.0, z];
        idx += 1;
    }
    for y in z_off {
        t[idx] = [0.0, y, 0.0];
        idx += 1;
    }
    for x in z_off {
        t[idx] = [x, 0.0, 0.0];
        idx += 1;
    }
    for x in [-0.125, 0.125] {
        for y in [-0.125, 0.125] {
            for z in [-0.125, 0.125] {
                t[idx] = [x, y, z];
                idx += 1;
            }
        }
    }
    for z in [0.0, 1.0, 6.0] {
        t[idx] = [0.0, 0.0, z];
        idx += 1;
    }
    for y in [-2.0, 0.0, 2.0] {
        t[idx] = [0.0, y, 0.0];
        idx += 1;
    }
    for x in [-2.0, 0.0, 2.0] {
        t[idx] = [x, 0.0, 0.0];
        idx += 1;
    }
    for z in [0.0, 1.0, 6.0] {
        for x in [-2.0, 0.0, 2.0] {
            for y in [-2.0, 0.0, 2.0] {
                t[idx] = [x, y, z];
                idx += 1;
            }
        }
    }
    t
}

fn is_liquid(c: i32) -> bool {
    (contents::CURRENT_DOWN..=contents::WATER).contains(&c)
}

/// `current_table` for CONTENTS_CURRENT_0 .. CURRENT_DOWN.
const CURRENT_TABLE: [Vec3; 6] =
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]];

struct Pm<'p, 'c, 'w> {
    p: &'p mut PlayerMove,
    c: &'c mut PmContext<'w>,
}

/// `PM_Move(pmove, server)`.
pub fn pm_move(p: &mut PlayerMove, ctx: &mut PmContext, server: bool) {
    let mut pm = Pm { p, c: ctx };
    pm.player_move(server);
    if pm.p.onground != -1 {
        pm.p.flags |= flags::ONGROUND;
    } else {
        pm.p.flags &= !flags::ONGROUND;
    }
    if !pm.p.multiplayer && pm.p.movetype == movetype::WALK {
        pm.p.friction = 1.0;
    }
    note!(
        pm,
        "end org {} vel {} hspd {:.3} onground {} flags {:#x} induck {} usehull {} fuser2 {:.3} fallvel {:.3}",
        V3(pm.p.origin),
        V3(pm.p.velocity),
        hspeed(pm.p.velocity),
        pm.p.onground,
        pm.p.flags,
        pm.p.in_duck,
        pm.p.usehull,
        pm.p.fuser[1],
        pm.p.fall_velocity
    );
}

impl Pm<'_, '_, '_> {
    fn trace(&self, start: Vec3, end: Vec3) -> PmTrace {
        player_trace(self.c.world, &self.p.physents, self.p.usehull, start, end, trace_flags::NORMAL, -1)
    }

    fn test_position(&self, pos: Vec3) -> (i32, PmTrace) {
        test_player_position(self.c.world, &self.p.physents, self.p.usehull, self.p.origin, pos)
    }

    fn contents(&self, p: Vec3) -> (i32, i32) {
        point_contents(self.c.world, &self.p.physents, p)
    }

    fn hull(&self) -> usize {
        usize::try_from(self.p.usehull).ok().filter(|&h| h < 4).unwrap_or(0)
    }

    fn play_sound(&mut self, sample: &str, volume: f32) {
        self.c.sounds.push(PmSound { channel: 4, sample: sample.to_owned(), volume, attenuation: 0.8 });
    }

    fn player_move(&mut self, server: bool) {
        self.p.server = server;
        self.check_parameters();
        self.p.touch.clear();
        let msec = self.p.cmd.msec;
        self.p.frametime = (f64::from(msec) * 0.001) as f32;
        // ReduceTimers
        if self.p.time_step_sound > 0 {
            self.p.time_step_sound = (self.p.time_step_sound - i32::from(msec)).max(0);
        }
        for t in [&mut self.p.duck_time, &mut self.p.swim_time, &mut self.p.fuser[1]] {
            if *t > 0.0 {
                *t -= f32::from(msec);
                if 0.0 > *t {
                    *t = 0.0;
                }
            }
        }
        let (f, r, u) = angle_vectors(self.p.angles);
        (self.p.forward, self.p.right, self.p.up) = (f, r, u);
        note!(
            self,
            "start msec {} btn {:#x} oldbtn {:#x} move {:.2} {:.2} {:.2} maxspeed {:.2}/{:.2} org {} vel {} hspd {:.3} flags {:#x} induck {} ducktime {:.0} fuser2 {:.3} usehull {} movetype {} basevel {} friction {:.3} gravity {:.3} aa {} accel {}",
            msec,
            self.p.cmd.buttons,
            self.p.oldbuttons,
            self.p.cmd.forwardmove,
            self.p.cmd.sidemove,
            self.p.cmd.upmove,
            self.p.maxspeed,
            self.p.clientmaxspeed,
            V3(self.p.origin),
            V3(self.p.velocity),
            hspeed(self.p.velocity),
            self.p.flags,
            self.p.in_duck,
            self.p.duck_time,
            self.p.fuser[1],
            self.p.usehull,
            self.p.movetype,
            V3(self.p.basevelocity),
            self.p.friction,
            self.p.gravity,
            self.p.movevars.airaccelerate,
            self.p.movevars.accelerate
        );

        let spect = self.p.spectator != 0 || self.p.iuser[0] > 0;
        let spect = if server { spect && (self.p.iuser[2] <= 0 || self.p.deadflag == 2) } else { spect };
        if spect {
            self.spectator_move();
            self.categorize_position();
            return;
        }
        if self.p.movetype != movetype::NOCLIP && self.p.movetype != movetype::NONE && self.check_stuck() {
            note!(self, "stuck: nudged, move skipped");
            return;
        }
        self.categorize_position();
        self.p.oldwaterlevel = self.p.waterlevel;
        if self.p.onground == -1 {
            self.p.fall_velocity = -self.p.velocity[2];
        }
        let ladder = if !self.p.dead && self.p.flags & flags::ONTRAIN == 0 { self.ladder() } else { None };
        let before = (self.p.usehull, self.p.in_duck, self.p.flags & flags::DUCKING, self.p.origin[2], self.p.view_ofs[2]);
        self.duck();
        if self.p.log.is_some() {
            let after = (self.p.usehull, self.p.in_duck, self.p.flags & flags::DUCKING, self.p.origin[2], self.p.view_ofs[2]);
            if after != before {
                note!(self, "duck: usehull {}->{} induck {}->{} ducking {}->{} z {:.3}->{:.3} viewz {:.2}->{:.2}", before.0, after.0, before.1, after.1, before.2 != 0, after.2 != 0, before.3, after.3, before.4, after.4);
            }
        }
        self.update_step_sound();
        if !self.p.dead && self.p.flags & flags::ONTRAIN == 0 {
            if let Some(l) = ladder.clone() {
                self.ladder_move(&l);
            } else if self.p.movetype != movetype::WALK && self.p.movetype != movetype::NOCLIP {
                self.p.movetype = movetype::WALK;
            }
        }
        match self.p.movetype {
            movetype::NONE => {}
            movetype::NOCLIP => {
                let (fm, sm) = (self.p.cmd.forwardmove, self.p.cmd.sidemove);
                normalize(&mut self.p.forward);
                normalize(&mut self.p.right);
                let (f, r) = (self.p.forward, self.p.right);
                let w = [f[0] * fm + r[0] * sm, f[1] * fm + r[1] * sm, (f[2] * fm + r[2] * sm) + self.p.cmd.upmove];
                self.p.origin = ma(self.p.origin, self.p.frametime, w);
                self.p.velocity = [0.0; 3];
            }
            movetype::TOSS | movetype::BOUNCE => self.physics_toss(),
            movetype::FLY => {
                self.check_water();
                if i32::from(self.p.cmd.buttons) & buttons::JUMP != 0 {
                    if ladder.is_none() {
                        self.jump();
                    }
                } else {
                    self.p.oldbuttons &= !buttons::JUMP;
                }
                self.add_base_velocity();
                self.fly_move();
                self.sub_base_velocity();
            }
            movetype::WALK => self.walk_branch(ladder.is_some()),
            other => log::debug!("Bogus pmove player movetype {other} on ({}) 0=cl 1=sv", i32::from(server)),
        }
    }

    fn add_base_velocity(&mut self) {
        for i in 0..3 {
            self.p.velocity[i] += self.p.basevelocity[i];
        }
    }

    fn sub_base_velocity(&mut self) {
        for i in 0..3 {
            self.p.velocity[i] -= self.p.basevelocity[i];
        }
    }

    fn walk_branch(&mut self, on_ladder: bool) {
        if self.p.waterlevel <= 1 && self.p.waterjumptime == 0.0 {
            // AddCorrectGravity
            let eg = if self.p.gravity != 0.0 { self.p.gravity } else { 1.0 };
            let g = eg * self.p.movevars.gravity;
            self.p.velocity[2] = (f64::from(self.p.velocity[2]) - f64::from(g) * 0.5 * f64::from(self.p.frametime)) as f32;
            self.p.velocity[2] += self.p.basevelocity[2] * self.p.frametime;
            self.p.basevelocity[2] = 0.0;
            self.check_velocity();
            note!(self, "gravity half: vz {:.3}", self.p.velocity[2]);
        }
        if self.p.waterjumptime != 0.0 {
            // WaterJump
            if self.p.waterjumptime > 10000.0 {
                self.p.waterjumptime = 10000.0;
            }
            self.p.waterjumptime -= f32::from(self.p.cmd.msec);
            if self.p.waterjumptime < 0.0 || self.p.waterlevel == 0 {
                self.p.waterjumptime = 0.0;
                self.p.flags &= !flags::WATERJUMP;
            }
            self.p.velocity[0] = self.p.movedir[0];
            self.p.velocity[1] = self.p.movedir[1];
            self.fly_move();
            self.check_water();
            return;
        }
        let jump_held = i32::from(self.p.cmd.buttons) & buttons::JUMP != 0;
        if self.p.waterlevel >= 2 {
            if self.p.waterlevel == 2 {
                self.check_water_jump();
            }
            if self.p.velocity[2] < 0.0 && self.p.waterjumptime != 0.0 {
                self.p.waterjumptime = 0.0;
            }
            if jump_held {
                self.jump();
            } else {
                self.p.oldbuttons &= !buttons::JUMP;
            }
            self.water_move();
            self.sub_base_velocity();
            self.categorize_position();
        } else {
            if jump_held {
                if !on_ladder {
                    self.jump();
                }
            } else {
                self.p.oldbuttons &= !buttons::JUMP;
            }
            if self.p.onground != -1 {
                self.p.velocity[2] = 0.0;
                self.friction();
            }
            self.check_velocity();
            if self.p.onground != -1 {
                self.walk_move();
            } else {
                self.air_move();
            }
            self.categorize_position();
            self.sub_base_velocity();
            self.check_velocity();
            if self.p.waterlevel <= 1 {
                self.fixup_gravity_velocity();
            }
            if self.p.onground != -1 {
                self.p.velocity[2] = 0.0;
            }
            self.check_falling();
            note!(self, "after move: org {} vel {} onground {}", V3(self.p.origin), V3(self.p.velocity), self.p.onground);
        }
        self.play_water_sounds();
    }

    // ------------------------------------------------------------------ parameters

    fn check_parameters(&mut self) {
        let c = &mut self.p.cmd;
        let (mut f, mut s, mut u) = (c.forwardmove, c.sidemove, c.upmove);
        let spd = ((f * f + s * s) + u * u).sqrt();
        if self.p.clientmaxspeed != 0.0 {
            self.p.maxspeed = if self.p.clientmaxspeed < self.p.maxspeed { self.p.clientmaxspeed } else { self.p.maxspeed };
        }
        if spd != 0.0 && spd > self.p.maxspeed {
            let r = self.p.maxspeed / spd;
            f *= r;
            s *= r;
            u *= r;
        }
        if self.p.flags & (flags::FROZEN | flags::ONTRAIN) != 0 || self.p.dead {
            f = 0.0;
            s = 0.0;
            u = 0.0;
        }
        (c.forwardmove, c.sidemove, c.upmove) = (f, s, u);
        self.drop_punch_angle();
        if !self.p.dead {
            let va = self.p.cmd.viewangles;
            let v = [va[0] + self.p.punchangle[0], va[1] + self.p.punchangle[1], va[2] + self.p.punchangle[2]];
            self.p.angles[2] = calc_roll(v, self.p.velocity, self.p.movevars.rollangle, self.p.movevars.rollspeed) * 4.0;
            self.p.angles[0] = v[0];
            self.p.angles[1] = v[1];
        } else {
            self.p.angles = self.p.oldangles;
            self.p.view_ofs[2] = -8.0;
        }
        if self.p.angles[1] > 180.0 {
            self.p.angles[1] -= 360.0;
        }
    }

    fn drop_punch_angle(&mut self) {
        let mut p = self.p.punchangle;
        let len = normalize(&mut p);
        let mut len = (f64::from(len) - (f64::from(len) * 0.5 + 10.0) * f64::from(self.p.frametime)) as f32;
        if len.is_nan() || len <= 0.0 {
            len = 0.0;
        }
        self.p.punchangle = scale(p, len);
    }

    // ------------------------------------------------------------------ position / water

    fn categorize_position(&mut self) {
        self.check_water();
        let mut point = self.p.origin;
        point[2] = self.p.origin[2] - 2.0;
        if self.p.velocity[2] > 180.0 {
            self.p.onground = -1;
            note!(self, "categorize: rising vz {:.3}, in air", self.p.velocity[2]);
            return;
        }
        let tr = self.trace(self.p.origin, point);
        self.p.onground = if f64::from(tr.plane_normal[2]) < 0.7 { -1 } else { tr.ent };
        if self.p.onground != -1 {
            self.p.waterjumptime = 0.0;
            if self.p.waterlevel < 2 && !tr.startsolid && !tr.allsolid {
                self.p.origin = tr.endpos;
            }
        }
        let normal_z = tr.plane_normal[2];
        if tr.ent > 0 {
            let v = self.p.velocity;
            self.add_to_touched(tr, v);
        }
        note!(self, "categorize: onground {} normal z {normal_z:.3} water {} org {}", self.p.onground, self.p.waterlevel, V3(self.p.origin));
    }

    fn check_water(&mut self) -> bool {
        let h = self.hull();
        let (mins, maxs, o) = (PLAYER_MINS[h], PLAYER_MAXS[h], self.p.origin);
        let mut point = [
            (f64::from(maxs[0] + mins[0]) * 0.5 + f64::from(o[0])) as f32,
            (f64::from(maxs[1] + mins[1]) * 0.5 + f64::from(o[1])) as f32,
            (mins[2] + o[2]) + 1.0,
        ];
        self.p.waterlevel = 0;
        self.p.watertype = contents::EMPTY;
        let (cont, truecont) = self.contents(point);
        if is_liquid(cont) {
            self.p.watertype = cont;
            self.p.waterlevel = 1;
            point[2] = (f64::from(maxs[2] + mins[2]) * 0.5) as f32 + o[2];
            if is_liquid(self.contents(point).0) {
                self.p.waterlevel = 2;
                point[2] = self.p.view_ofs[2] + o[2];
                if is_liquid(self.contents(point).0) {
                    self.p.waterlevel = 3;
                }
            }
            if (contents::CURRENT_DOWN..=contents::CURRENT_0).contains(&truecont) {
                let dir = CURRENT_TABLE[(contents::CURRENT_0 - truecont) as usize];
                let s = (f64::from(self.p.waterlevel) * 50.0) as f32;
                self.p.basevelocity = ma(self.p.basevelocity, s, dir);
            }
        }
        self.p.waterlevel > 1
    }

    fn check_velocity(&mut self) {
        let maxv = self.p.movevars.maxvelocity;
        for i in 0..3 {
            if self.p.velocity[i].to_bits() & 0x7F80_0000 == 0x7F80_0000 {
                log::warn!("PM  Got a NaN velocity {i}");
                self.p.velocity[i] = 0.0;
            }
            if self.p.origin[i].to_bits() & 0x7F80_0000 == 0x7F80_0000 {
                log::warn!("PM  Got a NaN origin on {i}");
                self.p.origin[i] = 0.0;
            }
            if self.p.velocity[i] > maxv {
                self.p.velocity[i] = maxv;
            } else if self.p.velocity[i] < -maxv {
                self.p.velocity[i] = -maxv;
            }
        }
    }

    fn add_to_touched(&mut self, mut tr: PmTrace, impactvel: Vec3) -> bool {
        if self.p.touch.iter().any(|t| t.ent == tr.ent) {
            return false;
        }
        if self.p.touch.len() >= MAX_TOUCH {
            log::debug!("Too many entities were touched!");
        }
        tr.deltavelocity = impactvel;
        self.p.touch.push(tr);
        true
    }

    fn fixup_gravity_velocity(&mut self) {
        if self.p.waterjumptime != 0.0 {
            return;
        }
        let eg = if self.p.gravity != 0.0 { self.p.gravity } else { 1.0 };
        let g = (eg * self.p.movevars.gravity) * self.p.frametime;
        self.p.velocity[2] = (f64::from(self.p.velocity[2]) - f64::from(g) * 0.5) as f32;
        self.check_velocity();
        note!(self, "gravity half (after move): vz {:.3}", self.p.velocity[2]);
    }

    fn add_gravity(&mut self) {
        let eg = if self.p.gravity != 0.0 { self.p.gravity } else { 1.0 };
        self.p.velocity[2] -= eg * self.p.movevars.gravity * self.p.frametime;
        self.p.velocity[2] += self.p.basevelocity[2] * self.p.frametime;
        self.p.basevelocity[2] = 0.0;
        self.check_velocity();
    }

    // ------------------------------------------------------------------ movement modes

    fn friction(&mut self) {
        if self.p.waterjumptime != 0.0 {
            return;
        }
        let v = self.p.velocity;
        let speed = ((v[0] * v[0] + v[1] * v[1]) + v[2] * v[2]).sqrt();
        if speed < 0.1 {
            return;
        }
        let mut drop = 0.0f32;
        let mut edge = false;
        if self.p.onground != -1 {
            let h = self.hull();
            let start = [(v[0] / speed) * 16.0 + self.p.origin[0], (v[1] / speed) * 16.0 + self.p.origin[1], PLAYER_MINS[h][2] + self.p.origin[2]];
            let mut stop = start;
            stop[2] = start[2] - 34.0;
            let tr = self.trace(start, stop);
            edge = tr.fraction == 1.0;
            let mv = &self.p.movevars;
            let mut fr = if edge { mv.friction * mv.edgefriction } else { mv.friction };
            fr *= self.p.friction;
            let control = if speed < mv.stopspeed { mv.stopspeed } else { speed };
            drop = ((fr * control) * self.p.frametime) + 0.0;
        }
        let mut newspeed = speed - drop;
        if newspeed < 0.0 {
            newspeed = 0.0;
        }
        newspeed /= speed;
        for c in self.p.velocity.iter_mut() {
            *c *= newspeed;
        }
        note!(self, "friction: speed {speed:.3} -> {:.3} drop {drop:.3} edge {edge}", speed * newspeed);
    }

    fn accelerate(&mut self, wishdir: Vec3, wishspeed: f32, accel: f32) {
        if self.p.dead || self.p.waterjumptime != 0.0 {
            return;
        }
        let cur = dot(self.p.velocity, wishdir);
        let add = wishspeed - cur;
        if add <= 0.0 {
            return;
        }
        let mut acc = ((accel * self.p.frametime) * wishspeed) * self.p.friction;
        if acc > add {
            acc = add;
        }
        for (v, d) in self.p.velocity.iter_mut().zip(wishdir) {
            *v += acc * d;
        }
    }

    fn air_accelerate(&mut self, wishdir: Vec3, wishspeed: f32, accel: f32) {
        if self.p.dead || self.p.waterjumptime != 0.0 {
            return;
        }
        let wspd = if wishspeed > 30.0 { 30.0 } else { wishspeed };
        let cur = dot(self.p.velocity, wishdir);
        let add = wspd - cur;
        if add <= 0.0 {
            return;
        }
        let mut acc = ((wishspeed * accel) * self.p.frametime) * self.p.friction;
        if acc > add {
            acc = add;
        }
        for (v, d) in self.p.velocity.iter_mut().zip(wishdir) {
            *v += acc * d;
        }
    }

    /// Flattened, normalised forward/right and the clamped wish velocity.
    fn flat_wish(&mut self) -> (Vec3, Vec3, f32) {
        let (fm, sm) = (self.p.cmd.forwardmove, self.p.cmd.sidemove);
        self.p.forward[2] = 0.0;
        self.p.right[2] = 0.0;
        normalize(&mut self.p.forward);
        normalize(&mut self.p.right);
        let (f, r) = (self.p.forward, self.p.right);
        let mut wishvel = [f[0] * fm + r[0] * sm, f[1] * fm + r[1] * sm, 0.0];
        let mut wishdir = wishvel;
        let mut wishspeed = normalize(&mut wishdir);
        if wishspeed > self.p.maxspeed {
            wishvel = scale(wishvel, self.p.maxspeed / wishspeed);
            wishspeed = self.p.maxspeed;
        }
        (wishvel, wishdir, wishspeed)
    }

    fn air_move(&mut self) {
        let (_, wishdir, wishspeed) = self.flat_wish();
        self.air_accelerate(wishdir, wishspeed, self.p.movevars.airaccelerate);
        note!(self, "air: wish {} {wishspeed:.3} vel {} hspd {:.3}", V3(wishdir), V3(self.p.velocity), hspeed(self.p.velocity));
        self.add_base_velocity();
        let blocked = self.fly_move();
        note!(self, "air: flymove blocked {blocked} org {} vel {}", V3(self.p.origin), V3(self.p.velocity));
    }

    fn walk_move(&mut self) {
        if self.p.fuser[1] > 0.0 {
            let r = ((100.0 - f64::from(self.p.fuser[1]) / 1000.0 * 19.0) / 100.0) as f32;
            self.p.velocity[0] *= r;
            self.p.velocity[1] *= r;
            note!(self, "walk: stamina fuser2 {:.3} ratio {r:.6} hspd {:.3}", self.p.fuser[1], hspeed(self.p.velocity));
        }
        let (_, wishdir, wishspeed) = self.flat_wish();
        self.p.velocity[2] = 0.0;
        self.accelerate(wishdir, wishspeed, self.p.movevars.accelerate);
        self.p.velocity[2] = 0.0;
        note!(self, "walk: wish {} {wishspeed:.3} vel {} hspd {:.3}", V3(wishdir), V3(self.p.velocity), hspeed(self.p.velocity));
        self.add_base_velocity();
        if length(self.p.velocity) < 1.0 {
            self.p.velocity = [0.0; 3];
            note!(self, "walk: below 1, stopped");
            return;
        }
        let oldonground = self.p.onground;
        let o = self.p.origin;
        let dest = [self.p.velocity[0] * self.p.frametime + o[0], self.p.velocity[1] * self.p.frametime + o[1], o[2]];
        let tr = self.trace(o, dest);
        if tr.fraction == 1.0 {
            self.p.origin = tr.endpos;
            note!(self, "walk: free to {}", V3(self.p.origin));
            return;
        }
        if oldonground == -1 && self.p.waterlevel == 0 {
            return;
        }
        if self.p.waterjumptime != 0.0 {
            return;
        }
        let original = self.p.origin;
        let originalvel = self.p.velocity;
        self.fly_move();
        let down = self.p.origin;
        let downvel = self.p.velocity;
        self.p.origin = original;
        self.p.velocity = originalvel;
        let mut dest = self.p.origin;
        dest[2] += self.p.movevars.stepsize;
        let tr = self.trace(self.p.origin, dest);
        if !tr.startsolid && !tr.allsolid {
            self.p.origin = tr.endpos;
        }
        self.fly_move();
        let mut dest = self.p.origin;
        dest[2] -= self.p.movevars.stepsize;
        let tr = self.trace(self.p.origin, dest);
        let use_down = if f64::from(tr.plane_normal[2]) < 0.7 {
            true
        } else {
            if !tr.startsolid && !tr.allsolid {
                self.p.origin = tr.endpos;
            }
            self.p.up = self.p.origin;
            let up = self.p.origin;
            let downdist = (down[0] - original[0]) * (down[0] - original[0]) + (down[1] - original[1]) * (down[1] - original[1]);
            let updist = (up[0] - original[0]) * (up[0] - original[0]) + (up[1] - original[1]) * (up[1] - original[1]);
            downdist > updist
        };
        if use_down {
            self.p.origin = down;
            self.p.velocity = downvel;
        } else {
            self.p.velocity[2] = downvel[2];
        }
        note!(self, "walk: blocked, stepmove {} org {} vel {}", if use_down { "down" } else { "up" }, V3(self.p.origin), V3(self.p.velocity));
    }

    fn fly_move(&mut self) -> i32 {
        let mut blocked = 0;
        let mut numplanes = 0usize;
        let mut planes = [[0.0f32; 3]; MAX_CLIP_PLANES];
        let mut original = self.p.velocity;
        let primal = self.p.velocity;
        let mut all_fraction = 0.0f32;
        let mut time_left = self.p.frametime;
        for _bump in 0..4 {
            let v = self.p.velocity;
            if v[0] == 0.0 && v[1] == 0.0 && v[2] == 0.0 {
                break;
            }
            let end = [0, 1, 2].map(|i| v[i] * time_left + self.p.origin[i]);
            let tr = self.trace(self.p.origin, end);
            all_fraction += tr.fraction;
            if tr.allsolid {
                self.p.velocity = [0.0; 3];
                return 4;
            }
            if tr.fraction > 0.0 {
                self.p.origin = tr.endpos;
                original = self.p.velocity;
                numplanes = 0;
            }
            if tr.fraction == 1.0 {
                break;
            }
            let vel = self.p.velocity;
            self.add_to_touched(tr, vel);
            if f64::from(tr.plane_normal[2]) > 0.7 {
                blocked |= 1;
            }
            if tr.plane_normal[2] == 0.0 {
                blocked |= 2;
            }
            time_left -= time_left * tr.fraction;
            if numplanes >= MAX_CLIP_PLANES {
                self.p.velocity = [0.0; 3];
                break;
            }
            planes[numplanes] = tr.plane_normal;
            numplanes += 1;
            if numplanes == 1 && self.p.movetype == movetype::WALK && (self.p.onground == -1 || self.p.friction != 1.0) {
                let ob = if f64::from(planes[0][2]) > 0.7 {
                    1.0
                } else {
                    (1.0 + f64::from(self.p.movevars.bounce * (1.0 - self.p.friction))) as f32
                };
                let new = clip_velocity(original, planes[0], ob).0;
                self.p.velocity = new;
                original = new;
                continue;
            }
            let mut i = 0;
            while i < numplanes {
                self.p.velocity = clip_velocity(original, planes[i], 1.0).0;
                let mut j = 0;
                while j < numplanes {
                    if j != i && dot(self.p.velocity, planes[j]) < 0.0 {
                        break;
                    }
                    j += 1;
                }
                if j == numplanes {
                    break;
                }
                i += 1;
            }
            if i == numplanes {
                if numplanes != 2 {
                    self.p.velocity = [0.0; 3];
                    break;
                }
                let dir = cross(planes[0], planes[1]);
                let d = dot(dir, self.p.velocity);
                self.p.velocity = scale(dir, d);
            }
            if dot(self.p.velocity, primal) <= 0.0 {
                self.p.velocity = [0.0; 3];
                break;
            }
        }
        if all_fraction == 0.0 {
            self.p.velocity = [0.0; 3];
        }
        blocked
    }

    fn water_move(&mut self) {
        let (fm, sm, um) = (self.p.cmd.forwardmove, self.p.cmd.sidemove, self.p.cmd.upmove);
        let (f, r) = (self.p.forward, self.p.right);
        let mut wishvel = [0, 1, 2].map(|i| f[i] * fm + r[i] * sm);
        if fm == 0.0 && sm == 0.0 && um == 0.0 {
            wishvel[2] -= 60.0;
        } else {
            wishvel[2] += um;
        }
        let mut wishdir = wishvel;
        let mut wishspeed = normalize(&mut wishdir);
        if wishspeed > self.p.maxspeed {
            wishvel = scale(wishvel, self.p.maxspeed / wishspeed);
            wishspeed = self.p.maxspeed;
        }
        wishspeed = (f64::from(wishspeed) * 0.8) as f32;
        self.add_base_velocity();
        let mut temp = self.p.velocity;
        let speed = normalize(&mut temp);
        let newspeed = if speed != 0.0 {
            let mut ns = speed - ((speed * self.p.frametime) * self.p.movevars.friction) * self.p.friction;
            if ns < 0.0 {
                ns = 0.0;
            }
            self.p.velocity = scale(self.p.velocity, ns / speed);
            ns
        } else {
            0.0
        };
        if wishspeed < 0.1 {
            return;
        }
        let add = wishspeed - newspeed;
        if add > 0.0 {
            normalize(&mut wishvel);
            let mut acc = ((wishspeed * self.p.movevars.accelerate) * self.p.frametime) * self.p.friction;
            if acc > add {
                acc = add;
            }
            for (v, w) in self.p.velocity.iter_mut().zip(wishvel) {
                *v += w * acc;
            }
        }
        let dest = ma(self.p.origin, self.p.frametime, self.p.velocity);
        let mut start = dest;
        start[2] = (self.p.movevars.stepsize + 1.0) + dest[2];
        let tr = self.trace(start, dest);
        if !tr.startsolid && !tr.allsolid {
            self.p.origin = tr.endpos;
            return;
        }
        self.fly_move();
    }

    fn spectator_move(&mut self) {
        if self.p.iuser[0] == OBS_ROAMING {
            // Client only: the HUD's camera jump (director cut, new target) snaps the camera.
            if let Some((origin, angles)) = self.p.jump_spectator.take() {
                self.p.origin = origin;
                self.p.angles = angles;
                self.p.velocity = [0.0; 3];
                return;
            }
            let speed = length(self.p.velocity);
            if speed < 1.0 {
                self.p.velocity = [0.0; 3];
            } else {
                let fr = (f64::from(self.p.movevars.friction) * 1.5) as f32;
                let control = if speed < self.p.movevars.stopspeed { self.p.movevars.stopspeed } else { speed };
                let drop = ((control * fr) * self.p.frametime) + 0.0;
                let mut newspeed = speed - drop;
                if newspeed < 0.0 {
                    newspeed = 0.0;
                }
                newspeed /= speed;
                self.p.velocity = scale(self.p.velocity, newspeed);
            }
            let (fm, sm) = (self.p.cmd.forwardmove, self.p.cmd.sidemove);
            normalize(&mut self.p.forward);
            normalize(&mut self.p.right);
            let (f, r) = (self.p.forward, self.p.right);
            let mut wishvel = [f[0] * fm + r[0] * sm, f[1] * fm + r[1] * sm, (f[2] * fm + r[2] * sm) + self.p.cmd.upmove];
            let mut wishdir = wishvel;
            let mut wishspeed = normalize(&mut wishdir);
            let smax = self.p.movevars.spectatormaxspeed;
            if wishspeed > smax {
                wishvel = scale(wishvel, smax / wishspeed);
                wishspeed = smax;
            }
            let _ = wishvel;
            let cur = dot(self.p.velocity, wishdir);
            let add = wishspeed - cur;
            if add <= 0.0 {
                return;
            }
            let mut acc = (self.p.movevars.accelerate * self.p.frametime) * wishspeed;
            if acc > add {
                acc = add;
            }
            for (v, d) in self.p.velocity.iter_mut().zip(wishdir) {
                *v += acc * d;
            }
            self.p.origin = ma(self.p.origin, self.p.frametime, self.p.velocity);
        } else if self.p.iuser[1] > 0 {
            let target = self.p.iuser[1];
            if let Some(pe) = self.p.physents.iter().find(|pe| pe.info == target) {
                let (angles, origin) = (pe.angles, pe.origin);
                self.p.angles = angles;
                self.p.origin = origin;
                self.p.velocity = [0.0; 3];
            }
        }
    }

    // ------------------------------------------------------------------ duck / jump / ladder

    fn duck(&mut self) {
        let cmd_buttons = i32::from(self.p.cmd.buttons);
        let pressed = (cmd_buttons ^ self.p.oldbuttons) & cmd_buttons;
        if cmd_buttons & buttons::DUCK != 0 {
            self.p.oldbuttons |= buttons::DUCK;
        } else {
            self.p.oldbuttons &= !buttons::DUCK;
        }
        if self.p.dead {
            return;
        }
        if cmd_buttons & buttons::DUCK == 0 && !self.p.in_duck && self.p.flags & flags::DUCKING == 0 {
            return;
        }
        let c = &mut self.p.cmd;
        c.forwardmove = (f64::from(c.forwardmove) * 0.333) as f32;
        c.sidemove = (f64::from(c.sidemove) * 0.333) as f32;
        c.upmove = (f64::from(c.upmove) * 0.333) as f32;
        if cmd_buttons & buttons::DUCK != 0 {
            if pressed & buttons::DUCK != 0 && self.p.flags & flags::DUCKING == 0 {
                self.p.duck_time = 1000.0;
                self.p.in_duck = true;
            }
            if !self.p.in_duck {
                return;
            }
            let t = f64::from(self.p.duck_time) / 1000.0;
            if t <= 0.6 || self.p.onground == -1 {
                self.p.usehull = 1;
                self.p.view_ofs[2] = 12.0;
                self.p.flags |= flags::DUCKING;
                self.p.in_duck = false;
                if self.p.onground != -1 {
                    self.p.origin[2] -= 18.0;
                    self.fix_crouch_stuck(1);
                    self.categorize_position();
                }
            } else {
                let time = (1.0 - t).max(0.0) as f32;
                let v = time * 2.5;
                let v2 = v * v;
                let frac = v2 * 3.0 - (v2 + v2) * v;
                self.p.view_ofs[2] = (1.0 - frac) * 17.0 + frac * -6.0;
            }
        } else {
            // PM_UnDuck
            let mut new_origin = self.p.origin;
            if self.p.onground != -1 {
                new_origin[2] += 18.0;
            }
            if self.trace(new_origin, new_origin).startsolid {
                return;
            }
            self.p.usehull = 0;
            if self.trace(new_origin, new_origin).startsolid {
                self.p.usehull = 1;
                return;
            }
            self.p.flags &= !flags::DUCKING;
            self.p.in_duck = false;
            self.p.view_ofs[2] = 17.0;
            self.p.duck_time = 0.0;
            self.p.time_step_sound = (self.p.time_step_sound - 100).max(0);
            self.p.origin = new_origin;
            self.categorize_position();
        }
    }

    fn fix_crouch_stuck(&mut self, dir: i32) {
        if self.test_position(self.p.origin).0 == -1 {
            return;
        }
        let save = self.p.origin;
        for _ in 0..36 {
            self.p.origin[2] += dir as f32;
            if self.test_position(self.p.origin).0 == -1 {
                return;
            }
        }
        self.p.origin = save;
    }

    fn jump(&mut self) {
        if self.p.dead {
            self.p.oldbuttons |= buttons::JUMP;
            return;
        }
        if self.p.waterjumptime != 0.0 {
            self.p.waterjumptime -= f32::from(self.p.cmd.msec);
            if self.p.waterjumptime < 0.0 {
                self.p.waterjumptime = 0.0;
            }
            return;
        }
        if self.p.waterlevel >= 2 {
            self.p.onground = -1;
            self.p.velocity[2] = match self.p.watertype {
                contents::WATER => 100.0,
                contents::SLIME => 80.0,
                _ => 50.0,
            };
            if self.p.swim_time <= 0.0 {
                self.p.swim_time = 1000.0;
                let k = (self.c.random)(0, 3);
                let sample = ["player/pl_wade1.wav", "player/pl_wade2.wav", "player/pl_wade3.wav", "player/pl_wade4.wav"];
                self.play_sound(sample[k.clamp(0, 3) as usize], 1.0);
            }
            return;
        }
        if self.p.onground == -1 {
            self.p.oldbuttons |= buttons::JUMP;
            note!(self, "jump: in air, ignored (oldbuttons gets JUMP)");
            return;
        }
        if self.p.oldbuttons & buttons::JUMP != 0 {
            note!(self, "jump: still held since the last jump, ignored");
            return;
        }
        if self.p.in_duck && self.p.flags & flags::DUCKING != 0 {
            note!(self, "jump: mid-duck, ignored");
            return;
        }
        self.categorize_texture_type();
        self.p.onground = -1;
        let before = self.p.velocity;
        self.prevent_mega_bunny_jumping();
        if length(self.p.velocity) >= 150.0 {
            let step = step_type(self.p.texture_type);
            self.play_step_sound(step, 1.0);
        }
        const JUMP_SPEED: f32 = 268.328_16;
        let stamina = self.p.fuser[1];
        self.p.velocity[2] = JUMP_SPEED;
        if self.p.fuser[1] > 0.0 {
            let r = ((100.0 - f64::from(self.p.fuser[1]) / 1000.0 * 19.0) / 100.0) as f32;
            self.p.velocity[2] = r * JUMP_SPEED;
        }
        self.p.fuser[1] = 1_315.789_4;
        note!(self, "jump: JUMPED vel before {} hspd {:.3} -> {} vz {:.3} (stamina fuser2 {stamina:.3})", V3(before), hspeed(before), V3(self.p.velocity), self.p.velocity[2]);
        self.fixup_gravity_velocity();
        self.p.oldbuttons |= buttons::JUMP;
    }

    fn prevent_mega_bunny_jumping(&mut self) {
        let maxs = 1.2 * self.p.maxspeed;
        if maxs <= 0.0 {
            return;
        }
        let spd = length(self.p.velocity);
        if maxs >= spd {
            return;
        }
        let f = (f64::from(maxs / spd) * 0.8) as f32;
        self.p.velocity = scale(self.p.velocity, f);
        note!(self, "bhop cap: speed {spd:.3} > 1.2*maxspeed {maxs:.3}, scaled by {f:.6}");
    }

    fn ladder(&self) -> Option<Physent> {
        self.p.moveents.iter().find_map(|pe| {
            let model = pe.model?;
            if pe.skin != contents::LADDER {
                return None;
            }
            let hull_index = match self.p.usehull {
                1 => 3,
                2 => 0,
                3 => 2,
                _ => 1,
            };
            let hull = self.c.world.hull(model, hull_index)?;
            let h = self.hull();
            let offset = [0, 1, 2].map(|k| (hull.clip_mins[k] - PLAYER_MINS[h][k]) + pe.origin[k]);
            let test = [0, 1, 2].map(|k| self.p.origin[k] - offset[k]);
            (hull.point_contents(hull.first, test, true) != contents::EMPTY).then(|| pe.clone())
        })
    }

    fn ladder_move(&mut self, ladder: &Physent) {
        if self.p.movetype == movetype::NOCLIP {
            return;
        }
        let Some(model) = ladder.model.and_then(|m| self.c.world.models.get(m)) else { return };
        let center = [0, 1, 2].map(|k| (model.mins[k] + model.maxs[k]) * 0.5);
        self.p.movetype = movetype::FLY;
        let h = self.hull();
        let mut floor = self.p.origin;
        floor[2] = (PLAYER_MINS[h][2] - 1.0) + self.p.origin[2];
        let on_floor = self.contents(floor).0 == contents::SOLID;
        self.p.gravity = 0.0;
        let Some(trace) = trace_model(self.c.world, ladder, self.p.origin, center) else { return };
        if trace.fraction == 1.0 {
            return;
        }
        let mut spd = if self.p.maxspeed < 200.0 { self.p.maxspeed } else { 200.0 };
        let (vpn, v_right, _) = angle_vectors(self.p.angles);
        if self.p.flags & flags::DUCKING != 0 {
            spd = (f64::from(spd) * 0.333) as f32;
        }
        let b = i32::from(self.p.cmd.buttons);
        let mut fwd = 0.0f32;
        if b & buttons::BACK != 0 {
            fwd = 0.0 - spd;
        }
        if b & buttons::FORWARD != 0 {
            fwd += spd;
        }
        let mut rgt = 0.0f32;
        if b & buttons::MOVELEFT != 0 {
            rgt = 0.0 - spd;
        }
        if b & buttons::MOVERIGHT != 0 {
            rgt += spd;
        }
        let n = trace.plane_normal;
        if b & buttons::JUMP != 0 {
            self.p.movetype = movetype::WALK;
            self.p.velocity = scale(n, 270.0);
            return;
        }
        if fwd == 0.0 && rgt == 0.0 {
            self.p.velocity = [0.0; 3];
            return;
        }
        let vel = ma(scale(vpn, fwd), rgt, v_right);
        let mut perp = cross([0.0, 0.0, 1.0], n);
        normalize(&mut perp);
        let nrm = dot(vel, n);
        let crs = scale(n, nrm);
        let lateral = [vel[0] - crs[0], vel[1] - crs[1], vel[2] - crs[2]];
        let tmp = cross(n, perp);
        self.p.velocity = ma(lateral, -nrm, tmp);
        if on_floor && nrm > 0.0 {
            self.p.velocity = ma(self.p.velocity, 200.0, n);
        }
    }

    fn check_water_jump(&mut self) {
        if self.p.waterjumptime != 0.0 || self.p.velocity[2] < -180.0 {
            return;
        }
        let mut flatvel = [self.p.velocity[0], self.p.velocity[1], 0.0];
        let curspeed = normalize(&mut flatvel);
        let mut flatfwd = [self.p.forward[0], self.p.forward[1], 0.0];
        normalize(&mut flatfwd);
        if curspeed != 0.0 && dot(flatvel, flatfwd) < 0.0 {
            return;
        }
        let mut start = self.p.origin;
        start[2] += 8.0;
        let end = ma(start, 24.0, flatfwd);
        let save = self.p.usehull;
        self.p.usehull = 2;
        let tr = self.trace(start, end);
        if tr.fraction < 1.0 && tr.plane_normal[2].abs() < 0.1 {
            let saved = usize::try_from(save).ok().filter(|&h| h < 4).unwrap_or(0);
            start[2] += PLAYER_MAXS[saved][2] - 8.0;
            let end = ma(start, 24.0, flatfwd);
            self.p.movedir = ma([0.0; 3], -50.0, tr.plane_normal);
            let tr = self.trace(start, end);
            if tr.fraction == 1.0 {
                self.p.waterjumptime = 2000.0;
                self.p.velocity[2] = 225.0;
                self.p.oldbuttons |= buttons::JUMP;
                self.p.flags |= flags::WATERJUMP;
            }
        }
        self.p.usehull = save;
    }

    fn check_falling(&mut self) {
        if self.p.onground == -1 {
            return;
        }
        if !self.p.dead && self.p.fall_velocity >= 250.0 {
            let mut fvol = 0.5f32;
            let fall = self.p.fall_velocity;
            let play = if self.p.waterlevel > 0 {
                true
            } else if fall > 580.0 {
                fvol = 1.0;
                true
            } else if fall > 290.0 {
                fvol = 0.85;
                true
            } else {
                fall >= 350.0
            };
            if play {
                self.categorize_texture_type();
                let step = step_type(self.p.texture_type);
                self.play_step_sound(step, fvol);
                self.p.time_step_sound = 300;
                self.p.punchangle[2] = (f64::from(self.p.fall_velocity) * 0.013) as f32;
                if self.p.punchangle[0] > 8.0 {
                    self.p.punchangle[0] = 8.0;
                }
            }
        }
        if self.p.fall_velocity != 0.0 {
            note!(self, "landed: fall velocity {:.3} punch z {:.3}", self.p.fall_velocity, self.p.punchangle[2]);
        }
        self.p.fall_velocity = 0.0;
    }

    fn play_water_sounds(&mut self) {
        let (old, new) = (self.p.oldwaterlevel, self.p.waterlevel);
        if (old == 0 && new != 0) || (old != 0 && new == 0) {
            let k = (self.c.random)(0, 3).clamp(0, 3) as usize;
            let sample = ["player/pl_wade1.wav", "player/pl_wade2.wav", "player/pl_wade3.wav", "player/pl_wade4.wav"][k];
            self.play_sound(sample, 1.0);
        }
    }

    // ------------------------------------------------------------------ stuck / sounds / toss

    fn check_stuck(&mut self) -> bool {
        let pi = usize::try_from(self.p.player_index).ok().filter(|&i| i < 32).unwrap_or(0);
        let server = usize::from(self.p.server);
        let (hitent, _) = self.test_position(self.p.origin);
        if hitent == -1 {
            self.c.stuck.last[pi][server] = 0;
            return false;
        }
        if let Some(pe) = self.p.physents.get(hitent as usize) {
            note!(
                self,
                "stuck: physent {hitent} ent {} player {} model {:?} solid {} skin {} origin {} mins {} maxs {} usehull {}",
                pe.info,
                pe.player,
                pe.model,
                pe.solid,
                pe.skin,
                V3(pe.origin),
                V3(pe.mins),
                V3(pe.maxs),
                self.p.usehull
            );
        }
        let base = self.p.origin;
        let table = stuck_table();
        let is_bsp = |pm: &Self, h: i32| h == 0 || pm.p.physents.get(h as usize).is_some_and(|pe| pe.model.is_some());
        if !self.p.server && is_bsp(self, hitent) {
            self.c.stuck.last[pi][server] = 0;
            for _ in 0..STUCK_TABLE_SIZE {
                let off = self.random_stuck_offset(pi, server, &table).1;
                let test = [base[0] + off[0], base[1] + off[1], base[2] + off[2]];
                if self.test_position(test).0 == -1 {
                    self.c.stuck.last[pi][server] = 0;
                    note!(self, "stuck: nudged by {}", V3(off));
                    self.p.origin = test;
                    return false;
                }
            }
        }
        let idx = if self.p.server { 0 } else { 1 };
        let ftime = self.c.float_time as f32;
        if f64::from(self.c.stuck.check_time[pi][idx]) >= f64::from(ftime) - 0.05 {
            return true;
        }
        self.c.stuck.check_time[pi][idx] = ftime;
        // PM_StuckTouch is a no-op on the client; the server bookkeeping is not modelled.
        let (i, off) = self.random_stuck_offset(pi, server, &table);
        let test = [base[0] + off[0], base[1] + off[1], base[2] + off[2]];
        let hitent = self.test_position(test).0;
        if hitent == -1 {
            self.c.stuck.last[pi][server] = 0;
            if i >= 27 {
                self.p.origin = test;
            }
            return false;
        }
        let b = i32::from(self.p.cmd.buttons);
        let hit_player = self.p.physents.get(hitent as usize).is_some_and(|pe| pe.player != 0);
        if b & (buttons::JUMP | buttons::DUCK | buttons::ATTACK) != 0 && hit_player {
            let mut z = 0.0f32;
            while z <= 72.0 {
                let mut x = -8.0f32;
                while x <= 8.0 {
                    let mut y = -8.0f32;
                    while y <= 8.0 {
                        let test = [base[0] + x, base[1] + y, base[2] + z];
                        if self.test_position(test).0 == -1 {
                            self.p.origin = test;
                            return false;
                        }
                        y += 8.0;
                    }
                    x += 8.0;
                }
                z += 18.0;
            }
        }
        true
    }

    fn random_stuck_offset(&mut self, pi: usize, server: usize, table: &[Vec3; STUCK_TABLE_SIZE]) -> (usize, Vec3) {
        let k = self.c.stuck.last[pi][server];
        self.c.stuck.last[pi][server] += 1;
        let k = (k % STUCK_TABLE_SIZE as i32).rem_euclid(STUCK_TABLE_SIZE as i32) as usize;
        (k, table[k])
    }

    fn update_step_sound(&mut self) {
        if self.p.time_step_sound > 0 || self.p.flags & flags::FROZEN != 0 {
            return;
        }
        let ladder = self.p.movetype == movetype::FLY;
        if length(self.p.velocity) <= 150.0 {
            self.p.time_step_sound = 400;
            return;
        }
        if !ladder && self.p.onground == -1 {
            return;
        }
        self.categorize_texture_type();
        let h = self.hull();
        let height = PLAYER_MAXS[h][2] - PLAYER_MINS[h][2];
        let mut knee = self.p.origin;
        knee[2] = (f64::from(self.p.origin[2]) - 0.3 * f64::from(height)) as f32;
        let mut feet = self.p.origin;
        feet[2] = (f64::from(self.p.origin[2]) - f64::from(height) * 0.5) as f32;
        let (step, mut fvol) = if ladder {
            self.p.time_step_sound = 350;
            (STEP_LADDER, 0.35f32)
        } else if self.contents(knee).0 == contents::WATER {
            self.p.time_step_sound = 600;
            (STEP_WADE, 0.65)
        } else if self.contents(feet).0 == contents::WATER {
            self.p.time_step_sound = 300;
            (STEP_SLOSH, 0.5)
        } else {
            self.p.time_step_sound = 300;
            let vol = match self.p.texture_type {
                b'D' => 0.55,
                b'V' => 0.7,
                _ => 0.5,
            };
            (step_type(self.p.texture_type), vol)
        };
        if self.p.flags & flags::DUCKING != 0 || ladder {
            self.p.time_step_sound += 100;
            if self.p.flags & flags::DUCKING != 0 && self.p.duck_time < 950.0 {
                fvol = (f64::from(fvol) * 0.35) as f32;
            }
        }
        self.play_step_sound(step, fvol);
    }

    fn play_step_sound(&mut self, step: i32, fvol: f32) {
        self.p.step_left = i32::from(self.p.step_left == 0);
        if !self.p.runfuncs {
            return;
        }
        let mut irand = (self.c.random)(0, 1) + 2 * self.p.step_left;
        if self.p.multiplayer && !self.p.movevars.footsteps {
            return;
        }
        let pick = |base: &str, irand: i32| {
            let suffix = [1, 3, 2, 4][irand.clamp(0, 3) as usize];
            format!("player/{base}{suffix}.wav")
        };
        let sample = match step {
            STEP_METAL => pick("pl_metal", irand),
            STEP_DIRT => pick("pl_dirt", irand),
            STEP_VENT => pick("pl_duct", irand),
            STEP_GRATE => pick("pl_grate", irand),
            STEP_SLOSH => pick("pl_slosh", irand),
            STEP_LADDER => pick("pl_ladder", irand),
            STEP_SNOW => pick("pl_snow", irand),
            STEP_TILE => {
                if (self.c.random)(0, 4) == 0 {
                    irand = 4;
                }
                format!("player/pl_tile{}.wav", [1, 3, 2, 4, 5][irand.clamp(0, 4) as usize])
            }
            STEP_WADE => {
                let s = &mut self.c.stuck.skip_step;
                if *s == 0 {
                    *s = 1;
                    return;
                }
                let was = *s;
                *s += 1;
                if was == 3 {
                    *s = 0;
                }
                format!("player/pl_wade{}.wav", irand.clamp(0, 3) + 1)
            }
            _ => pick("pl_step", irand),
        };
        self.play_sound(&sample, fvol);
    }

    fn categorize_texture_type(&mut self) {
        let start = self.p.origin;
        let mut end = self.p.origin;
        end[2] -= 64.0;
        self.p.texture_name.clear();
        self.p.texture_type = b'C';
        let Some(name) = trace_texture(self.c.world, &self.p.physents, self.p.onground, start, end) else { return };
        let mut name = name.as_bytes();
        if matches!(name.first(), Some(b'-' | b'+')) {
            name = name.get(2..).unwrap_or(&[]);
        }
        if matches!(name.first(), Some(b'{' | b'!' | b'~' | b' ')) {
            name = &name[1..];
        }
        let name = String::from_utf8_lossy(&name[..name.len().min(16)]).into_owned();
        self.p.texture_type = self.c.textures.find(&name);
        self.p.texture_name = name;
    }

    fn physics_toss(&mut self) {
        self.check_water();
        if self.p.velocity[2] > 0.0 {
            self.p.onground = -1;
        }
        if self.p.onground != -1 && self.p.basevelocity == [0.0; 3] && self.p.velocity == [0.0; 3] {
            return;
        }
        self.check_velocity();
        if ![movetype::FLY, movetype::FLYMISSILE, movetype::BOUNCEMISSILE].contains(&self.p.movetype) {
            self.add_gravity();
        }
        self.add_base_velocity();
        self.check_velocity();
        let mv = scale(self.p.velocity, self.p.frametime);
        self.sub_base_velocity();
        let tr = self.push_entity(mv);
        self.check_velocity();
        if tr.allsolid {
            self.p.onground = tr.ent;
            self.p.velocity = [0.0; 3];
            return;
        }
        if tr.fraction == 1.0 {
            self.check_water();
            return;
        }
        let backoff = match self.p.movetype {
            movetype::BOUNCE => (2.0 - f64::from(self.p.friction)) as f32,
            movetype::BOUNCEMISSILE => 2.0,
            _ => 1.0,
        };
        self.p.velocity = clip_velocity(self.p.velocity, tr.plane_normal, backoff).0;
        if f64::from(tr.plane_normal[2]) > 0.7 {
            if self.p.velocity[2] < self.p.movevars.gravity * self.p.frametime {
                self.p.onground = tr.ent;
                self.p.velocity[2] = 0.0;
            }
            let v = self.p.velocity;
            if dot(v, v) < 900.0 || ![movetype::BOUNCE, movetype::BOUNCEMISSILE].contains(&self.p.movetype) {
                self.p.onground = tr.ent;
                self.p.velocity = [0.0; 3];
            } else {
                let s = ((1.0 - f64::from(tr.fraction)) * f64::from(self.p.frametime) * 0.9) as f32;
                let mv = scale(self.p.velocity, s);
                self.push_entity(mv);
            }
        }
        self.check_water();
    }

    fn push_entity(&mut self, push: Vec3) -> PmTrace {
        let end = [self.p.origin[0] + push[0], self.p.origin[1] + push[1], self.p.origin[2] + push[2]];
        let tr = self.trace(self.p.origin, end);
        self.p.origin = tr.endpos;
        if tr.fraction < 1.0 && !tr.allsolid {
            let v = self.p.velocity;
            self.add_to_touched(tr, v);
        }
        tr
    }
}

const STEP_CONCRETE: i32 = 0;
const STEP_METAL: i32 = 1;
const STEP_DIRT: i32 = 2;
const STEP_VENT: i32 = 3;
const STEP_GRATE: i32 = 4;
const STEP_TILE: i32 = 5;
const STEP_SLOSH: i32 = 6;
const STEP_WADE: i32 = 7;
const STEP_LADDER: i32 = 8;
const STEP_SNOW: i32 = 9;

/// `PM_MapTextureTypeStepType`.
fn step_type(c: u8) -> i32 {
    match c {
        b'D' => STEP_DIRT,
        b'G' => STEP_GRATE,
        b'M' => STEP_METAL,
        b'N' => STEP_SNOW,
        b'S' => STEP_SLOSH,
        b'T' => STEP_TILE,
        b'V' => STEP_VENT,
        _ => STEP_CONCRETE,
    }
}

/// `PM_CalcRoll`.
fn calc_roll(angles: Vec3, velocity: Vec3, rollangle: f32, rollspeed: f32) -> f32 {
    let (_, r, _) = angle_vectors(angles);
    let mut side = dot(velocity, r);
    let sign = if side < 0.0 { -1.0 } else { 1.0 };
    side = side.abs();
    side = if side < rollspeed { (side * rollangle) / rollspeed } else { rollangle };
    side * sign
}

/// `PM_ClipVelocity`: returns the clipped velocity and the blocked flags.
pub fn clip_velocity(input: Vec3, normal: Vec3, overbounce: f32) -> (Vec3, i32) {
    let blocked = i32::from(normal[2] > 0.0) | if normal[2] == 0.0 { 2 } else { 0 };
    let backoff = dot(input, normal) * overbounce;
    let out = [0, 1, 2].map(|i| {
        let v = input[i] - normal[i] * backoff;
        if f64::from(v) > -STOP_EPSILON && f64::from(v) < STOP_EPSILON { 0.0 } else { v }
    });
    (out, blocked)
}
