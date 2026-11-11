//! Client movement prediction: `CL_SetSolidEntities` / `CL_SetSolidPlayers`, `CL_PredictMove`,
//! `CL_RunUsercmd` and prediction-error smoothing (docs/re/movement.md §B.12, §C.3, §C.4).

use std::sync::Arc;

use gs_common::rng::EngineRng;
use gs_pmove::math::{Vec3, length};
use gs_pmove::{PLAYER_MAXS, PLAYER_MINS, Physent, PlayerMove, PmContext, PmSound, StuckState, StudioPhys, TextureTypes, pm_move};
use gs_protocol::messages::MoveVars;
use gs_protocol::state::Frame;
use gs_protocol::structs::{ClientData, EntityState, UserCmd};
use gs_studio::StudioModel;
use gs_world::World;

use crate::entities::Entities;
use crate::events::EventQueue;
use crate::postrun::{PostRunCmd, PredRecord, WeaponEnv, WeaponFields};
use crate::predlog::{PredLog, ServerState};

const UPDATE_MASK: u32 = 63;
const FL_ONGROUND: i32 = 0x200;
const EF_NODRAW: i32 = 0x80;
const SOLID_BBOX: i32 = 2;
const SOLID_BSP: i32 = 4;
const CONTENTS_LADDER: i32 = -16;

/// `entity_state_t` fields of the local player that prediction uses.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PlayerFields {
    pub number: i32,
    pub origin: Vec3,
    pub angles: Vec3,
    pub basevelocity: Vec3,
    pub friction: f32,
    pub gravity: f32,
    pub usehull: i32,
    pub oldbuttons: i32,
    pub movetype: i32,
    pub effects: i32,
    pub step_left: i32,
    pub fall_velocity: f32,
}

/// `clientdata_t` fields prediction reads and writes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ClientFields {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub viewmodel: i32,
    pub punchangle: Vec3,
    pub flags: i32,
    pub waterlevel: i32,
    pub watertype: i32,
    pub view_ofs: Vec3,
    pub health: f32,
    pub in_duck: bool,
    pub time_step_sound: i32,
    pub duck_time: i32,
    pub swim_time: i32,
    pub waterjumptime: i32,
    pub maxspeed: f32,
    pub deadflag: i32,
    pub iuser: [i32; 4],
    pub fuser: [f32; 4],
    pub vuser: [Vec3; 4],
    /// Active weapon id (`m_iId`).
    pub m_id: i32,
    /// `ammo_shells`, `ammo_nails`, `ammo_cells`, `ammo_rockets`.
    pub ammo: [i32; 4],
    /// `m_flNextAttack`.
    pub next_attack: f32,
    pub fov: f32,
    pub weaponanim: i32,
}

impl ClientFields {
    pub(crate) fn from_clientdata(cd: &ClientData) -> Self {
        Self {
            origin: cd.origin(),
            velocity: cd.velocity(),
            viewmodel: cd.viewmodel(),
            punchangle: cd.punchangle(),
            flags: cd.flags(),
            waterlevel: cd.waterlevel(),
            watertype: cd.watertype(),
            view_ofs: cd.view_ofs(),
            health: cd.health(),
            in_duck: cd.in_duck() != 0,
            time_step_sound: cd.time_step_sound(),
            duck_time: cd.duck_time(),
            swim_time: cd.swim_time(),
            waterjumptime: cd.waterjumptime(),
            maxspeed: cd.maxspeed(),
            deadflag: cd.deadflag(),
            iuser: [cd.iuser1(), cd.iuser2(), cd.iuser3(), cd.iuser4()],
            fuser: [cd.fuser1(), cd.fuser2(), cd.fuser3(), cd.fuser4()],
            vuser: [cd.vuser1(), cd.vuser2(), cd.vuser3(), cd.vuser4()],
            m_id: cd.m_id(),
            ammo: [cd.ammo_shells(), cd.ammo_nails(), cd.ammo_cells(), cd.ammo_rockets()],
            next_attack: cd.next_attack(),
            fov: cd.fov(),
            weaponanim: cd.weaponanim(),
        }
    }
}

/// The weapon slots the client predicts (`weapon_data_t` 0..31).
pub const PREDICTED_WEAPONS: usize = 32;

/// `local_state_t`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LocalState {
    pub player: PlayerFields,
    pub client: ClientFields,
    pub weapons: [WeaponFields; PREDICTED_WEAPONS],
}

