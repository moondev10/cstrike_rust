//! Engine effects in the client: the model registry (`mod_known` + `cl.model_precache`), the
//! [`gs_effects::Env`] view of the client, link-time entity effects, studio events, and the
//! conversion of temp entities and effect geometry for the renderer.

use std::collections::HashMap;
use std::sync::Arc;

use cs_client::models::{FrameTimes, ModelDraw, ShadowRequest, StudioEvent, StudioRenderer};
use cs_client::particleman::ParticleEnv;
use gs_client::level::Level;
use gs_client::{ClEntity, Client, LinkKind};
use gs_effects::{BeamEntity, Effects, EntityInfo, Env, Geometry, ModelId, ModelInfo, ModelKind, Tent, Trace};
use gs_formats::sprite::Sprite;
use gs_fs::FileSystem;
use gs_protocol::structs::EntityState;
use gs_render::studio::{PointLight, StudioVertex};
use gs_render::{BrushInstance, DynamicLight, EffectBatch, EffectBlend, RenderMode, Renderer, SpriteDraw, SpriteHandle};
use gs_studio::StudioModel;
use gs_world::World;

use crate::audio::Audio;

/// Sprites `CL_InitTEnts` loads for the engine's own effects.
const ENGINE_SPRITES: [&str; 9] = [
    "sprites/dot.spr",
    "sprites/lgtning.spr",
    "sprites/white.spr",
    "sprites/animglow01.spr",
    "sprites/muzzleflash1.spr",
    "sprites/muzzleflash2.spr",
    "sprites/muzzleflash3.spr",
    "sprites/richo1.spr",
    "sprites/shellchrome.spr",
];

/// Sprites the client DLL draws in the world through `SPR_Load` + `GetSpritePointer`
/// (`CreateGasSmoke`, `CEnvironment::Initialize`, `CreateWindParticle`, the voice speaker icon,
/// `CHudSpectator::VidInit`'s overview icons), loaded with the level like the engine sprites.
const CLIENT_SPRITES: [&str; 20] = [
    "sprites/gas_puff_01.spr",
    "sprites/effects/rain.spr",
    "sprites/effects/snowflake.spr",
    "sprites/effects/ripple.spr",
    "sprites/wsplash3.spr",
    "sprites/grass_01.spr",
    "sprites/grass_03.spr",
    "sprites/voiceicon.spr",
    "sprites/iplayerred.spr",
    "sprites/iplayerblue.spr",
    "sprites/iplayer.spr",
    "sprites/iplayervip.spr",
    "sprites/iplayerc4.spr",
    "sprites/iplayerdead.spr",
    "sprites/ic4.spr",
    "sprites/ihostage.spr",
    "sprites/ibackpack.spr",
    "sprites/tile.spr",
    "sprites/laserbeam.spr",
    "sprites/camera.spr",
];

/// `EF_*` bits handled at link time.
mod ef {
    pub const BRIGHTFIELD: i32 = 1;
    pub const BRIGHTLIGHT: i32 = 4;
    pub const DIMLIGHT: i32 = 8;
    pub const LIGHT: i32 = 64;
}

/// Studio model flags that leave trails (`CL_LinkPacketEntities`).
mod model_flags {
    pub const ROCKET: i32 = 1;
    pub const GRENADE: i32 = 2;
    pub const GIB: i32 = 4;
    pub const TRACER: i32 = 0x10;
    pub const ZOMGIB: i32 = 0x20;
    pub const TRACER2: i32 = 0x40;
    pub const TRACER3: i32 = 0x80;
}

/// What a loaded model draws with.
#[derive(Debug, Clone)]
pub enum ModelData {
    /// BSP submodel index (0 = the world).
    Brush(usize),
    Studio(Arc<StudioModel>),
    Sprite(SpriteHandle),
}

#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub info: ModelInfo,
    pub data: ModelData,
}

/// Loaded models by name and by precache index.
#[derive(Debug, Default)]
pub struct Registry {
    entries: Vec<ModelEntry>,
    by_name: HashMap<String, usize>,
    precache: Vec<Option<usize>>,
}

