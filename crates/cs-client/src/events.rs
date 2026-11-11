//! The CS client's event handlers (`ev_cs`, docs/re/client-sim.md Part E §3–§6) and the shell /
//! corpse user messages that share their helpers (§6.7, Part G §5.1).
//!
//! The engine services the handlers use (`cl_enginefuncs`, `pEventAPI`, `pEfxAPI`) come through
//! [`EventApi`].

use gs_effects::{Effects, Env, TentCallback, TentHit, tent_flags};
use gs_pmove::math::{Vec3, add, angle_vectors, scale, sub};
use gs_pmove::{Physent, PmTrace};
use gs_protocol::structs::EventArgs;

use crate::particleman::ParticleMan;
use crate::weather::Environment;

const CHAN_AUTO: i32 = 0;
const CHAN_WEAPON: i32 = 1;
const CHAN_STATIC: i32 = 6;
/// `SND_CHANGE_PITCH`.
const SND_CHANGE_PITCH: i32 = 128;
/// `SOLID_BSP`.
const SOLID_BSP: i32 = 4;
/// `MOVETYPE_PUSHSTEP`.
const MOVETYPE_PUSHSTEP: i32 = 13;
/// `PM_NORMAL`.
const PM_NORMAL: i32 = 0;
/// `g_DeadPlayerModels` size.
const MAX_CORPSES: usize = 64;

/// Engine services for the handlers.
pub trait EventApi {
    fn max_clients(&self) -> i32;
    /// Engine `EV_IsLocal(playernum)`: `playernum` (0-based) is the local player.
    fn is_local(&self, playernum: i32) -> bool;
    /// `EV_LocalPlayerViewheight`.
    fn local_viewheight(&self) -> Vec3;
    /// `cl.time` (`gHUD.m_flTime` follows it).
    fn time(&self) -> f64;
    /// `EV_FindModelIndex`: precache index by name, 0 when absent.
    fn find_model_index(&self, name: &str) -> i32;
    /// `cl_entities[index].curstate.eflags`.
    fn entity_eflags(&self, index: i32) -> i32;
    /// The view model's attachments from its last draw.
    fn view_model_attachments(&self) -> [Vec3; 4];
    /// `GetViewModel()->curstate.effects |= EF_MUZZLEFLASH`.
    fn muzzle_flash(&mut self);
    /// `EV_WeaponAnimation(sequence, body)`.
    fn weapon_animation(&mut self, sequence: i32, body: i32);
    /// `EV_PlaySound`.
    #[allow(clippy::too_many_arguments)]
    fn play_sound(&mut self, ent: i32, origin: Vec3, channel: i32, sample: &str, volume: f32, attenuation: f32, flags: i32, pitch: i32);
    /// `EV_StopSound`.
    fn stop_sound(&mut self, ent: i32, channel: i32, sample: &str);
    /// `EV_SetUpPlayerPrediction(0, 1)` + `EV_PushPMStates` + `EV_SetSolidPlayers(playernum)`.
    fn set_solid_players(&mut self, playernum: i32);
    /// `EV_PopPMStates`.
    fn pop_pm_states(&mut self);
    /// `EV_PlayerTrace` with the point hull.
    fn player_trace(&self, start: Vec3, end: Vec3, flags: i32) -> PmTrace;
    /// `EV_GetPhysent(index)`.
    fn physent(&self, index: i32) -> Option<Physent>;
    /// `EV_TraceTexture(ent, start, end)`.
    fn trace_texture(&self, ent: i32, start: Vec3, end: Vec3) -> Option<String>;
    /// `PM_FindTextureType`.
    fn texture_type(&self, name: &str) -> u8;
    /// `R_DecalShoot(Draw_DecalIndex(Draw_DecalIndexFromName(name)), entity, 0, origin, 0)`.
    fn decal_shoot(&mut self, name: &str, entity: i32, origin: Vec3);
    /// `R_DecalRemoveAll` for every decal texture.
    fn remove_all_decals(&mut self);
    /// The engine effects (`pEfxAPI`) and their view of the client.
    fn efx(&mut self) -> (&mut Effects, &dyn Env);
}

/// `EV_IndexFromTrace`.
fn index_from_trace(api: &dyn EventApi, tr: &PmTrace) -> i32 {
    api.physent(tr.ent).map_or(0, |pe| pe.info)
}

/// Time-stamped spawn queue limiting shells and smoke puffs (`g_ShellQueue`,
/// `g_SmokePuffQueue`). It empties at once when the newest entry expires.
#[derive(Debug, Clone, Default)]
struct SpawnQueue {
    created: Vec<f64>,
}

impl SpawnQueue {
    fn think(&mut self, time: f64, duration: f64) {
        if self.created.last().is_some_and(|&t| time > t + duration) {
            self.created.clear();
        }
    }
}

/// The CS client DLL's event-side state.
#[derive(Debug, Clone)]
pub struct ClientEvents {
    shells: SpawnQueue,
    puffs: SpawnQueue,
    /// `gHUD.m_bRenderGunSmoke` (`gunsmoke` toggles it; off by default).
    pub gun_smoke: bool,
    /// `cl_righthand`.
    pub right_hand: bool,
    /// `g_iShotsFired` (dynamic crosshair).
    pub shots_fired: i32,
    /// `g_bHoldingShield`.
    pub holding_shield: bool,
    /// `g_DeadPlayerModels`: corpse tent ids.
    corpses: [Option<u32>; MAX_CORPSES],
    /// `g_pParticleMan`: smoke-grenade clouds and weather particles.
    pub particleman: ParticleMan,
    /// `g_Environment`: map weather and wind.
    pub environment: Environment,
    /// `g_iUser1`, `g_iUser2`: observer mode and target (in-eye makes the target "local").
    pub observer: (i32, i32),
    /// `g_ShellQueue.maxelements` / `g_SmokePuffQueue.maxelements`: 60 until `CHud::Redraw`'s
    /// check copies `(int)max_shells` / `(int)max_smokepuffs`.
    pub max_shells: i32,
    pub max_smoke_puffs: i32,
    /// `cl_corpsestay` and `fastsprites`, as the handlers read them.
    pub corpse_stay: f32,
    pub fast_sprites: f32,
}

impl Default for ClientEvents {
    fn default() -> Self {
        Self {
            shells: SpawnQueue::default(),
            puffs: SpawnQueue::default(),
            gun_smoke: false,
            right_hand: true,
            shots_fired: 0,
            holding_shield: false,
            corpses: [None; MAX_CORPSES],
            particleman: ParticleMan::default(),
            environment: Environment::default(),
            observer: (0, 0),
            max_shells: 60,
            max_smoke_puffs: 60,
            corpse_stay: 600.0,
            fast_sprites: 0.0,
        }
    }
}