/// Predicted view inputs (`cl.simorg`, `cl.simvel`, ...).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SimState {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub punchangle: Vec3,
    pub viewheight: Vec3,
    /// Entity number of the ground, or -1.
    pub onground: i32,
    pub waterlevel: i32,
    pub usehull: i32,
    /// Standing on a moving entity (no error smoothing).
    pub moving: bool,
    pub viewmodel: i32,
    /// A prediction has run since the level started.
    pub valid: bool,
    /// `cl.idealpitch` from the ideal-pitch probe (kept when the probe aborts).
    pub idealpitch: f32,
}

/// A command slot as prediction sees it.
#[derive(Debug, Clone, Copy, Default)]
pub struct PredCmd {
    pub cmd: UserCmd,
    /// `(f32)realtime` when the command was built.
    pub senttime: f32,
    pub processedfuncs: bool,
    /// `frame_lerp` (written by `CL_EmitEntities`), `heldback` (not sent this frame) and
    /// `sendsize` (the move message size): read only by net_graph.
    pub frame_lerp: f32,
    pub heldback: bool,
    pub sendsize: i32,
}

/// Everything the prediction needs from the engine for one run.
pub struct PredictInput<'a> {
    pub frame: &'a Frame,
    pub entities: &'a Entities,
    pub model_names: &'a [Option<String>],
    pub studio_models: &'a [Option<Arc<StudioModel>>],
    pub movevars: &'a MoveVars,
    pub maxclients: usize,
    pub playernum: usize,
    pub incoming_acknowledged: u32,
    pub outgoing_sequence: u32,
    /// `realtime`.
    pub realtime: f64,
    /// `host_frametime`.
    pub frametime: f64,
    /// `cl.time - ex_interp` for other players' positions.
    pub interp_time: f32,
    /// `cl.time` (locally predicted events are queued at it).
    pub cl_time: f64,
    /// `cl.event_precache` names.
    pub event_names: &'a [Option<String>],
    /// `cl.viewangles` (copied to `cl.simangles` when prediction starts).
    pub viewangles: Vec3,
    /// `cl_nosmooth`, `cl_smoothtime`, `cl_idealpitchscale`.
    pub no_smooth: bool,
    pub smooth_time: f32,
    pub ideal_pitch_scale: f32,
    /// `cl_solid_players`: `CL_SetSolidPlayers` returns at once when it is 0.
    pub solid_players: bool,
}

pub struct Prediction {
    pub world: Option<Arc<World>>,
    pub textures: TextureTypes,
    stuck: StuckState,
    states: Vec<LocalState>,
    predicted_origins: [Vec3; 64],
    pub sim: SimState,
    smooth_timer: f64,
    last_simorg: Vec3,
    /// The engine generator behind `pmove->RandomLong`.
    rng: EngineRng,
    /// `PM_PlaySound` requests from first-time command runs.
    pub sounds: Vec<PmSound>,
    /// `g_pred_onground`: ground entity after the last `CL_RunUsercmd`.
    pred_onground: i32,
    /// `pmove->usehull` after the last `CL_RunUsercmd` + `HUD_PostRunCmd` (a weapon's
    /// `EV_SetTraceHull` outlives the command); the ideal-pitch probe traces with it.
    pmove_usehull: i32,
    /// `pmove->physents` after `CL_PopPMStates`: world + solid entities, no players. Render-time
    /// traces (glow sprites, shadows) use this list.
    pub physents: Vec<Physent>,
    /// `cl_smoothtime` was at or below 0 during smoothing: `Cvar_DirectSet(cl_smoothtime, "0.1")`.
    pub smooth_time_reset: bool,
    /// `pfnWeaponAnim` calls of the client weapon code during the last prediction pass.
    pub weapon_anims: Vec<(i32, i32)>,
    /// The weapon code hid the view entity (resume zoom) during the last pass.
    pub hide_view_entity: bool,
    /// Events, weapon animations and sounds of first-time runs, for the demo writer.
    pub demo_log: Vec<PredRecord>,
    /// `pmove->movedir`: `CL_RunUsercmd` does not load it, so the global keeps the value of the
    /// last `PM_CheckWaterJump` for later commands (and repredictions).
    movedir: Vec3,
    /// `cl_predlog`'s writer while it is on.
    pub(crate) log: Option<PredLog>,
    /// The move trace of the command being run (kept only while logging).
    move_trace: Option<String>,
    /// The last run's simulated origin before error smoothing, and the share of the move the
    /// smoothing let through (1 without smoothing), for the log.
    last_raw_origin: Vec3,
    last_keep: f32,
}

