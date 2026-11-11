//! Engine client effects (`r_efx`, docs/re/render.md Part D): the temp-entity pool and its
//! update (`HUD_TempEntUpdate`, docs/re/client-sim.md Part G §5), particles, tracers, beams,
//! effect dlights/elights and the `svc_temp_entity` dispatch.
//!
//! The crate is renderer-agnostic: the host supplies models, entities, traces and the PVS
//! through [`Env`], and receives drawable temp entities, CPU-built particle/tracer/beam geometry
//! and side effects (sounds, decals, HUD text) as [`Event`]s.

mod anorms;
mod beams;
mod lights;
pub mod palette;
mod particles;
mod te;
mod tents;
mod vec;

#[cfg(test)]
mod tests;

pub use beams::{BeamEntity, flags as beam_flags};
pub use lights::DLight;
pub use tents::{Tent, TentCallback, TentHit, flags as tent_flags};

use beams::Beam;
use palette::Palette;
use particles::Particle;
pub use gs_common::rng::EngineRng;
use vec::Vec3;

/// Handle of a loaded model (`model_t*`), assigned by the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    Brush,
    Sprite,
    Studio,
}

/// What effects need to know about a model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelInfo {
    pub id: ModelId,
    pub kind: ModelKind,
    /// `ModelFrameCount`: sprite frames, product of studio bodypart model counts, else 1
    /// (minimum 1).
    pub frames: i32,
    pub mins: Vec3,
    pub maxs: Vec3,
}

/// A client entity (`cl_entities[i]`) as seen by effects.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityInfo {
    pub index: i32,
    /// Rendered origin (`cl.simorg` for the predicted local player).
    pub origin: Vec3,
    pub angles: Vec3,
    pub attachment: [Vec3; 4],
    pub model: Option<ModelInfo>,
    /// `curstate.messagenum == cl.parsecount`: present in the latest packet.
    pub in_current_packet: bool,
    pub rendercolor: [u8; 3],
}

/// `pmtrace_t` subset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trace {
    pub fraction: f32,
    pub endpos: Vec3,
    pub normal: Vec3,
    /// Physent index hit (0 = world).
    pub ent: i32,
    /// `physents[ent].info` (the entity number).
    pub ent_info: i32,
}

/// `PM_STUDIO_IGNORE`.
pub const PM_STUDIO_IGNORE: i32 = 1;
/// `PM_STUDIO_BOX`.
pub const PM_STUDIO_BOX: i32 = 2;
/// `PM_WORLD_ONLY`.
pub const PM_WORLD_ONLY: i32 = 8;

/// Host services.
pub trait Env {
    /// `CL_GetModelByIndex`: precache index → model.
    fn model(&self, index: i32) -> Option<ModelInfo>;
    /// `Mod_ForName` for the engine's own effect sprites (`sprites/dot.spr`, ...).
    fn model_by_name(&self, name: &str) -> Option<ModelInfo>;
    /// `cl_entities[index]` (`None` past `cl.max_edicts`).
    fn entity(&self, index: i32) -> Option<EntityInfo>;
    /// Point-hull `PM_PlayerTrace` against the world, solid entities and all players
    /// (`EV_SetSolidPlayers(-1)`), with `PM_*` trace `flags`.
    fn trace(&self, start: Vec3, end: Vec3, flags: i32) -> Trace;
    /// `PVSNode(world, mins, maxs) != NULL` for the current view leaf.
    fn box_in_pvs(&self, mins: Vec3, maxs: Vec3) -> bool;
    /// `cl.maxclients`.
    fn max_clients(&self) -> i32;
}

/// Cvars read by effects (defaults from `CL_InitTEnts` and the engine cvar table).
#[derive(Debug, Clone)]
pub struct Cvars {
    pub tracerspeed: f32,
    pub traceroffset: f32,
    pub tracerlength: f32,
    pub tracerred: f32,
    pub tracergreen: f32,
    pub tracerblue: f32,
    pub traceralpha: f32,
    pub violence_hblood: bool,
    pub violence_ablood: bool,
    pub r_decals: bool,
    /// `cl_corpsestay` (the CS corpse's `RemoveBody` reads it every frame).
    pub corpse_stay: f32,
}

impl Default for Cvars {
    fn default() -> Self {
        Self {
            tracerspeed: 6000.0,
            traceroffset: 30.0,
            tracerlength: 0.8,
            tracerred: 0.8,
            tracergreen: 0.8,
            tracerblue: 0.4,
            traceralpha: 0.5,
            violence_hblood: true,
            violence_ablood: true,
            r_decals: true,
            corpse_stay: 600.0,
        }
    }
}

