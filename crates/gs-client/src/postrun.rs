//! The client DLL's weapon prediction hook (docs/re/client-sim.md Part B): the `weapon_data_t`
//! slots of a predicted `local_state_t`, `HUD_PostRunCmd`, and the engine services it calls
//! while a command is predicted (`pfnPlaybackEvent`, `pfnWeaponAnim`, `CL_LoadModel`,
//! `pfnPrecacheEvent`, `EV_PlayerTrace`).

use gs_pmove::trace::player_trace;
use gs_pmove::{Physent, PmSound, PmTrace};
use gs_protocol::structs::{EventArgs, UserCmd, WeaponData};
use gs_world::World;

use crate::events::EventQueue;
use crate::predict::LocalState;

/// `FEV_SERVER`: events the client never plays locally.
const FEV_SERVER: i32 = 0x20;

/// What a first-time predicted command did that a demo records, in order: events (type 6),
/// view-model animations (type 7) and `PM_PlaySound`s (type 8).
#[derive(Debug, Clone, PartialEq)]
pub enum PredRecord {
    Event { flags: i32, index: u16, delay: f32, args: EventArgs },
    Anim { sequence: i32, body: i32 },
    Sound(PmSound),
}

/// `weapon_data_t` (one per weapon id; the client predicts slots 0..31).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WeaponFields {
    pub id: i32,
    pub clip: i32,
    pub next_primary_attack: f32,
    pub next_secondary_attack: f32,
    pub time_weapon_idle: f32,
    pub in_reload: i32,
    pub in_special_reload: i32,
    pub next_reload: f32,
    pub pump_time: f32,
    pub reload_time: f32,
    /// Carries `m_flLastFire` (absolute server time).
    pub aimed_damage: f32,
    pub next_aim_bonus: f32,
    /// Carries `m_iShotsFired`.
    pub in_zoom: i32,
    pub weapon_state: i32,
    /// `iuser1` carries the knife's `m_iSwing`.
    pub iuser: [i32; 4],
    /// `fuser2`/`fuser3` carry `m_flStartThrow`/`m_flReleaseThrow`.
    pub fuser: [f32; 4],
}

impl WeaponFields {
    pub fn from_data(d: &WeaponData) -> Self {
        Self {
            id: d.id(),
            clip: d.clip(),
            next_primary_attack: d.next_primary_attack(),
            next_secondary_attack: d.next_secondary_attack(),
            time_weapon_idle: d.time_weapon_idle(),
            in_reload: d.in_reload(),
            in_special_reload: d.in_special_reload(),
            next_reload: d.next_reload(),
            pump_time: d.pump_time(),
            reload_time: d.reload_time(),
            aimed_damage: d.aimed_damage(),
            next_aim_bonus: d.next_aim_bonus(),
            in_zoom: d.in_zoom(),
            weapon_state: d.weapon_state(),
            iuser: [d.iuser1(), d.iuser2(), d.iuser3(), d.iuser4()],
            fuser: [d.fuser1(), d.fuser2(), d.fuser3(), d.fuser4()],
        }
    }

    pub fn to_data(&self) -> WeaponData {
        let mut d = WeaponData::default();
        d.set_id(self.id);
        d.set_clip(self.clip);
        d.set_next_primary_attack(self.next_primary_attack);
        d.set_next_secondary_attack(self.next_secondary_attack);
        d.set_time_weapon_idle(self.time_weapon_idle);
        d.set_in_reload(self.in_reload);
        d.set_in_special_reload(self.in_special_reload);
        d.set_next_reload(self.next_reload);
        d.set_pump_time(self.pump_time);
        d.set_reload_time(self.reload_time);
        d.set_aimed_damage(self.aimed_damage);
        d.set_next_aim_bonus(self.next_aim_bonus);
        d.set_in_zoom(self.in_zoom);
        d.set_weapon_state(self.weapon_state);
        let [i1, i2, i3, i4] = self.iuser;
        d.set_iuser1(i1);
        d.set_iuser2(i2);
        d.set_iuser3(i3);
        d.set_iuser4(i4);
        let [f1, f2, f3, f4] = self.fuser;
        d.set_fuser1(f1);
        d.set_fuser2(f2);
        d.set_fuser3(f3);
        d.set_fuser4(f4);
        d
    }
}

/// `HUD_PostRunCmd(from, to, cmd, runfuncs, time, random_seed)`: called after each predicted
/// command's player move. `time` is the command's start time.
pub trait PostRunCmd {
    #[allow(clippy::too_many_arguments)]
    fn post_run_cmd(&mut self, env: &mut WeaponEnv, from: &LocalState, to: &mut LocalState, cmd: &UserCmd, runfuncs: bool, time: f64, random_seed: u32);
}