impl Registry {
    /// Forget everything (the renderer's sprite uploads were dropped with the level).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// `CL_RegisterResources` + `CL_InitTEnts`: every precached model and the engine sprites.
    pub fn load(&mut self, fs: &FileSystem, renderer: &mut Renderer, level: &Level, world: Option<&World>) {
        self.clear();
        let names = level.models.clone();
        self.precache =
            names.iter().enumerate().map(|(i, name)| self.load_named(fs, renderer, level, world, name.as_deref()?, Some(i))).collect();
        for name in ENGINE_SPRITES.into_iter().chain(CLIENT_SPRITES) {
            if self.load_named(fs, renderer, level, world, name, None).is_none() {
                log::warn!("{name}: not loaded");
            }
        }
    }

    /// `Mod_ForName`.
    fn load_named(
        &mut self,
        fs: &FileSystem,
        renderer: &mut Renderer,
        level: &Level,
        world: Option<&World>,
        name: &str,
        precache: Option<usize>,
    ) -> Option<usize> {
        let key = name.to_ascii_lowercase();
        if let Some(&id) = self.by_name.get(&key) {
            return Some(id);
        }
        let (data, kind, frames, mins, maxs) = if let Some(sub) = name.strip_prefix('*') {
            let sub: usize = sub.parse().ok()?;
            let m = world?.models.get(sub)?;
            (ModelData::Brush(sub), ModelKind::Brush, 1, m.mins, m.maxs)
        } else if key.ends_with(".bsp") {
            let m = world?.models.first()?;
            (ModelData::Brush(0), ModelKind::Brush, 1, m.mins, m.maxs)
        } else if key.ends_with(".spr") {
            let sprite = match fs.read(name).map_err(|e| e.to_string()).and_then(|d| Sprite::parse(&d).map_err(|e| e.to_string())) {
                Ok(s) => Arc::new(s),
                Err(e) => {
                    log::warn!("{name}: {e}");
                    return None;
                }
            };
            // Mod_LoadSpriteModel bounds: C integer halves of the largest frame.
            let (hw, hh) = ((sprite.max_width / 2) as f32, (sprite.max_height / 2) as f32);
            let frames = (sprite.frames.len() as i32).max(1);
            let handle = renderer.upload_sprite(name, sprite);
            (ModelData::Sprite(handle), ModelKind::Sprite, frames, [-hw, -hw, -hh], [hw, hw, hh])
        } else if key.ends_with(".mdl") {
            let model = match precache.and_then(|i| level.studio_model(i as i32).cloned()) {
                Some(m) => m,
                None => match gs_assets::load_studio(fs, name) {
                    Ok(m) => Arc::new(m),
                    Err(e) => {
                        log::warn!("{name}: {e}");
                        return None;
                    }
                },
            };
            // ModelFrameCount: the product of the bodypart model counts.
            let frames = model.bodyparts.iter().map(|b| b.models.len().max(1) as i32).product::<i32>().max(1);
            (ModelData::Studio(model), ModelKind::Studio, frames, [0.0; 3], [0.0; 3])
        } else {
            return None;
        };
        let id = self.entries.len();
        let info = ModelInfo { id: ModelId(id as u32), kind, frames, mins, maxs };
        self.entries.push(ModelEntry { info, data });
        self.by_name.insert(key, id);
        Some(id)
    }

    /// `CL_GetModelByIndex`.
    pub fn precached(&self, index: i32) -> Option<&ModelEntry> {
        let id = (*self.precache.get(usize::try_from(index).ok()?)?)?;
        self.entries.get(id)
    }

    pub fn named(&self, name: &str) -> Option<&ModelEntry> {
        self.entries.get(*self.by_name.get(&name.to_ascii_lowercase())?)
    }

    pub fn get(&self, id: ModelId) -> Option<&ModelEntry> {
        self.entries.get(id.0 as usize)
    }

    fn sprite(&self, id: ModelId) -> Option<SpriteHandle> {
        match self.get(id)?.data {
            ModelData::Sprite(h) => Some(h),
            _ => None,
        }
    }
}

/// The client as seen by effects.
pub struct ClientEnv<'a> {
    pub registry: &'a Registry,
    pub client: &'a Client,
    /// PVS row of the last rendered view leaf (`r_visframecount` marks).
    pub pvs: &'a [u8],
}

