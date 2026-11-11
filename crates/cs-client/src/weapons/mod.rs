//! Client weapon prediction (docs/re/client-sim.md Parts B–D): `HUD_PostRunCmd` →
//! `HUD_WeaponsPostThink`, `CBasePlayerWeapon::ItemPostFrame`, and every CS weapon's
//! deploy / attack / reload / idle code as the client runs it. Weapon objects persist between
//! predictions like the client's static `g_<WEAPON>` instances; only the networked subset is
//! reloaded from each command's starting state.

mod info;
mod melee;
mod pistols;
mod random;
mod rifles;
mod shotguns;

pub use info::id;

use gs_client::LocalState;
use gs_client::postrun::{LocalEvent, PostRunCmd, WeaponEnv, WeaponFields};
use gs_pmove::math::{Vec3, add};
use gs_protocol::structs::UserCmd;

use info::{FLAG_NOAUTORELOAD, Info, NO_SECONDARY_MASK, PISTOL_MASK, Reserve};

pub(crate) const IN_ATTACK: i32 = 0x1;
pub(crate) const IN_ATTACK2: i32 = 0x800;
const IN_RELOAD: i32 = 0x2000;
pub(crate) const FL_ONGROUND: i32 = 0x200;
pub(crate) const FL_DUCKING: i32 = 0x4000;
/// `m_iWeaponState` bit: the tactical shield is raised.
pub(crate) const SHIELD_DRAWN: i32 = 0x20;
/// `deadflag` DEAD_RESPAWNABLE.
const DEAD_RESPAWNABLE: i32 = 5;
/// `FEV_NOTHOST`: every weapon event's flags.
const FEV_NOTHOST: i32 = 1;

/// A `CBasePlayerWeapon`: the networked fields and the client-local ones.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Weapon {
    pub id: i32,
    pub clip: i32,
    pub npa: f32,
    pub nsa: f32,
    pub twi: f32,
    pub in_reload: bool,
    pub in_special_reload: i32,
    pub next_reload: f32,
    pub shots_fired: i32,
    pub weapon_state: i32,
    pub swing: i32,
    pub start_throw: f32,
    pub release_throw: f32,
    /// `m_flLastFire` (absolute time).
    pub last_fire: f32,
    pub primary_ammo_type: i32,
    // Client-local state.
    pub accuracy: f32,
    pub delay_fire: bool,
    pub fire_on_empty: bool,
    pub glock_shoot: f32,
    pub glock_shots: i32,
    pub famas_shoot: f32,
    pub famas_shots: i32,
    pub burst_spread: f32,
    pub pump_time: f32,
    pub started_arming: bool,
    pub bomb_placed_anim: bool,
    pub armed_time: f32,
}

/// `player.ammo_*`: the reserve mirrors the weapons check before reloading.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Ammo {
    pub buckshot: i32,
    pub nine: i32,
    pub nato556: i32,
    pub nato556box: i32,
    pub nato762: i32,
    pub acp45: i32,
    pub ae50: i32,
    pub mag338: i32,
    pub mm57: i32,
    pub sig357: i32,
}

impl Ammo {
    pub fn get(&self, r: Reserve) -> i32 {
        match r {
            Reserve::None => 0,
            Reserve::Buckshot => self.buckshot,
            Reserve::Nine => self.nine,
            Reserve::Nato556 => self.nato556,
            Reserve::Nato556Box => self.nato556box,
            Reserve::Nato762 => self.nato762,
            Reserve::Acp45 => self.acp45,
            Reserve::Ae50 => self.ae50,
            Reserve::Mag338 => self.mag338,
            Reserve::Mm57 => self.mm57,
            Reserve::Sig357 => self.sig357,
        }
    }
}

