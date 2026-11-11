//! Particles and tracers (docs/re/render.md §D.8).

use crate::palette::{RAMP1, RAMP2, RAMP3, SPARK_RAMP};
use crate::vec::{self, Vec3, add, dot, ma, normalized, scale, sub};
use crate::{Batch, BatchTexture, Effects, Env, Vertex, ViewParams};

/// `ptype_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Kind {
    #[default]
    Static,
    Grav,
    SlowGrav,
    Fire,
    Explode,
    Explode2,
    Blob,
    Blob2,
    VoxSlowGrav,
    VoxGrav,
}

impl Kind {
    /// Index into `gTracerSize`.
    fn index(self) -> usize {
        self as usize
    }
}

/// `particle_t` subset.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Particle {
    pub org: Vec3,
    /// Palette index (particles) or `gTracerColors` index (tracers).
    pub color: i16,
    /// Tracer alpha.
    pub packed: i16,
    pub vel: Vec3,
    /// Ramp position; the length factor for tracers.
    pub ramp: f32,
    pub die: f32,
    pub kind: Kind,
}

/// `gTracerColors` (entry 4 is replaced by the tracer cvars each frame; 12 reads past the table).
const TRACER_COLORS: [[u8; 3]; 13] = [
    [255, 255, 255],
    [255, 0, 0],
    [0, 255, 0],
    [0, 0, 255],
    [0, 0, 0],
    [255, 167, 17],
    [255, 130, 90],
    [55, 60, 144],
    [255, 130, 90],
    [255, 140, 90],
    [200, 130, 90],
    [255, 120, 70],
    [0, 0, 0x80],
];

/// `gTracerSize`, indexed by tracer type.
const TRACER_SIZE: [f32; 11] = [1.5, 0.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];

/// `R_RocketTrail` types.
pub mod trail {
    pub const ROCKET: i32 = 0;
    pub const SMOKE: i32 = 1;
    pub const BLOOD: i32 = 2;
    pub const TRACER: i32 = 3;
    pub const SLIGHT_BLOOD: i32 = 4;
    pub const TRACER2: i32 = 5;
    pub const VOOR: i32 = 6;
    pub const SPIRAL: i32 = 7;
}

impl Effects {
    fn free_particles(&self) -> usize {
        self.max_particles.saturating_sub(self.particles.len() + self.tracers.len() + self.chain_particles)
    }

    /// Pops a particle onto the active list (`None` when the pool is exhausted).
    pub(crate) fn alloc_particle(&mut self) -> Option<usize> {
        if self.free_particles() == 0 {
            return None;
        }
        self.particles.push(Particle::default());
        Some(self.particles.len() - 1)
    }

    /// Pops a particle onto the tracer list.
    fn alloc_tracer(&mut self) -> Option<usize> {
        if self.free_particles() == 0 {
            return None;
        }
        self.tracers.push(Particle::default());
        Some(self.tracers.len() - 1)
    }

    pub(crate) fn alloc_chain_particle(&mut self) -> bool {
        if self.free_particles() == 0 {
            return false;
        }
        self.chain_particles += 1;
        true
    }

    pub(crate) fn release_chain_particles(&mut self, n: usize) {
        self.chain_particles = self.chain_particles.saturating_sub(n);
    }

    /// `R_ReadPointFile_f`'s marker: a static particle of palette `color` living 99999 s;
    /// `false` when the pool is exhausted.
    pub fn point_particle(&mut self, org: Vec3, color: i32) -> bool {
        let Some(k) = self.alloc_particle() else { return false };
        self.particles[k] = Particle { org, color: color as i16, die: 99999.0, kind: Kind::Static, ..Particle::default() };
        true
    }

    pub(crate) fn die_at(&self, life: f32) -> f32 {
        (self.time + f64::from(life)) as f32
    }

    fn jitter(&mut self, org: Vec3, r: f32) -> Vec3 {
        [0, 1, 2].map(|i| org[i] + self.rng.float(-r, r))
    }

