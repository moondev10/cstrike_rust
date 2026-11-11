//! Beams (docs/re/render.md §D.9): the 128-slot pool, creators, `R_DrawBeams` and the shape
//! builders (segments, torus, disk, cylinder, follow trail, ring).

use std::f32::consts::PI;

use crate::vec::{Vec3, add, length, ma, normalized, scale, sub};
use crate::{Batch, BatchTexture, Effects, Env, ModelId, ModelInfo, Vertex, ViewParams};

/// `MAX_BEAMS`.
const MAX_BEAMS: usize = 128;
const NOISE_DIVISIONS: usize = 128;

/// `FBEAM_*` flags.
pub mod flags {
    pub const STARTENTITY: u32 = 0x1;
    pub const ENDENTITY: u32 = 0x2;
    pub const FADEIN: u32 = 0x4;
    pub const FADEOUT: u32 = 0x8;
    pub const SINENOISE: u32 = 0x10;
    pub const SOLID: u32 = 0x20;
    pub const SHADEIN: u32 = 0x40;
    pub const SHADEOUT: u32 = 0x80;
    pub const STARTVISIBLE: u32 = 0x1000_0000;
    pub const ENDVISIBLE: u32 = 0x2000_0000;
    pub const ISACTIVE: u32 = 0x4000_0000;
    pub const FOREVER: u32 = 0x8000_0000;
}

/// Beam types (the TE ids).
pub(crate) mod kind {
    pub const POINTS: i32 = 0;
    pub const TORUS: i32 = 19;
    pub const DISK: i32 = 20;
    pub const CYLINDER: i32 = 21;
    pub const FOLLOW: i32 = 22;
    pub const RING: i32 = 24;
}

/// A BEAMFOLLOW chain particle.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ChainPoint {
    org: Vec3,
    vel: Vec3,
    die: f32,
}

/// `BEAM`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Beam {
    kind: i32,
    flags: u32,
    source: Vec3,
    target: Vec3,
    delta: Vec3,
    freq: f32,
    die: f32,
    width: f32,
    amplitude: f32,
    rgb: [f32; 3],
    brightness: f32,
    speed: f32,
    frame_rate: f32,
    frame: f32,
    segments: i32,
    start_entity: i32,
    end_entity: i32,
    model_index: i32,
    model: Option<ModelInfo>,
    frame_count: i32,
    follow_model: Option<ModelId>,
    /// BEAMFOLLOW chain, head first.
    particles: Vec<ChainPoint>,
}

/// An `ET_BEAM` packet entity (`cl_beamentities`), drawn by `R_DrawBeams` step 3.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeamEntity {
    pub origin: Vec3,
    /// `curstate.angles`: the end point.
    pub angles: Vec3,
    /// The model index copied into `curstate.movetype` by `CL_LinkPacketEntities`.
    pub model_index: i32,
    /// `curstate.scale`: width.
    pub scale: f32,
    /// `curstate.body`: amplitude × 100.
    pub body: i32,
    /// `CL_FxBlend(e)`.
    pub blend: i32,
    /// `curstate.animtime`: scroll speed.
    pub animtime: f32,
    pub rendercolor: [u8; 3],
    pub frame: f32,
    pub rendermode: i32,
    /// `curstate.sequence` / `curstate.skin`: start / end entity.
    pub sequence: i32,
    pub skin: i32,
}

/// `FracNoise`: midpoint displacement over `noise[0..=divs]`.
fn frac_noise(rng: &crate::EngineRng, noise: &mut [f32], divs: usize) {
    if divs < 2 {
        return;
    }
    let half = divs / 2;
    noise[half] = (noise[0] + noise[divs]) * 0.5 + divs as f32 * rng.float(-0.125, 0.125);
    if half > 1 {
        frac_noise(rng, &mut noise[half..], half);
        frac_noise(rng, noise, half);
    }
}

/// Builds GL_QUADS / GL_QUAD_STRIP geometry as triangles. Solid (flat-shaded) quads take the
/// colour of their last vertex.
struct Shape {
    vertices: Vec<Vertex>,
    solid: bool,
    /// `tri.Color4f` state: colour × alpha.
    rgb: [f32; 3],
    alpha: f32,
    brightness: f32,
    strip: Vec<Vertex>,
}