/// Bullet penetration table (`CSWTCH.334/335`): distance and doubled power.
fn penetration(bullet: i32) -> (f32, i32) {
    match bullet {
        1 => (800.0, 42),
        4 => (500.0, 16),
        9 => (500.0, 30),
        10 => (8000.0, 90),
        11 => (5000.0, 78),
        12 => (4000.0, 70),
        13 => (1000.0, 60),
        14 => (2000.0, 60),
        15 => (800.0, 50),
        _ => (0.0, 0),
    }
}

/// Pitch/yaw adjustment of a fire event's angles.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Punch {
    /// `pitch += iparam1/100; yaw += iparam2/100`.
    PitchYaw,
    /// After `AngleVectors`: only the shell yaw sees it.
    PitchYawAfterVectors,
    PitchOnly,
    /// `iparam / 10000000` in integer division (Galil, Famas).
    IntegerDivision,
    None,
}

/// Local shell ejection (`EV_GetDefaultShellInfo` scales, then the attachment and velocity
/// post-processing of §6.4).
#[derive(Debug, Clone, Copy)]
struct LocalShell {
    forward: f32,
    up: f32,
    /// Sign follows `cl_righthand` unless `fixed_right`.
    right: f32,
    fixed_right: bool,
    reverse: bool,
    attachment: usize,
    mul: f32,
    add_z: f32,
}

#[derive(Debug, Clone, Copy)]
struct Shell {
    model: &'static str,
    sound: i32,
    avel: f32,
    local: LocalShell,
}

/// `(speed, scale, grey, type)` gun smoke puffs.
type Puff = (i32, f32, u8, i32);

/// Spread passed to `EV_HLDM_FireBullets`.
#[derive(Debug, Clone, Copy)]
enum Spread {
    /// `(fparam1, fparam2)`: the shooter's random offset.
    Params,
    /// `(fparam1/1000, fparam2/1000)` (Scout).
    Params1000,
    /// A fixed cone with random buckshot.
    Fixed(f32),
}

struct Weapon {
    punch: Punch,
    shell: Option<Shell>,
    shots: i32,
    distance: f32,
    bullet: i32,
    penetration: i32,
    spread: Spread,
    puffs: &'static [Puff],
    /// Puff origin attachment.
    puff_attachment: usize,
}

const fn ls(forward: f32, up: f32, right: f32, reverse: bool, mul: f32, add_z: f32) -> LocalShell {
    LocalShell { forward, up, right, fixed_right: false, reverse, attachment: 1, mul, add_z }
}

const PSHELL: &str = "models/pshell.mdl";
const RSHELL: &str = "models/rshell.mdl";

const fn weapon(punch: Punch, shell: Option<Shell>, bullet: i32, puffs: &'static [Puff]) -> Weapon {
    Weapon { punch, shell, shots: 1, distance: 8192.0, bullet, penetration: 2, spread: Spread::Params, puffs, puff_attachment: 0 }
}

const PISTOL_PUFFS: [Puff; 3] = [(0, 0.25, 10, 3), (25, 0.3, 15, 2), (50, 0.2, 20, 2)];

impl ClientEvents {
    /// `CHud::Think` queue pruning.
    pub fn think(&mut self, time: f64) {
        self.shells.think(time, 2.0);
        self.puffs.think(time, 1.0);
    }

    /// `EV_IsLocal(entindex)`: while spectating in-eye the observed player counts as local.
    fn is_local(&self, api: &dyn EventApi, entindex: i32) -> bool {
        match self.observer {
            (4, target) => target == entindex,
            _ => api.is_local(entindex - 1),
        }
    }

    /// `EV_IsLocal` without in-eye spectating: the local player's own eye.
    fn own_eye(&self, api: &dyn EventApi, entindex: i32) -> bool {
        self.observer.0 != 4 && self.is_local(api, entindex)
    }

    fn is_player(api: &dyn EventApi, entindex: i32) -> bool {
        (1..=api.max_clients()).contains(&entindex)
    }

    /// Dispatches one fired event by its script name (`CL_FireEvents` hook lookup).
    pub fn fire(&mut self, api: &mut dyn EventApi, name: &str, args: &EventArgs) {
        log::trace!(
            "event {name}: ent {} origin {:?} angles {:?} vel {:?} duck {} f {} {} i {} {} b {} {}",
            args.entindex(),
            args.origin(),
            args.angles(),
            args.velocity(),
            args.ducking(),
            args.fparam1(),
            args.fparam2(),
            args.iparam1(),
            args.iparam2(),
            args.bparam1(),
            args.bparam2()
        );
        match hook_name(name).as_str() {
            short @ ("train" | "vehicle") => Self::pitch_adjust(api, args, short == "train"),
            "knife" => {
                if self.is_local(api, args.entindex()) {
                    api.weapon_animation(args.iparam1(), 2);
                }
            }
            "decal_reset" => self.remove_all_decals(api),
            "createsmoke" | "createexplo" => self.create_smoke(api, args),
            "elite_left" => self.fire_elite(api, args, false),
            "elite_right" => self.fire_elite(api, args, true),
            other => {
                if let Some((w, anim, sound)) = fire_spec(other) {
                    self.fire_weapon(api, args, &w, anim, sound);
                }
            }
        }
    }

    /// Whether the client DLL hooked the event script `name` (`HookEvent`'s names compare
    /// case-insensitively with the precache name, `events/<x>.sc`).
    pub fn is_hooked(name: &str) -> bool {
        let short = hook_name(name);
        matches!(short.as_str(), "train" | "vehicle" | "knife" | "decal_reset" | "createsmoke" | "createexplo" | "elite_left" | "elite_right") || fire_spec(&short).is_some()
    }