    /// `R_RunParticleEffect` (also `svc_particle`); count 1024 is an explosion.
    pub fn run_particle_effect(&mut self, org: Vec3, dir: Vec3, color: i32, count: i32) {
        for i in 0..count {
            let Some(k) = self.alloc_particle() else { break };
            let p = if count == 1024 {
                let ramp = self.rng.long(0, 3) as f32;
                let o = self.jitter(org, 16.0);
                let vel = [0; 3].map(|_| self.rng.float(-256.0, 256.0));
                let kind = if i & 1 != 0 { Kind::Explode } else { Kind::Explode2 };
                Particle { org: o, color: RAMP1[0], packed: 0, vel, ramp, die: self.die_at(5.0), kind }
            } else {
                let die = self.rng.float(0.0, 0.4);
                let c = (color & !7) + self.rng.long(0, 7);
                let o = self.jitter(org, 8.0);
                Particle {
                    org: o,
                    color: c as i16,
                    packed: 0,
                    vel: scale(dir, 15.0),
                    ramp: 0.0,
                    die: self.die_at(die),
                    kind: Kind::SlowGrav,
                }
            };
            self.particles[k] = p;
        }
    }

    /// `R_ParticleExplosion`.
    pub fn particle_explosion(&mut self, org: Vec3) {
        for i in 0..1024 {
            let Some(k) = self.alloc_particle() else { break };
            let ramp = self.rng.long(0, 3) as f32;
            let vel = loop {
                let v = [0; 3].map(|_| self.rng.float(-512.0, 512.0));
                if dot(v, v) <= 262_144.0 {
                    break v;
                }
            };
            let kind = if i & 1 != 0 { Kind::Explode } else { Kind::Explode2 };
            self.particles[k] =
                Particle { org: ma(org, 0.25, vel), color: RAMP1[0], packed: 0, vel, ramp, die: self.die_at(5.0), kind };
        }
    }

    /// `R_ParticleExplosion2(org, colorStart, colorLength)`.
    pub fn particle_explosion2(&mut self, org: Vec3, start: i32, len: i32) {
        for i in 0..512 {
            let Some(k) = self.alloc_particle() else { break };
            let color = if len > 0 { start + i % len } else { start };
            let o = self.jitter(org, 16.0);
            let vel = [0; 3].map(|_| self.rng.float(-256.0, 256.0));
            self.particles[k] =
                Particle { org: o, color: color as i16, packed: 0, vel, ramp: 0.0, die: self.die_at(0.3), kind: Kind::Blob };
        }
    }

    /// `R_BlobExplosion`.
    pub fn blob_explosion(&mut self, org: Vec3) {
        for i in 0..1024 {
            let Some(k) = self.alloc_particle() else { break };
            let life = self.rng.float(1.0, 1.4);
            let (kind, color) =
                if i & 1 != 0 { (Kind::Blob, self.rng.long(66, 71)) } else { (Kind::Blob2, self.rng.long(150, 155)) };
            let o = self.jitter(org, 16.0);
            let vel = [0; 3].map(|_| self.rng.float(-256.0, 256.0));
            self.particles[k] = Particle { org: o, color: color as i16, packed: 0, vel, ramp: 0.0, die: self.die_at(life), kind };
        }
    }

    /// `R_FlickerParticles`.
    pub fn flicker_particles(&mut self, org: Vec3) {
        for _ in 0..15 {
            let Some(k) = self.alloc_particle() else { break };
            let vel = [self.rng.float(-32.0, 32.0), self.rng.float(-32.0, 32.0), self.rng.float(80.0, 143.0)];
            self.particles[k] = Particle { org, color: 254, packed: 0, vel, ramp: 0.0, die: self.die_at(2.0), kind: Kind::Blob2 };
        }
    }