/// The client's fake `CBasePlayer` and its `pev`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Player {
    pub flags: i32,
    pub velocity: Vec3,
    pub deadflag: i32,
    pub waterlevel: i32,
    pub maxspeed: f32,
    pub punchangle: Vec3,
    /// `pev->fov`.
    pub fov: f32,
    pub weaponanim: i32,
    pub viewmodel: i32,
    pub origin: Vec3,
    pub v_angle: Vec3,
    pub button: i32,
    pub next_attack: f32,
    pub can_shoot: bool,
    pub random_seed: u32,
    pub ammo: Ammo,
    /// `m_rgAmmo`.
    pub rg_ammo: [i32; 32],
    pub active: i32,
    pub last_item: i32,
    /// `m_iFOV` (client-local), `m_iLastZoom`, `m_bResumeZoom`.
    pub i_fov: i32,
    pub last_zoom: i32,
    pub resume_zoom: bool,
    pub shield_drawn: bool,
}

/// Per-frame client state the prediction reads, set before the host frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct WeaponInputs {
    /// `v_angles`: the last rendered view angles.
    pub v_angles: Vec3,
    /// `EV_LocalPlayerViewheight`.
    pub viewheight: Vec3,
    /// HUD health (`CL_IsDead` is health ≤ 0).
    pub health: i32,
    /// `g_iUser1`.
    pub observer_mode: i32,
    /// `g_lastFOV`: the last `SetFOV` byte.
    pub last_fov: i32,
}

/// The globals `HUD_WeaponsPostThink` leaves for the HUD (crosshair, shield).
#[derive(Debug, Clone, Copy, Default)]
pub struct WeaponGlobals {
    /// `g_iWeaponFlags`: the active weapon's `m_iWeaponState`.
    pub weapon_flags: i32,
    pub player_flags: i32,
    pub player_speed: f32,
    pub water_level: i32,
    pub freeze_time_over: bool,
    pub in_bomb_zone: bool,
    pub holding_shield: bool,
    pub holding_knife: bool,
}

/// What a weapon function can reach: the player, the engine, this command's clock.
pub(crate) struct Ctx<'c, 'e> {
    pub p: &'c mut Player,
    pub g: &'c mut WeaponGlobals,
    pub env: &'c mut WeaponEnv<'e>,
    pub time: f32,
    pub runfuncs: bool,
    /// `g_finalstate->playerstate.origin`: where predicted events play.
    pub event_origin: Vec3,
    pub v_angles: Vec3,
    pub viewheight: Vec3,
    /// `g_currentweapon`, `g_currentanim`.
    pub anim: &'c mut (i32, i32),
    /// The engine generator behind `pfnRandomLong/Float` (not shared with the server).
    pub rng: &'c gs_effects::EngineRng,
    /// `RetireWeapon` was called.
    pub retire: bool,
    /// The knife's `flLastFraction`.
    pub last_fraction: &'c mut f32,
}

impl Ctx<'_, '_> {
    /// `HUD_SendWeaponAnim(anim, id, body, force)`.
    pub fn hud_anim(&mut self, anim: i32, id: i32, body: i32, force: bool) {
        if self.runfuncs || force {
            *self.anim = (id, anim);
            self.env.weapon_anim(anim, body);
        }
    }

    /// `SendWeaponAnim(anim, skiplocal = UseDecrement() = 1)`.
    pub fn send_anim(&mut self, id: i32, anim: i32) {
        self.p.weaponanim = anim;
        self.hud_anim(anim, id, 0, false);
    }

    /// `PLAYBACK_EVENT_FULL(FEV_NOTHOST, …)` → `HUD_PlaybackEvent`: only on the first prediction,
    /// from the command's final origin along the view angles.
    pub fn playback(&mut self, event: &str, fparam: [f32; 2], iparam: [i32; 2], bparam: [bool; 2]) {
        if !self.runfuncs {
            return;
        }
        let index = self.env.event_index(&format!("events/{event}.sc"));
        self.env.playback_event(&LocalEvent {
            flags: FEV_NOTHOST,
            index,
            delay: 0.0,
            origin: self.event_origin,
            angles: self.v_angles,
            fparam,
            iparam,
            bparam,
        });
    }