impl Env for ClientEnv<'_> {
    fn model(&self, index: i32) -> Option<ModelInfo> {
        self.registry.precached(index).map(|e| e.info)
    }

    fn model_by_name(&self, name: &str) -> Option<ModelInfo> {
        self.registry.named(name).map(|e| e.info)
    }

    fn entity(&self, index: i32) -> Option<EntityInfo> {
        let ent = self.client.entities().get(usize::try_from(index).ok()?)?;
        let in_current_packet = self.client.protocol().current_frame().entities.iter().any(|s| s.number() == index);
        Some(EntityInfo {
            index,
            origin: ent.origin,
            angles: ent.angles,
            attachment: ent.attachment,
            model: self.model(ent.curstate.modelindex()),
            in_current_packet,
            rendercolor: ent.curstate.rendercolor(),
        })
    }

    fn trace(&self, start: [f32; 3], end: [f32; 3], flags: i32) -> Trace {
        // Point hull against the pmove physents (world and solid entities).
        match self.client.trace(start, end, 2, flags) {
            Some(tr) => Trace {
                fraction: tr.fraction,
                endpos: tr.endpos,
                normal: tr.plane_normal,
                ent: tr.ent,
                ent_info: self.client.physent_info(tr.ent),
            },
            None => Trace { fraction: 1.0, endpos: end, normal: [0.0; 3], ent: -1, ent_info: 0 },
        }
    }

    fn box_in_pvs(&self, mins: [f32; 3], maxs: [f32; 3]) -> bool {
        self.client.world().is_none_or(|w| self.pvs.is_empty() || w.box_in_pvs(self.pvs, mins, maxs))
    }

    fn max_clients(&self) -> i32 {
        i32::from(self.client.protocol().maxclients)
    }
}

/// The particle manager's view of the client: [`ClientEnv`] plus point contents, texture names
/// and types, and `R_LightPoint` under this frame's light styles.
pub struct ParticleHost<'a> {
    pub env: ClientEnv<'a>,
    /// `d_lightstylevalue`.
    pub styles: &'a [i32; 64],
}

impl Env for ParticleHost<'_> {
    fn model(&self, index: i32) -> Option<ModelInfo> {
        self.env.model(index)
    }

    fn model_by_name(&self, name: &str) -> Option<ModelInfo> {
        self.env.model_by_name(name)
    }

    fn entity(&self, index: i32) -> Option<EntityInfo> {
        self.env.entity(index)
    }

    fn trace(&self, start: [f32; 3], end: [f32; 3], flags: i32) -> Trace {
        self.env.trace(start, end, flags)
    }

    fn box_in_pvs(&self, mins: [f32; 3], maxs: [f32; 3]) -> bool {
        self.env.box_in_pvs(mins, maxs)
    }

    fn max_clients(&self) -> i32 {
        self.env.max_clients()
    }
}

impl ParticleEnv for ParticleHost<'_> {
    fn point_contents(&self, p: [f32; 3]) -> i32 {
        self.env.client.point_contents(p)
    }

    fn trace_texture(&self, ent: i32, start: [f32; 3], end: [f32; 3]) -> Option<String> {
        self.env.client.trace_hit_texture(ent, start, end)
    }

    /// `R_LightPoint`: `R_LightVec(p, p − (0,0,2048))`, clamped to 255.
    fn light_at_point(&self, p: [f32; 3]) -> [f32; 3] {
        let Some(world) = self.env.client.world() else { return [255.0; 3] };
        world.light_vec(self.styles, p, [p[0], p[1], p[2] - 2048.0]).map(|c| c.min(255) as f32)
    }

    fn texture_type(&self, name: &str) -> u8 {
        self.env.client.texture_type(name)
    }

    fn grass_textures(&self) -> bool {
        self.env.client.has_texture_type(b'X')
    }
}