    /// `R_EntityParticles` (EF_BRIGHTFIELD): one explode particle per vertex normal, 64 units
    /// out and swung 16 units along per-normal angular velocities. The original leaves the
    /// velocity and ramp of the reused free-list slot untouched; new slots start at zero here.
    pub fn entity_particles(&mut self, origin: Vec3) {
        if self.avelocities[0][0] == 0.0 {
            for v in self.avelocities.iter_mut().flatten() {
                *v = self.rng.float(0.0, 2.55);
            }
        }
        for (n, a) in crate::anorms::AVERTEXNORMALS.iter().zip(self.avelocities) {
            let a0 = (f64::from(a[0]) * self.time) as f32;
            let a1 = (f64::from(a[1]) * self.time) as f32;
            let cp = f64::from(a1).cos() as f32;
            let fwd = [f64::from(a0).cos() as f32 * cp, f64::from(a0).sin() as f32 * cp, -(f64::from(a1).sin() as f32)];
            let Some(k) = self.alloc_particle() else { return };
            let org = [0, 1, 2].map(|j| n[j] * 64.0 + origin[j] + fwd[j] * 16.0);
            self.particles[k] =
                Particle { org, color: 111, packed: 0, vel: [0.0; 3], ramp: 0.0, die: self.die_at(0.01), kind: Kind::Explode };
        }
    }

    /// `R_LavaSplash`.
    pub fn lava_splash(&mut self, org: Vec3) {
        for i in (-128..128).step_by(8) {
            for j in (-128..128).step_by(8) {
                let Some(k) = self.alloc_particle() else { return };
                let life = self.rng.float(2.0, 2.62);
                let color = self.rng.long(224, 231);
                let dir = [j as f32 + self.rng.float(0.0, 7.0), i as f32 + self.rng.float(0.0, 7.0), 256.0];
                let o = [org[0] + dir[0], org[1] + dir[1], org[2] + self.rng.float(0.0, 63.0)];
                let (n, _) = normalized(dir);
                let vel = scale(n, self.rng.float(50.0, 113.0));
                self.particles[k] =
                    Particle { org: o, color: color as i16, packed: 0, vel, ramp: 0.0, die: self.die_at(life), kind: Kind::SlowGrav };
            }
        }
    }

    /// `R_TeleportSplash`.
    pub fn teleport_splash(&mut self, org: Vec3) {
        for i in (-16..16).step_by(4) {
            for j in (-16..16).step_by(4) {
                for kz in (-24..32).step_by(4) {
                    let Some(k) = self.alloc_particle() else { return };
                    let life = self.rng.float(0.2, 0.34);
                    let color = self.rng.long(7, 14);
                    let o = [
                        org[0] + i as f32 + self.rng.float(0.0, 3.0),
                        org[1] + j as f32 + self.rng.float(0.0, 3.0),
                        org[2] + kz as f32 + self.rng.float(0.0, 3.0),
                    ];
                    let (n, _) = normalized([(j * 8) as f32, (i * 8) as f32, (kz * 8) as f32]);
                    let vel = scale(n, self.rng.float(50.0, 113.0));
                    self.particles[k] =
                        Particle { org: o, color: color as i16, packed: 0, vel, ramp: 0.0, die: self.die_at(life), kind: Kind::SlowGrav };
                }
            }
        }
    }

    /// `R_ParticleBurst(org, radius, color, life)`.
    pub fn particle_burst(&mut self, org: Vec3, radius: i32, color: i32, life: f32) {
        let r = radius as f32;
        for _ in 0..1024 {
            let Some(k) = self.alloc_particle() else { break };
            let c = color + self.rng.long(0, 10);
            let dir = [0; 3].map(|_| self.rng.float(-r, r));
            let (n, dist) = normalized(dir);
            let vel = scale(n, dist / life);
            let die = life + self.rng.float(-0.5, 0.5);
            self.particles[k] = Particle { org, color: c as i16, packed: 0, vel, ramp: 0.0, die: self.die_at(die), kind: Kind::Static };
        }
    }

    /// `R_LargeFunnel(org, reverse)`.
    pub fn large_funnel(&mut self, org: Vec3, reverse: bool) {
        for x in (-256..=256).step_by(32) {
            for y in (-256..=256).step_by(32) {
                let Some(k) = self.alloc_particle() else { return };
                let high = [org[0] + x as f32, org[1] + y as f32, org[2] + self.rng.float(100.0, 800.0)];
                let (start, dest) = if reverse { (org, high) } else { (high, org) };
                let (dir, dist) = normalized(sub(dest, start));
                let v = self.rng.float(64.0, 128.0) + (high[2] * 0.125).max(64.0);
                self.particles[k] = Particle {
                    org: start,
                    color: 244,
                    packed: 0,
                    vel: scale(dir, v),
                    ramp: 0.0,
                    die: self.die_at(dist / v),
                    kind: Kind::Static,
                };
            }
        }
    }

