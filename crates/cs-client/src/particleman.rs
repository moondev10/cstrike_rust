//! `particleman.dll` as the CS client drives it (docs/re/particleman.md): the 1875-block pool,
//! `Update` → `ProcessAll` (cull, think, die, sort far → near, draw), the `CCoreTriangleEffect`
//! methods with their flag dispatch, and the client's particle classes: the smoke-grenade puff
//! `CPartSmokeGrenade` (§10, client-sim.md §6.6) and the weather particles (docs/re/weather.md
//! §7–§9: `CPartRainDrop`, `CPartSnowFlake`, `CPartWind`, `CPartGrassPiece`, splash, ripple).
//! Not built: force fields (the client never applies one), `TRI_COLLIDEALL` and `TRI_SPIRAL`
//! (no CS class sets them).

use gs_effects::{Effects, Env, ModelId, Vertex};
use gs_pmove::math::{Vec3, add, angle_vectors, scale, sub};

/// `CMiniMem` blocks: the most particles alive at once.
const MAX_PARTICLES: usize = 1875;
/// `TRIANGLE_FPS`.
const TRIANGLE_FPS: f32 = 30.0;
/// `PM_WORLD_ONLY | PM_STUDIO_BOX`.
const TRACE_WORLD_ONLY: i32 = 0x0A;
/// `PM_WORLD_ONLY`.
pub(crate) const TRACE_WORLD: i32 = 0x08;
/// `CONTENTS_WATER`.
pub(crate) const CONTENTS_WATER: i32 = -3;
/// The hardware renderer's puff sprite.
pub(crate) const GAS_PUFF: &str = "sprites/gas_puff_01.spr";
const SPLASH: &str = "sprites/wsplash3.spr";
const RIPPLE: &str = "sprites/effects/ripple.spr";

/// `m_iRenderFlags` (particleman.md §7.2).
pub(crate) mod render {
    pub const FRUSTUM_POINT: u32 = 0x001;
    pub const FRUSTUM_SPHERE: u32 = 0x002;
    pub const FRUSTUM_PLANE: u32 = 0x004;
    pub const PVS: u32 = 0x008;
    pub const LIGHT_NONE: u32 = 0x010;
    pub const LIGHT_COLOR: u32 = 0x020;
    pub const LIGHT_INTENSITY: u32 = 0x040;
    pub const FACE_PLAYER: u32 = 0x080;
    pub const FACE_PLAYER_ROTATEZ: u32 = 0x100;
}

/// `m_iCollisionFlags` (particleman.md §7.1).
pub(crate) mod collide {
    pub const WORLD: u32 = 0x20;
    pub const KILL: u32 = 0x4000;
    /// Any bit: `AnimateAndDie` instead of `Animate`.
    pub const ANIMATE_DIE: u32 = 0x16000;
    /// Any bit: the once-only `CONTENTS_WATER` test.
    pub const WATER_TRACE: u32 = 0x32000;
    /// Any bit: `CheckCollision` runs at all.
    pub const ANY: u32 = 0x33020;
}

/// `(float)sqrt((double)((x*x + y*y) + z*z))`.
pub(crate) fn length(v: Vec3) -> f32 {
    f64::from((v[0] * v[0] + v[1] * v[1]) + v[2] * v[2]).sqrt() as f32
}

/// `x + c` evaluated in x87 precision and stored as a float.
pub(crate) fn later(t: f32, c: f64) -> f32 {
    (f64::from(t) + c) as f32
}

/// The client's `VectorAngles` (0x1006a990): yaw and pitch (up positive) in `[0, 360)`.
pub(crate) fn vector_angles(v: Vec3) -> Vec3 {
    let deg = |r: f64| {
        let a = (r * 180.0 / std::f64::consts::PI) as f32;
        if a < 0.0 { a + 360.0 } else { a }
    };
    if v[0] == 0.0 && v[1] == 0.0 {
        return [if v[2] > 0.0 { 90.0 } else { 270.0 }, 0.0, 0.0];
    }
    let yaw = deg(f64::from(v[1]).atan2(f64::from(v[0])));
    let pitch = deg(f64::from(v[2]).atan2(f64::from(v[0] * v[0] + v[1] * v[1]).sqrt()));
    [pitch, yaw, 0.0]
}

/// What the particles ask the engine besides the effects services.
pub trait ParticleEnv: Env {
    /// `PM_PointContents`.
    fn point_contents(&self, p: Vec3) -> i32;
    /// `EV_TraceTexture(ent, start, end)` for a hit of [`Env::trace`].
    fn trace_texture(&self, ent: i32, start: Vec3, end: Vec3) -> Option<String>;
    /// `TriAPI->LightAtPoint`: `R_LightPoint`, 0..255 per channel.
    fn light_at_point(&self, p: Vec3) -> Vec3;
    /// `PM_FindTextureType`.
    fn texture_type(&self, name: &str) -> u8;
    /// `PM_IsThereGrassTexture`: `sound/materials.txt` has an `'X'` entry.
    fn grass_textures(&self) -> bool;
}

/// The six frustum planes `(a, b, c, d)` of `CFrustum::CalculateFrustum`.
#[derive(Debug, Clone, Copy, Default)]
struct Frustum([[f32; 4]; 6]);

impl Frustum {
    /// Gribb–Hartmann extraction from projection · modelview (column-major). The near plane is
    /// `z >= 0` because the renderer's clip depth runs 0..w (GL's is `z >= -w`); both are the
    /// near clip plane.
    fn new(m: &[f32; 16]) -> Self {
        let row = |r: usize| [m[r], m[4 + r], m[8 + r], m[12 + r]];
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
        let comb = |a: [f32; 4], b: [f32; 4], s: f32| [a[0] + s * b[0], a[1] + s * b[1], a[2] + s * b[2], a[3] + s * b[3]];
        let planes = [comb(r3, r0, -1.0), comb(r3, r0, 1.0), comb(r3, r1, 1.0), comb(r3, r1, -1.0), comb(r3, r2, -1.0), r2];
        Self(planes.map(|p| {
            let len = length([p[0], p[1], p[2]]);
            if len == 0.0 { p } else { p.map(|v| v / len) }
        }))
    }