    /// The common fire skeleton (§6.1).
    fn fire_weapon(&mut self, api: &mut dyn EventApi, args: &EventArgs, w: &Weapon, anim: Anim, sound: Sound) {
        let idx = args.entindex();
        let origin = args.origin();
        let mut angles = args.angles();
        let mut velocity = args.velocity();
        let (p, y) = (args.iparam1() as f32 / 100.0, args.iparam2() as f32 / 100.0);
        match w.punch {
            Punch::PitchYaw => {
                angles[0] += p;
                angles[1] += y;
            }
            Punch::PitchOnly => angles[0] += p,
            Punch::IntegerDivision => {
                angles[0] += (args.iparam1() / 10_000_000) as f32;
                angles[1] += (args.iparam2() / 10_000_000) as f32;
            }
            Punch::PitchYawAfterVectors | Punch::None => {}
        }
        let (forward, right, up) = angle_vectors(angles);
        if w.punch == Punch::PitchYawAfterVectors {
            angles[0] += p;
            angles[1] += y;
        }
        let local = self.is_local(api, idx);
        if local {
            self.shots_fired += 1;
            let silenced_usp = matches!(sound, Sound::Usp) && (args.bparam2() != 0 || self.holding_shield);
            if !silenced_usp {
                api.muzzle_flash();
            }
            let seq = anim.sequence(api, args, self.holding_shield);
            api.weapon_animation(seq, 2);
            if self.gun_smoke {
                let att = api.view_model_attachments();
                let puff_origin = if matches!(sound, Sound::Usp) { sub(att[0], scale(forward, 3.0)) } else { att[w.puff_attachment] };
                self.gun_smoke_puffs(api, w, &sound, args, puff_origin, forward);
            }
        }
        if let Some(shell) = &w.shell {
            let (vel, org) = if local {
                let l = shell.local;
                let r = if l.fixed_right || self.right_hand { l.right } else { -l.right };
                let (v, _) = shell_info(api, args, local, origin, &mut velocity, forward, right, up, l.forward, l.up, r, l.reverse);
                let att = api.view_model_attachments();
                (add(scale(v, l.mul), [0.0, 0.0, l.add_z]), att[l.attachment])
            } else {
                shell_info(api, args, local, origin, &mut velocity, forward, right, up, 20.0, -12.0, 4.0, false)
            };
            let model = api.find_model_index(shell.model);
            self.eject_brass(api, org, vel, angles[1], model, shell.sound, idx, shell.avel);
        }
        let (sample, attn, pitch) = sound.choose(api, args);
        api.play_sound(idx, origin, CHAN_WEAPON, &sample, 1.0, attn, 0, pitch);
        let src = gun_position(api, args, self.own_eye(api, idx), origin);
        let spread = match w.spread {
            Spread::Params => [args.fparam1(), args.fparam2()],
            Spread::Params1000 => [args.fparam1() / 1000.0, args.fparam2() / 1000.0],
            Spread::Fixed(s) => [s, s],
        };
        self.fire_bullets(api, idx, forward, right, up, w.shots, src, forward, spread, w.distance, w.bullet, w.penetration);
    }

    /// Dual Elites: per-hand view animation, sound and offset bullet source (§6.5).
    fn fire_elite(&mut self, api: &mut dyn EventApi, args: &EventArgs, right_hand: bool) {
        let idx = args.entindex();
        let origin = args.origin();
        let angles = args.angles();
        let mut velocity = args.velocity();
        let (forward, right, up) = angle_vectors(angles);
        let local = self.is_local(api, idx);
        if local {
            self.shots_fired += 1;
            api.muzzle_flash();
            let (t, c) = (args.fparam1(), args.iparam2());
            let base = if right_hand { 8 } else { 2 };
            let seq = if c <= 1 {
                Some(base + 5)
            } else if t >= 0.5 {
                Some(base + 4)
            } else if t >= 0.4 {
                Some(base + 3)
            } else if t >= 0.3 {
                Some(base + 2)
            } else if t >= 0.2 {
                Some(base + 1)
            } else if t >= 0.0 {
                Some(base)
            } else {
                None
            };
            if let Some(seq) = seq {
                api.weapon_animation(seq, 2);
            }
            if self.gun_smoke {
                let att = api.view_model_attachments();
                let w = weapon(Punch::None, None, 13, &[(0, 0.25, 10, 3), (25, 0.3, 15, 2), (50, 0.2, 25, 2)]);
                self.gun_smoke_puffs(api, &w, &Sound::Fixed("", 0.8), args, att[usize::from(right_hand)], forward);
            }
        }
        let (vel, org) = if local {
            let r = if right_hand { 16.0 } else { -16.0 };
            let (v, _) = shell_info(api, args, local, origin, &mut velocity, forward, right, up, 35.0, -11.0, r, false);
            let att = api.view_model_attachments();
            (add(scale(v, 0.75), [0.0, 0.0, 25.0]), att[if right_hand { 3 } else { 2 }])
        } else {
            shell_info(api, args, local, origin, &mut velocity, forward, right, up, 20.0, -12.0, 4.0, false)
        };
        let model = api.find_model_index(PSHELL);
        self.eject_brass(api, org, vel, angles[1], model, 1, idx, 5.0);
        let pitch = 94 + api.efx().0.random_long(0, 15);
        api.play_sound(idx, origin, CHAN_WEAPON, "weapons/elite_fire.wav", 1.0, 0.8, 0, pitch);
        let gun = gun_position(api, args, self.own_eye(api, idx), origin);
        let src = if right_hand { add(gun, scale(right, 5.0)) } else { sub(gun, scale(right, 5.0)) };
        let spread = [args.fparam2(), args.iparam1() as f32 / 100.0];
        self.fire_bullets(api, idx, forward, right, up, 1, src, forward, spread, 8192.0, 13, 2);
    }

    /// Local gun smoke puffs (§6.3; only with `gunsmoke` on).
    fn gun_smoke_puffs(&mut self, api: &mut dyn EventApi, w: &Weapon, sound: &Sound, args: &EventArgs, origin: Vec3, forward: Vec3) {
        let base = args.velocity();
        if matches!(sound, Sound::Usp) {
            let b = api.efx().0.random_float(0.1, 0.25);
            for (speed, s, grey, kind) in [(0, b, 7, 3), (20, b + 0.1, 10, 2), (40, b, 13, 2)] {
                self.create_smoke_puff(api, origin, forward, speed, s, [grey; 3], kind, Some(base), false, 35);
            }
            return;
        }
        let silenced_m4 = matches!(sound, Sound::M4a1) && args.bparam1() != 0;
        if matches!(sound, Sound::M4a1) {
            let att = api.view_model_attachments();
            let (o, kind) = if silenced_m4 { (att[0], 3) } else { (att[2], 4) };
            self.create_smoke_puff(api, o, forward, 3, if silenced_m4 { 0.3 } else { 0.2 }, [16; 3], kind, Some(base), false, 35);
            return;
        }
        for &(speed, s, grey, kind) in w.puffs {
            self.create_smoke_puff(api, origin, forward, speed, s, [grey; 3], kind, Some(base), false, 35);
        }
    }

    /// `EV_EjectBrass`: a 2-second bouncing shell, rate-limited for other players.
    #[allow(clippy::too_many_arguments)]
    fn eject_brass(&mut self, api: &mut dyn EventApi, origin: Vec3, velocity: Vec3, rotation: f32, model: i32, sound: i32, idx: i32, avel: f32) {
        let time = api.time();
        if self.is_local(api, idx) {
            if (self.shells.created.len() as i32) < self.max_shells {
                self.shells.created.push(time);
            }
        } else {
            if self.shells.created.len() as i32 >= self.max_shells {
                return;
            }
            self.shells.created.push(time);
        }
        let (fx, env) = api.efx();
        let info = env.model(model);
        if let Some(i) = fx.temp_model(origin, velocity, [0.0, rotation, 0.0], 2.0, info, sound)
            && let Some(t) = fx.tent_mut(i)
        {
            t.avelocity = scale(t.avelocity, avel);
        }
    }