    /// `R_Blood(org, dir, color, speed)`.
    pub fn blood(&mut self, org: Vec3, dir: Vec3, color: i32, speed: i32) {
        let (dir, _) = normalized(dir);
        let mut s = (speed * 3) as f32;
        for _ in 0..speed / 2 {
            let base = self.jitter(org, 3.0);
            let d = self.jitter(dir, 0.06);
            for _ in 0..8 {
                let Some(k) = self.alloc_particle() else { return };
                let c = color + self.rng.long(0, 9);
                let o = self.jitter(base, 1.0);
                self.particles[k] =
                    Particle { org: o, color: c as i16, packed: 0, vel: scale(d, s), ramp: 0.0, die: self.die_at(1.5), kind: Kind::VoxGrav };
            }
            s -= speed as f32;
        }
    }

    /// `R_BloodStream(org, dir, color, speed)`.
    pub fn blood_stream(&mut self, org: Vec3, dir: Vec3, color: i32, speed: i32) {
        let (dir, _) = normalized(dir);
        let mut arc = 0.05f32;
        let mut sp = speed;
        for _ in 0..100 {
            let Some(k) = self.alloc_particle() else { break };
            let c = color + self.rng.long(0, 9);
            let vel = scale([dir[0], dir[1], dir[2] - arc], sp as f32);
            self.particles[k] = Particle { org, color: c as i16, packed: 0, vel, ramp: 0.0, die: self.die_at(2.0), kind: Kind::VoxGrav };
            arc -= 0.005;
            sp -= 1;
        }
        for _ in 0..speed / 5 {
            let mut arc2 = 0.075f32;
            let num = self.rng.float(0.0, 1.0);
            let sp2 = (speed as f32 * num) as i32 as f32;
            for drop in 0..3 {
                let Some(k) = self.alloc_particle() else { return };
                let c = color + self.rng.long(0, 9);
                let o = if drop == 0 { org } else { self.jitter(org, 1.0) };
                let vel = scale(scale([dir[0], dir[1], dir[2] - arc2], num * 1.7), sp2);
                self.particles[k] =
                    Particle { org: o, color: c as i16, packed: 0, vel, ramp: 0.0, die: self.die_at(3.0), kind: Kind::VoxSlowGrav };
                if drop == 0 {
                    arc2 -= 0.005;
                }
            }
        }
    }

    /// `R_BulletImpactParticles(pos)`: two spark tracers and dust scaled by view distance.
    pub fn bullet_impact_particles(&mut self, pos: Vec3) {
        let dist = vec::length(sub(pos, self.view_origin));
        let n = (((1000.0 - dist.min(1000.0)) / 100.0) as i32).max(1);
        for _ in 0..2 {
            let Some(k) = self.alloc_tracer() else { break };
            let vel = [0; 3].map(|_| self.rng.float(-200.0, 200.0));
            let life = self.rng.float(0.1, 0.5);
            self.tracers[k] = Particle { org: pos, color: 5, packed: 255, vel, ramp: 0.5, die: self.die_at(life), kind: Kind::Grav };
        }
        for _ in 0..4 * n {
            let Some(k) = self.alloc_particle() else { break };
            let dir = [0; 3].map(|_| self.rng.float(-1.0, 1.0));
            let vel = scale(dir, self.rng.float(50.0, 100.0));
            self.particles[k] =
                Particle { org: pos, color: ((30 * n) / 100) as i16, packed: 0, vel, ramp: 0.0, die: self.die_at(0.5), kind: Kind::Grav };
        }
    }

    /// `R_ShowLine(start, end)`: static dots every 5 units for 30 s.
    pub fn show_line(&mut self, start: Vec3, end: Vec3) {
        let (dir, mut len) = normalized(sub(end, start));
        let step = scale(dir, 5.0);
        let mut at = start;
        while len > 0.0 {
            len -= 5.0;
            let Some(k) = self.alloc_particle() else { break };
            self.particles[k] = Particle { org: at, color: 75, die: self.die_at(30.0), ..Particle::default() };
            at = add(at, step);
        }
    }