    fn dist(p: &[f32; 4], o: Vec3) -> f32 {
        ((p[0] * o[0] + p[1] * o[1]) + p[2] * o[2]) + p[3]
    }

    /// `PointInside`: strictly in front of every plane.
    fn point_inside(&self, o: Vec3) -> bool {
        self.0.iter().all(|p| Self::dist(p, o) > 0.0)
    }

    /// `SphereInside`: no plane has the sphere wholly behind it.
    fn sphere_inside(&self, o: Vec3, r: f32) -> bool {
        self.0.iter().all(|p| -r < Self::dist(p, o))
    }

    /// `CubeInside`: every plane has at least one of the 8 corners strictly in front.
    fn cube_inside(&self, o: Vec3, s: f32) -> bool {
        self.0.iter().all(|p| {
            (0..8).any(|i| {
                let c = [o[0] + if i & 1 == 0 { -s } else { s }, o[1] + if i & 2 == 0 { -s } else { s }, o[2] + if i & 4 == 0 { -s } else { s }];
                Self::dist(p, c) > 0.0
            })
        })
    }
}

/// The client class behind a particle (its `Think`/`Touch` overrides and extra members).
#[derive(Debug, Clone)]
pub(crate) enum Kind {
    /// `CBaseParticle`: the rain splash and ripple.
    Base,
    /// `CPartSmokeGrenade`.
    Smoke { damp_mod: bool, spawn_inside: bool, ring_origin: Vec3 },
    /// `CPartRainDrop`.
    RainDrop { touched: bool },
    /// `CPartSnowFlake`; `address` stands in for the heap address its jitter phase adds.
    SnowFlake { spiral: bool, touched: bool, spiral_time: f32, address: f32 },
    /// `CPartWind`.
    Wind,
    /// `CPartGrassPiece`.
    Grass,
}

/// One particle: the `CCoreTriangleEffect` fields the CS classes use.
#[derive(Debug, Clone)]
pub(crate) struct Particle {
    pub kind: Kind,
    /// `m_pTexture` (a missing sprite draws nothing).
    pub sprite: Option<ModelId>,
    pub render_flags: u32,
    next_pvs_check: f32,
    in_pvs: bool,
    pub collision_flags: u32,
    /// Squared distance to the local player (the sort key).
    distance: f32,
    pub size: f32,
    original_size: f32,
    pub scale_speed: f32,
    /// `m_flStretchX`, `m_flStretchY`.
    pub stretch: [f32; 2],
    pub brightness: f32,
    original_brightness: f32,
    pub fade_speed: f32,
    pub time_created: f32,
    pub die_time: f32,
    pub gravity: f32,
    pub damping_time: f32,
    pub framerate: i32,
    pub num_frames: i32,
    pub frame: i32,
    pub rendermode: i32,
    pub origin: Vec3,
    prev_origin: Vec3,
    pub angles: Vec3,
    original_angles: Vec3,
    pub avelocity: Vec3,
    pub velocity: Vec3,
    /// `m_vColor` (0..255).
    pub color: Vec3,
    pub bounce: f32,
    in_water: bool,
}

/// A `Touch` call: position, normal, physent.
type Touch = (Vec3, Vec3, i32);

/// The per-frame state `Think` reads, and the blocks it allocates.
struct ThinkCtx<'a> {
    t: f32,
    /// `g_flOldTime`.
    old_time: f32,
    /// `g_flGravity`.
    gravity: f32,
    /// `GetViewAngles()` (`cl.viewangles`).
    cl_viewangles: Vec3,
    /// `g_Environment.m_flOldTime`.
    weather_old_time: f32,
    /// `GetLocalPlayer()->origin`.
    player_origin: Vec3,
    fx: &'a mut Effects,
    env: &'a dyn ParticleEnv,
    /// Blocks in use outside `spawned`.
    live: usize,
    spawned: Vec<Particle>,
}

impl ThinkCtx<'_> {
    /// `RequestNewMemBlock` succeeds.
    fn can_alloc(&self) -> bool {
        let ok = self.live + self.spawned.len() < MAX_PARTICLES;
        if !ok {
            log::debug!("Particleman is out of memory, too many particles");
        }
        ok
    }
}

impl Particle {
    /// `InitializeSprite(org, normal, sprite, size, brightness)` at client time `now`.
    pub(crate) fn new(kind: Kind, origin: Vec3, angles: Vec3, sprite: Option<ModelId>, size: f32, brightness: f32, now: f32) -> Self {
        Self {
            kind,
            sprite,
            render_flags: 0,
            next_pvs_check: now,
            in_pvs: true,
            collision_flags: 0,
            distance: 0.0,
            size,
            original_size: size,
            scale_speed: 0.0,
            stretch: [1.0, 1.0],
            brightness,
            original_brightness: brightness,
            fade_speed: -1.0,
            time_created: now,
            die_time: 0.0,
            gravity: 0.0,
            damping_time: 0.0,
            framerate: 0,
            num_frames: 0,
            frame: 0,
            rendermode: 0,
            origin,
            prev_origin: origin,
            angles,
            original_angles: angles,
            avelocity: [0.0; 3],
            velocity: [0.0; 3],
            color: [255.0; 3],
            bounce: 1.0,
            in_water: false,
        }
    }

