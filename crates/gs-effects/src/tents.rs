//! Temp entities (docs/re/render.md §D.5–§D.7, docs/re/client-sim.md Part G §5): the 500-slot
//! pool, the efx spawners and the CS `HUD_TempEntUpdate`.

use crate::particles::trail;
use crate::vec::{self, Vec3, add, dot, ma, normalized, scale, sub};
use crate::{Effects, Env, ModelInfo, ModelKind, PM_STUDIO_BOX, PM_STUDIO_IGNORE, PM_WORLD_ONLY};

/// `MAX_TEMP_ENTITIES`.
pub const MAX_TEMP_ENTITIES: usize = 500;

/// `FTENT_*` flags.
pub mod flags {
    pub const SINEWAVE: u32 = 0x1;
    pub const GRAVITY: u32 = 0x2;
    pub const ROTATE: u32 = 0x4;
    pub const SLOWGRAVITY: u32 = 0x8;
    pub const SMOKETRAIL: u32 = 0x10;
    pub const COLLIDEWORLD: u32 = 0x20;
    pub const FLICKER: u32 = 0x40;
    pub const FADEOUT: u32 = 0x80;
    pub const SPRANIMATE: u32 = 0x100;
    pub const HITSOUND: u32 = 0x200;
    pub const SPIRAL: u32 = 0x400;
    pub const SPRCYCLE: u32 = 0x800;
    pub const COLLIDEALL: u32 = 0x1000;
    pub const PERSIST: u32 = 0x2000;
    pub const COLLIDEKILL: u32 = 0x4000;
    pub const PLYRATTACHMENT: u32 = 0x8000;
    pub const SPRANIMATELOOP: u32 = 0x10000;
    pub const SPARKSHOWER: u32 = 0x20000;
    pub const NOMODEL: u32 = 0x40000;
    pub const CLIENTCUSTOM: u32 = 0x80000;
    /// CS corpses (HLSDK `FTENT_SCALE`): short world trace, no angle damping, survive frame wraps.
    pub const CORPSE: u32 = 0x100000;
    /// CS `HitBody`: collision and gravity off.
    pub const STOPPED: u32 = 0x200000;
}

/// `kRender*` values used here.
pub(crate) mod render {
    pub const NORMAL: i32 = 0;
    pub const TRANS_TEXTURE: i32 = 2;
    pub const GLOW: i32 = 3;
    pub const TRANS_ALPHA: i32 = 4;
    pub const TRANS_ADD: i32 = 5;
    /// `kRenderFxNoDissipation`.
    pub const FX_NO_DISSIPATION: i32 = 14;
    /// `kRenderFxClampMinScale`.
    pub const FX_CLAMP_MIN_SCALE: i32 = 20;
}

/// `hitcallback` implementations (CS client).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum TentHit {
    #[default]
    None,
    /// `HitBody`: freeze a corpse on a floor hit.
    CorpseHitBody,
    /// `HugWalls`: smoke slides along the surface it hits.
    HugWalls,
}

/// `callback` implementations (FTENT_CLIENTCUSTOM, CS client).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum TentCallback {
    #[default]
    None,
    /// `RemoveBody`: sink once `t >= fuser2 + 5 + cl_corpsestay` (the cvar as it is now).
    CorpseRemoveBody,
    /// `SmokeRise`.
    SmokeRise,
    /// `SmokeRise_Wind`.
    SmokeRiseWind,
    /// `SmokeRise_Wind_Expand`.
    SmokeRiseWindExpand,
}

/// The CS client's shared smoke wind (`g_xWindDirection`, `g_yWindDirection`,
/// `g_xWindMagnitude`, `g_yWindMagnitude`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Wind {
    dir: [i32; 2],
    mag: [f32; 2],
}

/// `TEMPENTITY` with its `cl_entity_t` fields flattened.
#[derive(Debug, Clone, PartialEq)]
pub struct Tent {
    /// Serial number, stable while the tent lives (indices shift as tents die).
    pub id: u32,
    pub flags: u32,
    pub die: f32,
    pub frame_max: f32,
    /// Sine-wave base position.
    pub x: f32,
    pub y: f32,
    pub fade_speed: f32,
    pub bounce_factor: f32,
    /// `BOUNCE_*` sound type.
    pub hit_sound: i32,
    pub hit: TentHit,
    pub callback: TentCallback,
    /// 0 = low, 1 = high.
    pub priority: u8,
    pub client_index: i16,
    pub tent_offset: Vec3,
    /// Per-tent constant of the spiral motion (the struct address in the original).
    pub spiral_phase: f32,
    pub model: Option<ModelInfo>,
    pub origin: Vec3,
    pub angles: Vec3,
    /// `prevstate.origin`.
    pub prev_origin: Vec3,
    /// `baseline.origin`.
    pub velocity: Vec3,
    /// `baseline.angles`.
    pub avelocity: Vec3,
    /// `baseline.renderamt`.
    pub base_renderamt: i32,
    pub rendermode: i32,
    pub renderamt: i32,
    pub renderfx: i32,
    pub rendercolor: [u8; 3],
    pub scale: f32,
    pub frame: f32,
    pub framerate: f32,
    pub body: i32,
    pub skin: i32,
    pub sequence: i32,
    pub animtime: f32,
    /// `curstate.effects` (FLICKER phase).
    pub effects: i32,
    /// SPARKSHOWER: `baseline.scale` (next spark time) and `baseline.framerate` (budget).
    pub spark_time: f32,
    pub spark_budget: f32,
    /// SINEWAVE frequency (`prevstate.frame`).
    pub sine_freq: f32,
    pub fuser1: f32,
    pub fuser2: f32,
    /// `baseline.fuser1` (expanding smoke's target scale).
    pub base_fuser1: f32,
    /// Added for drawing this frame.
    visible: bool,
    /// Unlinked; compacted at the end of the update.
    dead: bool,
}

impl Tent {
    /// `CL_PrepareTEnt` on a fresh slot.
    fn prepared(origin: Vec3, model: Option<ModelInfo>, time: f64, priority: u8, spiral_phase: f32) -> Self {
        Self {
            id: 0,
            flags: if model.is_none() { flags::NOMODEL } else { 0 },
            die: (time + 0.75) as f32,
            frame_max: 0.0,
            x: 0.0,
            y: 0.0,
            fade_speed: 0.5,
            bounce_factor: 1.0,
            hit_sound: 0,
            hit: TentHit::None,
            callback: TentCallback::None,
            priority,
            client_index: -1,
            tent_offset: [0.0; 3],
            spiral_phase,
            model,
            origin,
            angles: [0.0; 3],
            prev_origin: [0.0; 3],
            velocity: [0.0; 3],
            avelocity: [0.0; 3],
            base_renderamt: 0,
            rendermode: render::NORMAL,
            renderamt: 0,
            renderfx: 0,
            rendercolor: [0; 3],
            scale: 0.0,
            frame: 0.0,
            framerate: 0.0,
            body: 0,
            skin: 0,
            sequence: 0,
            animtime: 0.0,
            effects: 0,
            spark_time: 0.0,
            spark_budget: 0.0,
            sine_freq: 0.0,
            fuser1: 0.0,
            fuser2: 0.0,
            base_fuser1: 0.0,
            visible: false,
            dead: false,
        }
    }

    fn frames(&self) -> i32 {
        self.model.map_or(1, |m| m.frames.max(1))
    }
}