impl Shape {
    fn new(rgb: [f32; 3], alpha: f32, solid: bool) -> Self {
        Self { vertices: Vec::new(), solid, rgb, alpha, brightness: 1.0, strip: Vec::new() }
    }

    fn vertex(&self, pos: Vec3, uv: [f32; 2]) -> Vertex {
        let k = self.brightness * self.alpha;
        Vertex { pos, uv, color: [self.rgb[0] * k, self.rgb[1] * k, self.rgb[2] * k, 1.0] }
    }

    fn quad(&mut self, mut q: [Vertex; 4]) {
        if self.solid {
            let c = q[3].color;
            for v in &mut q {
                v.color = c;
            }
        }
        self.vertices.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
    }

    /// One GL_QUAD_STRIP vertex.
    fn strip(&mut self, v: Vertex) {
        self.strip.push(v);
        let n = self.strip.len();
        if n >= 4 && n.is_multiple_of(2) {
            let s = &self.strip;
            let q = [s[n - 4], s[n - 3], s[n - 1], s[n - 2]];
            self.quad(q);
        }
    }
}

impl Effects {
    /// Pops a beam slot to the active list with `die = cl.time` (fields keep stale values).
    fn beam_alloc(&mut self) -> Option<usize> {
        if self.beams.len() >= MAX_BEAMS {
            return None;
        }
        let mut b = self.free_beams.pop().unwrap_or_default();
        b.die = self.time as f32;
        self.beams.push(b);
        Some(self.beams.len() - 1)
    }

    /// `R_BeamSetup`.
    #[allow(clippy::too_many_arguments)]
    fn beam_setup(&mut self, env: &dyn Env, i: usize, start: Vec3, end: Vec3, model: i32, life: f32, width: f32, amplitude: f32, brightness: f32, speed: f32) -> bool {
        let Some(info) = env.model(model) else { return false };
        let time = self.time;
        let b = &mut self.beams[i];
        b.kind = kind::POINTS;
        b.model_index = model;
        b.model = Some(info);
        b.frame = 0.0;
        b.frame_rate = 0.0;
        b.frame_count = info.frames.max(1);
        b.source = start;
        b.target = end;
        b.delta = sub(end, start);
        b.freq = (f64::from(speed) * time) as f32;
        b.amplitude = amplitude;
        b.speed = speed;
        b.die = (time + f64::from(life)) as f32;
        b.width = width;
        b.brightness = brightness;
        b.segments = (length(b.delta) * if amplitude < 0.5 { 0.075 } else { 0.25 } + 3.0) as i32;
        b.flags = 0;
        b.follow_model = None;
        true
    }

    /// Allocation shared by the creators: rejects negative models after taking a slot.
    #[allow(clippy::too_many_arguments)]
    fn beam_create(&mut self, env: &dyn Env, start: Vec3, end: Vec3, model: i32, life: f32, width: f32, amplitude: f32, brightness: f32, speed: f32) -> Option<usize> {
        let i = self.beam_alloc()?;
        if model < 0 {
            return None;
        }
        self.beam_setup(env, i, start, end, model, life, width, amplitude, brightness, speed).then_some(i)
    }

    /// `R_BeamCull(start, end, pvsOnly)`.
    fn beam_cull(env: &dyn Env, view: Option<&ViewParams>, start: Vec3, end: Vec3) -> bool {
        let mut mins = [0.0; 3];
        let mut maxs = [0.0; 3];
        for i in 0..3 {
            mins[i] = start[i].min(end[i]);
            maxs[i] = start[i].max(end[i]);
            if mins[i] == maxs[i] {
                maxs[i] += 1.0;
            }
        }
        env.box_in_pvs(mins, maxs) && view.is_none_or(|v| !v.cull_box(mins, maxs))
    }

    fn set_rgb(&mut self, i: usize, rgb: [f32; 3]) {
        self.beams[i].rgb = rgb;
    }