    /// `RANDOM_LONG` (engine RNG).
    pub fn random_long(&mut self, lo: i32, hi: i32) -> i32 {
        self.rng.long(lo, hi)
    }

    /// `RANDOM_FLOAT` (engine RNG).
    pub fn random_float(&mut self, lo: f32, hi: f32) -> f32 {
        self.rng.float(lo, hi)
    }

    pub fn on_ground(&self) -> bool {
        self.p.flags & FL_ONGROUND != 0
    }

    pub fn ducking(&self) -> bool {
        self.p.flags & FL_DUCKING != 0
    }

    pub fn speed2d(&self) -> f32 {
        let v = self.p.velocity;
        f64::from(v[0] * v[0] + v[1] * v[1]).sqrt() as f32
    }

    /// `GetGunPosition`: origin + `EV_LocalPlayerViewheight`.
    pub fn gun_position(&self) -> Vec3 {
        add(self.p.origin, self.viewheight)
    }

    /// `FireBullets3` on the client: only the shared-random spread offset.
    pub fn spread(&self, spread: f32) -> [f32; 2] {
        let s = self.p.random_seed;
        let x = random::shared_float(s, -0.5, 0.5) + random::shared_float(s.wrapping_add(1), -0.5, 0.5);
        let y = random::shared_float(s.wrapping_add(2), -0.5, 0.5) + random::shared_float(s.wrapping_add(3), -0.5, 0.5);
        [x * spread, y * spread]
    }

    /// `UTIL_SharedRandomLong(seed + k, lo, hi)`.
    pub fn shared_long(&self, k: u32, lo: i32, hi: i32) -> i32 {
        random::shared_long(self.p.random_seed.wrapping_add(k), lo, hi)
    }

    /// `UTIL_SharedRandomFloat(seed + k, lo, hi)`.
    pub fn shared_float(&self, k: u32, lo: f32, hi: f32) -> f32 {
        random::shared_float(self.p.random_seed.wrapping_add(k), lo, hi)
    }

    /// `m_rgAmmo[primary]` of the weapon.
    pub fn ammo(&self, w: &Weapon) -> i32 {
        self.p.rg_ammo.get(usize::try_from(w.primary_ammo_type).unwrap_or(0)).copied().unwrap_or(0)
    }

    pub fn ammo_mut(&mut self, w: &Weapon) -> &mut i32 {
        let i = usize::try_from(w.primary_ammo_type).unwrap_or(0).min(31);
        &mut self.p.rg_ammo[i]
    }

    /// `CBasePlayerWeapon::DefaultDeploy`: the view model, its draw animation, 0.75 s before the
    /// first attack and 1.5 s before idling.
    pub fn default_deploy(&mut self, w: &mut Weapon, model: &str, anim: i32) -> bool {
        self.p.viewmodel = self.env.model_index(model);
        self.send_anim(w.id, anim);
        self.p.next_attack = 0.75;
        w.twi = 1.5;
        true
    }

    /// `CBasePlayerWeapon::DefaultReload`: the clip fills when `m_flNextAttack` runs out.
    pub fn default_reload(&mut self, w: &mut Weapon, clip_size: i32, anim: i32, delay: f32) -> bool {
        let ammo = self.ammo(w);
        if ammo <= 0 || (clip_size - w.clip).min(ammo) == 0 {
            return false;
        }
        self.p.next_attack = (f64::from(delay) + 0.0) as f32;
        self.send_anim(w.id, anim);
        w.in_reload = true;
        w.twi = delay + 0.5;
        true
    }

    /// `ShieldSecondaryFire(up, down)`: raise or lower the shield; `false` without one.
    pub fn shield_secondary_fire(&mut self, w: &mut Weapon, up: i32, down: i32) -> bool {
        if !self.g.holding_shield {
            return false;
        }
        if w.weapon_state & SHIELD_DRAWN != 0 {
            w.weapon_state &= !SHIELD_DRAWN;
            self.send_anim(w.id, down);
            self.p.shield_drawn = false;
        } else {
            w.weapon_state |= SHIELD_DRAWN;
            self.send_anim(w.id, up);
            self.p.shield_drawn = true;
        }
        w.nsa = 0.4;
        w.npa = 0.4;
        w.twi = 0.6;
        true
    }