/// Link-time effects of `CL_LinkPlayers` / `CL_LinkPacketEntities` (docs/re/player-lights.md):
/// EF_BRIGHTLIGHT / EF_DIMLIGHT player lights, `CL_PlayerFlashlight`, then per packet entity
/// EF_BRIGHTFIELD particles, EF_BRIGHTLIGHT / EF_DIMLIGHT / EF_LIGHT dlights and model-flag
/// trails. `view_angles` is the last rendered view (the flashlight's direction).
pub fn link_effects(effects: &mut Effects, env: &ClientEnv, linked: &[(usize, LinkKind)], view_entity: usize, view_angles: [f32; 3], viewheight: [f32; 3], time: f64) {
    let entities = env.client.entities();
    let die = (time + 0.001) as f32;
    // CL_LinkPlayers: the key is a local the loop sets to 4 after each linked player; the first
    // one reads it uninitialised (an unkeyed allocation here).
    let mut player_key = 0;
    for &(index, _) in linked.iter().filter(|(_, k)| *k == LinkKind::Player) {
        let Some(ent) = entities.get(index) else { continue };
        if index != view_entity {
            let fx = ent.curstate.effects();
            if fx & ef::BRIGHTLIGHT != 0 {
                let radius = effects.random_float(400.0, 431.0);
                let o = ent.origin;
                add_light(effects, player_key, [o[0], o[1], o[2] + 16.0], radius, [250; 3], die);
            }
            if fx & ef::DIMLIGHT != 0 {
                let radius = effects.random_float(200.0, 231.0);
                add_light(effects, player_key, ent.origin, radius, [100; 3], die);
            }
        }
        player_key = 4;
    }
    player_flashlight(effects, env, view_angles, viewheight, time);
    for &(index, _) in linked.iter().filter(|(_, k)| *k == LinkKind::Normal) {
        let Some(ent) = entities.get(index) else { continue };
        let fx = ent.curstate.effects();
        if fx & ef::BRIGHTFIELD != 0 {
            effects.entity_particles(ent.origin);
        }
        // Every light of an entity shares the slot keyed by its number: the last write wins.
        let key = index as i32;
        if index != 1 {
            if fx & ef::BRIGHTLIGHT != 0 {
                let radius = effects.random_float(400.0, 431.0);
                let o = ent.origin;
                add_light(effects, key, [o[0], o[1], o[2] + 16.0], radius, [250; 3], die);
            }
            if fx & ef::DIMLIGHT != 0 {
                let radius = effects.random_float(200.0, 231.0);
                add_light(effects, key, ent.origin, radius, [100; 3], die);
            }
        }
        if fx & ef::LIGHT != 0 {
            add_light(effects, key, ent.origin, 200.0, [100; 3], die);
            effects.rocket_flare(env, ent.origin);
        }
        let flags = match env.registry.precached(ent.curstate.modelindex()).map(|e| &e.data) {
            Some(ModelData::Studio(m)) => m.flags,
            _ => 0,
        };
        let (from, to) = (ent.latched.prevorigin, ent.origin);
        let trail = if flags & model_flags::GIB != 0 {
            Some(2)
        } else if flags & model_flags::ZOMGIB != 0 {
            Some(4)
        } else if flags & model_flags::TRACER != 0 {
            Some(3)
        } else if flags & model_flags::TRACER2 != 0 {
            Some(5)
        } else if flags & model_flags::ROCKET != 0 {
            add_light(effects, key, to, 200.0, [200; 3], (time + 0.01) as f32);
            Some(0)
        } else if flags & model_flags::GRENADE != 0 {
            Some(1)
        } else if flags & model_flags::TRACER3 != 0 {
            Some(6)
        } else {
            None
        };
        if let Some(kind) = trail {
            effects.rocket_trail(from, to, kind);
        }
    }
}

fn add_light(effects: &mut Effects, key: i32, origin: [f32; 3], radius: f32, color: [u8; 3], die: f32) {
    let dl = effects.alloc_dlight(key);
    dl.origin = origin;
    dl.radius = radius;
    dl.color = color;
    dl.die = die;
}