    /// `R_BeamLightning(start, end, model, life, width, amp, bright, speed)`: colour left stale.
    #[allow(clippy::too_many_arguments)]
    pub fn beam_lightning(&mut self, env: &dyn Env, start: Vec3, end: Vec3, model: i32, life: f32, width: f32, amplitude: f32, brightness: f32, speed: f32) -> Option<usize> {
        self.beam_create(env, start, end, model, life, width, amplitude, brightness, speed)
    }

    /// `R_BeamPoints`.
    #[allow(clippy::too_many_arguments)]
    pub fn beam_points(
        &mut self,
        env: &dyn Env,
        start: Vec3,
        end: Vec3,
        model: i32,
        life: f32,
        width: f32,
        amplitude: f32,
        brightness: f32,
        speed: f32,
        start_frame: i32,
        framerate: f32,
        rgb: [f32; 3],
    ) {
        if life != 0.0 && !Self::beam_cull(env, None, start, end) {
            return;
        }
        let Some(i) = self.beam_create(env, start, end, model, life, width, amplitude, brightness, speed) else { return };
        let b = &mut self.beams[i];
        if life == 0.0 {
            b.flags |= flags::FOREVER;
        }
        b.frame = start_frame as f32;
        b.frame_rate = framerate;
        b.rgb = rgb;
    }

    fn beam_entity_ok(env: &dyn Env, index: i32, life: f32) -> bool {
        index >= 0 && env.entity(index & 0xFFF).is_some_and(|e| life == 0.0 || e.model.is_some())
    }

    /// `R_BeamEntPoint(startEnt, end, ...)`.
    #[allow(clippy::too_many_arguments)]
    pub fn beam_ent_point(
        &mut self,
        env: &dyn Env,
        start_ent: i32,
        end: Vec3,
        model: i32,
        life: f32,
        width: f32,
        amplitude: f32,
        brightness: f32,
        speed: f32,
        start_frame: i32,
        framerate: f32,
        rgb: [f32; 3],
    ) {
        if !Self::beam_entity_ok(env, start_ent, life) {
            return;
        }
        let Some(i) = self.beam_lightning(env, [0.0; 3], end, model, life, width, amplitude, brightness, speed) else { return };
        let b = &mut self.beams[i];
        b.kind = kind::POINTS;
        b.flags = if life == 0.0 { flags::STARTENTITY | flags::FOREVER } else { flags::STARTENTITY };
        b.start_entity = start_ent;
        b.end_entity = 0;
        b.frame = start_frame as f32;
        b.frame_rate = framerate;
        b.rgb = rgb;
    }

    /// `R_BeamEnts` (`ring` = `R_BeamRing`).
    #[allow(clippy::too_many_arguments)]
    pub fn beam_ents(
        &mut self,
        env: &dyn Env,
        ring: bool,
        start_ent: i32,
        end_ent: i32,
        model: i32,
        life: f32,
        width: f32,
        amplitude: f32,
        brightness: f32,
        speed: f32,
        start_frame: i32,
        framerate: f32,
        rgb: [f32; 3],
    ) {
        if !Self::beam_entity_ok(env, start_ent, life) || !Self::beam_entity_ok(env, end_ent, life) {
            return;
        }
        let Some(i) = self.beam_lightning(env, [0.0; 3], [0.0; 3], model, life, width, amplitude, brightness, speed) else { return };
        let b = &mut self.beams[i];
        b.kind = if ring { kind::RING } else { kind::POINTS };
        b.flags = flags::STARTENTITY | flags::ENDENTITY;
        if life == 0.0 {
            b.flags |= flags::FOREVER;
        }
        b.start_entity = start_ent;
        b.end_entity = end_ent;
        b.frame = start_frame as f32;
        b.frame_rate = framerate;
        b.rgb = rgb;
    }