impl Prediction {
    pub fn new(rng: EngineRng) -> Self {
        Self {
            world: None,
            textures: TextureTypes::default(),
            stuck: StuckState::default(),
            states: vec![LocalState::default(); 64],
            predicted_origins: [[0.0; 3]; 64],
            sim: SimState { onground: -1, ..SimState::default() },
            smooth_timer: 0.0,
            last_simorg: [0.0; 3],
            rng,
            sounds: Vec::new(),
            pred_onground: -1,
            pmove_usehull: 0,
            physents: Vec::new(),
            smooth_time_reset: false,
            weapon_anims: Vec::new(),
            hide_view_entity: false,
            demo_log: Vec::new(),
            movedir: [0.0; 3],
            log: None,
            move_trace: None,
            last_raw_origin: [0.0; 3],
            last_keep: 1.0,
        }
    }
}

fn brush_index(name: Option<&str>) -> Option<usize> {
    name?.strip_prefix('*')?.parse().ok()
}

/// `CL_AddStateToEntlist` for movement: the physent of a packet entity. BSP solids use the
/// brush; BBOX solids carry the studio model only when it traces hitboxes; other solids of a
/// studio model carry it, of a brush model use the brush.
fn physent_for(state: &EntityState, brush: Option<usize>, studio: Option<&Arc<StudioModel>>) -> Physent {
    let solid = i32::from(state.solid());
    let studio_phys = |m: &Arc<StudioModel>| StudioPhys { model: Arc::clone(m), frame: state.frame(), sequence: state.sequence(), controller: state.controller(), blending: state.blending() };
    let (model, studiomodel) = match solid {
        SOLID_BSP => (brush, None),
        SOLID_BBOX => (None, studio.filter(|m| m.flags & gs_pmove::studio_hull::STUDIO_TRACE_HITBOX != 0).map(studio_phys)),
        _ => match studio {
            Some(m) => (None, Some(studio_phys(m))),
            None => (brush, None),
        },
    };
    Physent {
        player: 0,
        origin: state.origin(),
        model,
        studiomodel,
        mins: state.mins(),
        maxs: state.maxs(),
        info: state.number(),
        angles: state.angles(),
        solid,
        skin: i32::from(state.skin()),
        rendermode: state.rendermode(),
        movetype: state.movetype(),
        team: state.team(),
        classnumber: state.playerclass(),
    }
}

impl Prediction {
    /// Forget the level (disconnect); the materials table and the engine RNG stay.
    pub fn reset(&mut self) {
        let textures = std::mem::take(&mut self.textures);
        *self = Self { textures, ..Self::new(self.rng.clone()) };
    }

    /// `CL_SetSolidEntities`: world + solid packet entities, ladders into `moveents`.
    fn solid_entities(input: &PredictInput) -> (Vec<Physent>, Vec<Physent>) {
        let mut physents = vec![Physent { model: Some(0), solid: SOLID_BSP, ..Physent::default() }];
        let mut moveents = Vec::new();
        for state in &input.frame.entities {
            let number = state.number();
            if (1..=input.maxclients as i32).contains(&number) || state.modelindex() == 0 {
                continue;
            }
            let Ok(mi) = usize::try_from(state.modelindex()) else { continue };
            let Some(name) = input.model_names.get(mi).and_then(|n| n.as_deref()) else { continue };
            if state.owner() != 0 && state.owner() - 1 == input.playernum as i32 {
                continue;
            }
            let solid = i32::from(state.solid());
            let skin = i32::from(state.skin());
            if solid == 1 || (solid == 0 && skin >= -1) {
                continue;
            }
            let (mins, maxs) = (state.mins(), state.maxs());
            if mins[2] == 0.0 && maxs[2] == 1.0 {
                continue;
            }
            if length([maxs[0] - mins[0], maxs[1] - mins[1], maxs[2] - mins[2]]) == 0.0 {
                continue;
            }
            let brush = brush_index(Some(name)).filter(|&b| b != 0);
            let studio = input.studio_models.get(mi).and_then(Option::as_ref);
            if brush.is_none() && studio.is_none() {
                continue;
            }
            let pe = physent_for(state, brush, studio);
            if solid == 0 && skin == CONTENTS_LADDER {
                if moveents.len() < 64 {
                    moveents.push(pe);
                } else {
                    gs_common::conprint::dprintf("CL_SetSolidEntities:  pmove->nummoveent >= MAX_MOVEENTS\n");
                }
            } else if physents.len() < 600 {
                physents.push(pe);
            } else {
                gs_common::conprint::dprintf("CL_SetSolidEntities:  pmove->numphysent >= MAX_PHYSENTS\n");
            }
        }
        (physents, moveents)
    }