/// No client weapon code (tools, headless runs).
#[derive(Debug, Default)]
pub struct NoWeapons;

impl PostRunCmd for NoWeapons {
    fn post_run_cmd(&mut self, _: &mut WeaponEnv, _: &LocalState, _: &mut LocalState, _: &UserCmd, _: bool, _: f64, _: u32) {}
}

/// A locally played event (`cle_pfnPlaybackEvent` arguments after `HUD_PlaybackEvent`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LocalEvent {
    pub flags: i32,
    pub index: u16,
    pub delay: f32,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub fparam: [f32; 2],
    pub iparam: [i32; 2],
    pub bparam: [bool; 2],
}

/// The engine as the client weapon code sees it during one predicted command.
pub struct WeaponEnv<'a> {
    pub(crate) world: &'a World,
    /// `pmove->physents` with the other players (`EV_SetSolidPlayers`).
    pub(crate) physents: &'a [Physent],
    pub(crate) events: &'a mut EventQueue,
    pub(crate) model_names: &'a [Option<String>],
    pub(crate) event_names: &'a [Option<String>],
    /// `cl.time` (delayed events fire from it).
    pub(crate) cl_time: f64,
    pub(crate) playernum: usize,
    /// The engine's current predicted velocity and ducked hull (event arguments).
    pub(crate) velocity: [f32; 3],
    pub(crate) usehull: i32,
    /// `pfnWeaponAnim` calls, applied after the prediction pass.
    pub(crate) weapon_anims: &'a mut Vec<(i32, i32)>,
    /// Set by [`WeaponEnv::hide_view_entity`], applied after the prediction pass.
    pub(crate) hide_view: &'a mut bool,
    /// The demo writer's view of this pass.
    pub(crate) demo_log: &'a mut Vec<PredRecord>,
    /// `pmove->usehull` as the last `EV_SetTraceHull` left it (`None`: untouched).
    pub(crate) trace_hull: Option<i32>,
}

impl WeaponEnv<'_> {
    /// `cle_pfnPlaybackEvent`: the event for the local player into the event queue.
    pub fn playback_event(&mut self, e: &LocalEvent) {
        if e.flags & FEV_SERVER != 0 || e.index == 0 {
            return;
        }
        let mut args = EventArgs::default();
        args.set_flags(0);
        args.set_entindex(self.playernum as i32 + 1);
        args.set_origin(e.origin);
        args.set_angles(e.angles);
        args.set_velocity(self.velocity);
        args.set_ducking(i32::from(self.usehull == 1));
        args.set_fparam1(e.fparam[0]);
        args.set_fparam2(e.fparam[1]);
        args.set_iparam1(e.iparam[0]);
        args.set_iparam2(e.iparam[1]);
        args.set_bparam1(i32::from(e.bparam[0]));
        args.set_bparam2(i32::from(e.bparam[1]));
        self.events.queue(e.flags, e.index, e.delay, &args, self.cl_time);
        self.demo_log.push(PredRecord::Event { flags: e.flags, index: e.index, delay: e.delay, args });
    }

    /// `cle_pfnWeaponAnim`.
    pub fn weapon_anim(&mut self, sequence: i32, body: i32) {
        self.weapon_anims.push((sequence, body));
        self.demo_log.push(PredRecord::Anim { sequence, body });
    }

    /// `GetViewEntity()->baseline.effects |= EF_NODRAW` and the same on `curstate`.
    pub fn hide_view_entity(&mut self) {
        *self.hide_view = true;
    }

    /// `CL_LoadModel`: the precache index of a model the server precached (0 if unknown).
    pub fn model_index(&self, name: &str) -> i32 {
        self.model_names.iter().position(|n| n.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name))).map_or(0, |i| i as i32)
    }

    /// `pfnPrecacheEvent(1, name)`: the server's index of an event script (0 if unknown).
    pub fn event_index(&self, name: &str) -> u16 {
        self.event_names.iter().position(|n| n.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name))).map_or(0, |i| i as u16)
    }

    /// `EV_SetTraceHull(usehull)` + `EV_PlayerTrace(start, end, flags, -1)`; the hull stays in
    /// `pmove->usehull` after the call.
    pub fn trace(&mut self, start: [f32; 3], end: [f32; 3], usehull: i32, flags: i32) -> PmTrace {
        self.trace_hull = Some(usehull);
        player_trace(self.world, self.physents, usehull, start, end, flags, -1)
    }
}