/// `BOUNCE_*` → (samples, base volume, shell casing).
fn bounce_samples(kind: i32) -> Option<(&'static [&'static str], f32, bool)> {
    Some(match kind {
        0x1 => (&["debris/glass1.wav", "debris/glass2.wav", "debris/glass3.wav"], 0.8, false),
        0x2 => (&["debris/metal1.wav", "debris/metal2.wav", "debris/metal3.wav"], 0.8, false),
        0x4 => (
            &[
                "debris/flesh1.wav",
                "debris/flesh2.wav",
                "debris/flesh3.wav",
                "debris/flesh5.wav",
                "debris/flesh6.wav",
                "debris/flesh7.wav",
            ],
            0.8,
            false,
        ),
        0x8 => (&["debris/wood1.wav", "debris/wood2.wav", "debris/wood3.wav"], 0.8, false),
        0x10 => (&["weapons/ric1.wav", "weapons/ric2.wav", "weapons/ric3.wav", "weapons/ric4.wav", "weapons/ric5.wav"], 0.8, false),
        0x20 => (&["player/pl_shell1.wav", "player/pl_shell2.wav", "player/pl_shell3.wav"], 0.8, true),
        0x40 => (&["debris/concrete1.wav", "debris/concrete2.wav", "debris/concrete3.wav"], 0.8, false),
        0x80 => (&["weapons/sshell1.wav", "weapons/sshell2.wav", "weapons/sshell3.wav"], 0.5, true),
        _ => return None,
    })
}

impl Effects {
    /// Active temp entities.
    pub fn tents(&self) -> impl Iterator<Item = &Tent> {
        self.tents.iter()
    }

    /// Temp entities added for drawing this frame (`CL_TEntAddVisibleEntity` / `AddTEntity`).
    pub fn visible_tents(&self) -> impl Iterator<Item = &Tent> {
        self.tents.iter().filter(|t| t.visible && !t.dead)
    }

    fn spiral_phase(&self) -> f32 {
        // Any per-tent constant: the slot address in the original.
        (self.tents.len() as f32) * 3068.0
    }

    /// `CL_TempEntAlloc(org, model)` (low priority); `None` when the pool is full.
    pub fn tent_alloc(&mut self, org: Vec3, model: Option<ModelInfo>) -> Option<usize> {
        let model = model?;
        self.tent_push(org, Some(model), 0)
    }

    fn tent_push(&mut self, org: Vec3, model: Option<ModelInfo>, priority: u8) -> Option<usize> {
        if self.tents.len() >= MAX_TEMP_ENTITIES {
            log::debug!("Overflow {MAX_TEMP_ENTITIES} temporary ents!");
            return None;
        }
        let mut tent = Tent::prepared(org, model, self.time, priority, self.spiral_phase());
        self.next_tent_id = self.next_tent_id.wrapping_add(1);
        tent.id = self.next_tent_id;
        self.tents.push(tent);
        Some(self.tents.len() - 1)
    }

    /// `CL_TempEntAllocNoModel(org)`.
    pub fn tent_alloc_no_model(&mut self, org: Vec3) -> Option<usize> {
        self.tent_push(org, None, 0)
    }

    /// `CL_TempEntAllocHigh(org, model)`: may recycle the newest low-priority tent.
    pub fn tent_alloc_high(&mut self, org: Vec3, model: Option<ModelInfo>) -> Option<usize> {
        let Some(model) = model else {
            log::debug!("temporary ent model invalid");
            return None;
        };
        if self.tents.len() < MAX_TEMP_ENTITIES {
            return self.tent_push(org, Some(model), 1);
        }
        let Some(i) = self.tents.iter().rposition(|t| t.priority == 0 && !t.dead) else {
            log::debug!("Couldn't alloc a high priority TENT!");
            return None;
        };
        let phase = self.tents[i].spiral_phase;
        self.next_tent_id = self.next_tent_id.wrapping_add(1);
        self.tents[i] = Tent { id: self.next_tent_id, ..Tent::prepared(org, Some(model), self.time, 1, phase) };
        Some(i)
    }

    /// A tent by index (as returned by the allocators).
    pub fn tent_mut(&mut self, index: usize) -> Option<&mut Tent> {
        self.tents.get_mut(index)
    }

    /// A live tent by [`Tent::id`].
    pub fn tent_by_id_mut(&mut self, id: u32) -> Option<&mut Tent> {
        self.tents.iter_mut().find(|t| t.id == id && !t.dead)
    }

    fn sprite_model(env: &dyn Env, index: i32) -> Option<ModelInfo> {
        env.model(index).filter(|m| m.kind == ModelKind::Sprite)
    }

    // --- sprites (§D.6.1) ---

    /// `R_DefaultSprite(pos, spriteIndex, framerate)`.
    pub fn default_sprite(&mut self, env: &dyn Env, pos: Vec3, sprite: i32, framerate: f32) -> Option<usize> {
        if self.time == self.oldtime {
            return None;
        }
        let Some(model) = Self::sprite_model(env, sprite) else {
            log::debug!("No Sprite {sprite}!");
            return None;
        };
        let i = self.tent_alloc(pos, Some(model))?;
        let time = self.time;
        let t = &mut self.tents[i];
        let frames = t.frames() as f32;
        let rate = if framerate == 0.0 { 10.0 } else { framerate };
        t.flags |= flags::SPRANIMATE;
        t.frame_max = frames;
        t.framerate = rate;
        t.frame = 0.0;
        t.die = (time + f64::from(frames / rate)) as f32;
        t.origin = pos;
        Some(i)
    }

    /// `R_Sprite_Explode(t, scale, flags)`.
    pub fn sprite_explode(&mut self, i: usize, scale: f32, explode_flags: i32) {
        let Some(t) = self.tents.get_mut(i) else { return };
        if explode_flags & 1 != 0 {
            t.rendermode = render::NORMAL;
            t.renderamt = 255;
        } else {
            t.rendermode = render::TRANS_ADD;
            t.renderamt = 180;
        }
        t.renderfx = 0;
        t.velocity[2] = 8.0;
        t.rendercolor = [0; 3];
        t.origin[2] += 10.0;
        t.scale = scale;
    }

    /// `R_Sprite_Smoke(t, scale)`.
    pub fn sprite_smoke(&mut self, i: usize, scale: f32) {
        let g = self.rng.long(20, 35) as u8;
        let Some(t) = self.tents.get_mut(i) else { return };
        t.rendermode = render::TRANS_ALPHA;
        t.renderamt = 255;
        t.renderfx = 0;
        t.velocity[2] = 30.0;
        t.rendercolor = [g; 3];
        t.origin[2] += 20.0;
        t.scale = scale;
    }

    /// `R_Sprite_WallPuff(t, scale)`.
    pub fn sprite_wall_puff(&mut self, i: usize, scale: f32) {
        let roll = self.rng.long(0, 359) as f32;
        let die = self.die_at(0.01);
        let Some(t) = self.tents.get_mut(i) else { return };
        t.scale = scale;
        t.rendermode = render::TRANS_ALPHA;
        t.renderamt = 255;
        t.renderfx = 0;
        t.rendercolor = [0; 3];
        t.die = die;
        t.frame = 0.0;
        t.angles[2] = roll;
    }