    /// `CheckVisibility` (uses the state left by the previous frame).
    fn visible(&mut self, now: f32, frustum: &Frustum, env: &dyn Env) -> bool {
        let r = self.size / 5.0;
        if now >= self.next_pvs_check {
            self.in_pvs = env.box_in_pvs(sub(self.origin, [r; 3]), add(self.origin, [r; 3]));
            self.next_pvs_check = later(now, 0.1_f32.into());
        }
        let f = self.render_flags;
        let inside = if f & render::FRUSTUM_SPHERE != 0 {
            frustum.sphere_inside(self.origin, r)
        } else if f & render::FRUSTUM_PLANE != 0 {
            frustum.cube_inside(self.origin, r)
        } else if f & render::FRUSTUM_POINT != 0 {
            frustum.point_inside(self.origin)
        } else {
            true
        };
        inside && (self.in_pvs || f & render::PVS == 0)
    }

    /// The class `Think`.
    fn think(&mut self, ctx: &mut ThinkCtx) {
        let t = ctx.t;
        match &mut self.kind {
            Kind::Base => self.core_think(ctx),
            Kind::Smoke { .. } => self.smoke_think(ctx),
            Kind::RainDrop { .. } => {
                // CPartRainDrop::Think: a vertical streak at the view yaw, rolled along the
                // velocity (180/3.1415, not 180/π).
                let va = ctx.cl_viewangles;
                let (_, right, _) = angle_vectors(va);
                let v = self.velocity;
                self.angles[1] = va[1];
                let r = ((right[0] * v[0] + right[1] * v[1]) + right[2] * v[2]) / v[2];
                self.angles[2] = (f64::from(r).atan() * 57.297_469_361_769_85) as f32;
                if 155.0 > self.brightness {
                    self.brightness = (f64::from(self.brightness) + 6.5) as f32;
                }
                self.core_think(ctx);
            }
            Kind::SnowFlake { touched, .. } => {
                // CPartSnowFlake::Think: no CoreThink; Fade, Spin, the spiral toggle, then the
                // jittered or plain move, then the collision.
                let touched = *touched;
                let dt = t - ctx.weather_old_time;
                if 130.0 > self.brightness && !touched {
                    self.brightness = (f64::from(self.brightness) + 4.5) as f32;
                }
                self.fade(t);
                self.spin(t);
                let Kind::SnowFlake { spiral, spiral_time, address, .. } = &mut self.kind else { return };
                if t >= *spiral_time {
                    let k = ctx.fx.random_long(2, 4);
                    *spiral_time = later(t, f64::from(k as f32));
                    *spiral = !*spiral;
                }
                let (jitter, address) = (*spiral && !touched, *address);
                if jitter {
                    let ph = t * 5.0 + address;
                    let v = self.velocity;
                    self.origin[0] = ((f64::from(dt * v[0]) + 2.0 * f64::from(ph).sin() * 0.3) + f64::from(self.origin[0])) as f32;
                    self.origin[1] += dt * v[1];
                    self.origin[2] += dt * v[2];
                } else {
                    self.calculate_velocity(t, ctx.old_time, ctx.gravity);
                }
                if let Some(touch) = self.check_collision(t, ctx.old_time, ctx.gravity, ctx.env) {
                    self.touch(touch, ctx);
                }
            }
            Kind::Wind => {
                // CPartWind::Think: per-frame fade in for the first 3 s, then out.
                let b = self.brightness;
                if self.die_time - t > 3.0 {
                    if 105.0 > b {
                        self.brightness = ((t - self.time_created) * 5.0 + 4.0) + b;
                    }
                } else {
                    let mut b = b;
                    if b > 0.0 {
                        b = (f64::from(b) - f64::from(t - self.time_created) * 0.4) as f32;
                        self.brightness = b;
                    }
                    if 0.0 > b {
                        self.brightness = 0.0;
                        self.die_time = t;
                    }
                }
                self.core_think(ctx);
            }
            Kind::Grass => {
                // CPartGrassPiece::Think: fades with the distance to the local player.
                let d = length(sub(self.origin, ctx.player_origin));
                if d > 855.0 {
                    self.die_time = t;
                }
                if d > 500.0 {
                    let b = 255.0 - (d - 500.0);
                    self.brightness = if b > 0.0 { b } else { 1.0 };
                }
                self.core_think(ctx);
            }
        }
    }

    /// `CPartSmokeGrenade::Think`. While more than 5 s are left nothing happens to the
    /// brightness: the fade-in needs `m_flFadeSpeed == 0` (it is −1) and the brightness never
    /// exceeds its initial 190.
    fn smoke_think(&mut self, ctx: &mut ThinkCtx) {
        let t = ctx.t;
        if self.die_time - t <= 5.0 {
            let mut b = self.brightness;
            if b > 0.0 {
                b -= (t - self.time_created) * 0.1;
                self.brightness = b;
            }
            if 0.0 > b {
                self.brightness = 0.0;
                self.die_time = t;
            }
        }
        self.core_think(ctx);
        let Kind::Smoke { damp_mod, spawn_inside, ring_origin } = &mut self.kind else { return };
        if self.damping_time != 0.0 && t >= self.damping_time && !*damp_mod {
            *damp_mod = true;
            self.damping_time = later(t, 0.2);
            if *spawn_inside {
                let ring = *ring_origin;
                gas_inside(ctx, ring);
            }
        }
        if self.damping_time != 0.0 && t >= self.damping_time && *damp_mod {
            self.damping_time = 0.0;
            self.velocity = scale(self.velocity, 0.035);
        }
    }

    /// `CCoreTriangleEffect::Think`.
    fn core_think(&mut self, ctx: &mut ThinkCtx) {
        let t = ctx.t;
        if self.collision_flags & collide::ANIMATE_DIE != 0 {
            self.animate_and_die(t);
        } else {
            self.animate(t);
        }
        self.expand(t);
        self.fade(t);
        self.spin(t);
        self.calculate_velocity(t, ctx.old_time, ctx.gravity);
        if let Some(touch) = self.check_collision(t, ctx.old_time, ctx.gravity, ctx.env) {
            self.touch(touch, ctx);
        }
    }