/// `CL_PlayerFlashlight`: the local player's EF_BRIGHTLIGHT sphere, or its EF_DIMLIGHT spot
/// traced 2000 units along the last rendered view from the (unpredicted in first person)
/// entity origin plus `cl.viewheight`. The slot is kept in `cl.pLight` while its key stays 1.
fn player_flashlight(effects: &mut Effects, env: &ClientEnv, view_angles: [f32; 3], viewheight: [f32; 3], time: f64) {
    const PM_STUDIO_BOX: i32 = 2;
    let client = env.client;
    let player = usize::from(client.protocol().playernum) + 1;
    let Some(ent) = client.entities().get(player) else { return };
    let fx = ent.curstate.effects();
    if fx & (ef::BRIGHTLIGHT | ef::DIMLIGHT) == 0 || client.world().is_none() {
        if let Some(i) = effects.flashlight.take()
            && effects.dlights[i].key == 1
        {
            effects.dlights[i].die = time as f32;
        }
        return;
    }
    let slot = match effects.flashlight {
        Some(i) if effects.dlights[i].key == 1 => i,
        _ => effects.alloc_dlight_slot(1),
    };
    let (origin, color, radius) = if fx & ef::BRIGHTLIGHT != 0 {
        let o = ent.origin;
        ([o[0], o[1], o[2] + 16.0], [250; 3], 400.0)
    } else {
        let (fwd, _, _) = cs_client::view::angle_vectors(view_angles);
        let start = [0, 1, 2].map(|i| ent.origin[i] + viewheight[i]);
        let end = [0, 1, 2].map(|i| start[i] + 2000.0 * fwd[i]);
        let Some(tr) = client.trace(start, end, 2, PM_STUDIO_BOX) else { return };
        let spot = match client.physent_studio_origin(tr.ent) {
            Some(o) if tr.ent > 0 => o,
            _ => tr.endpos,
        };
        let d = tr.fraction * 2000.0;
        let k = if d < 500.0 { 1.0 } else { (500.0 / f64::from(d)) as f32 };
        let c = (k * k * 255.0) as i32 as u8;
        (spot, [c; 3], 80.0)
    };
    let dl = &mut effects.dlights[slot];
    dl.origin = origin;
    dl.color = color;
    dl.radius = radius;
    dl.die = (time + 0.2) as f32;
    effects.flashlight = Some(slot);
}

/// Runs `HUD_StudioEvent`s and EF_MUZZLEFLASH lights from the studio renderer.
pub fn studio_events(effects: &mut Effects, env: &ClientEnv, mut audio: Option<&mut Audio>, events: &[StudioEvent], time: f64) {
    for ev in events {
        match ev {
            StudioEvent::MuzzleLight { origin } => {
                let el = effects.alloc_elight(0);
                el.origin = *origin;
                el.radius = 16.0;
                el.color = [255, 192, 64];
                el.die = (time + 0.05) as f32;
                el.decay = 320.0;
            }
            StudioEvent::MuzzleFlash { origin, kind } => effects.muzzle_flash(env, *origin, *kind),
            StudioEvent::Sparks { origin, count } => effects.spark_effect(env, *origin, *count, -100, 100),
            // pfnPlaySoundByNameAtLocation(options, 1.0, attachment[0]).
            StudioEvent::Sound { name, origin } => {
                if let Some(audio) = audio.as_deref_mut() {
                    audio.sound_at_location(name, 1.0, *origin);
                }
            }
        }
    }
}

/// `R_PushDlights`' lights for the world lightmaps: `cl.time <= die && radius != 0`.
pub fn world_dlights(effects: &Effects, time: f64) -> Vec<DynamicLight> {
    effects
        .dlights
        .iter()
        .enumerate()
        .filter(|(_, d)| d.is_active(time))
        .map(|(slot, d)| DynamicLight { slot, origin: d.origin.into(), radius: d.radius, color: d.color, minlight: d.minlight })
        .collect()
}

/// Dynamic lights for `StudioDynamicLight` (`die >= cl.time`).
pub fn studio_dlights(effects: &Effects, time: f64) -> Vec<PointLight> {
    effects
        .dlights
        .iter()
        .filter(|d| f64::from(d.die) >= time)
        .map(|d| PointLight { origin: d.origin, radius: d.radius, color: d.color })
        .collect()
}

/// Entity lights for `StudioEntityLight` (`die > cl.time`, positive radius).
pub fn studio_elights(effects: &Effects, time: f64) -> Vec<PointLight> {
    effects
        .elights
        .iter()
        .filter(|e| f64::from(e.die) > time && e.radius > 0.0)
        .map(|e| PointLight { origin: e.origin, radius: e.radius, color: e.color })
        .collect()
}