    /// `CL_SetUpPlayerPrediction` + `CL_SetSolidPlayers(playernum)`: other players as boxes at
    /// their interpolated positions.
    fn solid_players(input: &PredictInput, physents: &mut Vec<Physent>) {
        for state in &input.frame.entities {
            let number = state.number();
            let Ok(j) = usize::try_from(number - 1) else { continue };
            if j >= input.maxclients || j == input.playernum || state.modelindex() == 0 {
                continue;
            }
            if state.effects() & EF_NODRAW != 0 && j == input.playernum {
                continue;
            }
            let solid = i32::from(state.solid());
            if solid == 0 {
                continue;
            }
            let (origin, angles) = input
                .entities
                .player_position(number as usize, input.interp_time)
                .unwrap_or((state.origin(), state.angles()));
            let hull = usize::try_from(state.usehull()).ok().filter(|&h| h < 4).unwrap_or(0);
            physents.push(Physent {
                player: 1,
                origin,
                model: None,
                studiomodel: None,
                mins: PLAYER_MINS[hull],
                maxs: PLAYER_MAXS[hull],
                info: number,
                angles,
                solid,
                skin: 0,
                rendermode: state.rendermode(),
                movetype: state.movetype(),
                team: state.team(),
                classnumber: state.playerclass(),
            });
        }
    }

    /// The prediction start state for a received frame (`HUD_TxferLocalOverrides`,
    /// `HUD_ProcessPlayerState`, `HUD_TxferPredictionData`).
    fn seed(&self, input: &PredictInput) -> LocalState {
        let cd = &input.frame.clientdata;
        let client = ClientFields::from_clientdata(cd);
        let predicted = self.states[(input.incoming_acknowledged & UPDATE_MASK) as usize].player;
        let player = input
            .frame
            .entities
            .iter()
            .find(|s| s.number() == input.playernum as i32 + 1)
            .map(|s| PlayerFields {
                number: s.number(),
                origin: cd.origin(),
                angles: s.angles(),
                basevelocity: s.basevelocity(),
                friction: s.friction(),
                gravity: s.gravity(),
                usehull: s.usehull(),
                oldbuttons: predicted.oldbuttons,
                movetype: s.movetype(),
                effects: s.effects(),
                step_left: predicted.step_left,
                fall_velocity: predicted.fall_velocity,
            })
            .unwrap_or(PlayerFields { number: input.playernum as i32 + 1, origin: cd.origin(), ..predicted });
        let mut weapons = [WeaponFields::default(); PREDICTED_WEAPONS];
        for (w, d) in weapons.iter_mut().zip(&input.frame.weapondata) {
            *w = WeaponFields::from_data(d);
        }
        LocalState { player, client, weapons }
    }

    /// `HUD_TxferPredictionData` for the weapon slots: a slot the server did not send in this
    /// packet keeps the value predicted after the newly acknowledged command, not the delta base.
    pub fn transfer_weapons(&self, frame: &mut Frame, incoming_acknowledged: u32) {
        if !self.sim.valid {
            return;
        }
        let predicted = &self.states[(incoming_acknowledged & UPDATE_MASK) as usize].weapons;
        for (i, (slot, w)) in frame.weapondata.iter_mut().zip(predicted).enumerate() {
            if frame.weapondata_sent & (1 << i) == 0 {
                *slot = w.to_data();
            }
        }
    }

    /// `CL_CheckPredictionError`: how far the server put us from where we predicted. Returns the
    /// error for `cl_showerror`'s print when it is above 0.5 units.
    pub fn check_error(&mut self, frame: &Frame, incoming_acknowledged: u32, parsecount: u32, maxclients: usize, smooth_time: f32) -> Option<f32> {
        let server = frame.clientdata.origin();
        let predicted = self.predicted_origins[(incoming_acknowledged & UPDATE_MASK) as usize];
        let err = [server[0] - predicted[0], server[1] - predicted[1], server[2] - predicted[2]];
        let len = length(err);
        if len > 64.0 {
            return None;
        }
        // Original bug: stored at the parse index, not the acknowledged command's.
        self.predicted_origins[(parsecount & UPDATE_MASK) as usize] = server;
        if len > 0.25 && maxclients != 1 {
            self.smooth_timer = f64::from(smooth_time);
        }
        (len > 0.5).then_some(len)
    }