    fn animate(&mut self, t: f32) {
        if self.framerate != 0 && self.num_frames != 0 {
            self.frame = ((t - self.time_created) * self.framerate as f32 + 0.0) as i32 % self.num_frames;
        }
    }

    fn animate_and_die(&mut self, t: f32) {
        if self.framerate != 0 && self.num_frames != 0 {
            let f = (t - self.time_created) * self.framerate as f32 + 0.0;
            let fr = if f > self.num_frames as f32 {
                self.die_time = t;
                self.frame
            } else {
                self.frame = f as i32;
                self.frame
            };
            if fr > self.num_frames {
                self.frame = self.num_frames - 1;
            }
        }
    }

    /// `Expand`: recomputed from the creation time.
    fn expand(&mut self, t: f32) {
        if self.scale_speed != 0.0 {
            self.size = (t - self.time_created) * (self.scale_speed * TRIANGLE_FPS) + self.original_size;
            if 0.0001 > self.size {
                self.die_time = t;
            }
        }
    }

    /// `Fade`: linear to the die time (speed 0) or at `speed·30` per second.
    fn fade(&mut self, t: f32) {
        if -0.5 > self.fade_speed {
            return;
        }
        let age = t - self.time_created;
        let b = if self.fade_speed == 0.0 {
            (1.0 - age / (self.die_time - self.time_created)) * self.original_brightness
        } else {
            self.original_brightness - (self.fade_speed * TRIANGLE_FPS) * age
        };
        self.brightness = b;
        if 1.0 > b {
            self.die_time = t;
        }
    }

    /// `Spin`: the angular velocity is not normalised, so the rate is `|av|·av·30` °/s.
    fn spin(&mut self, t: f32) {
        if self.avelocity == [0.0; 3] {
            return;
        }
        let k = (t - self.time_created) * (length(self.avelocity) * TRIANGLE_FPS);
        self.angles = [0, 1, 2].map(|i| k * self.avelocity[i] + self.original_angles[i]);
    }

    /// `CalculateVelocity`: an explicit Euler step over the manager's frame time, then gravity.
    fn calculate_velocity(&mut self, t: f32, old_time: f32, gravity: f32) {
        let dt = t - old_time;
        let g = ((-dt) * gravity) * self.gravity;
        if self.velocity == [0.0; 3] && g == 0.0 {
            return;
        }
        let v = self.velocity;
        self.origin = [v[0] * dt + self.origin[0], dt * v[1] + self.origin[1], v[2] * dt + self.origin[2]];
        self.velocity[2] += g;
    }

    /// `CheckCollision`: the world-only point trace from the last checked position, then the
    /// once-only water test; the `Touch` to make. The rest test keeps the binary's negated
    /// comparisons (a NaN velocity counts as at rest).
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn check_collision(&mut self, t: f32, old_time: f32, gravity: f32, env: &dyn ParticleEnv) -> Option<Touch> {
        if self.collision_flags & collide::ANY == 0 {
            return None;
        }
        let mut touch = None;
        let tr = (self.collision_flags & collide::WORLD != 0).then(|| env.trace(self.prev_origin, self.origin, TRACE_WORLD_ONLY));
        if let Some(tr) = tr.filter(|tr| tr.fraction != 1.0) {
            let n = tr.normal;
            self.velocity = scale(self.velocity, 0.6);
            if 10.0 > length(self.velocity) {
                self.collision_flags = 0;
                self.velocity = [0.0; 3];
                self.avelocity = [0.0; 3];
            }
            let dt = t - old_time;
            let gstep = ((-dt) * gravity) * self.gravity;
            self.origin = add(scale(self.velocity, dt * tr.fraction), self.prev_origin);
            let mut bounce = self.bounce * 0.5;
            if n[2] > 0.9 && !(self.velocity[2] > 0.0) && !(self.velocity[2] < gstep * 3.0) {
                self.velocity = [0.0; 3];
                self.avelocity = [0.0; 3];
                self.collision_flags = 0;
                bounce = 0.0;
            }
            if self.collision_flags & collide::KILL != 0 {
                self.die_time = t;
            } else {
                if bounce != 0.0 {
                    let v = self.velocity;
                    let d = (f64::from((n[0] * v[0] + n[1] * v[1]) + n[2] * v[2]) * -2.0) as f32;
                    self.velocity = [0, 1, 2].map(|i| n[i] * d + v[i]);
                    self.angles[1] = -self.angles[1];
                }
                if bounce != 1.0 {
                    self.velocity = scale(self.velocity, bounce);
                    self.angles = scale(self.angles, 0.9);
                }
            }
            touch = Some((tr.endpos, n, tr.ent));
        } else if self.collision_flags & collide::WATER_TRACE != 0 && !self.in_water && env.point_contents(self.origin) == CONTENTS_WATER {
            touch = Some((self.origin, [0.0, 0.0, 1.0], 0));
            self.in_water = true;
            if self.collision_flags & collide::KILL != 0 {
                self.die_time = t;
            }
        }
        self.prev_origin = self.origin;
        touch
    }

    /// The class `Touch`.
    fn touch(&mut self, (_, normal, _): Touch, ctx: &mut ThinkCtx) {
        let now = ctx.t;
        match &mut self.kind {
            Kind::RainDrop { touched } => {
                if *touched {
                    return;
                }
                *touched = true;
                self.rain_impact(normal, ctx);
            }
            Kind::SnowFlake { touched, .. } => {
                // CPartSnowFlake::Touch: stop and fade out over 0.5 s.
                if *touched {
                    return;
                }
                self.render_flags |= render::FACE_PLAYER;
                self.original_brightness = self.brightness;
                *touched = true;
                self.velocity = [0.0; 3];
                self.rendermode = 5;
                self.fade_speed = 0.0;
                self.scale_speed = 0.0;
                self.damping_time = 0.0;
                self.frame = 0;
                self.gravity = 0.0;
                self.color = [128.0; 3];
                self.die_time = later(now, 0.5);
                self.time_created = now;
            }
            _ => {}
        }
    }