/// A sprite draw before its glow visibility test.
pub struct PendingSprite {
    pub draw: SpriteDraw,
    pub renderfx: i32,
    pub renderamt: i32,
}

/// Temp entities turned into draws.
#[derive(Default)]
pub struct TentDraws {
    pub sprites: Vec<PendingSprite>,
    pub studios: Vec<ModelDraw>,
    pub brushes: Vec<BrushInstance>,
    pub events: Vec<StudioEvent>,
}

/// `tent->entity` as a `cl_entity_t` for the entity renderers.
fn tent_entity(t: &Tent, ent: &mut ClEntity) {
    ent.curstate = EntityState::default();
    let s = &mut ent.curstate;
    s.set_origin(t.origin);
    s.set_angles(t.angles);
    s.set_rendermode(t.rendermode);
    s.set_renderamt(t.renderamt);
    s.set_renderfx(t.renderfx);
    s.set_rendercolor(t.rendercolor);
    s.set_scale(t.scale);
    s.set_frame(t.frame);
    s.set_framerate(t.framerate);
    s.set_body(t.body);
    s.set_skin(t.skin as i16);
    s.set_sequence(t.sequence);
    s.set_animtime(t.animtime);
    s.set_effects(t.effects);
    ent.index = 0;
    ent.origin = t.origin;
    ent.angles = t.angles;
    ent.latched = Default::default();
    ent.attachment = [t.origin; 4];
}

/// Draws for this frame's visible temp entities (`CL_AddVisibleEntity(ET_TEMPENTITY)`).
pub fn tent_draws(effects: &Effects, registry: &Registry, studio: &mut StudioRenderer, level: &Level, times: &FrameTimes) -> TentDraws {
    let mut out = TentDraws::default();
    let mut scratch = ClEntity::default();
    for t in effects.visible_tents() {
        let Some(entry) = t.model.and_then(|m| registry.get(m.id)) else { continue };
        tent_entity(t, &mut scratch);
        let blend = studio.entity_blend(&mut scratch, times);
        match &entry.data {
            ModelData::Sprite(handle) => out.sprites.push(PendingSprite {
                draw: SpriteDraw {
                    sprite: *handle,
                    frame: t.frame,
                    origin: t.origin,
                    angles: t.angles,
                    scale: t.scale,
                    rendermode: RenderMode::from_i32(t.rendermode),
                    blend,
                    color: t.rendercolor,
                },
                renderfx: t.renderfx,
                renderamt: scratch.curstate.renderamt(),
            }),
            ModelData::Studio(model) => {
                let frame = studio.draw_temp_entity(&mut scratch, model.clone(), level, times);
                out.studios.extend(frame.draws);
                out.events.extend(frame.events);
            }
            ModelData::Brush(sub) => out.brushes.push(BrushInstance {
                model: *sub,
                origin: glam::Vec3::from(t.origin),
                angles: glam::Vec3::from(t.angles),
                rendermode: RenderMode::from_i32(t.rendermode),
                blend,
                color: t.rendercolor,
            }),
        }
    }
    out
}

/// `ET_BEAM` packet entities for `R_DrawBeams`.
pub fn beam_entities(client: &mut Client, studio: &mut StudioRenderer, linked: &[(usize, LinkKind)], times: &FrameTimes) -> Vec<BeamEntity> {
    let entities = client.entities_mut();
    linked
        .iter()
        .filter(|(_, k)| *k == LinkKind::Beam)
        .filter_map(|&(index, _)| {
            let ent = entities.get_mut(index)?;
            let blend = if ent.curstate.rendermode() == 0 { 255 } else { (studio.entity_blend(ent, times) * 255.0).round() as i32 };
            let s = &ent.curstate;
            Some(BeamEntity {
                origin: ent.origin,
                angles: s.angles(),
                model_index: s.modelindex(),
                scale: s.scale(),
                body: s.body(),
                blend,
                animtime: s.animtime(),
                rendercolor: s.rendercolor(),
                frame: s.frame(),
                rendermode: s.rendermode(),
                sequence: s.sequence(),
                skin: i32::from(s.skin()),
            })
        })
        .collect()
}