    /// `R_RocketTrail(start, end, type)`.
    pub fn rocket_trail(&mut self, start: Vec3, end: Vec3, kind: i32) {
        let (mut vec, mut len) = normalized(sub(end, start));
        let (mut right, mut up) = ([0.0; 3], [0.0; 3]);
        let mut kind = kind;
        let dec = if kind == trail::SPIRAL {
            (right, up) = crate::vec::vector_vectors(vec);
            1.0
        } else if kind >= 128 {
            kind -= 128;
            1.0
        } else {
            3.0
        };
        vec = scale(vec, dec);
        let mut start = start;
        while len > 0.0 {
            len -= dec;
            let Some(k) = self.alloc_particle() else { return };
            let mut p = Particle { die: self.die_at(2.0), ..Particle::default() };
            match kind {
                trail::ROCKET | trail::VOOR => {
                    let r = self.rng.long(0, 3);
                    p.ramp = r as f32;
                    p.color = RAMP3[r as usize];
                    p.kind = Kind::Fire;
                    p.org = if kind == trail::ROCKET { self.jitter(start, 3.0) } else { start };
                }
                trail::SMOKE => {
                    let r = self.rng.long(2, 5);
                    p.ramp = r as f32;
                    p.color = RAMP3[r as usize];
                    p.kind = Kind::Fire;
                    p.org = self.jitter(start, 3.0);
                }
                trail::BLOOD | trail::SLIGHT_BLOOD => {
                    p.kind = Kind::Grav;
                    p.color = self.rng.long(67, if kind == trail::BLOOD { 74 } else { 70 }) as i16;
                    p.org = self.jitter(start, 3.0);
                    if kind == trail::SLIGHT_BLOOD {
                        len -= 3.0;
                    }
                }
                trail::TRACER | trail::TRACER2 => {
                    p.die = self.die_at(0.5);
                    p.color = (if kind == trail::TRACER { 52 } else { 230 } + ((self.tracer_count & 4) << 1)) as i16;
                    p.org = start;
                    self.tracer_count += 1;
                    p.vel = if self.tracer_count & 1 != 0 {
                        [vec[1] * 30.0, -vec[0] * 30.0, 0.0]
                    } else {
                        [-vec[1] * 30.0, vec[0] * 30.0, 0.0]
                    };
                }
                trail::SPIRAL => {
                    let a = self.rng.long(0, 65535) as f32;
                    let r = self.rng.long(8, 16) as f32;
                    p.org = add(add(start, scale(right, a.sin() * r)), scale(up, a.cos() * r));
                    p.vel = add(scale(sub(start, p.org), 2.0), scale(vec, self.rng.float(96.0, 111.0)));
                    p.kind = Kind::Explode2;
                    let rr = self.rng.long(0, 3);
                    p.ramp = rr as f32;
                    p.color = RAMP3[rr as usize];
                }
                // Unknown types keep a stale particle in the original; leave it static.
                _ => p.org = start,
            }
            self.particles[k] = p;
            start = add(start, vec);
        }
    }

    /// `R_ParticlePoint(org, color, life, zOffset, zVel)`.
    fn particle_point(&mut self, org: Vec3, color: i16, life: f32, zvel: i32) -> bool {
        let Some(k) = self.alloc_particle() else { return false };
        self.particles[k] = Particle {
            org,
            color,
            vel: [0.0, 0.0, zvel as f32],
            die: self.die_at(life),
            kind: Kind::Static,
            ..Particle::default()
        };
        true
    }

    /// `R_ParticleLineIdx(start, end, color, life, zvel)`: a dot every 2 units.
    pub(crate) fn particle_line_idx(&mut self, start: Vec3, end: Vec3, color: i16, life: f32, zvel: f32) {
        let (dir, len) = normalized(sub(end, start));
        let mut t = 0.0f32;
        while t <= len {
            if !self.particle_point(ma(start, t, dir), color, life, zvel as i32) {
                return;
            }
            t += 2.0;
        }
    }