    /// `R_BeamCirclePoints(type, ...)` (TE_BEAMTORUS / DISK / CYLINDER).
    #[allow(clippy::too_many_arguments)]
    pub fn beam_circle_points(
        &mut self,
        env: &dyn Env,
        beam_kind: i32,
        start: Vec3,
        end: Vec3,
        model: i32,
        life: f32,
        width: f32,
        amplitude: f32,
        brightness: f32,
        speed: f32,
        start_frame: i32,
        framerate: f32,
        rgb: [f32; 3],
    ) {
        let Some(i) = self.beam_create(env, start, end, model, life, width, amplitude, brightness, speed) else { return };
        let b = &mut self.beams[i];
        b.kind = beam_kind;
        if life == 0.0 {
            b.flags |= flags::FOREVER;
        }
        b.frame = start_frame as f32;
        b.frame_rate = framerate;
        b.rgb = rgb;
    }

    /// `R_BeamFollow(ent, model, life, width, r, g, b, bright)`.
    #[allow(clippy::too_many_arguments)]
    pub fn beam_follow(&mut self, env: &dyn Env, ent: i32, model: i32, life: f32, width: f32, rgb: [f32; 3], brightness: f32) {
        let Some(i) = self.beam_create(env, [0.0; 3], [0.0; 3], model, 0.0, width, life, brightness, 1.0) else { return };
        self.set_rgb(i, rgb);
        let b = &mut self.beams[i];
        b.kind = kind::FOLLOW;
        b.flags = flags::STARTENTITY;
        b.start_entity = ent;
        b.frame = 0.0;
        b.frame_rate = 1.0;
    }

    /// `R_BeamKill(ent)` (TE_KILLBEAM).
    pub fn beam_kill(&mut self, ent: i32) {
        let time = self.time as f32;
        for b in &mut self.beams {
            if b.flags & flags::STARTENTITY != 0 && b.start_entity == ent {
                b.flags &= !(flags::STARTENTITY | flags::FOREVER);
                if b.kind != kind::FOLLOW {
                    b.die = time;
                }
            }
            if b.flags & flags::ENDENTITY != 0 && b.end_entity == ent {
                b.flags &= !(flags::ENDENTITY | flags::FOREVER);
                b.die = time;
            }
        }
    }

    /// `R_KillDeadBeams(ent)`: an entity left the packet; its beams die (follow trails fade).
    pub fn kill_dead_beams(&mut self, ent: i32) {
        let mut i = 0;
        while i < self.beams.len() {
            let b = &mut self.beams[i];
            if b.start_entity != ent {
                i += 1;
                continue;
            }
            b.flags &= !(flags::STARTENTITY | flags::ENDENTITY);
            if b.kind == kind::FOLLOW {
                i += 1;
                continue;
            }
            let held = b.particles.len();
            let removed = self.beams.remove(i);
            drop(removed);
            self.release_chain_particles(held);
            self.free_beams.push(Beam::default());
        }
    }

    fn beam_entity(env: &dyn Env, index: i32) -> Option<crate::EntityInfo> {
        if index < 0 {
            return None;
        }
        env.entity(index & 0xFFF)
    }

    /// `R_GetAttachmentPoint`.
    fn attachment_point(ent: &crate::EntityInfo, index: i32) -> Vec3 {
        let att = (index.unsigned_abs() >> 12) & 0xF;
        if att != 0 { ent.attachment[(att - 1).min(3) as usize] } else { ent.origin }
    }