    /// `R_TempSprite(pos, vel, scale, model, rendermode, renderfx, a, life, flags)`.
    #[allow(clippy::too_many_arguments)]
    pub fn temp_sprite(
        &mut self,
        pos: Vec3,
        vel: Vec3,
        scale: f32,
        model: Option<ModelInfo>,
        rendermode: i32,
        renderfx: i32,
        a: f32,
        life: f32,
        tent_flags: u32,
    ) -> Option<usize> {
        let model = model?;
        let i = self.tent_alloc(pos, Some(model))?;
        let time = self.time;
        let t = &mut self.tents[i];
        let frames = t.frames() as f32;
        t.rendermode = rendermode;
        t.renderfx = renderfx;
        t.framerate = 10.0;
        t.frame_max = frames;
        t.rendercolor = [255; 3];
        t.renderamt = (a * 255.0) as i32;
        t.base_renderamt = t.renderamt;
        t.flags |= tent_flags;
        t.scale = scale;
        t.velocity = vel;
        t.origin = pos;
        t.die = if life == 0.0 { (time + f64::from(frames * 0.1) + 1.0) as f32 } else { (time + f64::from(life)) as f32 };
        t.frame = 0.0;
        Some(i)
    }

    /// `R_RicochetSprite(pos, model, duration, scale)`: a glow that fades out over 1/8 s.
    pub fn ricochet_sprite(&mut self, pos: Vec3, model: Option<ModelInfo>, scale: f32) {
        let Some(i) = self.tent_alloc(pos, model) else { return };
        let roll = 45.0 * self.rng.long(0, 7) as f32;
        let time = self.time as f32;
        let t = &mut self.tents[i];
        t.scale = scale;
        t.rendermode = render::GLOW;
        t.renderfx = render::FX_NO_DISSIPATION;
        t.renderamt = 200;
        t.base_renderamt = 200;
        t.flags = flags::FADEOUT;
        t.velocity = [0.0; 3];
        t.origin = pos;
        t.fade_speed = 8.0;
        t.frame = 0.0;
        t.die = time;
        t.angles[2] = roll;
    }

    /// `R_RocketFlare(pos)`.
    pub fn rocket_flare(&mut self, env: &dyn Env, pos: Vec3) {
        if self.time == self.oldtime {
            return;
        }
        let Some(i) = self.tent_alloc(pos, env.model_by_name("sprites/animglow01.spr")) else { return };
        let frames = self.tents[i].frames();
        let frame = self.rng.long(0, frames - 1) as f32;
        let die = self.die_at(0.01);
        let t = &mut self.tents[i];
        t.rendermode = render::GLOW;
        t.renderamt = 255;
        t.renderfx = render::FX_NO_DISSIPATION;
        t.framerate = 1.0;
        t.frame_max = frames as f32;
        t.origin = pos;
        t.die = die;
        t.frame = frame;
    }

    /// `R_MuzzleFlash(pos, type)`: drawn this frame without a PVS test.
    pub fn muzzle_flash(&mut self, env: &dyn Env, pos: Vec3, kind: i32) {
        let idx = (kind % 10) % 3;
        let mut scale = (kind / 10) as f32 * 0.1;
        if scale == 0.0 {
            scale = 0.5;
        }
        let name = format!("sprites/muzzleflash{}.spr", idx + 1);
        let Some(i) = self.tent_alloc(pos, env.model_by_name(&name)) else { return };
        let frames = self.tents[i].frames();
        let frame = self.rng.long(0, frames - 1) as f32;
        let roll = if idx == 0 { self.rng.long(0, 20) } else { self.rng.long(0, 359) } as f32;
        let die = self.die_at(0.01);
        let t = &mut self.tents[i];
        t.scale = scale;
        t.rendermode = render::TRANS_ADD;
        t.renderamt = 255;
        t.renderfx = 0;
        t.rendercolor = [255; 3];
        t.origin = pos;
        t.die = die;
        t.frame = frame;
        t.frame_max = frames as f32;
        t.angles = [0.0, 0.0, roll];
        t.visible = true;
    }

    /// `R_Sprite_Trail(type, start, end, model, count, life, scale, randomAmp, renderamt, speed)`.
    #[allow(clippy::too_many_arguments)]
    pub fn sprite_trail(
        &mut self,
        env: &dyn Env,
        start: Vec3,
        end: Vec3,
        sprite: i32,
        count: i32,
        life: f32,
        scale: f32,
        amplitude: f32,
        renderamt: i32,
        speed: f32,
    ) {
        let Some(model) = env.model(sprite) else { return };
        let delta = sub(end, start);
        let (dir, _) = normalized(delta);
        for i in 0..count {
            let f = if i == 0 { 0.0 } else { i as f32 / (count as f32 - 1.0) };
            let pos = ma(start, f, delta);
            let Some(k) = self.tent_alloc(pos, Some(model)) else { return };
            let jitter = [0; 3].map(|_| self.rng.long(-127, 128) as f32 * (amplitude / 256.0));
            let extra = self.rng.float(0.0, 4.0);
            let die = self.die_at(life + extra);
            let frame = self.rng.long(0, model.frames.max(1) - 1) as f32;
            let t = &mut self.tents[k];
            t.flags |= flags::SLOWGRAVITY | flags::COLLIDEWORLD | flags::FADEOUT | flags::SPRCYCLE;
            t.velocity = add(vec::scale(dir, speed), jitter);
            t.rendermode = render::GLOW;
            t.renderfx = render::FX_NO_DISSIPATION;
            t.renderamt = renderamt;
            t.base_renderamt = renderamt;
            t.scale = scale;
            t.die = die;
            t.frame = frame;
            t.frame_max = model.frames.max(1) as f32;
            t.rendercolor = [255; 3];
        }
    }

    fn spray_velocity(&mut self, dir: Vec3, noise: f32, speed: f32) -> Vec3 {
        let zn = (noise * 1.5).min(1.0);
        let v = [dir[0] + self.rng.float(-noise, noise), dir[1] + self.rng.float(-noise, noise), dir[2] + self.rng.float(0.0, zn)];
        scale(v, self.rng.float(speed * 0.8, speed * 1.2))
    }

    /// `R_Sprite_Spray(pos, dir, model, count, speed, noise)`.
    #[allow(clippy::too_many_arguments)]
    pub fn sprite_spray(&mut self, env: &dyn Env, pos: Vec3, dir: Vec3, sprite: i32, count: i32, speed: f32, noise: i32) {
        let Some(model) = env.model(sprite) else { return };
        let n = noise as f32 / 100.0;
        let last = model.frames.max(1) - 1;
        for _ in 0..count {
            let Some(k) = self.tent_alloc(pos, Some(model)) else { return };
            let vel = self.spray_velocity(dir, n, speed);
            let die = self.die_at(0.35);
            let frame = self.rng.long(0, last) as f32;
            let t = &mut self.tents[k];
            t.flags |= flags::SLOWGRAVITY | flags::FADEOUT;
            t.rendermode = render::TRANS_ALPHA;
            t.renderamt = 255;
            t.base_renderamt = 255;
            t.renderfx = render::FX_NO_DISSIPATION;
            t.framerate = 0.5;
            t.fade_speed = 2.0;
            t.velocity = vel;
            t.origin = pos;
            t.die = die;
            t.frame = frame;
        }
    }