    /// `R_ParticleBoxIdx(mins, maxs, center, color, life)`: every face's edges (each edge twice).
    pub(crate) fn particle_box_idx(&mut self, mins: Vec3, maxs: Vec3, center: Vec3, color: i16, life: f32) {
        const FACES: [[usize; 4]; 6] = [[0, 4, 6, 2], [0, 1, 5, 4], [0, 2, 3, 1], [7, 5, 1, 3], [7, 3, 2, 6], [7, 6, 4, 5]];
        let corner = |i: usize| {
            add(
                center,
                [
                    if i & 1 != 0 { mins[0] } else { maxs[0] },
                    if i & 2 != 0 { mins[1] } else { maxs[1] },
                    if i & 4 != 0 { mins[2] } else { maxs[2] },
                ],
            )
        };
        for [a, b, c, d] in FACES {
            for (from, to) in [(b, a), (a, d), (d, c), (c, b)] {
                self.particle_line_idx(corner(from), corner(to), color, life, 0.0);
            }
        }
    }

    /// `efx_R_ParticleLine(start, end, r, g, b, life)`.
    pub fn particle_line(&mut self, start: Vec3, end: Vec3, rgb: [u8; 3], life: f32) {
        let c = self.palette.lookup(rgb[0], rgb[1], rgb[2]);
        self.particle_line_idx(start, end, c, life, 0.0);
    }

    /// `efx_R_ParticleBox(mins, maxs, r, g, b, life)`.
    pub fn particle_box(&mut self, mins: Vec3, maxs: Vec3, rgb: [u8; 3], life: f32) {
        let c = self.palette.lookup(rgb[0], rgb[1], rgb[2]);
        let center = scale(add(mins, maxs), 0.5);
        self.particle_box_idx(sub(mins, center), sub(maxs, center), center, c, life);
    }

    // --- tracers (§D.8.7) ---

    /// `R_TracerParticles(org, vel, life)`: a cvar-coloured tracer; returns its index.
    pub(crate) fn tracer_particles(&mut self, org: Vec3, vel: Vec3, life: f32) -> Option<usize> {
        let k = self.alloc_tracer()?;
        self.tracers[k] =
            Particle { org, color: 4, packed: 255, vel, ramp: self.cvars.tracerlength, die: self.die_at(life), kind: Kind::Static };
        Some(k)
    }

    /// `R_UserTracerParticle(org, vel, life, colorIndex, length)`.
    pub fn user_tracer(&mut self, org: Vec3, vel: Vec3, life: f32, color: i32, length: f32) {
        if !(0..=12).contains(&color) {
            log::debug!("UserTracer with color {color}");
            return;
        }
        let Some(k) = self.alloc_tracer() else { return };
        self.tracers[k] = Particle { org, color: color as i16, packed: 255, vel, ramp: length, die: self.die_at(life), kind: Kind::Static };
    }

    /// `R_TracerEffect(start, end)`: a bullet tracer from `start` (offset forward) to `end`.
    pub fn tracer_effect(&mut self, start: Vec3, end: Vec3) {
        if self.cvars.tracerspeed <= 0.0 {
            self.cvars.tracerspeed = 3.0;
        }
        let (dir, len) = normalized(sub(end, start));
        let offset = self.rng.long(-10, 9) as f32 + self.cvars.traceroffset;
        let start = ma(start, offset, dir);
        let speed = self.cvars.tracerspeed;
        self.tracer_particles(start, scale(dir, speed), len / speed);
    }

    /// `R_SparkStreaks(org, count, vmin, vmax)`.
    pub fn spark_streaks(&mut self, org: Vec3, count: i32, vmin: i32, vmax: i32) {
        let (lo, hi) = (vmin as f32, vmax as f32);
        for _ in 0..count {
            let Some(k) = self.alloc_tracer() else { break };
            let life = self.rng.float(0.1, 0.5);
            let vel = [0; 3].map(|_| self.rng.float(lo, hi));
            self.tracers[k] = Particle { org, color: 5, packed: 255, vel, ramp: 0.5, die: self.die_at(life), kind: Kind::Grav };
        }
    }