    /// `CPartRainDrop::Touch` after the first-touch test: a ripple where a 32-up/16-down probe
    /// crosses a contents change (bisected to 4 units), else a splash.
    fn rain_impact(&mut self, normal: Vec3, ctx: &mut ThinkCtx) {
        let (env, now) = (ctx.env, ctx.t);
        let start = add(self.origin, [0.0, 0.0, 32.0]);
        let end = sub(self.origin, [0.0, 0.0, 16.0]);
        let tr = env.trace(start, end, TRACE_WORLD);
        let ripple_angles = vector_angles([normal[0], normal[1], -normal[2]]);
        if env.point_contents(tr.endpos) != env.point_contents(start) {
            // WaterEntryPoint.
            let (mut a, mut b) = (start, tr.endpos);
            let mut d = sub(b, a);
            while length(d) > 4.0 {
                d = scale(d, 0.5);
                let c = add(a, d);
                if env.point_contents(a) == env.point_contents(c) {
                    a = c;
                } else {
                    b = c;
                }
                d = sub(b, a);
            }
            if !ctx.can_alloc() {
                return;
            }
            let sprite = env.model_by_name(RIPPLE).map(|m| m.id);
            let mut r = Particle::new(Kind::Base, a, ripple_angles, sprite, 15.0, 110.0, now);
            r.render_flags = render::FRUSTUM_SPHERE | render::PVS | render::LIGHT_INTENSITY;
            r.rendermode = 5;
            r.fade_speed = 2.0;
            r.scale_speed = 1.0;
            r.die_time = later(now, 2.0);
            ctx.spawned.push(r);
        } else {
            if !ctx.can_alloc() {
                return;
            }
            // A missing wsplash3.spr crashes the original here; it is skipped.
            let Some(sprite) = env.model_by_name(SPLASH) else { return };
            let size = ctx.fx.random_long(20, 25) as f32;
            let mut s = Particle::new(Kind::Base, add(self.origin, normal), [90.0, 0.0, 0.0], Some(sprite.id), size, 125.0, now);
            s.render_flags = render::FRUSTUM_SPHERE | render::PVS | render::LIGHT_INTENSITY | render::FACE_PLAYER;
            s.rendermode = 5;
            s.gravity = 0.1;
            s.num_frames = sprite.frames - 1;
            s.framerate = ctx.fx.random_long(30, 45);
            s.collision_flags |= 0x16000;
            s.die_time = later(now, 0.3);
            ctx.spawned.push(s);
        }
    }

    /// `CCoreTriangleEffect::Draw`: lighting by the render flags, facing, the stretched quad,
    /// and `tri_Color4f` under the particle's render mode.
    fn draw(&mut self, now: f32, view_angles: Vec3, env: &dyn ParticleEnv) -> Option<Quad> {
        if self.die_time == now {
            return None;
        }
        let f = self.render_flags;
        let (lc, intensity) = if f & render::LIGHT_NONE == 0 {
            let lc = env.light_at_point(self.origin);
            (lc, ((lc[1] + lc[0]) + lc[2]) / 3.0)
        } else {
            ([0.0; 3], 0.0)
        };
        if f & (render::FACE_PLAYER | render::FACE_PLAYER_ROTATEZ) != 0 {
            self.angles[0] = view_angles[0];
            self.angles[1] = view_angles[1];
            if f & render::FACE_PLAYER_ROTATEZ == 0 {
                self.angles[2] = view_angles[2];
            }
        }
        let c = [0, 1, 2].map(|i| {
            let c = if f & render::LIGHT_NONE != 0 {
                self.color[i]
            } else if f & render::LIGHT_COLOR != 0 {
                lc[i] / self.color[i] * 255.0
            } else if f & render::LIGHT_INTENSITY != 0 {
                intensity / self.color[i] * 255.0
            } else {
                now
            };
            c.clamp(0.0, 255.0) / 255.0
        });
        let sprite = self.sprite?;
        let (_, right, up) = angle_vectors(self.angles);
        let r = scale(right, self.size * self.stretch[0]);
        let ll = sub(sub(self.origin, scale(r, 0.5)), scale(scale(up, self.size), 0.5));
        let lr = add(ll, r);
        let u = scale(up, self.size * self.stretch[1]);
        let (tl, tr) = (add(ll, u), add(lr, u));
        let a = (self.brightness / 255.0).clamp(0.0, 1.0);
        let additive = self.rendermode != 4;
        let color = if additive {
            // glColor4f(r·a, g·a, b·a, 1) with ONE/ONE.
            [c[0] * a, c[1] * a, c[2] * a, 1.0]
        } else {
            // glColor4ub(c·255.9, ..., a·255).
            let q = |v: f32| f32::from((v * 255.9) as u8) / 255.0;
            [q(c[0]), q(c[1]), q(c[2]), f32::from((a * 255.0) as u8) / 255.0]
        };
        let v = |pos: Vec3, uv: [f32; 2]| Vertex { pos, uv, color };
        let (vtl, vll, vlr, vtr) = (v(tl, [0.0, 0.0]), v(ll, [0.0, 1.0]), v(lr, [1.0, 1.0]), v(tr, [1.0, 0.0]));
        Some(Quad { sprite, frame: self.frame, additive, vertices: [vtl, vll, vlr, vtl, vlr, vtr] })
    }
}