    /// `R_Spray(pos, dir, model, count, speed, noise, rendermode)`.
    #[allow(clippy::too_many_arguments)]
    pub fn spray(&mut self, env: &dyn Env, pos: Vec3, dir: Vec3, model: i32, count: i32, speed: i32, noise: i32, rendermode: i32) {
        let Some(model) = env.model(model) else { return };
        let n = noise as f32 / 100.0;
        let last = model.frames.max(1) - 1;
        for _ in 0..count {
            let Some(k) = self.tent_alloc(pos, Some(model)) else { return };
            let vel = self.spray_velocity(dir, n, speed as f32);
            let time = self.time;
            let t = &mut self.tents[k];
            t.velocity = vel;
            t.rendermode = rendermode;
            t.renderamt = 255;
            t.base_renderamt = 255;
            t.renderfx = render::FX_NO_DISSIPATION;
            t.framerate = 1.0;
            t.flags |= flags::COLLIDEWORLD | flags::SLOWGRAVITY;
            if last > 1 {
                t.flags |= flags::SPRANIMATE;
                t.framerate = 10.0;
                t.die = (time + f64::from(last as f32 * 0.1)) as f32;
            } else {
                t.die = (time + 0.35) as f32;
            }
            t.origin = pos;
            t.frame = 0.0;
            t.frame_max = last as f32;
        }
    }

    /// `R_FunnelSprite(pos, model, reverse)`.
    pub fn funnel_sprite(&mut self, env: &dyn Env, pos: Vec3, sprite: i32, reverse: bool) {
        let Some(model) = env.model(sprite) else { return };
        for x in (-256..=256).step_by(64) {
            for y in (-256..=256).step_by(64) {
                let Some(k) = self.tent_alloc(pos, Some(model)) else { return };
                let high = [pos[0] + x as f32, pos[1] + y as f32, pos[2] + self.rng.float(100.0, 800.0)];
                let (origin, dest) = if reverse { (pos, high) } else { (high, pos) };
                let framerate = self.rng.float(0.1, 0.4);
                let (dir, dist) = normalized(sub(dest, origin));
                let v = self.rng.float(64.0, 128.0) + (high[2] * 0.125).max(64.0);
                let die = self.die_at(dist / v - 0.5);
                let t = &mut self.tents[k];
                t.origin = origin;
                t.rendermode = render::GLOW;
                t.renderfx = render::FX_NO_DISSIPATION;
                t.renderamt = 200;
                t.base_renderamt = 200;
                t.frame_max = model.frames.max(1) as f32;
                t.framerate = framerate;
                t.flags = flags::ROTATE | flags::FADEOUT;
                t.fade_speed = 2.0;
                t.velocity = scale(dir, v);
                t.die = die;
            }
        }
    }

    /// `R_FireField(pos, radius, model, count, flags, life)`.
    #[allow(clippy::too_many_arguments)]
    pub fn fire_field(&mut self, env: &dyn Env, pos: Vec3, radius: i32, model: i32, count: i32, field_flags: i32, life: f32) {
        let Some(model) = env.model(model) else { return };
        let r = radius as f32;
        let last = model.frames.max(1) - 1;
        for _ in 0..count {
            let z = if field_flags & 16 != 0 { 0.0 } else { self.rng.float(-r, r) };
            let origin = [pos[0] + self.rng.float(-r, r), pos[1] + self.rng.float(-r, r), pos[2] + z];
            let Some(k) = self.tent_alloc(origin, Some(model)) else { return };
            let framerate = self.rng.float(0.75, 1.25);
            let l = life + self.rng.float(-0.25, 0.5);
            let float_up = field_flags & 1 != 0 || (field_flags & 2 != 0 && self.rng.long(0, 1) == 0);
            let vz = if float_up { self.rng.float(10.0, 30.0) } else { 0.0 };
            let die = self.die_at(l);
            let t = &mut self.tents[k];
            if field_flags & 8 != 0 {
                t.rendermode = render::TRANS_ALPHA;
                t.renderamt = 128;
                t.base_renderamt = 128;
                t.renderfx = render::FX_NO_DISSIPATION;
            } else if field_flags & 32 != 0 {
                t.rendermode = render::TRANS_ADD;
                t.renderamt = 180;
                t.renderfx = 0;
            } else {
                t.rendermode = render::NORMAL;
                t.renderamt = 255;
                t.base_renderamt = 255;
                t.renderfx = render::FX_NO_DISSIPATION;
            }
            t.framerate = framerate;
            t.die = die;
            if last > 1 {
                t.flags |= flags::SPRANIMATE;
                if field_flags & 4 != 0 {
                    t.framerate = 15.0;
                    t.flags |= flags::SPRANIMATELOOP;
                } else {
                    t.framerate = last as f32 / l;
                }
            }
            t.frame = 0.0;
            t.velocity = [0.0, 0.0, vz];
            t.frame_max = last as f32;
        }
    }

    /// `R_PlayerSprites(player, model, count, variance)`.
    pub fn player_sprites(&mut self, env: &dyn Env, player: i32, model: i32, count: i32, variance: i32) {
        if player < 1 || player > env.max_clients() {
            log::debug!("Bad ent in R_PlayerSprites!");
            return;
        }
        let (Some(ent), Some(model)) = (env.entity(player), env.model(model)) else { return };
        let last = model.frames.max(1) - 1;
        let v = variance as f32 / 100.0;
        for i in 0..count {
            let offset = [self.rng.float(-10.0, 10.0), self.rng.float(-10.0, 10.0), self.rng.float(-20.0, 36.0)];
            let origin = add(ent.origin, offset);
            let Some(k) = self.tent_alloc(origin, Some(model)) else { return };
            let first_vel = if i == 0 {
                let (n, _) = normalized(offset);
                Some([n[0] * 60.0, n[1] * 60.0, self.rng.float(20.0, 60.0)])
            } else {
                None
            };
            let framerate = self.rng.float(1.0 - v, 1.0);
            let time = self.time;
            let t = &mut self.tents[k];
            t.tent_offset = offset;
            match first_vel {
                Some(vel) => t.velocity = vel,
                None => {
                    t.flags |= flags::PLYRATTACHMENT;
                    t.client_index = player as i16;
                }
            }
            t.rendermode = render::NORMAL;
            t.renderamt = 255;
            t.base_renderamt = 255;
            t.renderfx = render::FX_NO_DISSIPATION;
            t.framerate = framerate;
            if last > 1 {
                t.flags |= flags::SPRANIMATE;
                t.framerate = 20.0;
                t.die = (time + f64::from(last as f32 * 0.05)) as f32;
            } else {
                t.die = (time + 0.35) as f32;
            }
            t.frame = 0.0;
            t.frame_max = last as f32;
        }
    }

    /// `R_AttachTentToPlayer(player, model, zoffset, life)` (the CS radio icon).
    pub fn attach_tent_to_player(&mut self, env: &dyn Env, player: i32, model: i32, zoffset: f32, life: f32) {
        if player < 0 || player > env.max_clients() {
            log::debug!("Bad client in R_AttachTentToPlayer()!");
            return;
        }
        let Some(ent) = env.entity(player).filter(|e| e.in_current_packet) else { return };
        let Some(model) = env.model(model) else { return };
        let Some(k) = self.tent_alloc_high(add(ent.origin, [0.0, 0.0, zoffset]), Some(model)) else { return };
        let die = self.die_at(life);
        let t = &mut self.tents[k];
        t.framerate = 1.0;
        t.tent_offset = [0.0, 0.0, zoffset];
        t.rendermode = render::NORMAL;
        t.renderamt = 255;
        t.base_renderamt = 255;
        t.renderfx = render::FX_NO_DISSIPATION;
        t.client_index = player as i16;
        t.flags |= flags::PLYRATTACHMENT | flags::PERSIST;
        t.die = die;
        if model.kind == ModelKind::Sprite {
            t.framerate = 10.0;
            t.flags |= flags::SPRANIMATE | flags::SPRANIMATELOOP;
            t.frame_max = model.frames.max(1) as f32;
        } else {
            t.frame_max = 0.0;
        }
        t.frame = 0.0;
    }