    /// `R_StreakSplash(org, dir, color, count, speed, vmin, vmax)`.
    #[allow(clippy::too_many_arguments)]
    pub fn streak_splash(&mut self, org: Vec3, dir: Vec3, color: i32, count: i32, speed: f32, vmin: i32, vmax: i32) {
        let (lo, hi) = (vmin as f32, vmax as f32);
        let base = scale(dir, speed);
        for _ in 0..count {
            let Some(k) = self.alloc_tracer() else { break };
            let life = self.rng.float(0.1, 0.5);
            let vel = [0, 1, 2].map(|i| base[i] + self.rng.float(lo, hi));
            self.tracers[k] = Particle { org, color: color as i16, packed: 255, vel, ramp: 1.0, die: self.die_at(life), kind: Kind::Grav };
        }
    }

    /// `R_Implosion(org, radius, count, life)`.
    pub fn implosion(&mut self, org: Vec3, radius: f32, count: i32, life: f32) {
        for _ in 0..count {
            let Some(k) = self.alloc_tracer() else { break };
            let off = scale([self.rng.float(-100.0, 100.0), self.rng.float(-100.0, 100.0), self.rng.float(0.0, 100.0)], radius / 100.0);
            self.tracers[k] = Particle {
                org: add(org, off),
                color: 4,
                packed: 255,
                vel: scale(off, -1.0 / life),
                ramp: self.cvars.tracerlength,
                die: self.die_at(life),
                kind: Kind::Static,
            };
        }
    }

    // --- drawing (§D.8.3, §D.8.7) ---

    /// The particle half of `R_DrawParticles`: draw, then move and age every particle.
    pub(crate) fn draw_particles(&mut self, view: &ViewParams, out: &mut Vec<Vertex>) {
        let up = scale(view.up, 1.5);
        let right = scale(view.right, 1.5);
        let ft = (self.time - self.oldtime) as f32;
        let (time1, time2, time3, dvel) = (ft * 5.0, ft * 10.0, ft * 15.0, ft * 4.0);
        let grav = (f64::from(ft) * f64::from(view.gravity) * 0.05) as f32;
        let time = self.time;
        self.particles.retain(|p| f64::from(p.die) >= time);
        for p in self.particles.iter_mut().rev() {
            if p.kind != Kind::Blob {
                let d = dot(sub(p.org, view.origin), view.forward);
                let s = if d < 20.0 { 1.0 } else { 1.0 + d * 0.004 };
                let c = self.palette.color(i32::from(p.color));
                let color = [f32::from(c[0]) / 255.0, f32::from(c[1]) / 255.0, f32::from(c[2]) / 255.0, 1.0];
                out.push(Vertex { pos: p.org, uv: [0.0, 0.0], color });
                out.push(Vertex { pos: ma(p.org, s, up), uv: [1.0, 0.0], color });
                out.push(Vertex { pos: ma(p.org, s, right), uv: [0.0, 1.0], color });
            }
            p.org = ma(p.org, ft, p.vel);
            match p.kind {
                Kind::Static => {}
                Kind::Grav => p.vel[2] -= grav * 20.0,
                Kind::SlowGrav => p.vel[2] = grav,
                Kind::Fire => {
                    p.ramp += time1;
                    if p.ramp >= 6.0 {
                        p.die = -1.0;
                    } else {
                        p.color = RAMP3[p.ramp as usize];
                    }
                    p.vel[2] += grav;
                }
                Kind::Explode => {
                    p.ramp += time2;
                    if p.ramp >= 8.0 {
                        p.die = -1.0;
                    } else {
                        p.color = RAMP1[p.ramp as usize];
                    }
                    p.vel[0] += p.vel[0] * dvel;
                    p.vel[1] += p.vel[1] * dvel;
                    p.vel[2] = p.vel[2] + p.vel[2] * dvel - grav;
                }
                Kind::Explode2 => {
                    p.ramp += time3;
                    if p.ramp >= 8.0 {
                        p.die = -1.0;
                    } else {
                        p.color = RAMP2[p.ramp as usize];
                    }
                    p.vel = sub(p.vel, scale(p.vel, ft));
                    p.vel[2] -= grav;
                }
                Kind::Blob | Kind::Blob2 => {
                    p.ramp += time2;
                    if p.ramp >= 9.0 {
                        p.ramp = 0.0;
                    }
                    p.color = SPARK_RAMP[p.ramp as usize];
                    p.vel[0] = (f64::from(p.vel[0]) - f64::from(p.vel[0]) * 0.5 * f64::from(ft)) as f32;
                    p.vel[1] = (f64::from(p.vel[1]) - f64::from(p.vel[1]) * 0.5 * f64::from(ft)) as f32;
                    p.vel[2] -= grav * 5.0;
                    p.kind = if self.rng.long(0, 3) != 0 { Kind::Blob } else { Kind::Blob2 };
                }
                Kind::VoxSlowGrav => p.vel[2] -= grav * 4.0,
                Kind::VoxGrav => p.vel[2] -= grav * 8.0,
            }
        }
    }