/// One particle as two triangles (`TRI_QUADS`) textured with `SpriteTexture(sprite, frame)`,
/// depth tested without depth writes and without culling: `kRenderTransAlpha` alpha blending,
/// or `kRenderTransAdd` (`ONE, ONE`, colour premultiplied by the alpha).
#[derive(Debug, Clone, PartialEq)]
pub struct Quad {
    pub sprite: ModelId,
    pub frame: i32,
    pub additive: bool,
    pub vertices: [Vertex; 6],
}

/// What `SetVariables` and `Update` take from the client each frame.
#[derive(Debug, Clone, Copy)]
pub struct FrameInput {
    /// `GetClientTime()`.
    pub time: f32,
    /// `v_angles` while spectating in a mode other than 3, else `gHUD.m_vecAngles`.
    pub view_angles: Vec3,
    /// `GetViewAngles()`: `cl.viewangles` (the rain streak's yaw).
    pub cl_viewangles: Vec3,
    /// The local player entity's origin (the sort key and the grass fade, not the camera).
    pub player_origin: Vec3,
    /// Projection · modelview of this frame's view, column-major, clip depth 0..w.
    pub view_projection: [f32; 16],
    /// `g_flGravity`: `sv_gravity` as `HUD_TempEntUpdate` cached it.
    pub gravity: f32,
    /// `g_Environment.m_flOldTime` (the snowflake jitter step).
    pub weather_old_time: f32,
}

/// The particle manager (`IParticleMan_Active` with its `CMiniMem` pool).
#[derive(Debug, Clone, Default)]
pub struct ParticleMan {
    /// Live particles, oldest first (the active list walked from its tail).
    particles: Vec<Particle>,
    /// `g_flOldTime`: client time of the last `ProcessAll`.
    old_time: f32,
    /// `g_vViewAngles`.
    view_angles: Vec3,
    /// Snowflakes made so far (their stand-in heap addresses).
    serial: u32,
}

impl ParticleMan {
    /// `ResetParticles`: every particle goes at once, without `Die()`.
    pub fn reset(&mut self) {
        self.particles.clear();
    }