    /// `R_KillAttachedTents(player)`.
    pub fn kill_attached_tents(&mut self, env: &dyn Env, player: i32) {
        if player < 0 || player > env.max_clients() {
            log::debug!("Bad client in R_KillAttachedTents()!");
            return;
        }
        let time = self.time as f32;
        for t in self.tents.iter_mut().filter(|t| t.flags & flags::PLYRATTACHMENT != 0 && i32::from(t.client_index) == player) {
            t.die = time;
        }
    }

    /// `R_BloodSprite(pos, colorIndex, sprayModel, dropModel, size)`.
    pub fn blood_sprite(&mut self, env: &dyn Env, pos: Vec3, color: i32, spray: i32, drop: i32, size: f32) {
        let k = color + self.rng.long(1, 3);
        let drip_color = self.palette.color(k - 1);
        let splash_color = self.palette.color(k);
        if let Some(model) = env.model(drop).filter(|_| drop != 0) {
            let frames = model.frames.max(1);
            let count = ((self.rng.long(1, 8) + self.rng.long(1, 8)) as f32 + size) as i32;
            for _ in 0..count {
                let Some(i) = self.tent_alloc(pos, Some(model)) else { break };
                let scale = self.rng.float(size / 15.0, size / 25.0);
                let vel = [self.rng.float(-96.0, 95.0), self.rng.float(-96.0, 95.0), self.rng.float(-32.0, 95.0)];
                let avel = [0; 3].map(|_| self.rng.float(-256.0, -255.0));
                let life = self.rng.float(1.0, 2.0);
                let die = self.die_at(life);
                let mut frame = self.rng.long(1, frames - 1);
                if frame > 8 {
                    frame = frames - 1;
                }
                let roll = self.rng.long(0, 360) as f32;
                let t = &mut self.tents[i];
                t.rendermode = render::NORMAL;
                t.renderfx = render::FX_CLAMP_MIN_SCALE;
                t.renderamt = 250;
                t.base_renderamt = 250;
                t.scale = scale;
                t.rendercolor = drip_color;
                t.flags = flags::ROTATE | flags::SLOWGRAVITY | flags::COLLIDEWORLD;
                t.velocity = vel;
                t.avelocity = avel;
                t.framerate = 0.0;
                t.die = die;
                t.frame = frame as f32;
                t.bounce_factor = 0.0;
                t.angles[2] = roll;
            }
        }
        if let Some(model) = env.model(spray).filter(|_| spray != 0) {
            let Some(i) = self.tent_alloc_high(pos, Some(model)) else { return };
            let scale = self.rng.float(size / 25.0, size / 35.0);
            let roll = self.rng.long(0, 360) as f32;
            let die = self.die_at(0.25);
            let frames = model.frames.max(1) as f32;
            let t = &mut self.tents[i];
            t.rendermode = render::NORMAL;
            t.renderfx = render::FX_CLAMP_MIN_SCALE;
            t.renderamt = 250;
            t.base_renderamt = 250;
            t.scale = scale;
            t.rendercolor = splash_color;
            t.flags = flags::SPRANIMATE;
            t.velocity = [0.0; 3];
            t.framerate = 4.0 * frames;
            t.frame = 0.0;
            t.frame_max = frames;
            t.die = die;
            t.bounce_factor = 0.0;
            t.angles[2] = roll;
        }
    }

    /// `R_SparkEffect(pos, count, vmin, vmax)`: spark streaks plus a ricochet glow.
    pub fn spark_effect(&mut self, env: &dyn Env, pos: Vec3, count: i32, vmin: i32, vmax: i32) {
        self.spark_streaks(pos, count, vmin, vmax);
        let scale = self.rng.float(0.5, 1.0);
        self.ricochet_sprite(pos, env.model_by_name("sprites/richo1.spr"), scale);
    }

    /// `R_SparkShower(pos)`.
    pub fn spark_shower(&mut self, pos: Vec3) {
        let Some(i) = self.tent_alloc_no_model(pos) else { return };
        let vel = [self.rng.float(-300.0, 300.0), self.rng.float(-300.0, 300.0), self.rng.float(-200.0, 200.0)];
        let budget = self.rng.float(0.5, 1.5);
        let (die, now) = (self.die_at(0.5), self.time as f32);
        let t = &mut self.tents[i];
        t.velocity = vel;
        t.flags |= flags::SPARKSHOWER | flags::COLLIDEWORLD | flags::SLOWGRAVITY;
        t.avelocity = [0.0; 3];
        t.die = die;
        t.spark_budget = budget;
        t.spark_time = now;
    }

    // --- models (§D.6.2) ---

    /// `R_TempModel(pos, vel, angles, life, model, soundType)`.
    pub fn temp_model(&mut self, pos: Vec3, vel: Vec3, angles: Vec3, life: f32, model: Option<ModelInfo>, sound: i32) -> Option<usize> {
        let i = self.tent_alloc(pos, model)?;
        let frames = self.tents[i].frames();
        let avel = if sound == 1 || sound == 2 {
            Some([self.rng.float(-512.0, 511.0), self.rng.float(-256.0, 255.0), self.rng.float(-256.0, 255.0)])
        } else {
            None
        };
        let pick = self.rng.long(0, frames - 1);
        let die = self.die_at(life);
        let t = &mut self.tents[i];
        t.angles = angles;
        t.flags |= flags::GRAVITY | flags::COLLIDEWORLD;
        t.frame_max = frames as f32;
        match sound {
            1 => t.hit_sound = 0x20,
            2 => {
                t.hit_sound = 0x80;
                t.flags |= flags::SLOWGRAVITY;
            }
            _ => t.hit_sound = 0,
        }
        if let Some(avel) = avel {
            t.avelocity = avel;
            t.flags |= flags::ROTATE;
        }
        t.velocity = vel;
        t.die = die;
        if t.model.is_some_and(|m| m.kind == ModelKind::Sprite) {
            t.frame = pick as f32;
        } else {
            t.body = pick;
        }
        Some(i)
    }

    /// `R_TempSphereModel(pos, speed, life, count, model)`.
    pub fn temp_sphere_model(&mut self, env: &dyn Env, pos: Vec3, speed: f32, life: f32, count: i32, model: i32) {
        let Some(model) = env.model(model) else { return };
        for i in 0..count {
            let Some(k) = self.tent_alloc(pos, Some(model)) else { return };
            let body = self.rng.long(0, model.frames.max(1) - 1);
            let mut f = if self.rng.long(0, 255) < 10 { flags::SLOWGRAVITY } else { flags::GRAVITY };
            let mut avel = [0.0; 3];
            if self.rng.long(0, 255) < 220 {
                f |= flags::ROTATE;
                avel = [0; 3].map(|_| self.rng.float(-256.0, -255.0));
            }
            if self.rng.long(0, 255) < 100 {
                f |= flags::SMOKETRAIL;
            }
            let (dir, _) = normalized([0; 3].map(|_| self.rng.float(-1.0, 1.0)));
            let die = self.die_at(life);
            let t = &mut self.tents[k];
            t.body = body;
            t.flags |= f | flags::COLLIDEWORLD | flags::FLICKER;
            t.avelocity = avel;
            t.rendermode = render::NORMAL;
            t.effects = i & 31;
            t.velocity = scale(dir, speed);
            t.die = die;
        }
    }