    /// `RetireWeapon`: switch to the next best weapon after this command.
    pub fn retire(&mut self) {
        self.retire = true;
    }
}

/// The view model path of a weapon, or its shield variant.
pub(crate) fn view_model(model: &str, shield: bool) -> String {
    if shield { format!("models/shield/v_shield_{model}.mdl") } else { format!("models/v_{model}.mdl") }
}

/// Client weapon prediction state (`g_pWpns`, the fake `player`, the HUD globals).
#[derive(Debug, Clone)]
pub struct ClientWeapons {
    weapons: [Weapon; 32],
    player: Player,
    pub globals: WeaponGlobals,
    pub inputs: WeaponInputs,
    lasthealth: i32,
    /// `g_currentweapon`, `g_currentanim`.
    anim: (i32, i32),
    /// The engine generator behind `pfnRandomLong` / `pfnRandomFloat`.
    rng: gs_effects::EngineRng,
    last_fraction: f32,
}

impl ClientWeapons {
    /// `HUD_InitClientWeapons`: every weapon spawned with its initial client state.
    pub fn new(rng: gs_effects::EngineRng) -> Self {
        let mut weapons = [Weapon::default(); 32];
        for (i, w) in weapons.iter_mut().enumerate() {
            w.id = i as i32;
            spawn(w);
        }
        Self {
            weapons,
            player: Player::default(),
            globals: WeaponGlobals::default(),
            inputs: WeaponInputs::default(),
            lasthealth: 0,
            anim: (0, 0),
            rng,
            last_fraction: 1.0,
        }
    }
}

/// `Spawn` state of a weapon object.
fn spawn(w: &mut Weapon) {
    use id::*;
    w.weapon_state &= !SHIELD_DRAWN;
    match w.id {
        GLOCK18 | P228 | DEAGLE => w.accuracy = 0.9,
        USP | FIVESEVEN => w.accuracy = 0.92,
        ELITE => w.accuracy = 0.88,
        MAC10 => w.accuracy = 0.15,
        TMP | P90 | AK47 | SG552 | AUG | M249 => w.accuracy = 0.2,
        M4A1 => {
            w.accuracy = 0.2;
            w.delay_fire = true;
        }
        HEGRENADE | FLASHBANG | SMOKEGRENADE => {
            w.start_throw = 0.0;
            w.release_throw = -1.0;
        }
        _ => {}
    }
}

impl ClientWeapons {
    /// Whether the active weapon is the knife or a shield is held (HUD crosshair rules).
    pub fn current_anim(&self) -> (i32, i32) {
        self.anim
    }

    fn info(id: i32) -> Option<Info> {
        info::item(id)
    }