    /// `R_DrawBeams`: expire, then draw active beams and `ET_BEAM` entities.
    pub(crate) fn draw_beams(&mut self, env: &dyn Env, view: &ViewParams, beam_entities: &[BeamEntity], out: &mut Vec<Batch>) {
        let time = self.time;
        let mut i = 0;
        while i < self.beams.len() {
            if self.beams[i].flags & flags::FOREVER == 0 && time > f64::from(self.beams[i].die) {
                let b = self.beams.remove(i);
                self.free_beams.push(b);
            } else {
                i += 1;
            }
        }
        let ft = (self.time - self.oldtime) as f32;
        for i in 0..self.beams.len() {
            let mut b = std::mem::take(&mut self.beams[i]);
            if let Some(batch) = self.draw_beam(env, view, &mut b, ft) {
                out.push(batch);
            }
            self.beams[i] = b;
        }
        for e in beam_entities {
            let mut b = Beam::default();
            let Some(info) = env.model(e.model_index) else { continue };
            b.kind = kind::POINTS;
            b.model_index = e.model_index;
            b.model = Some(info);
            b.frame_count = info.frames.max(1);
            b.source = e.origin;
            b.target = e.angles;
            b.delta = sub(e.angles, e.origin);
            b.freq = (f64::from(e.animtime) * time) as f32;
            b.amplitude = e.body as f32 * 0.01;
            b.speed = e.animtime;
            b.die = time as f32;
            b.width = e.scale;
            b.brightness = e.blend as f32 / 255.0;
            b.segments = (length(b.delta) * if b.amplitude < 0.5 { 0.075 } else { 0.25 } + 3.0) as i32;
            b.rgb = e.rendercolor.map(|c| f32::from(c) / 255.0);
            b.frame = (e.frame as i32) as f32;
            b.frame_rate = 0.0;
            b.flags = match e.rendermode & 0xF {
                1 => {
                    b.start_entity = 0;
                    b.end_entity = e.skin;
                    flags::ENDENTITY
                }
                2 => {
                    b.start_entity = e.sequence;
                    b.end_entity = e.skin;
                    flags::STARTENTITY | flags::ENDENTITY
                }
                _ => 0,
            } | (e.rendermode as u32 & (flags::SINENOISE | flags::SOLID | flags::SHADEIN | flags::SHADEOUT));
            if let Some(batch) = self.draw_beam(env, view, &mut b, ft) {
                out.push(batch);
            }
        }
    }

    /// `R_DrawBeam`.
    fn draw_beam(&mut self, env: &dyn Env, view: &ViewParams, b: &mut Beam, ft: f32) -> Option<Batch> {
        if b.model_index < 0 {
            b.die = self.time as f32;
            return None;
        }
        let model = b.model?;
        let solid = b.flags & flags::SOLID != 0;
        b.freq += ft;
        let mut noise = [0.0f32; NOISE_DIVISIONS + 1];
        if b.amplitude != 0.0 {
            if b.flags & flags::SINENOISE != 0 {
                for (i, n) in noise.iter_mut().take(NOISE_DIVISIONS).enumerate() {
                    *n = (i as f32 * PI / NOISE_DIVISIONS as f32).sin();
                }
            } else {
                frac_noise(&self.rng, &mut noise, NOISE_DIVISIONS);
            }
        }
        let had_entities = b.flags & (flags::STARTENTITY | flags::ENDENTITY) != 0;
        if b.flags & flags::STARTENTITY != 0 {
            let e = Self::beam_entity(env, b.start_entity)?;
            match e.model {
                Some(m) if b.follow_model.is_none_or(|f| f == m.id) => {
                    b.source = Self::attachment_point(&e, b.start_entity);
                    b.flags |= flags::STARTVISIBLE;
                    if b.follow_model.is_none() {
                        b.follow_model = Some(m.id);
                    }
                }
                _ if b.flags & flags::FOREVER == 0 => b.flags &= !flags::STARTENTITY,
                _ => {}
            }
        }
        if b.flags & flags::ENDENTITY != 0 {
            let e = Self::beam_entity(env, b.end_entity)?;
            if e.model.is_some() {
                b.target = Self::attachment_point(&e, b.end_entity);
                b.flags |= flags::ENDVISIBLE;
            } else if b.flags & flags::FOREVER == 0 {
                b.flags &= !flags::ENDENTITY;
                b.die = self.time as f32;
                return None;
            }
        }
        if had_entities {
            if b.flags & (flags::STARTVISIBLE | flags::STARTENTITY) == flags::STARTENTITY {
                return None;
            }
            let d = sub(b.target, b.source);
            if length(d) > 1e-7 {
                b.delta = d;
            }
            b.segments = (length(b.delta) * if b.amplitude < 0.5 { 0.075 } else { 0.25 } + 3.0) as i32;
        }
        if b.kind == kind::POINTS && !Self::beam_cull(env, Some(view), b.source, b.target) {
            return None;
        }
        let frame = ((f64::from(b.frame_rate) * self.time + f64::from(b.frame)) as i32).rem_euclid(b.frame_count.max(1));
        // R_DrawBeam's SINENOISE tracer spray scales by `egon_amplitude`, an engine cvar_t that is
        // never registered or written, so it stays 0 and the spray never happens.
        let mut t = b.freq + (b.die - self.time as f32);
        if t != 0.0 {
            t = 1.0 - b.freq / t;
        }
        let alpha = if b.flags & flags::FADEIN != 0 {
            t * b.brightness
        } else if b.flags & flags::FADEOUT != 0 {
            (1.0 - t) * b.brightness
        } else {
            b.brightness
        };
        let mut shape = Shape::new(b.rgb, alpha, solid);
        match b.kind {
            kind::TORUS => Self::draw_torus(view, b, &noise, &mut shape),
            kind::DISK => Self::draw_disk(b, false, &mut shape),
            kind::CYLINDER => Self::draw_disk(b, true, &mut shape),
            kind::FOLLOW => self.draw_follow(view, b, &mut shape),
            kind::RING => self.draw_ring(env, view, b, &mut noise, &mut shape),
            _ => Self::draw_segs(view, b, &noise, &mut shape),
        }
        if shape.vertices.is_empty() {
            return None;
        }
        Some(Batch { texture: BatchTexture::Sprite { model: model.id, frame }, additive: !solid, vertices: shape.vertices })
    }