/// Side effects for the host.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// `S_StartDynamicSound(-1, CHAN_AUTO, sample, origin, volume, attenuation, 0, pitch)`.
    Sound { sample: String, origin: Vec3, volume: f32, attenuation: f32, pitch: i32 },
    /// `R_DecalShoot(Draw_DecalIndex(decal), entity, model, origin, flags)`; `decal` is the server
    /// decal index (`+256` for the HIGH variants).
    Decal { decal: i32, entity: i32, model: i32, origin: Vec3, flags: i32 },
    /// TE_PLAYERDECAL (§D.12): a player's custom spray, or `decal` without customization.
    PlayerDecal { player: u8, origin: Vec3, entity: i32, decal: u8 },
    /// TE_TEXTMESSAGE (§D.11).
    TextMessage(gs_protocol::messages::TextMessage),
}

/// View inputs of the draw passes (`r_origin`, `vpn`, `vright`, `vup`, `worldToScreen`).
#[derive(Debug, Clone, Copy)]
pub struct ViewParams {
    pub origin: Vec3,
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    /// Projection · modelview, column-major (GL layout; clip-space y up).
    pub world_to_screen: [f32; 16],
    /// `movevars.gravity`.
    pub gravity: f32,
}

impl ViewParams {
    /// Clip-space `(x, y, w)` of `p`.
    fn clip(&self, p: Vec3) -> (f32, f32, f32) {
        let m = &self.world_to_screen;
        (
            m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
            m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
            m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15],
        )
    }

    /// `ScreenTransform`: normalized device x, y (undivided when w is 0).
    fn screen(&self, p: Vec3) -> (f32, f32) {
        let (x, y, w) = self.clip(p);
        if w != 0.0 { (x / w, y / w) } else { (x, y) }
    }

    /// The screen-space perpendicular of the segment `a → b` in world units:
    /// `vup·s.x − vright·s.y` with `s` the normalized screen direction.
    fn perpendicular(&self, a: Vec3, b: Vec3) -> Vec3 {
        let (xa, ya) = self.screen(a);
        let (xb, yb) = self.screen(b);
        let (s, _) = vec::normalized([xb - xa, yb - ya, 0.0]);
        vec::add(vec::scale(self.up, s[0]), vec::scale(self.right, -s[1]))
    }

    /// `R_CullBox` against the four side planes of the view frustum.
    fn cull_box(&self, mins: Vec3, maxs: Vec3) -> bool {
        cull_box(&self.world_to_screen, mins, maxs)
    }
}

/// `R_CullBox`: the box `mins..maxs` lies wholly outside one of the four side planes of the
/// frustum that `world_to_screen` (projection · modelview, column-major) projects through.
pub fn cull_box(world_to_screen: &[f32; 16], mins: Vec3, maxs: Vec3) -> bool {
    let m = world_to_screen;
    let clip = |p: Vec3| {
        (
            m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
            m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
            m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15],
        )
    };
    let corners: [(f32, f32, f32); 8] = std::array::from_fn(|i| {
        clip([
            if i & 1 != 0 { maxs[0] } else { mins[0] },
            if i & 2 != 0 { maxs[1] } else { mins[1] },
            if i & 4 != 0 { maxs[2] } else { mins[2] },
        ])
    });
    let outside = |test: fn(&(f32, f32, f32)) -> bool| corners.iter().all(test);
    outside(|c| c.0 < -c.2) || outside(|c| c.0 > c.2) || outside(|c| c.1 < -c.2) || outside(|c| c.1 > c.2)
}

/// One vertex of effect geometry; `color` is the final GL vertex colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub pos: Vec3,
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// Texture of a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchTexture {
    /// A sprite model frame (`tri.SpriteTexture`).
    Sprite { model: ModelId, frame: i32 },
}

/// Triangles drawn with one texture and render mode, in submission order.
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    pub texture: BatchTexture,
    /// `kRenderTransAdd` (blend ONE,ONE, no depth write); otherwise `kRenderNormal` (opaque,
    /// depth write; flat colours already resolved per quad).
    pub additive: bool,
    pub vertices: Vec<Vertex>,
}

/// Geometry of `R_DrawParticles` for one frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Geometry {
    /// Particle triangles with the 8×8 dot texture: GL_MODULATE, alpha blend, alpha test
    /// `!= 0`, depth test and write on, no culling.
    pub particles: Vec<Vertex>,
    /// Tracers then beams, no culling, no alpha test.
    pub batches: Vec<Batch>,
}

/// Default `r_numparticles`.
pub const DEFAULT_PARTICLES: usize = 2048;