    /// `HUD_WeaponsPostThink(from, to, cmd, time, random_seed)` (Part B §5).
    #[allow(clippy::too_many_arguments)]
    fn weapons_post_think(&mut self, env: &mut WeaponEnv, from: &LocalState, to: &mut LocalState, cmd: &UserCmd, runfuncs: bool, time: f64, seed: u32) {
        let gtime = time as f32;
        let cur = from.client.m_id;
        if !(1..=30).contains(&cur) || cur == 2 {
            return;
        }
        for (w, f) in self.weapons.iter_mut().zip(&from.weapons) {
            if Self::info(w.id).is_none() {
                continue;
            }
            w.in_reload = f.in_reload != 0;
            w.in_special_reload = f.in_special_reload;
            w.clip = f.clip;
            w.npa = f.next_primary_attack;
            w.nsa = f.next_secondary_attack;
            w.next_reload = f.next_reload;
            w.twi = f.time_weapon_idle;
            w.shots_fired = f.in_zoom;
            w.start_throw = f.fuser[1];
            w.release_throw = f.fuser[2];
            w.swing = f.iuser[0];
            w.weapon_state = f.weapon_state;
            w.last_fire = f.aimed_damage;
        }
        let c = &from.client;
        let p = &mut self.player;
        let t = c.vuser[3][0] as i32;
        let cw = &mut self.weapons[cur as usize];
        if (0..=31).contains(&t) {
            cw.primary_ammo_type = t;
            p.rg_ammo[t as usize] = c.vuser[3][1] as i32;
        } else {
            cw.primary_ammo_type = 0;
        }
        let g = &mut self.globals;
        g.weapon_flags = cw.weapon_state;
        p.random_seed = seed;
        let buttons = i32::from(cmd.buttons);
        p.button = buttons;
        p.velocity = c.velocity;
        p.flags = c.flags;
        p.deadflag = c.deadflag;
        p.waterlevel = c.waterlevel;
        p.maxspeed = c.maxspeed;
        p.punchangle = c.punchangle;
        p.fov = c.fov;
        p.weaponanim = c.weaponanim;
        p.viewmodel = c.viewmodel;
        p.next_attack = c.next_attack;
        g.player_flags = c.flags;
        g.water_level = c.waterlevel;
        let v = c.velocity;
        g.player_speed = f64::from((v[0] * v[0] + v[1] * v[1]) + v[2] * v[2]).sqrt() as f32;
        p.origin = c.origin;
        p.v_angle = self.inputs.v_angles;
        let iuser3 = c.iuser[2];
        p.can_shoot = iuser3 & 1 != 0;
        g.freeze_time_over = iuser3 & 2 == 0;
        g.in_bomb_zone = iuser3 & 4 != 0;
        g.holding_shield = iuser3 & 8 != 0;
        g.holding_knife = cur == id::KNIFE;
        p.ammo = Ammo {
            buckshot: c.ammo[0],
            nine: c.ammo[1],
            nato556: c.ammo[2],
            nato556box: c.ammo[3],
            nato762: c.vuser[1][0] as i32,
            acp45: c.vuser[1][1] as i32,
            ae50: c.vuser[1][2] as i32,
            mag338: c.vuser[2][0] as i32,
            mm57: c.vuser[2][1] as i32,
            sig357: c.vuser[2][2] as i32,
        };
        p.active = cur;
        let event_origin = to.player.origin;

        // Think: alive, a view model, not observing.
        let alive = p.deadflag != DEAD_RESPAWNABLE && self.inputs.health > 0 && p.viewmodel != 0 && self.inputs.observer_mode == 0;
        let shield_reload = self.globals.holding_shield && self.weapons[cur as usize].in_reload && buttons & IN_ATTACK2 != 0;
        let mut retire = false;
        if alive && (shield_reload || self.player.next_attack <= 0.0) {
            if shield_reload {
                self.player.next_attack = 0.0;
            }
            retire = self.with_ctx(env, cur, gtime, runfuncs, event_origin, item_post_frame);
        }
        if retire {
            self.retire_weapon(env, from, cur, gtime, runfuncs, event_origin);
        }

        // Death / respawn edge.
        if runfuncs {
            let health = to.client.health;
            let active = self.player.active;
            if health <= 0.0 && self.lasthealth > 0 {
                self.with_ctx(env, active, gtime, runfuncs, event_origin, holster);
            } else if health > 0.0 && self.lasthealth <= 0 {
                self.with_ctx(env, active, gtime, runfuncs, event_origin, |w, c| {
                    deploy(w, c);
                });
            }
            self.lasthealth = health as i32;
        }

        // Weapon switch (`weaponselect`).
        to.client.m_id = from.client.m_id;
        let ws = i32::from(cmd.weaponselect);
        if ws != 0
            && self.player.deadflag != DEAD_RESPAWNABLE
            && from.weapons.get(ws as usize).is_some_and(|f| f.id == ws)
            && Self::info(ws).is_some()
            && ws != cur
        {
            let active = self.player.active;
            if active != 0 {
                self.with_ctx(env, active, gtime, runfuncs, event_origin, holster);
            }
            self.player.last_item = active;
            self.player.active = ws;
            self.with_ctx(env, ws, gtime, runfuncs, event_origin, |w, c| {
                deploy(w, c);
            });
            to.client.m_id = ws;
        }

        // Copy back.
        let p = &self.player;
        let tc = &mut to.client;
        tc.viewmodel = p.viewmodel;
        tc.fov = p.fov;
        tc.weaponanim = p.weaponanim;
        tc.next_attack = p.next_attack;
        tc.maxspeed = p.maxspeed;
        tc.punchangle = p.punchangle;
        let a = p.ammo;
        tc.ammo = [a.buckshot, a.nine, a.nato556, a.nato556box];
        tc.vuser[1] = [a.nato762 as f32, a.acp45 as f32, a.ae50 as f32];
        tc.vuser[2] = [a.mag338 as f32, a.mm57 as f32, a.sig357 as f32];
        let cw = self.weapons[cur as usize];
        if runfuncs && (self.anim.1 != tc.weaponanim || self.anim.0 != cw.id) {
            let anim = tc.weaponanim;
            self.anim = (cw.id, anim);
            env.weapon_anim(anim, 2);
        }
        tc.vuser[3] = if cw.primary_ammo_type > 31 {
            [-1.0, 0.0, tc.vuser[3][2]]
        } else {
            [cw.primary_ammo_type as f32, self.player.rg_ammo[cw.primary_ammo_type.max(0) as usize] as f32, tc.vuser[3][2]]
        };
        let dt = (f64::from(cmd.msec) / 1000.0) as f32;
        for (i, (out, w)) in to.weapons.iter_mut().zip(&self.weapons).enumerate() {
            if Self::info(i as i32).is_none() {
                *out = WeaponFields::default();
                continue;
            }
            out.clip = w.clip;
            out.in_reload = i32::from(w.in_reload);
            out.in_special_reload = w.in_special_reload;
            out.in_zoom = w.shots_fired;
            out.weapon_state = w.weapon_state;
            out.iuser[0] = w.swing;
            out.fuser[1] = w.start_throw;
            out.fuser[2] = w.release_throw;
            out.aimed_damage = w.last_fire;
            out.next_reload = (w.next_reload - dt).max(-0.001);
            out.next_aim_bonus = (out.next_aim_bonus - dt).max(-1.0);
            out.next_primary_attack = (w.npa - dt).max(-1.0);
            out.next_secondary_attack = (w.nsa - dt).max(-0.001);
            out.time_weapon_idle = (w.twi - dt).max(-0.001);
            if out.pump_time != -9999.0 {
                out.pump_time = (out.pump_time - dt).max(-0.001);
            }
        }
        to.client.next_attack = (to.client.next_attack - dt).max(-0.001);
    }