fn vertex(v: &gs_effects::Vertex) -> StudioVertex {
    StudioVertex { pos: v.pos, uv: v.uv, color: v.color }
}

/// Effect geometry → renderer particles and batches.
pub fn scene_geometry(geometry: &Geometry, registry: &Registry) -> (Vec<StudioVertex>, Vec<EffectBatch>) {
    let particles = geometry.particles.iter().map(vertex).collect();
    let batches = geometry
        .batches
        .iter()
        .filter_map(|b| {
            let gs_effects::BatchTexture::Sprite { model, frame } = b.texture;
            Some(EffectBatch {
                sprite: registry.sprite(model)?,
                frame: u32::try_from(frame).ok()?,
                blend: if b.additive { EffectBlend::Additive } else { EffectBlend::Opaque },
                vertices: b.vertices.iter().map(vertex).collect(),
            })
        })
        .collect();
    (particles, batches)
}

/// Particle-manager quads → sprite batches (alpha-blended, or additive for `kRenderTransAdd`);
/// neighbours with the same sprite frame and blend share a batch, keeping the far → near order.
pub fn particle_batches(quads: &[cs_client::particleman::Quad], registry: &Registry) -> Vec<EffectBatch> {
    let mut batches: Vec<EffectBatch> = Vec::new();
    for q in quads {
        let (Some(sprite), Ok(frame)) = (registry.sprite(q.sprite), u32::try_from(q.frame)) else { continue };
        let blend = if q.additive { EffectBlend::Additive } else { EffectBlend::Alpha };
        let vertices = q.vertices.iter().map(vertex);
        match batches.last_mut() {
            Some(b) if b.sprite == sprite && b.frame == frame && b.blend == blend => b.vertices.extend(vertices),
            _ => batches.push(EffectBatch { sprite, frame, blend, vertices: vertices.collect() }),
        }
    }
    batches
}

/// `StudioDrawShadow` + `StudioRenderShadow`: black alpha-blended sprite quads on the floor
/// under players and hostages, sprite `ShadowIdx` frame 0.
pub fn shadow_batches(requests: &[ShadowRequest], shadow_sprite: i32, registry: &Registry, client: &Client) -> Vec<EffectBatch> {
    let Some(ModelData::Sprite(sprite)) = registry.precached(shadow_sprite).filter(|_| shadow_sprite > 0).map(|e| e.data.clone()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for r in requests {
        let o = r.origin;
        // Point hull, PM_STUDIO_BOX | PM_WORLD_ONLY.
        let Some(tr) = client.trace(o, [o[0], o[1], o[2] - 150.0], 2, 10) else { continue };
        if tr.startsolid || tr.fraction >= 1.0 {
            continue;
        }
        let n = tr.plane_normal;
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let n = if len == 0.0 { [0.0, 0.0, 1.0] } else { n.map(|c| c / len) };
        if n[2] <= 0.7 {
            continue;
        }
        let e = [tr.endpos[0], tr.endpos[1], tr.endpos[2] + 1.0];
        let s = (1.0 - tr.fraction) * r.scale;
        // A = n × (1,0,0), B = n × (0,1,0), both scaled.
        let a = [0.0, n[2] * s, -n[1] * s];
        let b = [-n[2] * s, 0.0, n[0] * s];
        let p = |sa: f32, sb: f32| [e[0] + a[0] * sa + b[0] * sb, e[1] + a[1] * sa + b[1] * sb, e[2] + a[2] * sa + b[2] * sb];
        let v = |pos: [f32; 3], uv: [f32; 2]| StudioVertex { pos, uv, color: [0.0, 0.0, 0.0, 1.0] };
        let q = [v(p(1.0, 1.0), [0.0, 0.0]), v(p(1.0, -1.0), [0.0, 1.0]), v(p(-1.0, -1.0), [1.0, 1.0]), v(p(-1.0, 1.0), [1.0, 0.0])];
        out.push(EffectBatch { sprite, frame: 0, blend: EffectBlend::Alpha, vertices: vec![q[0], q[1], q[2], q[0], q[2], q[3]] });
    }
    out
}