    fn noise_at(noise: &[f32], index: i32) -> f32 {
        noise[((index >> 16) & 0x7F) as usize]
    }

    /// `R_DrawSegs`.
    fn draw_segs(view: &ViewParams, b: &Beam, noise: &[f32], shape: &mut Shape) {
        if b.segments < 2 {
            return;
        }
        let mut segs = b.segments.min(NOISE_DIVISIONS as i32);
        let div = 1.0 / (segs - 1) as f32;
        let len = (length(b.delta) * 0.01).max(0.5);
        let v_step = div * len;
        let mut v_last = (b.freq * b.speed) % 1.0;
        let sine = b.flags & flags::SINENOISE != 0;
        let (noise_scale, sine_len) = if sine {
            segs = segs.max(16);
            (b.amplitude * 100.0, segs as f32 * 0.1)
        } else {
            (b.amplitude * len, 0.0)
        };
        let noise_step = (div * 128.0 * 65536.0) as i32;
        let mut idx = if sine { 0 } else { noise_step };
        let mut bright = if b.flags & flags::SHADEIN != 0 { 0.0 } else { 1.0 };
        let perp = view.perpendicular(b.source, ma(b.source, div, b.delta));
        let mut last1 = ma(b.source, b.width, perp);
        let mut last2 = ma(b.source, -b.width, perp);
        let mut last_center = b.source;
        for i in 1..segs {
            shape.brightness = bright;
            let l1 = shape.vertex(last1, [0.0, v_last]);
            let l2 = shape.vertex(last2, [1.0, v_last]);
            let f = i as f32 * div;
            if b.flags & flags::SHADEIN != 0 {
                bright = f;
            } else if b.flags & flags::SHADEOUT != 0 {
                bright = 1.0 - f;
            }
            let mut p = ma(b.source, f, b.delta);
            if noise_scale != 0.0 {
                let n = noise_scale * Self::noise_at(noise, idx);
                if sine {
                    let s = f * PI * sine_len + b.freq;
                    p = add(p, add(scale(view.up, s.sin() * n), scale(view.right, s.cos() * n)));
                } else {
                    p = add(p, add(scale(view.up, n), scale(view.right, n * (f * PI * 3.0 + b.freq).cos())));
                }
            }
            let perp = view.perpendicular(last_center, p);
            let next1 = ma(p, b.width, perp);
            let next2 = ma(p, -b.width, perp);
            let v_next = v_last + v_step;
            shape.brightness = bright;
            let n2 = shape.vertex(next2, [1.0, v_next]);
            let n1 = shape.vertex(next1, [0.0, v_next]);
            shape.quad([l1, l2, n2, n1]);
            last1 = next1;
            last2 = next2;
            last_center = p;
            idx = idx.wrapping_add(noise_step);
            v_last = v_next % 1.0;
        }
    }