/// All effect state of one client (`cl_tempents`, particles, beams, `cl_dlights`, `cl_elights`).
pub struct Effects {
    rng: EngineRng,
    palette: Palette,
    pub cvars: Cvars,
    /// `cl.time` / `cl.oldtime`.
    time: f64,
    oldtime: f64,
    /// `r_origin` of the last drawn view (R_BulletImpactParticles distance).
    view_origin: Vec3,
    /// `r_numparticles`.
    max_particles: usize,
    /// Active particles, oldest first (the list head is the last element).
    particles: Vec<Particle>,
    /// Active tracers, oldest first.
    tracers: Vec<Particle>,
    /// Particles held by beam-follow chains.
    chain_particles: usize,
    tracer_count: i32,
    /// Active temp entities, oldest first.
    tents: Vec<Tent>,
    /// `gTempEntFrame`.
    tent_frame: i32,
    /// CS smoke puff wind.
    wind: tents::Wind,
    /// Last [`Tent::id`] handed out.
    next_tent_id: u32,
    /// Active beams, oldest first.
    beams: Vec<Beam>,
    /// Freed beam slots (fields stay stale, as in the original's free list).
    free_beams: Vec<Beam>,
    pub dlights: [DLight; 32],
    pub elights: [DLight; 64],
    events: Vec<Event>,
    /// `avelocities` of `R_EntityParticles`, seeded on first use.
    avelocities: [[f32; 3]; 162],
    /// `cl.pLight`: the flashlight's `cl_dlights` slot.
    pub flashlight: Option<usize>,
}

impl Effects {
    /// `palette` is `gfx/palette.lmp`; `rng` is the engine's shared generator.
    pub fn new(palette: &[u8], rng: EngineRng) -> Self {
        Self {
            rng,
            palette: Palette::new(palette),
            cvars: Cvars::default(),
            time: 0.0,
            oldtime: 0.0,
            view_origin: [0.0; 3],
            max_particles: DEFAULT_PARTICLES,
            particles: Vec::new(),
            tracers: Vec::new(),
            chain_particles: 0,
            tracer_count: 0,
            tents: Vec::new(),
            tent_frame: 0,
            wind: tents::Wind::default(),
            next_tent_id: 0,
            beams: Vec::new(),
            free_beams: Vec::new(),
            dlights: [DLight::default(); 32],
            elights: [DLight::default(); 64],
            events: Vec::new(),
            avelocities: [[0.0; 3]; 162],
            flashlight: None,
        }
    }

    /// `CL_ClearTEnts` + `R_ClearParticles` + light reset (new map / disconnect).
    pub fn clear(&mut self) {
        self.particles.clear();
        self.tracers.clear();
        self.chain_particles = 0;
        self.tents.clear();
        self.beams.clear();
        self.free_beams.clear();
        self.dlights = [DLight::default(); 32];
        self.elights = [DLight::default(); 64];
        self.events.clear();
        self.flashlight = None;
    }

    /// Sets `cl.time` / `cl.oldtime` for this host frame.
    pub fn set_time(&mut self, time: f64, oldtime: f64) {
        self.time = time;
        self.oldtime = oldtime;
    }

    /// Side effects produced since the last call.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// `RandomLong` on the engine RNG (for client code sharing the generator).
    pub fn random_long(&mut self, lo: i32, hi: i32) -> i32 {
        self.rng.long(lo, hi)
    }

    /// `RandomFloat` on the engine RNG.
    pub fn random_float(&mut self, lo: f32, hi: f32) -> f32 {
        self.rng.float(lo, hi)
    }

    fn sound(&mut self, sample: impl Into<String>, origin: Vec3, volume: f32, attenuation: f32, pitch: i32) {
        self.events.push(Event::Sound { sample: sample.into(), origin, volume, attenuation, pitch });
    }

    /// `R_RicochetSound`: weapons/ric1..5.
    pub fn ricochet_sound(&mut self, pos: Vec3) {
        let n = self.rng.long(0, 4) + 1;
        self.sound(format!("weapons/ric{n}.wav"), pos, 1.0, 1.0, 100);
    }

    /// `R_DrawParticles` (with `R_TracerDraw` and `R_DrawBeams`): simulates particles,
    /// tracers and beams for this frame and returns their geometry. `beam_entities` are the
    /// linked `ET_BEAM` packet entities.
    pub fn draw(&mut self, env: &dyn Env, view: &ViewParams, beam_entities: &[BeamEntity]) -> Geometry {
        self.view_origin = view.origin;
        let mut geometry = Geometry::default();
        self.draw_particles(view, &mut geometry.particles);
        if let Some(batch) = self.draw_tracers(env, view) {
            geometry.batches.push(batch);
        }
        self.draw_beams(env, view, beam_entities, &mut geometry.batches);
        geometry
    }
}