    /// `R_BreakModel(pos, size, dir, random, life, count, model, flags)`.
    #[allow(clippy::too_many_arguments)]
    pub fn break_model(&mut self, env: &dyn Env, pos: Vec3, size: Vec3, dir: Vec3, random: f32, life: f32, count: i32, model: i32, break_flags: i32) {
        let Some(model) = env.model(model) else { return };
        let [sx, sy, sz] = size;
        let mut count = if count == 0 { ((sx * sy + sy * sz + sx * sz) / 432.0) as i32 } else { count };
        count = count.min(100);
        for _ in 0..count {
            let origin = [
                pos[0] + self.rng.float(-0.5, 0.5) * sx,
                pos[1] + self.rng.float(-0.5, 0.5) * sy,
                pos[2] + self.rng.float(-0.5, 0.5) * sz,
            ];
            let Some(k) = self.tent_alloc(origin, Some(model)) else { return };
            let hit_sound = break_flags & 0x4F;
            let pick = self.rng.long(0, model.frames.max(1) - 1);
            let mut f = flags::SLOWGRAVITY | flags::COLLIDEWORLD | flags::FADEOUT;
            let mut avel = [0.0; 3];
            if self.rng.long(0, 255) < 200 {
                f |= flags::ROTATE;
                avel = [0; 3].map(|_| self.rng.float(-256.0, 255.0));
            }
            if self.rng.long(0, 255) < 100 && break_flags & 0x10 != 0 {
                f |= flags::SMOKETRAIL;
            }
            let vel = [dir[0] + self.rng.float(-random, random), dir[1] + self.rng.float(-random, random), dir[2] + self.rng.float(0.0, random)];
            let extra = self.rng.float(0.0, 1.0);
            let die = self.die_at(life + extra);
            let t = &mut self.tents[k];
            t.hit_sound = hit_sound;
            if model.kind == ModelKind::Sprite {
                t.frame = pick as f32;
            } else {
                t.body = pick;
            }
            t.flags |= f;
            t.avelocity = avel;
            if hit_sound == 1 || break_flags & 0x20 != 0 {
                t.rendermode = render::TRANS_TEXTURE;
                t.renderamt = 128;
                t.base_renderamt = 128;
            } else {
                t.rendermode = render::NORMAL;
                t.base_renderamt = 255;
            }
            t.velocity = vel;
            t.die = die;
        }
    }

    /// `R_Projectile(pos, vel, model, life, owner)`.
    pub fn projectile(&mut self, env: &dyn Env, pos: Vec3, vel: Vec3, model: i32, life: i32, owner: i32) {
        if owner < 0 || owner > env.max_clients() {
            log::debug!("Bad ent in R_Projectile!");
            return;
        }
        let Some(model) = env.model(model) else { return };
        let Some(k) = self.tent_alloc_high(pos, Some(model)) else { return };
        let die = self.die_at(life as f32);
        let t = &mut self.tents[k];
        t.flags |= flags::COLLIDEALL | flags::PERSIST | flags::COLLIDEKILL;
        t.velocity = vel;
        t.client_index = owner as i16;
        if model.kind == ModelKind::Sprite {
            t.flags |= flags::SPRANIMATE;
            t.frame_max = model.frames.max(1) as f32;
            if model.frames >= 10 {
                // Integer division; a zero life divides by zero in the original.
                t.framerate = if life != 0 { (model.frames / life) as f32 } else { 0.0 };
            } else {
                t.flags |= flags::SPRANIMATELOOP;
                t.framerate = 10.0;
            }
        } else {
            t.frame_max = 0.0;
            let (dir, _) = normalized(vel);
            t.angles = vec::vector_angles(dir);
        }
        t.rendermode = render::NORMAL;
        t.renderfx = 0;
        t.renderamt = 255;
        t.base_renderamt = 255;
        t.frame = 0.0;
        t.body = 0;
        t.scale = 1.0;
        t.rendercolor = [255; 3];
        t.die = die;
    }

    fn bubble(&mut self, model: ModelInfo, origin: Vec3, floor: f32, height: f32, speed: f32) -> bool {
        let Some(k) = self.tent_alloc(origin, Some(model)) else { return false };
        let a = self.rng.long(-3, 3) as f32;
        let vz = self.rng.long(80, 140) as f32;
        let frame = self.rng.long(0, model.frames.max(1) - 1) as f32;
        let scale = 1.0 / self.rng.float(2.0, 5.0);
        let time = self.time;
        let t = &mut self.tents[k];
        t.flags |= flags::SINEWAVE;
        t.x = origin[0];
        t.y = origin[1];
        t.velocity = [a.cos() * speed, a.sin() * speed, vz];
        t.die = (time - 0.1 + f64::from((height - (origin[2] - floor)) / vz)) as f32;
        t.frame = frame;
        t.rendermode = render::TRANS_ALPHA;
        t.renderamt = 255;
        t.scale = scale;
        true
    }

    /// `R_Bubbles(mins, maxs, height, model, count, speed)`.
    #[allow(clippy::too_many_arguments)]
    pub fn bubbles(&mut self, env: &dyn Env, mins: Vec3, maxs: Vec3, height: f32, model: i32, count: i32, speed: f32) {
        let Some(model) = env.model(model) else { return };
        for _ in 0..count {
            let origin = [0, 1, 2].map(|i| self.rng.long(mins[i] as i32, maxs[i] as i32) as f32);
            if !self.bubble(model, origin, mins[2], height, speed) {
                return;
            }
        }
    }

    /// `R_BubbleTrail(start, end, height, model, count, speed)`.
    #[allow(clippy::too_many_arguments)]
    pub fn bubble_trail(&mut self, env: &dyn Env, start: Vec3, end: Vec3, height: f32, model: i32, count: i32, speed: f32) {
        let Some(model) = env.model(model) else { return };
        for _ in 0..count {
            let f = self.rng.float(0.0, 1.0);
            let origin = ma(start, f, sub(end, start));
            if !self.bubble(model, origin, start[2], height, speed) {
                return;
            }
        }
    }

    /// `R_FizzEffect(ent, model, density)`.
    pub fn fizz_effect(&mut self, env: &dyn Env, entity: i32, model: i32, density: i32) {
        let Some(ent) = env.entity(entity) else { return };
        let Some(brush) = ent.model else { return };
        let Some(model) = env.model(model) else { return };
        let (mins, maxs) = (brush.mins, brush.maxs);
        let width = (maxs[0] - mins[0]) as i32;
        let depth = (maxs[1] - mins[1]) as i32;
        let height = maxs[2] - mins[2];
        let c = ent.rendercolor;
        let mut speed = (i32::from(c[0]) * 256 + i32::from(c[1])) as f32;
        if c[2] != 0 {
            speed = -speed;
        }
        let yaw = ent.angles[1] * std::f32::consts::PI / 180.0;
        let (vx, vy) = (yaw.cos() * speed, yaw.sin() * speed);
        for _ in 0..=density {
            let origin = [mins[0] + self.rng.long(0, width - 1) as f32, mins[1] + self.rng.long(0, depth - 1) as f32, mins[2]];
            let Some(k) = self.tent_alloc(origin, Some(model)) else { return };
            let vz = self.rng.long(80, 140) as f32;
            let frame = self.rng.long(0, model.frames.max(1) - 1) as f32;
            let scale = 1.0 / self.rng.float(2.0, 5.0);
            let die = self.die_at(height / vz - 0.1);
            let t = &mut self.tents[k];
            t.flags |= flags::SINEWAVE;
            t.x = origin[0];
            t.y = origin[1];
            t.velocity = [vx, vy, vz];
            t.die = die;
            t.frame = frame;
            t.rendermode = render::TRANS_ALPHA;
            t.renderamt = 255;
            t.scale = scale;
        }
    }