    /// `R_DrawTorus` (GL_QUAD_STRIP).
    fn draw_torus(view: &ViewParams, b: &Beam, noise: &[f32], shape: &mut Shape) {
        if b.segments < 2 {
            return;
        }
        let segs = b.segments.min(NOISE_DIVISIONS as i32);
        let div = 1.0 / (segs - 1) as f32;
        let len = (length(b.delta) * 0.01).max(0.5);
        let v_step = div * len;
        let mut v_last = (b.freq * b.speed) % 1.0;
        let noise_scale = b.amplitude * len;
        let noise_step = (div * 128.0 * 65536.0) as i32;
        let mut idx = 0i32;
        let mut last = b.source;
        for i in 0..=segs {
            let f = i as f32 * div;
            let a = 2.0 * PI * f;
            let r = b.freq * b.delta[2];
            let mut p = add(b.source, [a.sin() * r, a.cos() * r, 0.0]);
            let n = noise_scale * Self::noise_at(noise, idx);
            p = add(p, add(scale(view.up, n), scale(view.right, n * (f * PI * 3.0 + b.freq).cos())));
            if i > 0 {
                let perp = view.perpendicular(last, p);
                v_last += v_step;
                let v0 = shape.vertex(ma(p, -b.width, perp), [1.0, v_last]);
                let v1 = shape.vertex(ma(p, b.width, perp), [0.0, v_last]);
                shape.strip(v0);
                shape.strip(v1);
            }
            v_last %= 1.0;
            idx = idx.wrapping_add(noise_step);
            last = p;
        }
    }

    /// TE_BEAMDISK / TE_BEAMCYLINDER (GL_QUAD_STRIP).
    fn draw_disk(b: &Beam, cylinder: bool, shape: &mut Shape) {
        if b.segments < 2 {
            return;
        }
        let segs = b.segments.min(NOISE_DIVISIONS as i32);
        let div = 1.0 / (segs - 1) as f32;
        let len = (length(b.delta) * 0.01).max(0.5);
        let v_step = div * len;
        let mut v_last = (b.freq * b.speed) % 1.0;
        let r = b.freq * b.delta[2];
        for i in 0..segs {
            let a = 2.0 * PI * (i as f32 * div);
            let (s, c) = (a.sin(), a.cos());
            if cylinder {
                shape.brightness = 0.0;
                let v0 = shape.vertex(add(b.source, [s * r, c * r, b.width]), [1.0, v_last]);
                shape.brightness = 1.0;
                let r2 = b.freq * (b.delta[2] + b.width);
                let v1 = shape.vertex(add(b.source, [s * r2, c * r2, -b.width]), [0.0, v_last]);
                shape.strip(v0);
                shape.strip(v1);
            } else {
                shape.brightness = 1.0;
                let v0 = shape.vertex(b.source, [1.0, v_last]);
                let v1 = shape.vertex(add(b.source, [s * r, c * r, 0.0]), [0.0, v_last]);
                shape.strip(v0);
                shape.strip(v1);
            }
            v_last = (v_last + v_step) % 1.0;
        }
    }