    /// `EV_HLDM_FireBullets` (CS): traces with penetration, impact decals, sparks and puffs.
    #[allow(clippy::too_many_arguments)]
    fn fire_bullets(
        &mut self,
        api: &mut dyn EventApi,
        idx: i32,
        _forward: Vec3,
        right: Vec3,
        up: Vec3,
        shots: i32,
        src: Vec3,
        dir_shooting: Vec3,
        spread: [f32; 2],
        distance: f32,
        bullet: i32,
        penetration_count: i32,
    ) {
        let (pen_distance, mut power) = penetration(bullet);
        let right0 = right;
        let (mut rgb, mut smoke_scale) = ([40u8; 3], 0.4f32);
        let mut src = src;
        let mut distance = distance;
        for _ in 0..shots {
            let dir = if bullet == 4 {
                let (x, y) = loop {
                    let rng = &mut api.efx().0;
                    let x = rng.random_float(-0.5, 0.5) + rng.random_float(-0.5, 0.5);
                    let y = rng.random_float(-0.5, 0.5) + rng.random_float(-0.5, 0.5);
                    if x * x + y * y <= 1.0 {
                        break (x, y);
                    }
                };
                add(add(dir_shooting, scale(right0, x * spread[0])), scale(up, y * spread[1]))
            } else {
                add(add(dir_shooting, scale(right, spread[0])), scale(up, spread[1]))
            };
            let mut end = add(src, scale(dir, distance));
            api.set_solid_players(idx - 1);
            let mut sparks = false;
            let mut pen = penetration_count;
            while pen != 0 {
                let tr = api.player_trace(src, end, PM_NORMAL);
                let dist = distance * tr.fraction;
                if dist == 0.0 {
                    break;
                }
                pen = if dist > pen_distance { 0 } else { pen - 1 };
                // Buckshot never reads the texture type (uninitialised in the original: 0).
                let tex = if bullet != 4 { play_texture_sound(api, idx, &tr, src, end, bullet) } else { 0 };
                match tex {
                    b'C' => {
                        sparks = true;
                        rgb = [65; 3];
                        power = (power as f32 * 0.25) as i32;
                    }
                    b'G' | b'V' => {
                        sparks = true;
                        power = (power as f32 * 0.5) as i32;
                    }
                    b'M' => {
                        sparks = true;
                        power = (power as f32 * 0.15) as i32;
                    }
                    b'P' => {
                        sparks = true;
                        power = (power as f32 * 0.4) as i32;
                    }
                    b'T' => {
                        sparks = true;
                        power = (power as f32 * 0.65) as i32;
                    }
                    b'W' => {
                        sparks = false;
                        smoke_scale = 0.5;
                        rgb = [75, 42, 15];
                    }
                    _ => {}
                }
                self.decal_gunshot(api, &tr, smoke_scale, rgb, sparks, tex);
                if pen == 0 {
                    break;
                }
                distance = (distance - dist) * 0.5;
                src = add(tr.endpos, scale(dir, power as f32));
                end = add(src, scale(dir, distance));
                for i in 1..power.max(1) {
                    let a = add(tr.endpos, scale(dir, i as f32));
                    let b = add(a, dir);
                    let t2 = api.player_trace(a, b, PM_NORMAL);
                    if t2.startsolid && t2.inopen {
                        let back = api.player_trace(b, a, PM_NORMAL);
                        self.decal_gunshot(api, &back, smoke_scale, rgb, sparks, tex);
                        break;
                    }
                }
            }
            api.pop_pm_states();
        }
    }

    /// `EV_HLDM_DecalGunshot`: decal, sparks and a wall puff on brush hits.
    fn decal_gunshot(&mut self, api: &mut dyn EventApi, tr: &PmTrace, smoke_scale: f32, rgb: [u8; 3], sparks: bool, tex: u8) {
        let Some(pe) = api.physent(tr.ent).filter(|pe| pe.solid == SOLID_BSP) else { return };
        let decal = damage_decal(api, &pe);
        gunshot_decal_trace(api, tr, &decal, tex);
        let n = tr.plane_normal;
        if sparks {
            let fx = api.efx().0;
            let d = [n[0] * fx.random_float(4.0, 10.0), n[1] * fx.random_float(4.0, 10.0), n[2] * fx.random_float(4.0, 10.0)];
            let speed = fx.random_float(4.0, 10.0);
            let count = fx.random_long(15, 30);
            fx.streak_splash(tr.endpos, d, 4, count, speed, -75, 75);
        }
        let p = add(tr.endpos, scale(n, 5.0));
        self.create_smoke_puff(api, p, n, 25, smoke_scale, rgb, 2, None, true, 35);
    }

    /// `EV_HLDM_CreateSmoke`.
    #[allow(clippy::too_many_arguments)]
    fn create_smoke_puff(
        &mut self,
        api: &mut dyn EventApi,
        origin: Vec3,
        dir: Vec3,
        speed: i32,
        smoke_scale: f32,
        rgb: [u8; 3],
        kind: i32,
        base_velocity: Option<Vec3>,
        wind: bool,
        framerate: i32,
    ) {
        if kind != 0 && self.puffs.created.len() as i32 >= self.max_smoke_puffs {
            return;
        }
        if (self.puffs.created.len() as i32) < self.max_smoke_puffs {
            self.puffs.created.push(api.time());
        }
        let name = {
            let fx = api.efx().0;
            match kind {
                // The digit is drawn either way; fastsprites > 1 then takes the fast puff.
                0 | 1 => {
                    let name = format!("sprites/black_smoke{}.spr", 1 + fx.random_long(0, 3));
                    if self.fast_sprites > 1.0 { "sprites/fast_wallpuff1.spr".to_owned() } else { name }
                }
                2 => format!("sprites/wall_puff{}.spr", 1 + fx.random_long(0, 3)),
                3 => format!("sprites/pistol_smoke{}.spr", if fx.random_long(0, 1) != 0 { 1 } else { 2 }),
                _ => format!("sprites/rifle_smoke{}.spr", 1 + fx.random_long(0, 2)),
            }
        };
        let mut model = api.find_model_index(&name);
        if model == 0 && kind <= 1 {
            model = 293;
        }
        let (fx, env) = api.efx();
        let Some(i) = fx.default_sprite(env, origin, model, framerate as f32) else { return };
        let extra = fx.random_float(0.0, smoke_scale * 0.5);
        let speed = if speed != 0 { speed + fx.random_long(0, 5) } else { 0 };
        let Some(t) = fx.tent_mut(i) else { return };
        t.flags |= tent_flags::COLLIDEWORLD | tent_flags::CLIENTCUSTOM;
        t.scale = smoke_scale + extra;
        if kind <= 1 {
            // fastsprites: > 1 the fast puff (Normal, renderamt kept, 3 less expansion), 1
            // TransAdd, else TransAlpha.
            if self.fast_sprites > 1.0 {
                t.rendermode = 0;
            } else {
                t.rendermode = if self.fast_sprites == 1.0 { 5 } else { 4 };
                t.renderamt = 200;
            }
            t.callback = TentCallback::SmokeRiseWindExpand;
            t.base_fuser1 = if kind == 1 { 1.0 } else { 8.5 } - if self.fast_sprites > 1.0 { 3.0 } else { 0.0 };
        } else {
            t.renderamt = 150;
            t.rendermode = 5;
            t.renderfx = 5;
            t.callback = if wind { TentCallback::SmokeRiseWind } else { TentCallback::SmokeRise };
        }
        t.hit = TentHit::HugWalls;
        t.rendercolor = rgb;
        if origin[0] != dir[0] && origin[1] != dir[1] && origin[2] != dir[2] {
            t.origin = origin;
            let mut v = scale(dir, speed as f32);
            if let Some(bv) = base_velocity {
                v = add(v, [0.9 * bv[0], 0.9 * bv[1], 0.5 * bv[2]]);
            }
            t.velocity = v;
        }
    }