    // --- bullets and explosions (§D.6.3, §D.7.3) ---

    /// `R_MultiGunshot(org, dir, noise, count, decalCount, decals)`.
    pub fn multi_gunshot(&mut self, env: &dyn Env, org: Vec3, dir: Vec3, noise: [f32; 2], count: i32, decals: &[i32]) {
        let (_, right, up) = gs_pmove::math::angle_vectors(vec::vector_angles(dir));
        for i in 0..count {
            let (x, y) = loop {
                let x = self.rng.float(-0.5, 0.5) + self.rng.float(-0.5, 0.5);
                let y = self.rng.float(-0.5, 0.5) + self.rng.float(-0.5, 0.5);
                if x * x + y * y <= 1.0 {
                    break (x, y);
                }
            };
            let d = self.rng.long(0, decals.len() as i32 - 1);
            let aim = add(add(dir, scale(right, x * noise[0])), scale(up, y * noise[1]));
            let tr = env.trace(org, ma(org, 4096.0, aim), PM_STUDIO_IGNORE);
            if tr.fraction != 1.0 {
                if i & 2 != 0 {
                    self.ricochet_sound(tr.endpos);
                }
                self.bullet_impact_particles(tr.endpos);
                if self.cvars.r_decals
                    && let Some(&decal) = usize::try_from(d).ok().and_then(|d| decals.get(d))
                {
                    self.events.push(crate::Event::Decal { decal, entity: tr.ent_info, model: 0, origin: tr.endpos, flags: 0 });
                }
            }
        }
    }

    /// `efx_R_Explosion(pos, sprite, scale, framerate, flags)`.
    pub fn explosion(&mut self, env: &dyn Env, pos: Vec3, sprite: i32, scale: f32, framerate: f32, explode_flags: i32) {
        if scale != 0.0 {
            if let Some(t) = self.default_sprite(env, pos, sprite, framerate) {
                self.sprite_explode(t, scale, explode_flags);
            }
            if explode_flags & 8 == 0 {
                self.flicker_particles(pos);
            }
            if explode_flags & 2 == 0 {
                self.add_dlight(pos, 200.0, [250, 250, 150], 0.01, 800.0);
                self.add_dlight(pos, 150.0, [255, 190, 40], 1.0, 200.0);
            }
        }
        if explode_flags & 4 == 0 {
            let n = self.rng.long(0, 2) + 3;
            self.sound(format!("weapons/explode{n}.wav"), pos, 1.0, 0.3, 100);
        }
    }

    // --- the update (`HUD_TempEntUpdate`) ---

    /// `CL_TempEntPlaySound(t, damp)`.
    fn tent_play_sound(&mut self, t: &Tent, damp: f32) {
        let Some((samples, base, shell)) = bounce_samples(t.hit_sound) else { return };
        let zvel = (t.velocity[2] as i32).abs();
        if shell {
            if zvel < 200 && self.rng.long(0, 3) != 0 {
                return;
            }
        } else if self.rng.long(0, 5) != 0 {
            return;
        }
        if damp <= 0.0 {
            return;
        }
        let vol = base * (zvel as f32 / if shell { 350.0 } else { 450.0 }).min(1.0);
        let pitch = if self.rng.long(0, 3) == 0 && !shell { self.rng.long(90, 124) } else { 100 };
        let sample = samples[self.rng.long(0, samples.len() as i32 - 1) as usize];
        self.sound(sample, t.origin, vol, 1.0, pitch);
    }

    /// `CL_TEntAddVisibleEntity`: false without a model or outside the PVS.
    fn tent_add_visible(env: &dyn Env, t: &mut Tent) -> bool {
        let Some(m) = t.model else { return false };
        if !env.box_in_pvs(add(t.origin, m.mins), add(t.origin, m.maxs)) {
            return false;
        }
        t.visible = true;
        true
    }

    /// `CL_UpdateTEnts` → CS `HUD_TempEntUpdate(frametime, client_time, cl_gravity)`.
    pub fn update_tents(&mut self, env: &dyn Env, cl_gravity: f32) {
        for t in &mut self.tents {
            t.visible = false;
        }
        if self.tents.is_empty() {
            return;
        }
        self.tent_frame = (self.tent_frame + 1) & 31;
        let frametime = self.time - self.oldtime;
        let client_time = self.time;
        if frametime <= 0.0 {
            for t in self.tents.iter_mut().filter(|t| t.flags & flags::NOMODEL == 0) {
                Self::tent_add_visible(env, t);
            }
            return;
        }
        let ft = frametime as f32;
        let gravity = (-frametime * f64::from(cl_gravity)) as f32;
        let gravity_slow = gravity * 0.5;
        let count = self.tents.len();
        for i in (0..count).rev() {
            if self.tents[i].dead {
                continue;
            }
            let mut t = self.tents[i].clone();
            let alive = self.tent_think(env, &mut t, ft, client_time, gravity, gravity_slow);
            if let Some(slot) = self.tents.get_mut(i) {
                *slot = t;
                slot.dead |= !alive;
            }
        }
        self.tents.retain(|t| !t.dead);
    }