    /// `R_DrawBeamFollow`: a fading trail of points dropped every 32 units behind the entity.
    fn draw_follow(&mut self, view: &ViewParams, b: &mut Beam, shape: &mut Shape) {
        let time = self.time;
        let before = b.particles.len();
        b.particles.retain(|p| f64::from(p.die) >= time);
        self.release_chain_particles(before - b.particles.len());
        let mut added = false;
        let mut moved = false;
        if b.flags & flags::STARTENTITY != 0 {
            let dist = b.particles.first().map(|p| length(sub(p.org, b.source)));
            moved = dist.is_some_and(|d| d != 0.0);
            if dist.is_none_or(|d| d >= 32.0) {
                if !self.alloc_chain_particle() {
                    return;
                }
                let die = (time + f64::from(b.amplitude)) as f32;
                b.particles.insert(0, ChainPoint { org: b.source, vel: [0.0; 3], die });
                b.die = die;
                added = true;
            }
        }
        let (start, targets) = if b.flags & flags::STARTENTITY != 0 && moved && !added {
            (b.source, &b.particles[..])
        } else {
            match b.particles.split_first() {
                Some((head, rest)) if !rest.is_empty() => (head.org, rest),
                _ => return,
            }
        };
        let div = 1.0 / b.amplitude;
        let b_prev = (b.die - time as f32) * div;
        let Some(first) = targets.first() else { return };
        let perp = view.perpendicular(start, first.org);
        let mut last1 = ma(start, b.width, perp);
        let mut last2 = ma(start, -b.width, perp);
        let mut last_center = start;
        let mut b_cur = b_prev;
        for (k, p) in targets.iter().enumerate() {
            shape.brightness = b_prev;
            let l1 = shape.vertex(last1, [0.0, 0.0]);
            shape.brightness = b_cur;
            let l2 = shape.vertex(last2, [1.0, 0.0]);
            let perp = view.perpendicular(last_center, p.org);
            let new1 = ma(p.org, b.width, perp);
            let new2 = ma(p.org, -b.width, perp);
            b_cur = if k + 1 < targets.len() { (p.die - time as f32) * div } else { 0.0 };
            shape.brightness = b_cur;
            let n2 = shape.vertex(new2, [1.0, 1.0]);
            let n1 = shape.vertex(new1, [0.0, 1.0]);
            shape.quad([l1, l2, n2, n1]);
            last1 = new1;
            last2 = new2;
            last_center = p.org;
        }
        let ft = (self.time - self.oldtime) as f32;
        for p in &mut b.particles {
            p.org = ma(p.org, ft, p.vel);
        }
    }

    /// `R_DrawRing` (GL_QUAD_STRIP).
    fn draw_ring(&mut self, env: &dyn Env, view: &ViewParams, b: &mut Beam, noise: &mut [f32], shape: &mut Shape) {
        if b.segments < 2 {
            return;
        }
        let segs = ((b.segments as f32 * PI) as i32).min(1024);
        let len = (length(b.delta) * 0.01 * PI).max(0.5);
        let div = 1.0 / (segs - 1) as f32;
        let v_step = div * len * 0.125;
        let mut v_last = (b.freq * b.speed) % 1.0;
        let noise_scale = b.amplitude * len * 0.125;
        let noise_step = 8 * (div * 128.0 * 65536.0) as i32;
        b.delta = scale(b.delta, 0.5);
        let center = add(b.source, b.delta);
        let radius = length(b.delta);
        let xaxis = b.delta;
        let (y, _) = normalized([b.delta[1], -b.delta[0], 0.0]);
        let yaxis = scale(y, radius);
        let ext = [radius, radius, noise_scale];
        let (mins, maxs) = (sub(center, ext), add(center, ext));
        if !env.box_in_pvs(mins, maxs) || view.cull_box(mins, maxs) {
            return;
        }
        let mut idx = 0i32;
        let mut last = center;
        let regen = (segs / 8).max(1);
        for i in 0..=segs {
            let f = i as f32 * div;
            let a = 2.0 * PI * f;
            let mut p = add(add(center, scale(xaxis, a.sin())), scale(yaxis, a.cos()));
            let n = noise_scale * Self::noise_at(noise, idx);
            p = add(p, add(scale(view.up, n), scale(view.right, n * (f * PI * 3.0 * 8.0 + b.freq).cos())));
            if i > 0 {
                let perp = view.perpendicular(last, p);
                v_last += v_step;
                let v0 = shape.vertex(ma(p, -b.width, perp), [1.0, v_last]);
                let v1 = shape.vertex(ma(p, b.width, perp), [0.0, v_last]);
                shape.strip(v0);
                shape.strip(v1);
            }
            v_last %= 1.0;
            idx = idx.wrapping_add(noise_step);
            last = p;
            if i % regen == 0 && b.amplitude != 0.0 {
                frac_noise(&self.rng, noise, NOISE_DIVISIONS);
            }
        }
    }
}