    /// `EV_TrainPitchAdjust` / `EV_VehiclePitchAdjust`.
    fn pitch_adjust(api: &mut dyn EventApi, args: &EventArgs, train: bool) {
        let p = args.iparam1();
        let s = ((p as u16) >> 12) & 7;
        let n = match s {
            1..=4 => s,
            5 => 6,
            6 => 7,
            _ => return,
        };
        let sample = format!("plats/{}{n}.wav", if train { "ttrain" } else { "vehicle" });
        if args.bparam1() != 0 {
            api.stop_sound(args.entindex(), CHAN_STATIC, &sample);
        } else {
            let volume = (p & 0x3F) as f32 / 40.0;
            let pitch = ((p >> 6) & 0x3F) * 10;
            api.play_sound(args.entindex(), args.origin(), CHAN_STATIC, &sample, volume, 0.8, SND_CHANGE_PITCH, pitch);
        }
    }

    /// `EV_RemoveAllDecals`: decals, particle-manager particles and client corpses.
    fn remove_all_decals(&mut self, api: &mut dyn EventApi) {
        api.remove_all_decals();
        self.particleman.reset();
        let (fx, _) = api.efx();
        for slot in &mut self.corpses {
            if let Some(id) = slot.take()
                && let Some(t) = fx.tent_by_id_mut(id)
            {
                t.die = 0.0;
            }
        }
    }

    /// `EV_CreateSmoke`: a detonation is the particle-manager gas cloud (`CreateSmokePop`,
    /// hardware renderer); other calls are smoke puffs.
    fn create_smoke(&mut self, api: &mut dyn EventApi, args: &EventArgs) {
        if args.iparam2() == 1 {
            let time = api.time() as f32;
            let (fx, env) = api.efx();
            self.particleman.smoke_pop(fx, env, time, args.origin());
            return;
        }
        let mut d = [args.fparam1(), args.fparam2(), args.iparam1() as f32 / 100.0];
        let (grey, speed) = {
            let fx = api.efx().0;
            let grey = fx.random_long(155, 175) as u8;
            (grey, fx.random_float(5.0, 10.0) as i32)
        };
        self.create_smoke_puff(api, args.origin(), d, speed, 1.0, [grey; 3], 1, None, true, 25);
        let (c, s) = (0.965_927_4_f32, 0.258_812_95_f32);
        let (x, y) = (d[0], d[1]);
        d = [(c * x - s * y) * 0.75, (c * y + s * x) * 0.75, d[2] * 0.75];
        let speed = api.efx().0.random_float(2.0, 5.0) as i32;
        self.create_smoke_puff(api, args.origin(), d, speed, 1.2, [grey; 3], 1, None, true, 20);
    }

    /// `CHud::MsgFunc_Brass` (cstrike layout): a shell ejected by the server (M3, AWP, Scout).
    pub fn brass_message(&mut self, api: &mut dyn EventApi, data: &[u8]) {
        let mut r = gs_protocol::MsgReader::new(data);
        r.read_u8();
        let mut origin = r.read_coord3();
        r.read_coord3();
        let mut velocity = r.read_coord3();
        let rotation = r.read_angle();
        let model = i32::from(r.read_i16());
        let sound = i32::from(r.read_u8());
        r.read_u8();
        let entindex = i32::from(r.read_u8());
        if r.bad() {
            return;
        }
        // The original's own 3.1416, not π.
        #[allow(clippy::approx_constant)]
        let rad = 3.1416 * rotation / 180.0;
        let (s, c) = (rad.sin(), rad.cos());
        if self.right_hand || !self.is_local(api, entindex) {
            velocity[0] += -120.0 * s;
            velocity[1] += 120.0 * c;
            origin = add(origin, [9.0 * s, -9.0 * c, 0.0]);
        } else {
            origin = add(origin, [-9.0 * s, 9.0 * c, 0.0]);
        }
        // The rotation in radians is passed where degrees are expected (as in the original).
        self.eject_brass(api, origin, velocity, rad, model, sound, 0, 1.0);
    }

    /// `CHudHealth::MsgFunc_ClCorpse` → `CreateCorpse` (Part G §5.1): the message's model, or
    /// with `cl_minmodels` the team model.
    pub fn corpse_message(&mut self, api: &mut dyn EventApi, data: &[u8], min_models: &crate::models::MinModels) {
        let mut r = gs_protocol::MsgReader::new(data);
        let model = r.read_string();
        let origin = [0; 3].map(|_| r.read_i32() as f32 / 128.0);
        let angles = r.read_coord3();
        let delay = r.read_i32() as f32 / 100.0;
        let sequence = i32::from(r.read_u8());
        let body = i32::from(r.read_u8());
        let team = i32::from(r.read_u8());
        let player = usize::from(r.read_u8());
        if r.bad() {
            return;
        }
        let path = if min_models.value != 0.0 {
            match min_models.team_model(team, min_models.player(player).1) {
                Some(n) => format!("models/player/{n}/{n}.mdl"),
                None => "models/player/player.mdl".to_owned(),
            }
        } else if model.contains("models/") {
            model
        } else {
            format!("models/player/{model}/{model}.mdl")
        };
        let index = api.find_model_index(&path);
        let time = api.time() as f32;
        let (fx, env) = api.efx();
        let Some(i) = fx.temp_model(origin, [0.0; 3], angles, 100.0, env.model(index), 0) else { return };
        let Some(t) = fx.tent_mut(i) else { return };
        t.flags |= tent_flags::COLLIDEWORLD
            | tent_flags::FADEOUT
            | tent_flags::SPRANIMATE
            | tent_flags::PERSIST
            | tent_flags::CLIENTCUSTOM
            | tent_flags::CORPSE;
        t.framerate = 1.0;
        t.animtime = time + delay;
        t.frame_max = 255.0;
        t.renderamt = 255;
        t.base_renderamt = 255;
        t.sequence = sequence;
        t.frame = 0.0;
        t.body = body;
        t.fuser1 = time + 1.0;
        t.fuser2 = time + self.corpse_stay;
        t.hit_sound = 0;
        t.hit = TentHit::CorpseHitBody;
        t.callback = TentCallback::CorpseRemoveBody;
        t.bounce_factor = 0.0;
        t.die = time + self.corpse_stay + 9.0;
        let id = t.id;
        if let Some(slot) = self.corpses.iter_mut().find(|s| s.is_none()) {
            *slot = Some(id);
        }
    }
}