    /// `cl_predlog`'s check of a packet's acknowledged command (before `check_error` stores the
    /// server origin at the parse slot).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn log_check(&mut self, now: f64, phase: &str, frame: &Frame, seq: u32, ack: u32, outgoing: u32, playernum: usize) {
        let Some(log) = self.log.as_mut() else { return };
        let ent = frame.entities.iter().find(|s| s.number() == playernum as i32 + 1);
        let server = ServerState {
            client: ClientFields::from_clientdata(&frame.clientdata),
            usehull: ent.map(|s| s.usehull()),
            basevelocity: ent.map(|s| s.basevelocity()),
            friction: ent.map(|s| s.friction()),
            gravity: ent.map(|s| s.gravity()),
            movetype: ent.map(|s| s.movetype()),
            time: frame.time,
        };
        let slot = (ack & UPDATE_MASK) as usize;
        log.check(now, phase, seq, ack, outgoing, &server, &self.states[slot], self.predicted_origins[slot]);
    }

    /// `cl_predlog`'s frame line: the simulated origin, its smoothing and the view origin.
    pub(crate) fn log_view(&mut self, now: f64, frametime: f64, vieworg: Vec3) {
        let Some(log) = self.log.as_mut() else { return };
        log.view(now, frametime, self.sim.origin, self.last_raw_origin, self.last_keep, self.sim.onground, self.sim.viewheight[2], vieworg);
    }

    /// `CL_PredictMove(repredicting)`; each command runs `HUD_PostRunCmd` through `post`, whose
    /// locally played events go to `events`.
    pub fn predict(&mut self, input: &PredictInput, commands: &mut [PredCmd; 64], repredicting: bool, events: &mut EventQueue, post: &mut dyn PostRunCmd) {
        let Some(world) = self.world.clone() else { return };
        let (ack, outgoing) = (input.incoming_acknowledged, input.outgoing_sequence);
        if !input.frame.valid || outgoing.wrapping_sub(ack).wrapping_add(1) >= 63 {
            return;
        }
        let rt = input.realtime as f32;
        let mut from = self.seed(input);
        log::trace!(
            "predict ack {ack} outgoing {outgoing} redo {repredicting}: seed origin {:?} vel {:?} friction {} gravity {} movetype {} hull {} maxspeed {} flags {:#x}",
            from.player.origin,
            from.client.velocity,
            from.player.friction,
            from.player.gravity,
            from.player.movetype,
            from.player.usehull,
            from.client.maxspeed,
            from.client.flags
        );
        let mut fromcmd = commands[(ack & UPDATE_MASK) as usize];
        let mut time = input.frame.time;
        let (mut physents, moveents) = Self::solid_entities(input);
        self.physents = physents.clone();
        if input.solid_players {
            Self::solid_players(input, &mut physents);
        }
        let mut pass = Pass { world: &world, physents: &physents, moveents: &moveents, input, events, post, spectator: false, jump: None };
        let stop = u32::from(!repredicting);
        let mut to = from;
        let mut tocmd = fromcmd;
        let mut ran = false;
        let mut i = 1u32;
        while i < 63 {
            if ack.wrapping_add(i) >= outgoing.wrapping_add(stop) {
                break;
            }
            let slot = (ack.wrapping_add(i) & UPDATE_MASK) as usize;
            let runfuncs = !repredicting && !commands[slot].processedfuncs;
            let cmd = commands[slot].cmd;
            self.move_trace = self.log.as_mut().map(|l| l.take_trace(slot));
            to = self.run_usercmd(&mut pass, from, &cmd, runfuncs, &mut time, ack.wrapping_add(i));
            if let (Some(log), Some(trace)) = (self.log.as_mut(), self.move_trace.take()) {
                log.store_move(slot, ack.wrapping_add(i), &cmd, trace);
                if runfuncs {
                    log.cmd(input.realtime, ack.wrapping_add(i), &cmd, &from, &to);
                }
            }
            commands[slot].processedfuncs = true;
            tocmd = commands[slot];
            ran = true;
            self.states[slot] = to;
            self.predicted_origins[slot] = to.player.origin;
            if commands[slot].senttime >= rt {
                break;
            }
            from = to;
            fromcmd = commands[slot];
            i += 1;
        }
        if i >= 63 {
            return;
        }
        if !ran {
            if !repredicting {
                return;
            }
            to = from;
            tocmd = fromcmd;
        }
        let f = if tocmd.senttime == fromcmd.senttime {
            0.0
        } else {
            f64::from((rt - fromcmd.senttime) / (tocmd.senttime - fromcmd.senttime)).max(0.0) as f32
        };
        let lerp = |a: Vec3, b: Vec3| [0, 1, 2].map(|k| (b[k] - a[k]) * f + a[k]);
        let teleported = (0..3).any(|k| f64::from(to.player.origin[k] - from.player.origin[k]).abs() > 128.0);
        let sim = &mut self.sim;
        if teleported {
            sim.velocity = to.client.velocity;
            sim.origin = to.player.origin;
            sim.punchangle = to.client.punchangle;
            sim.viewheight = to.client.view_ofs;
        } else {
            sim.origin = lerp(from.player.origin, to.player.origin);
            sim.velocity = lerp(from.client.velocity, to.client.velocity);
            sim.punchangle = lerp(from.client.punchangle, to.client.punchangle);
            sim.viewheight = if from.player.usehull == to.player.usehull {
                lerp(from.client.view_ofs, to.client.view_ofs)
            } else {
                to.client.view_ofs
            };
        }
        sim.waterlevel = to.client.waterlevel;
        sim.usehull = to.player.usehull;
        sim.viewmodel = to.client.viewmodel;
        sim.onground = if to.client.flags & FL_ONGROUND != 0 { self.pred_onground } else { -1 };
        sim.moving = false;
        sim.valid = true;
        if sim.onground > 0
            && let Some(ent) = input.entities.get(sim.onground as usize)
        {
            let (c, p) = (ent.curstate.origin(), ent.prevstate.origin());
            if length([c[0] - p[0], c[1] - p[1], 0.0]) > 0.0 {
                sim.moving = true;
                self.smooth_timer = 0.0;
            }
        }
        // Error smoothing; a cl_smoothtime at or below 0 is put back to 0.1.
        let raw = sim.origin;
        let mut keep_share = 1.0f32;
        if self.smooth_timer > 0.0 && !input.no_smooth && input.smooth_time != 0.0 {
            if !repredicting {
                self.smooth_timer -= input.frametime;
            }
            let mut smooth_time = f64::from(input.smooth_time);
            if smooth_time <= 0.0 {
                self.smooth_time_reset = true;
                smooth_time = 0.1;
            }
            self.smooth_timer = self.smooth_timer.clamp(0.0, smooth_time);
            let frac = (self.smooth_timer / smooth_time) as f32;
            let keep = (1.0 - f64::from(frac)) as f32;
            keep_share = keep;
            let last = self.last_simorg;
            sim.origin = [0, 1, 2].map(|k| last[k] + (sim.origin[k] - last[k]) * keep);
        }
        self.last_simorg = sim.origin;
        self.last_raw_origin = raw;
        self.last_keep = keep_share;
        // The probe runs after CL_PopPMStates: world and solid entities, without the players
        // CL_SetSolidPlayers appended.
        if sim.onground != -1 {
            let without_players = physents.len().min(self.physents.len());
            self.ideal_pitch(&world, &physents[..without_players], input.viewangles[1], input.ideal_pitch_scale);
        }
    }

    /// `CL_PredictMove`'s ideal-pitch probe (QuakeWorld's `V_SetIdealPitch`): six traces down
    /// from points ahead along the view yaw; a constant step between them gives
    /// `cl.idealpitch = -step * cl_idealpitchscale`, level ground 0; anything else keeps it.
    fn ideal_pitch(&mut self, world: &World, physents: &[Physent], yaw: f32, scale: f32) {
        let sim = self.sim;
        let ang = ((f64::from(yaw) * std::f64::consts::PI + f64::from(yaw) * std::f64::consts::PI) / 360.0) as f32;
        let (sinval, cosval) = (f64::from(ang).sin() as f32, f64::from(ang).cos() as f32);
        let mut z = [0.0f32; 6];
        for (i, zi) in z.iter_mut().enumerate() {
            let n = (i + 3) as f32;
            let top = [sim.origin[0] + cosval * n * 12.0, sim.origin[1] + sinval * n * 12.0, sim.origin[2] + sim.viewheight[2]];
            let bottom = [top[0], top[1], top[2] - 160.0];
            let tr = gs_pmove::trace::player_trace(world, physents, self.pmove_usehull, top, bottom, 2, -1);
            if tr.allsolid || tr.fraction == 1.0 {
                return;
            }
            *zi = top[2] + (bottom[2] - top[2]) * tr.fraction;
        }
        let (mut dir, mut steps) = (0i32, 0);
        for j in 0..5 {
            let step = (z[j + 1] - z[j]) as i32;
            if step == 0 {
                continue;
            }
            if dir != 0 && step != dir {
                return;
            }
            steps += 1;
            dir = step;
        }
        if dir == 0 {
            self.sim.idealpitch = 0.0;
        } else if steps >= 2 {
            self.sim.idealpitch = -(dir as f32) * scale;
        }
    }

    /// `CL_MoveSpectatorCamera`: the HLTV spectator's own pmove (`pmove->spectator`, players
    /// as physents) on `state` with this frame's `cl.cmd`, seeded `(u32)(i64)(cl.time*100)`;
    /// `jump` is the HUD's `iJumpSpectator` snap, taken when roaming consumes it. The result
    /// becomes `cl.simorg` & co.
    pub fn move_spectator(
        &mut self,
        input: &PredictInput,
        state: &mut LocalState,
        cmd: &UserCmd,
        jump: &mut Option<([f32; 3], [f32; 3])>,
        events: &mut EventQueue,
        post: &mut dyn PostRunCmd,
    ) {
        let Some(world) = self.world.clone() else { return };
        let (mut physents, moveents) = Self::solid_entities(input);
        // CL_Move's CL_SetSolidEntities runs every frame, HLTV included.
        self.physents = physents.clone();
        if input.solid_players {
            Self::solid_players(input, &mut physents);
        }
        let mut time = input.cl_time;
        let seed = (input.cl_time * 100.0) as i64 as u32;
        let mut pass = Pass { world: &world, physents: &physents, moveents: &moveents, input, events, post, spectator: true, jump: jump.take() };
        *state = self.run_usercmd(&mut pass, *state, cmd, true, &mut time, seed);
        *jump = pass.jump.take();
        self.sim.velocity = state.client.velocity;
        self.sim.origin = state.player.origin;
        self.sim.punchangle = state.client.punchangle;
        self.sim.viewheight = state.client.view_ofs;
        self.sim.valid = true;
    }

    /// `CL_RunUsercmd`: one command through `PM_Move` and `HUD_PostRunCmd`, split when longer
    /// than 50 ms (both halves with the same seed).
    fn run_usercmd(&mut self, pass: &mut Pass, from: LocalState, cmd: &UserCmd, runfuncs: bool, time: &mut f64, seed: u32) -> LocalState {
        if f64::from(cmd.msec) > 50.0 {
            let mut split = *cmd;
            split.msec = (f64::from(cmd.msec) * 0.5) as u8;
            let mid = self.run_usercmd(pass, from, &split, runfuncs, time, seed);
            split.impulse = 0;
            split.weaponselect = 0;
            return self.run_usercmd(pass, mid, &split, runfuncs, time, seed);
        }
        let (world, physents, moveents, input) = (pass.world, pass.physents, pass.moveents, pass.input);
        let (ps, cd) = (from.player, from.client);
        let mut pm = PlayerMove {
            player_index: ps.number - 1,
            server: false,
            multiplayer: input.maxclients > 1,
            time: (*time * 1000.0) as f32,
            frametime: (f64::from(cmd.msec) / 1000.0) as f32,
            origin: ps.origin,
            angles: ps.angles,
            oldangles: ps.angles,
            velocity: cd.velocity,
            basevelocity: ps.basevelocity,
            view_ofs: cd.view_ofs,
            duck_time: cd.duck_time as f32,
            in_duck: cd.in_duck,
            time_step_sound: cd.time_step_sound,
            step_left: ps.step_left,
            fall_velocity: ps.fall_velocity,
            punchangle: cd.punchangle,
            swim_time: cd.swim_time as f32,
            effects: ps.effects,
            flags: cd.flags,
            usehull: ps.usehull,
            gravity: ps.gravity,
            friction: ps.friction,
            oldbuttons: ps.oldbuttons,
            waterjumptime: cd.waterjumptime as f32,
            dead: !pass.spectator && cd.health <= 0.0,
            deadflag: cd.deadflag,
            spectator: i32::from(pass.spectator),
            jump_spectator: pass.jump.take(),
            movetype: ps.movetype,
            onground: cd.flags & FL_ONGROUND,
            waterlevel: cd.waterlevel,
            watertype: cd.watertype,
            maxspeed: input.movevars.maxspeed,
            clientmaxspeed: cd.maxspeed,
            iuser: cd.iuser,
            fuser: cd.fuser,
            vuser: cd.vuser,
            physents: physents.to_vec(),
            moveents: moveents.to_vec(),
            cmd: *cmd,
            movevars: input.movevars.clone(),
            runfuncs,
            movedir: self.movedir,
            log: self.move_trace.take(),
            ..PlayerMove::default()
        };
        let rng = self.rng.clone();
        let mut random = |lo: i32, hi: i32| rng.long(lo, hi);
        let mut ctx = PmContext {
            world,
            textures: &self.textures,
            stuck: &mut self.stuck,
            random: &mut random,
            float_time: input.realtime,
            sounds: Vec::new(),
        };
        pm_move(&mut pm, &mut ctx, false);
        self.movedir = pm.movedir;
        self.move_trace = pm.log.take();
        pass.jump = pm.jump_spectator.take();
        let sounds = std::mem::take(&mut ctx.sounds);
        if runfuncs {
            self.demo_log.extend(sounds.iter().cloned().map(PredRecord::Sound));
            self.sounds.extend(sounds);
        }
        // g_pred_onground: entity number of the ground physent.
        self.pred_onground = if pm.onground > 0 { pm.physents.get(pm.onground as usize).map_or(-1, |pe| pe.info) } else { pm.onground };
        let mut to = from;
        let c = &mut to.client;
        c.flags = pm.flags;
        c.in_duck = pm.in_duck;
        c.time_step_sound = pm.time_step_sound;
        c.duck_time = pm.duck_time as i32;
        c.swim_time = pm.swim_time as i32;
        c.waterjumptime = pm.waterjumptime as i32;
        c.waterlevel = pm.waterlevel;
        c.watertype = pm.watertype;
        c.maxspeed = pm.clientmaxspeed;
        c.deadflag = pm.deadflag;
        c.velocity = pm.velocity;
        c.view_ofs = pm.view_ofs;
        c.punchangle = pm.punchangle;
        c.iuser = pm.iuser;
        c.fuser = pm.fuser;
        c.vuser = pm.vuser;
        let p = &mut to.player;
        p.origin = pm.origin;
        p.basevelocity = pm.basevelocity;
        p.oldbuttons = i32::from(pm.cmd.buttons);
        p.friction = pm.friction;
        p.movetype = pm.movetype;
        p.effects = pm.effects;
        p.usehull = pm.usehull;
        p.step_left = pm.step_left;
        p.fall_velocity = pm.fall_velocity;
        // ClientDLL_PostRunCmd with the command's start time, then the clock advances.
        let mut env = WeaponEnv {
            world,
            physents,
            events: &mut *pass.events,
            model_names: input.model_names,
            event_names: input.event_names,
            cl_time: input.cl_time,
            playernum: input.playernum,
            velocity: self.sim.velocity,
            usehull: self.sim.usehull,
            weapon_anims: &mut self.weapon_anims,
            hide_view: &mut self.hide_view_entity,
            demo_log: &mut self.demo_log,
            trace_hull: None,
        };
        pass.post.post_run_cmd(&mut env, &from, &mut to, cmd, runfuncs, *time, seed);
        self.pmove_usehull = env.trace_hull.unwrap_or(pm.usehull);
        *time += f64::from(cmd.msec) / 1000.0;
        to
    }
}

/// What every command of one prediction pass shares.
struct Pass<'a, 'b> {
    world: &'a World,
    /// World, solid entities and players (`EV_SetSolidPlayers`).
    physents: &'a [Physent],
    moveents: &'a [Physent],
    input: &'a PredictInput<'b>,
    events: &'a mut EventQueue,
    post: &'a mut dyn PostRunCmd,
    /// `cls.spectator`'s camera pmove (`CL_MoveSpectatorCamera`).
    spectator: bool,
    /// The HUD's pending camera jump.
    jump: Option<([f32; 3], [f32; 3])>,
}