    /// Runs `f` on weapon `id` with this command's context; `true` if it retired the weapon.
    #[allow(clippy::too_many_arguments)]
    fn with_ctx(&mut self, env: &mut WeaponEnv, id: i32, time: f32, runfuncs: bool, event_origin: Vec3, f: impl FnOnce(&mut Weapon, &mut Ctx)) -> bool {
        let Some(w) = usize::try_from(id).ok().and_then(|i| self.weapons.get_mut(i)) else { return false };
        let mut c = Ctx {
            p: &mut self.player,
            g: &mut self.globals,
            env,
            time,
            runfuncs,
            event_origin,
            v_angles: self.inputs.v_angles,
            viewheight: self.inputs.viewheight,
            anim: &mut self.anim,
            rng: &self.rng,
            retire: false,
            last_fraction: &mut self.last_fraction,
        };
        f(w, &mut c);
        c.retire
    }

    /// `RetireWeapon` → `UTIL_GetNextBestWeapon`: the heaviest other owned weapon in HUD slot
    /// order (ties keep the first), if it can be deployed.
    fn retire_weapon(&mut self, env: &mut WeaponEnv, from: &LocalState, current: i32, time: f32, runfuncs: bool, origin: Vec3) {
        let mut owned: Vec<(i32, Info)> = (1..=30).filter_map(|i| Some((i, Self::info(i)?))).filter(|(i, _)| *i != current && from.weapons[*i as usize].id == *i).collect();
        owned.sort_by_key(|(_, info)| (info.slot, info.position));
        let mut best: Option<(i32, i32)> = None;
        for (i, info) in owned {
            if best.is_none_or(|(_, w)| info.weight > w) {
                best = Some((i, info.weight));
            }
        }
        let Some((next, _)) = best else { return };
        let mut ok = false;
        self.with_ctx(env, next, time, runfuncs, origin, |w, c| ok = can_deploy(w, c));
        if !ok {
            return;
        }
        self.with_ctx(env, current, time, runfuncs, origin, holster);
        self.player.active = next;
        self.with_ctx(env, next, time, runfuncs, origin, |w, c| {
            deploy(w, c);
        });
    }
}