/// `EV_GetGunPosition`; `own_eye` = the local player and not spectating in-eye.
fn gun_position(api: &dyn EventApi, args: &EventArgs, own_eye: bool, origin: Vec3) -> Vec3 {
    let idx = args.entindex();
    let mut view_ofs = [0.0, 0.0, 17.0];
    if ClientEvents::is_player(api, idx) {
        if own_eye {
            view_ofs = api.local_viewheight();
        } else if args.ducking() == 1 {
            view_ofs[2] = 12.0;
        }
    }
    add(origin, view_ofs)
}

/// `EV_GetDefaultShellInfo` (CS): returns (velocity, origin); `velocity` is halved in place.
#[allow(clippy::too_many_arguments)]
fn shell_info(
    api: &mut dyn EventApi,
    args: &EventArgs,
    local: bool,
    origin: Vec3,
    velocity: &mut Vec3,
    forward: Vec3,
    right: Vec3,
    up: Vec3,
    forward_scale: f32,
    up_scale: f32,
    right_scale: f32,
    reverse: bool,
) -> (Vec3, Vec3) {
    let idx = args.entindex();
    let mut view_ofs = [0.0, 0.0, 28.0];
    if ClientEvents::is_player(api, idx) {
        if local {
            view_ofs = api.local_viewheight();
        } else if args.ducking() == 1 {
            view_ofs[2] = 12.0;
        }
    }
    // client.dll@0x10049ca2: `comiss rightScale, 0; dir = -1; ja; dir = +1` — a positive right
    // scale (every right-handed local shell, every remote shell) flips the random side speed.
    let dir = if right_scale > 0.0 { -1.0 } else { 1.0 };
    let fx = api.efx().0;
    let fr = fx.random_float(50.0, 90.0) * dir;
    let fu = fx.random_float(75.0, 175.0);
    let ff = fx.random_float(25.0, 150.0);
    *velocity = scale(*velocity, 0.5);
    let side = if reverse { scale(right, -fr) } else { scale(right, fr) };
    let shell_velocity = add(add(add(*velocity, side), scale(up, fu)), scale(forward, ff));
    let shell_origin = add(
        add(add(add(scale(*velocity, 0.2), origin), view_ofs), scale(up, up_scale)),
        add(scale(forward, forward_scale), scale(right, right_scale)),
    );
    (shell_velocity, shell_origin)
}

/// `EV_HLDM_PlayTextureSound`: the impact material sound; returns the texture type.
fn play_texture_sound(api: &mut dyn EventApi, _idx: i32, tr: &PmTrace, src: Vec3, end: Vec3, bullet: i32) -> u8 {
    let ent = index_from_trace(api, tr);
    let tex_type = if ent == 0 {
        match api.trace_texture(tr.ent, src, end) {
            Some(tex) => {
                let mut name = tex.as_str();
                if name.starts_with(['+', '-']) {
                    name = name.get(2..).unwrap_or("");
                }
                if name.starts_with(['~', '{', ' ', '!']) {
                    name = name.get(1..).unwrap_or("");
                }
                api.texture_type(&gs_common::bytestr::truncate_encoded(name, 12))
            }
            None => 0,
        }
    } else if api.entity_eflags(ent) & 2 != 0 {
        b'F'
    } else {
        0
    };
    let (samples, volume, attn): (&[&str], f32, f32) = match tex_type {
        b'D' => (&["player/pl_dirt1.wav", "player/pl_dirt2.wav", "player/pl_dirt3.wav"], 0.9, 0.8),
        b'G' => (&["player/pl_grate1.wav", "player/pl_grate4.wav"], 0.9, 0.8),
        b'M' => (&["player/pl_metal1.wav", "player/pl_metal2.wav"], 0.9, 0.8),
        b'N' => (&["player/pl_snow1.wav", "player/pl_snow2.wav", "player/pl_snow3.wav", "player/pl_snow4.wav"], 0.7, 0.8),
        b'P' | b'Y' => (&["debris/glass1.wav", "debris/glass2.wav", "debris/glass3.wav"], 0.8, 0.8),
        b'S' => (&["player/pl_slosh1.wav", "player/pl_slosh3.wav", "player/pl_slosh2.wav", "player/pl_slosh4.wav"], 0.9, 0.8),
        b'T' => (&["player/pl_tile1.wav", "player/pl_tile3.wav", "player/pl_tile2.wav", "player/pl_tile4.wav"], 0.8, 0.8),
        b'V' => (&["player/pl_duct1.wav", "player/pl_duct1.wav"], 0.5, 0.8),
        b'W' => (&["debris/wood1.wav", "debris/wood2.wav", "debris/wood3.wav"], 0.9, 0.8),
        b'F' => {
            if bullet == 5 {
                return b'F';
            }
            (&["weapons/bullet_hit1.wav", "weapons/bullet_hit2.wav"], 1.0, 1.0)
        }
        _ => (&["player/pl_step1.wav", "player/pl_step2.wav"], 0.9, 0.8),
    };
    let fx = api.efx().0;
    let pitch = 96 + fx.random_long(0, 15);
    let sample = samples[fx.random_long(0, samples.len() as i32 - 1) as usize];
    api.play_sound(0, tr.endpos, CHAN_STATIC, sample, volume, attn, 0, pitch);
    tex_type
}

/// `EV_HLDM_DamageDecal`.
fn damage_decal(api: &mut dyn EventApi, pe: &Physent) -> String {
    let fx = api.efx().0;
    if pe.classnumber == 1 {
        format!("{{break{}", fx.random_long(0, 2) + 1)
    } else if pe.rendermode == 0 {
        format!("{{shot{}", fx.random_long(0, 4) + 1)
    } else {
        "{bproof1".to_owned()
    }
}