    /// One tent of the update; false when it is unlinked.
    fn tent_think(&mut self, env: &dyn Env, t: &mut Tent, ft: f32, client_time: f64, gravity: f32, gravity_slow: f32) -> bool {
        let life = (f64::from(t.die) - client_time) as f32;
        if life < 0.0 {
            if t.flags & flags::FADEOUT == 0 {
                return false;
            }
            if t.rendermode == render::NORMAL {
                t.rendermode = render::TRANS_TEXTURE;
            }
            t.renderamt = (t.base_renderamt as f32 * (1.0 + life * t.fade_speed)) as i32;
            if t.renderamt <= 0 {
                return false;
            }
        }
        t.prev_origin = t.origin;
        let now = client_time as f32;
        if t.flags & flags::SPARKSHOWER != 0 {
            if now > t.spark_time {
                self.spark_effect(env, t.origin, 8, -200, 200);
                t.spark_budget -= 0.1;
                if t.spark_budget <= 0.0 {
                    t.die = now;
                } else {
                    t.die = (client_time + 0.5) as f32;
                    t.spark_time = (client_time + 0.1) as f32;
                }
            }
        } else if t.flags & flags::PLYRATTACHMENT != 0 {
            if let Some(ent) = env.entity(i32::from(t.client_index)) {
                t.origin = add(ent.origin, t.tent_offset);
            }
        } else if t.flags & flags::SINEWAVE != 0 {
            t.x += t.velocity[0] * ft;
            t.y += t.velocity[1] * ft;
            let tf = client_time as f32;
            t.origin[0] = t.x + (t.velocity[2] + tf * t.sine_freq).sin() * (t.framerate * 10.0);
            t.origin[1] = t.y + (t.velocity[2] + tf * 5.5 + 0.7).sin() * (t.framerate * 8.0);
            t.origin[2] += t.velocity[2] * ft;
        } else if t.flags & flags::SPIRAL != 0 {
            let tf = client_time as f32;
            t.origin[0] += t.velocity[0] * ft + (tf * 20.0 + t.spiral_phase).sin() * 8.0;
            t.origin[1] += t.velocity[1] * ft + (tf * 30.0 + t.spiral_phase).sin() * 4.0;
            t.origin[2] += t.velocity[2] * ft;
        } else {
            t.origin = ma(t.origin, ft, t.velocity);
        }
        if t.flags & flags::SPRANIMATE != 0 {
            t.frame += ft * t.framerate;
            if t.frame >= t.frame_max {
                t.frame -= (t.frame as i32) as f32;
                if t.flags & flags::CORPSE != 0 {
                    return true;
                }
                if t.flags & flags::SPRANIMATELOOP == 0 {
                    t.die = now;
                    return true;
                }
            }
        } else if t.flags & flags::SPRCYCLE != 0 {
            t.frame += ft * 10.0;
            if t.frame >= t.frame_max {
                t.frame -= (t.frame as i32) as f32;
            }
        }
        if t.flags & flags::ROTATE != 0 {
            t.angles = ma(t.angles, ft, t.avelocity);
        }
        if t.flags & (flags::COLLIDEALL | flags::COLLIDEWORLD) != 0 && t.flags & flags::STOPPED == 0 {
            self.tent_collide(env, t, ft, client_time, gravity);
        }
        if t.flags & flags::FLICKER != 0 && t.effects == self.tent_frame {
            let die = (client_time + 0.01) as f32;
            let dl = self.alloc_dlight(0);
            dl.origin = t.origin;
            dl.radius = 60.0;
            dl.color = [255, 120, 0];
            dl.die = die;
        }
        if t.flags & flags::SMOKETRAIL != 0 {
            self.rocket_trail(t.prev_origin, t.origin, trail::SMOKE);
        }
        if t.flags & flags::STOPPED == 0 {
            if t.flags & flags::GRAVITY != 0 {
                t.velocity[2] += gravity;
            } else if t.flags & flags::SLOWGRAVITY != 0 {
                t.velocity[2] += gravity_slow;
            }
        }
        if t.flags & flags::CLIENTCUSTOM != 0 {
            self.tent_callback(t, ft, now);
        }
        if t.flags & flags::NOMODEL == 0 && !Self::tent_add_visible(env, t) && t.flags & flags::PERSIST == 0 {
            t.die = now;
            t.flags &= !flags::FADEOUT;
        }
        true
    }

    /// Collision step of the update.
    fn tent_collide(&mut self, env: &dyn Env, t: &mut Tent, ft: f32, client_time: f64, gravity: f32) {
        let mut hit = None;
        if t.flags & flags::COLLIDEALL != 0 {
            let tr = env.trace(t.prev_origin, t.origin, PM_STUDIO_BOX);
            if tr.fraction != 1.0 && !(tr.ent != 0 && tr.ent_info == i32::from(t.client_index)) {
                hit = Some(tr);
            }
        } else {
            let end = if t.flags & flags::CORPSE != 0 {
                let (dir, _) = normalized(t.velocity);
                ma(t.prev_origin, 36.0, dir)
            } else {
                t.origin
            };
            let tr = env.trace(t.prev_origin, end, PM_STUDIO_BOX | PM_WORLD_ONLY);
            if tr.fraction != 1.0 {
                if t.flags & flags::SPARKSHOWER != 0 {
                    t.velocity = scale(t.velocity, 0.6);
                    if vec::length(t.velocity) < 10.0 {
                        t.spark_budget = 0.0;
                    }
                }
                hit = Some(tr);
            }
        }
        let Some(tr) = hit else { return };
        match t.hit {
            TentHit::CorpseHitBody if tr.normal[2] > 0.0 => t.flags |= flags::STOPPED,
            TentHit::HugWalls => {
                let (u, speed) = normalized(t.velocity);
                let w = vec::cross(vec::cross(u, tr.normal), tr.normal);
                t.velocity = scale(w, if speed <= 2000.0 { 1.5 * speed } else { 3000.0 });
            }
            _ => {}
        }
        t.origin = ma(t.prev_origin, tr.fraction * ft, t.velocity);
        let mut damp = t.bounce_factor;
        if t.flags & (flags::GRAVITY | flags::SLOWGRAVITY) != 0 {
            damp *= 0.5;
            if tr.normal[2] > 0.9 && t.velocity[2] <= 0.0 && t.velocity[2] >= gravity * 3.0 {
                damp = 0.0;
                t.flags &= !(flags::ROTATE | flags::GRAVITY | flags::SLOWGRAVITY | flags::COLLIDEWORLD | flags::SMOKETRAIL);
                t.angles[0] = 0.0;
                t.angles[2] = 0.0;
            }
        }
        if t.hit_sound != 0 {
            self.tent_play_sound(t, damp);
        }
        if t.flags & flags::COLLIDEKILL != 0 {
            t.flags &= !flags::FADEOUT;
            t.die = client_time as f32;
            return;
        }
        if damp != 0.0 {
            let proj = dot(t.velocity, tr.normal);
            t.velocity = ma(t.velocity, -proj * 2.0, tr.normal);
            t.angles[1] = -t.angles[1];
        }
        if damp != 1.0 {
            t.velocity = scale(t.velocity, damp);
            if t.flags & flags::CORPSE == 0 {
                t.angles = scale(t.angles, 0.9);
            }
        }
    }
}

impl Effects {
    /// The CS client's FTENT_CLIENTCUSTOM callbacks.
    fn tent_callback(&mut self, t: &mut Tent, ft: f32, now: f32) {
        let rise = |t: &mut Tent| {
            if t.frame > 7.0 {
                t.velocity[0] *= 0.97;
                t.velocity[1] *= 0.97;
                t.velocity[2] = (0.97 * t.velocity[2] + 0.7).min(70.0);
            }
        };
        match t.callback {
            TentCallback::None => {}
            TentCallback::CorpseRemoveBody => {
                if now >= t.fuser2 + 5.0 + self.cvars.corpse_stay {
                    t.origin[2] -= 5.0 * ft;
                }
            }
            TentCallback::SmokeRise => rise(t),
            TentCallback::SmokeRiseWind => {
                rise(t);
                if t.frame > 6.0 {
                    let w = &mut self.wind;
                    for a in 0..2 {
                        w.mag[a] = (w.mag[a] + 0.075).min(5.0);
                        match w.dir[a] {
                            1 => t.velocity[a] += w.mag[a],
                            0 => t.velocity[a] -= w.mag[a],
                            _ => {}
                        }
                    }
                    for a in [1, 0] {
                        if self.rng.long(0, 10) == 0 && self.wind.mag[a] > 3.0 {
                            self.wind.mag[a] = 0.0;
                            self.wind.dir[a] = 1 - self.wind.dir[a];
                        }
                    }
                }
            }
            TentCallback::SmokeRiseWindExpand => {
                if t.renderamt > 74 && self.rng.long(0, 9) == 0 {
                    t.renderamt -= 1;
                }
                if t.base_fuser1 > t.scale {
                    t.scale += t.scale / 40.0;
                }
            }
        }
    }
}