impl PostRunCmd for ClientWeapons {
    /// `HUD_PostRunCmd`: the weapons, then the FOV is always the last `SetFOV` value.
    fn post_run_cmd(&mut self, env: &mut WeaponEnv, from: &LocalState, to: &mut LocalState, cmd: &UserCmd, runfuncs: bool, time: f64, random_seed: u32) {
        self.weapons_post_think(env, from, to, cmd, runfuncs, time, random_seed);
        to.client.fov = self.inputs.last_fov as f32;
    }
}

/// `CBasePlayerWeapon::ItemPostFrame` (client order, Part B §6).
fn item_post_frame(w: &mut Weapon, c: &mut Ctx) {
    let Some(info) = info::item(w.id) else { return };
    let mut usable = c.p.button;
    if (0..=30).contains(&w.id) && NO_SECONDARY_MASK >> w.id & 1 != 0 {
        usable &= !IN_ATTACK2;
    }
    if c.g.holding_shield && w.in_reload && c.p.button & IN_ATTACK2 != 0 {
        secondary_attack(w, c);
        c.p.button &= !IN_ATTACK2;
        w.in_reload = false;
        c.p.next_attack = 0.0;
    }
    if w.in_reload && c.p.next_attack <= 0.0 {
        let ammo = c.ammo(w);
        let j = (info.max_clip - w.clip).min(ammo);
        w.clip += j;
        *c.ammo_mut(w) -= j;
        w.in_reload = false;
    }
    if w.glock_shoot != 0.0 {
        pistols::fire_remaining_glock(w, c);
    } else if c.time > w.famas_shoot && w.famas_shoot != 0.0 {
        rifles::fire_remaining_famas(w, c);
    }
    if w.npa <= 0.0 && c.p.resume_zoom {
        c.p.i_fov = c.p.last_zoom;
        if c.p.i_fov == c.p.last_zoom {
            c.env.hide_view_entity();
            c.p.resume_zoom = false;
        }
    }
    if usable & IN_ATTACK2 != 0 && w.nsa <= 0.0 {
        secondary_attack(w, c);
        c.p.button &= !IN_ATTACK2;
    } else if c.p.button & IN_ATTACK != 0 && w.npa <= 0.0 {
        if (w.clip == 0 && info.reserve != Reserve::None) || (info.max_clip == -1 && c.ammo(w) == 0) {
            w.fire_on_empty = true;
        }
        if w.weapon_state & SHIELD_DRAWN != 0 {
            return;
        }
        if c.p.can_shoot {
            primary_attack(w, c);
        }
    } else if c.p.button & IN_RELOAD != 0 && info.max_clip != -1 && !w.in_reload && w.npa < 0.0 && w.famas_shoot == 0.0 && w.glock_shoot == 0.0 {
        if w.weapon_state & SHIELD_DRAWN == 0 {
            reload(w, c);
        }
    } else if usable & (IN_ATTACK | IN_ATTACK2) == 0 {
        if w.delay_fire {
            w.delay_fire = false;
            if w.shots_fired <= 15 {
                w.shots_fired += 4;
            }
        }
        if (0..=30).contains(&w.id) && PISTOL_MASK >> w.id & 1 != 0 {
            w.shots_fired = 0;
        }
        w.fire_on_empty = false;
        if w.clip == 0 && info.flags & FLAG_NOAUTORELOAD == 0 && w.npa < 0.0 && w.famas_shoot == 0.0 && w.glock_shoot == 0.0 {
            if w.weapon_state & SHIELD_DRAWN == 0 {
                reload(w, c);
            }
            return;
        }
        weapon_idle(w, c);
    }
}