/// `EV_HLDM_GunshotDecalTrace`: impact particles, a ricochet half the time, the decal.
fn gunshot_decal_trace(api: &mut dyn EventApi, tr: &PmTrace, decal: &str, tex: u8) {
    let r = {
        let fx = api.efx().0;
        fx.bullet_impact_particles(tr.endpos);
        fx.random_long(0, 0x7FFF)
    };
    if r <= 16382 {
        let sample = if tex == b'M' || tex == b'V' {
            if r % 2 == 1 { "weapons/ric_metal-1.wav" } else { "weapons/ric_metal-2.wav" }
        } else {
            match r % 6 {
                0 => "weapons/ric1.wav",
                1 => "weapons/ric2.wav",
                2 => "weapons/ric3.wav",
                3 => "weapons/ric4.wav",
                4 => "weapons/ric5.wav",
                _ => "weapons/ric_conc-1.wav",
            }
        };
        api.play_sound(-1, tr.endpos, CHAN_AUTO, sample, 1.0, 0.8, 0, 100);
    }
    if !decal.is_empty()
        && api.physent(tr.ent).is_some_and(|pe| pe.solid == SOLID_BSP || pe.movetype == MOVETYPE_PUSHSTEP)
    {
        let entity = index_from_trace(api, tr);
        api.decal_shoot(decal, entity, tr.endpos);
    }
}

/// View animation choice of a fire event.
#[derive(Debug, Clone, Copy)]
enum Anim {
    /// `base + RandomLong(0, spread)`.
    Random(i32, i32),
    /// `bparam1 ? last : base + RandomLong(0, spread)`.
    LastShot { last: i32, base: i32, spread: i32 },
    Glock18,
    P228,
    Usp,
    M4a1,
}

impl Anim {
    fn sequence(self, api: &mut dyn EventApi, args: &EventArgs, shield: bool) -> i32 {
        let empty = args.bparam1() != 0;
        let mut rl = |lo: i32, hi: i32| api.efx().0.random_long(lo, hi);
        match self {
            Anim::Random(base, spread) => base + rl(0, spread),
            Anim::LastShot { last, base, spread } => {
                if empty {
                    last
                } else {
                    base + rl(0, spread)
                }
            }
            Anim::Glock18 => match (shield, empty) {
                (true, true) => 3,
                (true, false) => 1,
                (false, true) => 6,
                (false, false) => 5,
            },
            Anim::P228 => match (shield, empty) {
                (true, true) => 3,
                (true, false) => 1 + rl(0, 1),
                (false, true) => 4,
                (false, false) => 1 + rl(0, 2),
            },
            Anim::Usp => {
                if shield {
                    if empty { 3 } else { 1 + rl(0, 1) }
                } else if args.bparam2() != 0 {
                    if empty { 4 } else { 1 + rl(0, 2) }
                } else if empty {
                    12
                } else {
                    9 + rl(0, 2)
                }
            }
            Anim::M4a1 => {
                if args.bparam1() != 0 {
                    1 + rl(0, 2)
                } else {
                    8 + rl(0, 2)
                }
            }
        }
    }
}