    /// Particles alive.
    pub fn len(&self) -> usize {
        self.particles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn particles(&self) -> &[Particle] {
        &self.particles
    }

    /// `RequestNewMemBlock` would succeed (outside `ProcessAll`).
    pub(crate) fn can_alloc(&self) -> bool {
        let ok = self.particles.len() < MAX_PARTICLES;
        if !ok {
            log::debug!("Particleman is out of memory, too many particles");
        }
        ok
    }

    /// Adds a particle made after `can_alloc` said yes.
    pub(crate) fn push(&mut self, p: Particle) {
        self.particles.push(p);
    }

    /// A stand-in for a new snowflake's block address: blocks are 304 bytes apart in a pool
    /// the size of the particle limit [INFERENCE: the base address].
    pub(crate) fn next_address(&mut self) -> f32 {
        self.serial = self.serial.wrapping_add(1);
        (0x0B00_0000 + 304 * (self.serial % MAX_PARTICLES as u32)) as f32
    }

    /// `CreateSmokePop`: the 18 moving puffs of a detonation; the one at yaw 270 later spawns
    /// the inside ring.
    pub fn smoke_pop(&mut self, fx: &mut Effects, env: &dyn Env, time: f32, origin: Vec3) {
        let up = [0.0, 0.0, 1.0];
        let mut pop = |this: &mut Self, yaw: f32, lift: f32, spawn_inside: bool| {
            let (fwd, _, _) = angle_vectors([0.0, yaw, 0.0]);
            let vel = add(scale(fwd, 375.0), scale(up, lift));
            if let Some(p) = gas_smoke(fx, env, time, this.particles.len(), origin, vel, false, spawn_inside) {
                this.particles.push(p);
            }
        };
        for lift in [165.0, 265.0] {
            for i in 0..8 {
                pop(self, i as f32 * 45.0, lift, false);
            }
        }
        pop(self, 45.0, 120.0, false);
        pop(self, 270.0, 120.0, true);
    }

    /// `SetVariables` (angles only while the clock moves) and `Update` → `ProcessAll`: newest
    /// first, visibility (last frame's state), think unless paused, death; then the visible
    /// particles far → near. Particles made meanwhile wait for the next frame.
    pub fn update(&mut self, fx: &mut Effects, env: &dyn ParticleEnv, input: &FrameInput) -> Vec<Quad> {
        let time = input.time;
        let paused = time == self.old_time;
        if !paused {
            self.view_angles = input.view_angles;
        }
        let frustum = Frustum::new(&input.view_projection);
        let mut visible = Vec::new();
        let mut dead = vec![false; self.particles.len()];
        let mut dead_count = 0;
        let mut ctx = ThinkCtx {
            t: time,
            old_time: self.old_time,
            gravity: input.gravity,
            cl_viewangles: input.cl_viewangles,
            weather_old_time: input.weather_old_time,
            player_origin: input.player_origin,
            fx,
            env,
            live: self.particles.len(),
            spawned: Vec::new(),
        };
        let total = self.particles.len();
        for i in (0..total).rev() {
            let p = &mut self.particles[i];
            if p.visible(time, &frustum, env) {
                let d = sub(input.player_origin, p.origin);
                p.distance = ((d[0] * d[0] + 0.0) + d[1] * d[1]) + d[2] * d[2];
                visible.push(i);
            }
            if !paused {
                ctx.live = total - dead_count;
                p.think(&mut ctx);
            }
            if p.die_time != 0.0 && time >= p.die_time {
                dead[i] = true;
                dead_count += 1;
            }
        }
        let spawned = std::mem::take(&mut ctx.spawned);
        visible.sort_by(|&a, &b| self.particles[b].distance.total_cmp(&self.particles[a].distance));
        let view_angles = self.view_angles;
        let quads = visible.iter().filter_map(|&i| self.particles[i].draw(time, view_angles, env)).collect();
        let mut index = 0;
        self.particles.retain(|_| {
            index += 1;
            !dead[index - 1]
        });
        self.particles.extend(spawned);
        self.old_time = time;
        quads
    }
}

/// `CreateGasSmoke(origin, vel, bInside, bSpawnInside, bBlowable 1)`; `live` particles already
/// use pool blocks.
#[allow(clippy::too_many_arguments)]
fn gas_smoke(fx: &mut Effects, env: &dyn Env, now: f32, live: usize, origin: Vec3, vel: Vec3, inside: bool, spawn_inside: bool) -> Option<Particle> {
    let Some(sprite) = env.model_by_name(GAS_PUFF) else {
        log::debug!("Couldn't load Sprite: {GAS_PUFF}");
        return None;
    };
    let size = fx.random_float(175.0, 225.0);
    if live >= MAX_PARTICLES {
        log::debug!("Particleman is out of memory, too many particles");
        return None;
    }
    let frame = fx.random_long(0, 1);
    let (avelocity, damping_time, die_time, scale_speed) = if inside {
        ([0.0, 0.0, fx.random_float(-0.8, 0.8)], 0.0, later(now, 30.0), 0.05)
    } else {
        ([0.0, 0.0, fx.random_float(-1.0, 1.0)], later(now, 0.001), later(now, 20.0), 0.1)
    };
    // m_flMass: read only by force fields, but the draw still advances the engine RNG.
    fx.random_float(1.0, 1.5);
    let grey = fx.random_float(155.0, 175.0);
    // InitializeSprite with normal (0,0,1): the puff starts at roll 1°.
    let kind = Kind::Smoke { damp_mod: false, spawn_inside, ring_origin: origin };
    let mut p = Particle::new(kind, origin, [0.0, 0.0, 1.0], Some(sprite.id), size, 190.0, now);
    p.frame = frame;
    p.collision_flags = collide::WORLD;
    p.render_flags = render::FRUSTUM_PLANE | render::PVS | render::LIGHT_NONE | render::FACE_PLAYER_ROTATEZ;
    p.rendermode = 4;
    p.scale_speed = scale_speed;
    p.die_time = die_time;
    p.damping_time = damping_time;
    p.avelocity = avelocity;
    p.velocity = vel;
    p.color = [grey; 3];
    Some(p)
}

/// `CreateGasInside(o)`: the 19 static inside puffs.
fn gas_inside(ctx: &mut ThinkCtx, o: Vec3) {
    let up = [0.0, 0.0, 1.0];
    let add_puff = |ctx: &mut ThinkCtx, at: Vec3| {
        let live = ctx.live + ctx.spawned.len();
        if let Some(p) = gas_smoke(ctx.fx, ctx.env, ctx.t, live, at, [0.0; 3], true, false) {
            ctx.spawned.push(p);
        }
    };
    let fwd = |yaw: f32| angle_vectors([0.0, yaw, 0.0]).0;
    for i in 0..8 {
        let dist = ctx.fx.random_float(90.0, 110.0);
        add_puff(ctx, add(add(o, scale(fwd(i as f32 * 45.0), dist)), scale(up, 45.0)));
    }
    for i in 0..8 {
        add_puff(ctx, add(add(o, scale(fwd(i as f32 * 45.0), 90.0)), scale(up, 60.0)));
    }
    for yaw in [45.0, 270.0] {
        add_puff(ctx, add(add(o, scale(fwd(yaw), 45.0)), scale(up, 55.0)));
    }
    add_puff(ctx, o);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use gs_effects::{EntityInfo, ModelInfo, ModelKind, Trace};

    /// A flat floor at z = 0 with water below z = −64 (from a pool rim at z = 0), sprites by
    /// name, full light, everything in the PVS.
    pub(crate) struct Floor {
        pub sky_above: bool,
    }

    impl Env for Floor {
        fn model(&self, _: i32) -> Option<ModelInfo> {
            None
        }
        fn model_by_name(&self, name: &str) -> Option<ModelInfo> {
            let id = name.len() as u32;
            Some(ModelInfo { id: ModelId(id), kind: ModelKind::Sprite, frames: 4, mins: [0.0; 3], maxs: [0.0; 3] })
        }
        fn entity(&self, _: i32) -> Option<EntityInfo> {
            None
        }
        fn trace(&self, start: Vec3, end: Vec3, _: i32) -> Trace {
            let miss = Trace { fraction: 1.0, endpos: end, normal: [0.0; 3], ent: -1, ent_info: 0 };
            if start[2] >= 0.0 && end[2] < 0.0 {
                let f = start[2] / (start[2] - end[2]);
                let endpos = [0, 1, 2].map(|i| start[i] + (end[i] - start[i]) * f);
                Trace { fraction: f, endpos, normal: [0.0, 0.0, 1.0], ent: 0, ent_info: 0 }
            } else if end[2] >= 4000.0 {
                Trace { fraction: 0.5, endpos: [end[0], end[1], 1000.0], normal: [0.0, 0.0, -1.0], ent: 0, ent_info: 0 }
            } else {
                miss
            }
        }
        fn box_in_pvs(&self, _: Vec3, _: Vec3) -> bool {
            true
        }
        fn max_clients(&self) -> i32 {
            32
        }
    }

    impl ParticleEnv for Floor {
        fn point_contents(&self, p: Vec3) -> i32 {
            if p[2] < -64.0 { CONTENTS_WATER } else { -1 }
        }
        fn trace_texture(&self, ent: i32, _: Vec3, end: Vec3) -> Option<String> {
            (ent == 0).then(|| if end[2] > 0.0 && self.sky_above { "sky".into() } else { "floor".into() })
        }
        fn light_at_point(&self, _: Vec3) -> Vec3 {
            [200.0; 3]
        }
        fn texture_type(&self, _: &str) -> u8 {
            b'C'
        }
        fn grass_textures(&self) -> bool {
            false
        }
    }

    /// A 90° view down +x (right-handed, y left, z up) with clip depth 0..w, column-major.
    pub(crate) fn view_down_x() -> [f32; 16] {
        let (n, f) = (4.0, 4096.0);
        let a = f / (f - n);
        // Row-major rows of P·V with view space x' = -y, y' = z, z' = x.
        let rows = [[0.0, -1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [a, 0.0, 0.0, -a * n], [1.0, 0.0, 0.0, 0.0]];
        let mut m = [0.0; 16];
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                m[4 * c + r] = *v;
            }
        }
        m
    }