/// `Deploy`.
fn deploy(w: &mut Weapon, c: &mut Ctx) -> bool {
    use id::*;
    match w.id {
        GLOCK18 | USP | P228 | DEAGLE | ELITE | FIVESEVEN => pistols::deploy(w, c),
        M3 | XM1014 => shotguns::deploy(w, c),
        KNIFE | HEGRENADE | FLASHBANG | SMOKEGRENADE | C4 => melee::deploy(w, c),
        _ => rifles::deploy(w, c),
    }
}

/// `CanDeploy`.
fn can_deploy(w: &mut Weapon, c: &mut Ctx) -> bool {
    match w.id {
        id::HEGRENADE | id::FLASHBANG | id::SMOKEGRENADE => c.ammo(w) != 0,
        _ => true,
    }
}

/// `Holster(0)`: the knife, grenades and C4 only block attacks for 0.5 s.
fn holster(w: &mut Weapon, c: &mut Ctx) {
    use id::*;
    match w.id {
        KNIFE | HEGRENADE | FLASHBANG | SMOKEGRENADE | C4 => melee::holster(w, c),
        _ => {
            w.in_reload = false;
            c.p.viewmodel = 0;
        }
    }
}

fn primary_attack(w: &mut Weapon, c: &mut Ctx) {
    use id::*;
    match w.id {
        GLOCK18 | USP | P228 | DEAGLE | ELITE | FIVESEVEN => pistols::primary(w, c),
        M3 | XM1014 => shotguns::primary(w, c),
        KNIFE | HEGRENADE | FLASHBANG | SMOKEGRENADE | C4 => melee::primary(w, c),
        _ => rifles::primary(w, c),
    }
}

fn secondary_attack(w: &mut Weapon, c: &mut Ctx) {
    use id::*;
    match w.id {
        GLOCK18 | USP | P228 | DEAGLE | ELITE | FIVESEVEN => pistols::secondary(w, c),
        KNIFE | HEGRENADE | FLASHBANG | SMOKEGRENADE | C4 => melee::secondary(w, c),
        M3 | XM1014 => {}
        _ => rifles::secondary(w, c),
    }
}

fn reload(w: &mut Weapon, c: &mut Ctx) {
    use id::*;
    match w.id {
        GLOCK18 | USP | P228 | DEAGLE | ELITE | FIVESEVEN => pistols::reload(w, c),
        M3 | XM1014 => shotguns::reload(w, c),
        KNIFE | HEGRENADE | FLASHBANG | SMOKEGRENADE | C4 => {}
        _ => rifles::reload(w, c),
    }
}

fn weapon_idle(w: &mut Weapon, c: &mut Ctx) {
    use id::*;
    match w.id {
        GLOCK18 | USP | P228 | DEAGLE | ELITE | FIVESEVEN => pistols::idle(w, c),
        M3 | XM1014 => shotguns::idle(w, c),
        KNIFE | HEGRENADE | FLASHBANG | SMOKEGRENADE | C4 => melee::idle(w, c),
        _ => rifles::idle(w, c),
    }
}

/// `(int)(v · scale)` with C truncation.
pub(crate) fn trunc(v: f32, scale: f32) -> i32 {
    (v * scale) as i32
}