/// Fire sound choice.
#[derive(Debug, Clone, Copy)]
enum Sound {
    /// One sample with attenuation; pitch `94 + RandomLong(0, 15)`.
    Fixed(&'static str, f32),
    /// `RandomLong(0,1) ? a : b` drawn before the pitch.
    Pick(&'static str, &'static str, f32),
    Usp,
    M4a1,
}

impl Sound {
    fn choose(self, api: &mut dyn EventApi, args: &EventArgs) -> (String, f32, i32) {
        let fx = api.efx().0;
        match self {
            Sound::Fixed(s, attn) => (s.to_owned(), attn, 94 + fx.random_long(0, 15)),
            Sound::Pick(a, b, attn) => {
                let s = if fx.random_long(0, 1) != 0 { a } else { b };
                (s.to_owned(), attn, 94 + fx.random_long(0, 15))
            }
            Sound::Usp => {
                if args.bparam2() != 0 {
                    ("weapons/usp1.wav".to_owned(), 2.0, 94 + fx.random_long(0, 15))
                } else {
                    ("weapons/usp_unsil-1.wav".to_owned(), 0.8, 87 + fx.random_long(0, 18))
                }
            }
            Sound::M4a1 => {
                if args.bparam1() != 0 {
                    ("weapons/m4a1-1.wav".to_owned(), 1.4, 94 + fx.random_long(0, 15))
                } else {
                    let s = if fx.random_long(0, 1) != 0 { "weapons/m4a1_unsil-1.wav" } else { "weapons/m4a1_unsil-2.wav" };
                    (s.to_owned(), 0.52, 94 + fx.random_long(0, 15))
                }
            }
        }
    }
}

const fn shell(model: &'static str, avel: f32, local: LocalShell) -> Option<Shell> {
    Some(Shell { model, sound: 1, avel, local })
}

/// The hooked script's short name: `events/<x>.sc`, lower case.
fn hook_name(name: &str) -> String {
    let short = name.to_ascii_lowercase();
    short.strip_prefix("events/").unwrap_or(&short).strip_suffix(".sc").unwrap_or(&short).to_owned()
}

/// Per-weapon parameters (§6.2–§6.4).
fn fire_spec(name: &str) -> Option<(Weapon, Anim, Sound)> {
    use Punch::*;
    Some(match name {
        "ak47" => (
            weapon(PitchYaw, shell(RSHELL, 9.0, ls(20.0, -8.0, 10.0, false, 1.0, -75.0)), 11, &[(3, 0.35, 20, 4)]),
            Anim::Random(3, 2),
            Sound::Fixed("weapons/ak47-1.wav", 0.4),
        ),
        "aug" => (
            weapon(PitchYaw, shell(RSHELL, 8.0, ls(17.0, -8.0, 14.0, true, 1.25, -122.0)), 12, &[(3, 0.26, 15, 4)]),
            Anim::Random(3, 2),
            Sound::Fixed("weapons/aug-1.wav", 0.48),
        ),
        "awp" => (
            Weapon { penetration: 3, ..weapon(PitchYaw, Option::None, 10, &[(3, 0.5, 20, 3), (40, 0.5, 15, 2), (80, 0.45, 10, 2)]) },
            Anim::Random(1, 2),
            Sound::Fixed("weapons/awp1.wav", 0.28),
        ),
        "deagle" => (
            weapon(PitchYaw, shell(PSHELL, 5.0, ls(35.0, -11.0, 16.0, false, 0.75, 25.0)), 13, &[(0, 0.25, 10, 3), (25, 0.3, 15, 2), (50, 0.2, 25, 2)]),
            Anim::LastShot { last: 3, base: 1, spread: 1 },
            Sound::Fixed("weapons/deagle-1.wav", 0.6),
        ),
        "famas" => (
            weapon(IntegerDivision, shell(RSHELL, 8.0, ls(17.0, -8.0, 14.0, true, 1.25, -122.0)), 12, &[]),
            Anim::Random(3, 2),
            Sound::Pick("weapons/famas-1.wav", "weapons/famas-2.wav", 0.48),
        ),
        "fiveseven" => (
            weapon(PitchYawAfterVectors, shell(PSHELL, 4.0, ls(36.0, -14.0, 14.0, false, 0.75, 35.0)), 14, &PISTOL_PUFFS),
            Anim::LastShot { last: 3, base: 1, spread: 1 },
            Sound::Fixed("weapons/fiveseven-1.wav", 0.8),
        ),
        "g3sg1" => (
            weapon(PitchYaw, shell(RSHELL, 17.0, ls(17.0, -8.0, 10.0, false, 1.5, -50.0)), 11, &[(3, 0.3, 35, 4), (35, 0.35, 30, 2), (70, 0.3, 25, 2)]),
            Anim::Random(1, 1),
            Sound::Fixed("weapons/g3sg1-1.wav", 0.4),
        ),
        "galil" => (
            weapon(IntegerDivision, shell(RSHELL, 9.0, ls(20.0, -8.0, 10.0, false, 1.0, -75.0)), 11, &[]),
            Anim::Random(3, 2),
            Sound::Pick("weapons/galil-1.wav", "weapons/galil-2.wav", 0.4),
        ),
        "glock18" => (
            Weapon { distance: 4096.0, ..weapon(PitchYaw, shell(PSHELL, 4.0, ls(36.0, -14.0, 14.0, false, 0.65, 25.0)), 1, &[(0, 0.2, 10, 3), (40, 0.4, 20, 2)]) },
            Anim::Glock18,
            Sound::Fixed("weapons/glock18-2.wav", 0.8),
        ),
        "m249" => (
            weapon(PitchYaw, shell(RSHELL, 10.0, ls(20.0, -10.0, 13.0, true, 1.2, -75.0)), 12, &[(3, 0.26, 15, 4)]),
            Anim::Random(1, 1),
            Sound::Fixed("weapons/m249-1.wav", 0.52),
        ),
        "m3" => (
            Weapon {
                shots: 9,
                penetration: 1,
                spread: Spread::Fixed(0.0675),
                ..weapon(None, Option::None, 4, &[(3, 0.5, 25, 3), (25, 0.5, 20, 2), (50, 0.45, 15, 2), (75, 0.35, 7, 2)])
            },
            Anim::Random(1, 1),
            Sound::Fixed("weapons/m3-1.wav", 0.48),
        ),
        "m4a1" => (
            weapon(PitchYaw, shell(RSHELL, 10.0, ls(20.0, -8.0, 10.0, false, 1.0, -45.0)), 12, &[]),
            Anim::M4a1,
            Sound::M4a1,
        ),
        "mac10" => (
            weapon(PitchYaw, shell(PSHELL, 7.0, ls(32.0, -9.0, 11.0, false, 0.75, -65.0)), 1, &[(2, 0.25, 8, 3)]),
            Anim::Random(3, 2),
            Sound::Fixed("weapons/mac10-1.wav", 0.72),
        ),
        "mp5n" => (
            weapon(PitchYaw, shell(PSHELL, 13.0, ls(34.0, -10.0, 11.0, false, 1.0, -50.0)), 1, &[(3, 0.25, 10, 3)]),
            Anim::Random(3, 2),
            Sound::Fixed("weapons/mp5-1.wav", 0.64),
        ),
        "p228" => (
            weapon(PitchYaw, shell(PSHELL, 4.0, ls(36.0, -14.0, 14.0, false, 0.75, 35.0)), 15, &PISTOL_PUFFS),
            Anim::P228,
            Sound::Fixed("weapons/p228-1.wav", 0.8),
        ),
        "p90" => (
            weapon(PitchYaw, shell(RSHELL, 10.0, ls(35.0, -16.0, 22.0, true, 1.25, -50.0)), 14, &[(3, 0.16, 7, 4)]),
            Anim::Random(3, 2),
            Sound::Fixed("weapons/p90-1.wav", 0.64),
        ),
        "scout" => (
            Weapon { spread: Spread::Params1000, ..weapon(PitchYaw, Option::None, 11, &[(3, 0.45, 25, 3), (35, 0.45, 20, 2), (70, 0.35, 15, 2)]) },
            Anim::Random(1, 1),
            Sound::Fixed("weapons/scout_fire-1.wav", 1.6),
        ),
        "sg550" => (
            weapon(PitchYawAfterVectors, shell(RSHELL, 17.0, ls(17.0, -8.0, 10.0, false, 1.5, -50.0)), 12, &[(3, 0.3, 35, 4), (35, 0.35, 30, 2), (70, 0.3, 25, 2)]),
            Anim::Random(1, 1),
            Sound::Fixed("weapons/sg550-1.wav", 0.4),
        ),
        "sg552" => (
            weapon(PitchYaw, shell(RSHELL, 15.0, ls(20.0, -8.0, 10.0, false, 1.65, -120.0)), 12, &[(3, 0.2, 18, 4)]),
            Anim::Random(3, 2),
            Sound::Fixed("weapons/sg552-1.wav", 0.4),
        ),
        "tmp" => (
            weapon(PitchYaw, shell(PSHELL, 8.0, ls(32.0, -6.0, 11.0, false, 1.2, -50.0)), 1, &[(3, 0.2, 10, 3)]),
            Anim::Random(3, 2),
            Sound::Fixed("weapons/tmp-1.wav", 1.6),
        ),
        "ump45" => (
            weapon(PitchYaw, shell(PSHELL, 13.0, ls(34.0, -10.0, 11.0, false, 1.0, -50.0)), 9, &[(3, 0.25, 10, 3)]),
            Anim::Random(3, 2),
            Sound::Fixed("weapons/ump45-1.wav", 0.64),
        ),
        "usp" => (
            Weapon { distance: 4096.0, ..weapon(PitchOnly, shell(PSHELL, 5.0, ls(36.0, -14.0, 14.0, false, 0.5, 45.0)), 9, &[]) },
            Anim::Usp,
            Sound::Usp,
        ),
        "xm1014" => (
            Weapon {
                shots: 6,
                penetration: 1,
                spread: Spread::Fixed(0.0725),
                ..weapon(
                    PitchYaw,
                    Some(Shell { model: "models/shotgunshell.mdl", sound: 2, avel: 3.0, local: ls(22.0, -9.0, 11.0, false, 1.25, -50.0) }),
                    4,
                    &[(3, 0.45, 15, 3), (40, 0.35, 9, 2)],
                )
            },
            Anim::Random(1, 1),
            Sound::Fixed("weapons/xm1014-1.wav", 0.52),
        ),
        _ => return Option::None,
    })
}