    pub(crate) fn input(time: f32) -> FrameInput {
        FrameInput {
            time,
            view_angles: [0.0; 3],
            cl_viewangles: [0.0; 3],
            player_origin: [0.0; 3],
            view_projection: view_down_x(),
            gravity: 800.0,
            weather_old_time: time,
        }
    }

    #[test]
    fn frustum_cube_test_keeps_boxes_touching_the_view() {
        let frustum = Frustum::new(&view_down_x());
        assert!(frustum.cube_inside([100.0, 0.0, 0.0], 1.0));
        assert!(!frustum.cube_inside([-100.0, 0.0, 0.0], 1.0));
        assert!(!frustum.cube_inside([100.0, 300.0, 0.0], 10.0));
        // Partly inside the left plane still passes.
        assert!(frustum.cube_inside([100.0, 105.0, 0.0], 10.0));
        assert!(frustum.sphere_inside([100.0, 105.0, 0.0], 10.0));
        assert!(!frustum.point_inside([100.0, 105.0, 0.0]));
    }

    fn raindrop(origin: Vec3, now: f32) -> Particle {
        let mut p = Particle::new(Kind::RainDrop { touched: false }, origin, [0.0; 3], Some(ModelId(1)), 2.0, 1.0, now);
        p.stretch[1] = 40.0;
        p.velocity = [0.0, 0.0, -1000.0];
        p.render_flags = 0x1C;
        p.collision_flags |= 0x36020;
        p.rendermode = 4;
        p.die_time = now + 1.0;
        p
    }

    #[test]
    fn a_raindrop_dies_on_the_floor_and_leaves_a_splash_next_frame() {
        let (env, mut fx) = (Floor { sky_above: true }, Effects::new(&[0; 768], gs_effects::EngineRng::new(1)));
        let mut pm = ParticleMan::default();
        pm.update(&mut fx, &env, &input(10.0));
        pm.push(raindrop([100.0, 0.0, 30.0], 10.0));
        // 0.05 s at 1000 u/s crosses the floor at z = 0.
        let quads = pm.update(&mut fx, &env, &input(10.05));
        assert!(quads.is_empty(), "a drop killed by its collision is not drawn");
        assert_eq!(pm.len(), 1, "the drop died and its splash waits for the next frame");
        let Particle { kind: Kind::Base, rendermode, render_flags, num_frames, origin, .. } = &pm.particles[0] else { panic!("splash") };
        assert_eq!((*rendermode, *render_flags, *num_frames), (5, 0xCA, 3));
        assert!((origin[2] - 13.0).abs() < 0.01, "short of the floor (0.6 of the step) plus the normal: {}", origin[2]);
        let quads = pm.update(&mut fx, &env, &input(10.1));
        assert!(!quads.is_empty() && quads.iter().all(|q| q.additive), "the splash is drawn additive");
    }

    #[test]
    fn a_raindrop_entering_water_leaves_a_horizontal_ripple_at_the_surface() {
        // Water from z = −64 down, no floor in the way: the probe from 32 above the drop's
        // position in the water finds the surface by bisection.
        struct Pool;
        impl Env for Pool {
            fn model(&self, _: i32) -> Option<ModelInfo> {
                None
            }
            fn model_by_name(&self, n: &str) -> Option<ModelInfo> {
                Floor { sky_above: false }.model_by_name(n)
            }
            fn entity(&self, _: i32) -> Option<EntityInfo> {
                None
            }
            fn trace(&self, _: Vec3, end: Vec3, _: i32) -> Trace {
                Trace { fraction: 1.0, endpos: end, normal: [0.0; 3], ent: -1, ent_info: 0 }
            }
            fn box_in_pvs(&self, _: Vec3, _: Vec3) -> bool {
                true
            }
            fn max_clients(&self) -> i32 {
                32
            }
        }
        impl ParticleEnv for Pool {
            fn point_contents(&self, p: Vec3) -> i32 {
                if p[2] < -64.0 { CONTENTS_WATER } else { -1 }
            }
            fn trace_texture(&self, _: i32, _: Vec3, _: Vec3) -> Option<String> {
                None
            }
            fn light_at_point(&self, _: Vec3) -> Vec3 {
                [255.0; 3]
            }
            fn texture_type(&self, _: &str) -> u8 {
                b'C'
            }
            fn grass_textures(&self) -> bool {
                false
            }
        }
        let mut fx = Effects::new(&[0; 768], gs_effects::EngineRng::new(1));
        let mut pm = ParticleMan::default();
        pm.update(&mut fx, &Pool, &input(10.0));
        pm.push(raindrop([0.0, 0.0, -50.0], 10.0));
        pm.update(&mut fx, &Pool, &input(10.02));
        let r = &pm.particles[0];
        assert!(matches!(r.kind, Kind::Base) && r.scale_speed == 1.0 && r.fade_speed == 2.0, "a ripple");
        assert!((r.origin[2] - -64.0).abs() <= 4.0, "within the 4-unit bisection of the surface: {}", r.origin[2]);
        assert_eq!(r.angles[0], 270.0, "a horizontal quad");
    }
}