    /// `R_TracerDraw`: additive streaks with `sprites/dot.spr`, then tracer motion.
    pub(crate) fn draw_tracers(&mut self, env: &dyn Env, view: &ViewParams) -> Option<Batch> {
        let c = &self.cvars;
        let mut colors = TRACER_COLORS;
        colors[4] = [
            (c.traceralpha * c.tracerred * 255.0) as i32 as u8,
            (c.tracergreen * c.traceralpha * 255.0) as i32 as u8,
            (c.tracerblue * c.traceralpha * 255.0) as i32 as u8,
        ];
        let time = self.time;
        self.tracers.retain(|p| f64::from(p.die) >= time);
        if self.tracers.is_empty() {
            return None;
        }
        let dot_model = env.model_by_name("sprites/dot.spr")?;
        let ft = (self.time - self.oldtime) as f32;
        let gravity = view.gravity;
        let vel_scale = (1.0 - ft * 0.9).max(0.0);
        let mut vertices = Vec::with_capacity(self.tracers.len() * 6);
        let eye = dot(view.origin, view.forward);
        for p in self.tracers.iter_mut().rev() {
            let s = if f64::from(p.die - 0.1) <= time { (f64::from(p.die) - time) as f32 * p.ramp } else { p.ramp * 0.1 };
            let mut a = p.org;
            let mut b = ma(p.org, s, p.vel);
            let (_, _, wa) = view.clip(a);
            let (_, _, wb) = view.clip(b);
            let mut draw = true;
            if !(1.0 / wa > 0.0 && 1.0 / wb > 0.0) {
                let da = dot(a, view.forward) - eye;
                let db = dot(b, view.forward) - eye;
                if (da <= 0.0 && db <= 0.0) || db - da < 0.01 {
                    draw = false;
                } else {
                    let f = da / (db - da);
                    if da > 0.0 {
                        b = add(a, scale(sub(b, a), f));
                    } else {
                        a = add(a, scale(sub(b, a), f));
                    }
                }
            }
            if draw {
                let width = TRACER_SIZE.get(p.kind.index()).copied().unwrap_or(1.0);
                let off = scale(view.perpendicular(a, b), width);
                let rgb = colors.get(p.color as usize).copied().unwrap_or([0; 3]);
                let alpha = f32::from(p.packed as u8) / 255.0;
                let lit = |x: f32| [0, 1, 2].map(|i| f32::from(rgb[i]) / 255.0 * x * alpha);
                let (c0, c1) = (lit(0.0), lit(1.0));
                let v = |pos: Vec3, uv: [f32; 2], c: [f32; 3]| Vertex { pos, uv, color: [c[0], c[1], c[2], 1.0] };
                let quad = [
                    v(add(a, off), [0.0, 0.0], c0),
                    v(add(b, off), [0.0, 1.0], c1),
                    v(sub(b, off), [1.0, 1.0], c1),
                    v(sub(a, off), [1.0, 0.0], c0),
                ];
                vertices.extend_from_slice(&[quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]]);
            }
            p.org = ma(p.org, ft, p.vel);
            match p.kind {
                Kind::Grav => {
                    p.vel[2] -= ft * gravity;
                    p.vel[0] *= vel_scale;
                    p.vel[1] *= vel_scale;
                    p.packed = (((f64::from(p.die) - time) * 2.0 * 255.0) as i32).min(255) as i16;
                }
                Kind::SlowGrav => p.vel[2] = ft * gravity * 0.05,
                _ => {}
            }
        }
        Some(Batch { texture: BatchTexture::Sprite { model: dot_model.id, frame: 0 }, additive: true, vertices })
    }
}
