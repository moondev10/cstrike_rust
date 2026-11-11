//! `CEnvironment g_Environment` (docs/re/weather.md): the map weather from `ReceiveW` (1 rain,
//! 2 snow), the wind, and the spawning of raindrops, snowflakes, wind puffs and grass pieces
//! into the particle manager. `Update` runs after the particle pass, so new particles are first
//! simulated and drawn the next frame.

use gs_effects::Effects;
use gs_pmove::math::{Vec3, add, angle_vectors, scale, sub};

use crate::particleman::{Kind, Particle, ParticleEnv, ParticleMan, TRACE_WORLD, later, length, render, vector_angles};

const RAIN: &str = "sprites/effects/rain.spr";
const SNOW: &str = "sprites/effects/snowflake.spr";
const GRASS: [&str; 2] = ["sprites/grass_01.spr", "sprites/grass_03.spr"];

/// What `CEnvironment::Update` reads from the client.
#[derive(Debug, Clone, Copy)]
pub struct WeatherInput {
    /// `GetClientTime()`.
    pub time: f32,
    /// `gHUD.m_vecOrigin`: the view entity's origin from the client data (not the eye).
    pub origin: Vec3,
    /// `g_iUser1`, `g_iUser2`.
    pub observer: (i32, i32),
    /// `cl_weather`.
    pub cl_weather: f32,
    /// `g_vPlayerVelocity` (the predicted velocity).
    pub player_velocity: Vec3,
}

/// `CEnvironment`.
#[derive(Debug, Clone, Default)]
pub struct Environment {
    /// `m_iWeatherType`: 1 rain, 2 snow, anything else none.
    pub weather_type: i32,
    grass_active: bool,
    weather_origin: Vec3,
    weather_time: f32,
    grass_origin: Vec3,
    /// Not reset by `Initialize`.
    grass_time: f32,
    wind: Vec3,
    desired_wind_direction: Vec3,
    /// Not reset by `Initialize`.
    desired_wind_speed: f32,
    next_wind_change_time: f32,
    /// The time of the last `Update` (not reset by `Initialize`).
    old_time: f32,
    /// Not reset by `Initialize`.
    ideal_yaw: f32,
    weather_value: f32,
}

/// `AngleMod`.
fn angle_mod(a: f32) -> f32 {
    if a < 0.0 {
        a + 360.0 * (1 - (a / -360.0) as i32) as f32
    } else if a >= 360.0 {
        a - 360.0 * (a / 360.0) as i32 as f32
    } else {
        a
    }
}

/// The client's π for `VecToYaw` (the double 3.1415 at 0x100d1c78).
#[allow(clippy::approx_constant)]
const TRUNCATED_PI: f64 = 3.1415;

/// `VecToYaw`: whole degrees of `atan2` with π taken as 3.1415, in `[0, 360)`.
fn vec_to_yaw(v: Vec3) -> f32 {
    let l = length(v);
    if l == 0.0 {
        return 0.0;
    }
    let (nx, ny) = (v[0] * (1.0 / l), v[1] * (1.0 / l));
    if ny == 0.0 && nx == 0.0 {
        return 0.0;
    }
    let yaw = (f64::from(ny).atan2(f64::from(nx)) * 180.0 / TRUNCATED_PI) as i32 as f32;
    if yaw < 0.0 { yaw + 360.0 } else { yaw }
}

/// The texture name starts with `sky` (case-sensitive).
fn is_sky(name: Option<String>) -> bool {
    name.is_some_and(|n| n.as_bytes().starts_with(b"sky"))
}

impl Environment {
    /// `MsgFunc_ReceiveW`: one byte, unchecked (a short read stores −1).
    pub fn receive_w(&mut self, data: &[u8]) {
        self.weather_type = data.first().map_or(-1, |&b| i32::from(b));
    }

    /// `g_Environment.m_flOldTime` (the snowflakes' jitter step).
    pub fn old_time(&self) -> f32 {
        self.old_time
    }

    /// `CEnvironment::Initialize` (from `InitHUD`): no weather until the next `ReceiveW`, grass
    /// if materials.txt has an `X` texture, and a random wind (y from the first draw).
    pub fn initialize(&mut self, fx: &mut Effects, env: &dyn ParticleEnv, time: f32, cl_weather: f32) {
        self.weather_origin = [0.0; 3];
        self.weather_time = 0.0;
        self.grass_origin = [0.0; 3];
        self.weather_type = 0;
        self.grass_active = env.grass_textures();
        let a = fx.random_float(-80.0, 80.0);
        let b = fx.random_float(-80.0, 80.0);
        self.wind = [b, a, 0.0];
        let c = fx.random_float(-80.0, 80.0);
        let d = fx.random_float(-80.0, 80.0);
        self.desired_wind_direction = [c, d, 0.0];
        self.next_wind_change_time = time;
        self.weather_value = cl_weather;
    }

    /// `CEnvironment::Update`: the weather origin, wind, grass, and a rain or snow burst when its
    /// timer is due (`!(m_flWeatherTime > now)`, as in the binary). Returns the clamped
    /// `cl_weather` to write back when it exceeded 3.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    pub fn update(&mut self, pm: &mut ParticleMan, fx: &mut Effects, env: &dyn ParticleEnv, input: &WeatherInput) -> Option<f32> {
        let now = input.time;
        let mut origin = add(input.origin, [0.0, 0.0, 36.0]);
        let (user1, user2) = input.observer;
        if user1 != 3 && user1 > 0
            && let Some(e) = env.entity(user2)
        {
            origin = add(e.origin, [0.0, 0.0, 36.0]);
        }
        let clamped = (input.cl_weather > 3.0).then_some(3.0);
        self.weather_value = clamped.unwrap_or(input.cl_weather);
        self.weather_origin = origin;
        self.update_wind(fx, now);
        if self.grass_active {
            self.update_grass(pm, fx, env, now);
        }
        if !(self.weather_time > now) {
            match self.weather_type {
                2 => self.update_snow(pm, fx, env, now, input.player_velocity),
                1 => self.update_rain(pm, fx, env, now, input.player_velocity, input.origin[2]),
                _ => {}
            }
        }
        self.old_time = now;
        clamped
    }

    /// `UpdateWind`: a new target every 15–30 s; the speed jumps, the direction turns at 5°/s
    /// towards the whole-degree ideal yaw.
    fn update_wind(&mut self, fx: &mut Effects, now: f32) {
        let dt = (f64::from(now) - f64::from(self.old_time)) as f32;
        if now >= self.next_wind_change_time {
            let a = fx.random_float(-80.0, 80.0);
            let b = fx.random_float(-80.0, 80.0);
            self.desired_wind_direction = [a, b, 0.0];
            self.next_wind_change_time = later(fx.random_float(15.0, 30.0), f64::from(now));
            self.desired_wind_speed = length(self.desired_wind_direction);
            self.ideal_yaw = vec_to_yaw(self.desired_wind_direction);
        }
        let l = length(self.wind);
        let d = if l == 0.0 { [0.0, 0.0, 1.0] } else { scale(self.wind, 1.0 / l) };
        let mut ang = vector_angles(d);
        let mut y = angle_mod(ang[1]);
        if y != self.ideal_yaw {
            let speed = dt * 0.5 * 10.0;
            let mut step = self.ideal_yaw - y;
            if self.ideal_yaw > y {
                if step >= 180.0 {
                    step -= 360.0;
                }
            } else if step <= -180.0 {
                step += 360.0;
            }
            y += if step > 0.0 { speed.min(step) } else { (-speed).max(step) };
            ang[1] = angle_mod(y);
        }
        let (fwd, _, _) = angle_vectors(ang);
        self.wind = scale(fwd, self.desired_wind_speed);
    }

    /// The shared burst: `ceil(150·W)` attempts in a box of half-width `r` around the weather
    /// origin, 100–300 up, shifted by a second of player velocity; `make` runs for every spawn
    /// point whose upward trace ends on a `sky` texture.
    fn burst(&self, fx: &mut Effects, env: &dyn ParticleEnv, r: f32, velocity: Vec3, mut make: impl FnMut(&mut Effects, Vec3)) {
        let len = length(velocity);
        let dir = if len == 0.0 { [0.0, 0.0, 1.0] } else { scale(velocity, 1.0 / len) };
        let off = scale(dir, len);
        let o = self.weather_origin;
        let (mut j, mut sky) = (0, 0);
        while 150.0 * self.weather_value > j as f32 {
            j += 1;
            let mut f = fx.random_float(0.0, r);
            if fx.random_long(0, 5) < 3 {
                f = -f;
            }
            let x = o[0] + f;
            let y = (f64::from(fx.random_float(-r, r)) + f64::from(o[1])) as f32;
            let z = (f64::from(fx.random_float(100.0, 300.0)) + f64::from(o[2])) as f32;
            let p = [x + off[0], y + off[1], z + off[2]];
            let end = [p[0], p[1], 8000.0];
            let tr = env.trace(p, end, TRACE_WORLD);
            if is_sky(env.trace_texture(tr.ent, tr.endpos, end)) {
                sky += 1;
                make(fx, p);
            }
        }
        log::debug!("[weather] type {}: {j} attempts, {sky} under sky, wind {:?}", self.weather_type, self.wind);
    }

    /// `UpdateRain`: raindrops, and a wind puff attempt on every 15th sky hit after the 15th.
    fn update_rain(&mut self, pm: &mut ParticleMan, fx: &mut Effects, env: &dyn ParticleEnv, now: f32, velocity: Vec3, hud_z: f32) {
        self.weather_time = later(now, 0.3);
        let (wind, sprite) = (self.wind, env.model_by_name(RAIN).map(|m| m.id));
        let mut wind_counter = 0;
        self.burst(fx, env, 400.0, velocity, |fx, p| {
            if sprite.is_some() && pm.can_alloc() {
                let mut d = Particle::new(Kind::RainDrop { touched: false }, p, [0.0; 3], sprite, 2.0, 1.0, now);
                d.stretch[1] = 40.0;
                d.velocity[0] = (f64::from(fx.random_float(1.0, 2.0)) * f64::from(wind[0])) as f32;
                d.velocity[1] = (f64::from(fx.random_float(1.0, 2.0)) * f64::from(wind[1])) as f32;
                d.velocity[2] = fx.random_float(-500.0, -1800.0);
                d.render_flags = render::FRUSTUM_PLANE | render::PVS | render::LIGHT_NONE;
                d.collision_flags |= 0x36020;
                d.rendermode = 4;
                d.die_time = now + 1.0;
                pm.push(d);
            }
            if wind_counter == 15 {
                let v = [p[0], p[1], hud_z];
                if env.box_in_pvs(v, v) {
                    let up = [p[0], p[1], 8000.0];
                    let tr = env.trace(v, up, TRACE_WORLD);
                    if is_sky(env.trace_texture(tr.ent, tr.endpos, up)) {
                        let tr = env.trace(v, [p[0], p[1], -8000.0], TRACE_WORLD);
                        wind_particle(pm, fx, env, now, wind, tr.endpos);
                    }
                }
                wind_counter = 1;
            } else {
                wind_counter += 1;
            }
        });
    }

    /// `UpdateSnow`: snowflakes.
    fn update_snow(&mut self, pm: &mut ParticleMan, fx: &mut Effects, env: &dyn ParticleEnv, now: f32, velocity: Vec3) {
        self.weather_time = later(now, 0.7);
        let (wind, sprite) = (self.wind, env.model_by_name(SNOW));
        self.burst(fx, env, 300.0, velocity, |fx, p| {
            // CreateSnowFlake.
            let Some(sprite) = sprite else { return };
            if !pm.can_alloc() {
                return;
            }
            let size = fx.random_float(2.0, 2.5);
            let address = pm.next_address();
            let kind = Kind::SnowFlake { spiral: false, touched: false, spiral_time: 0.0, address };
            let mut s = Particle::new(kind, p, [0.0; 3], Some(sprite.id), size, 1.0, now);
            s.num_frames = sprite.frames;
            s.velocity[0] = (f64::from(wind[0]) / f64::from(fx.random_float(1.0, 2.0))) as f32;
            s.velocity[1] = (f64::from(wind[1]) / f64::from(fx.random_float(1.0, 2.0))) as f32;
            s.velocity[2] = fx.random_float(-100.0, -200.0);
            s.collision_flags |= 0x20;
            let r = f64::from(fx.random_float(0.0, 1.0));
            if r < 0.1 {
                s.velocity[0] *= 0.5;
                s.velocity[1] *= 0.5;
            } else if r < 0.2 {
                s.velocity[2] = -65.0;
            } else if r < 0.3 {
                s.velocity[2] = -75.0;
            }
            s.render_flags = render::FRUSTUM_SPHERE | render::PVS | render::LIGHT_NONE | render::FACE_PLAYER;
            s.color = [128.0; 3];
            s.rendermode = 5;
            s.bounce = 0.0;
            s.die_time = later(now, 3.0);
            let spiral = fx.random_long(0, 1) != 0;
            let k = fx.random_long(2, 4);
            s.kind = Kind::SnowFlake { spiral, touched: false, spiral_time: later(now, f64::from(k as f32)), address };
            pm.push(s);
        });
    }

    /// `UpdateGrass`: every 0.3 s once the player moved 100 units, pieces in two strips
    /// 800–850 units to either side (world x) on `X`-material ground.
    fn update_grass(&mut self, pm: &mut ParticleMan, fx: &mut Effects, env: &dyn ParticleEnv, now: f32) {
        if self.grass_time > now || length(sub(self.weather_origin, self.grass_origin)) < 100.0 {
            return;
        }
        self.grass_origin = self.weather_origin;
        self.grass_time = later(now, 0.3_f32.into());
        let g = self.grass_origin;
        let mut k = 0;
        while 35.0 * self.weather_value > k as f32 {
            k += 1;
            let mut f = fx.random_float(800.0, 850.0);
            if fx.random_long(0, 5) < 3 {
                f = -f;
            }
            let origin = [g[0] + f, (f64::from(fx.random_float(-800.0, 800.0)) + f64::from(g[1])) as f32, g[2] + 36.0];
            let end = [origin[0], origin[1], -8000.0];
            let tr = env.trace(origin, end, TRACE_WORLD);
            let yaw = fx.random_float(0.0, 359.0);
            let Some(name) = env.trace_texture(tr.ent, tr.endpos, end) else { continue };
            let mut n = name.as_str();
            if n.starts_with(['-', '+']) {
                n = n.get(2..).unwrap_or("");
            }
            if n.starts_with(['{', '!', '~', ' ']) {
                n = &n[1..];
            }
            if env.texture_type(&gs_common::bytestr::truncate_encoded(n, 12)) != b'X' {
                continue;
            }
            let pick = if fx.random_long(0, 1) != 0 { GRASS[0] } else { GRASS[1] };
            let Some(sprite) = env.model_by_name(pick) else { continue };
            let size = fx.random_float(60.0, 75.0) * 0.5;
            if !pm.can_alloc() {
                continue;
            }
            let pos = add(tr.endpos, [0.0, 0.0, 10.0]);
            let mut p = Particle::new(Kind::Grass, pos, [0.0, yaw, 0.0], Some(sprite.id), size, 1.0, now);
            p.render_flags = render::FRUSTUM_PLANE | render::PVS | render::LIGHT_COLOR;
            p.rendermode = 4;
            p.color = [125.0; 3];
            p.die_time = (f64::from(now) + f64::from(99999.0_f32)) as f32;
            pm.push(p);
        }
    }
}

/// `CreateWindParticle`: a grey puff drifting with the wind, 10 units above the ground below.
fn wind_particle(pm: &mut ParticleMan, fx: &mut Effects, env: &dyn ParticleEnv, now: f32, wind: Vec3, ground: Vec3) {
    let Some(sprite) = env.model_by_name(crate::particleman::GAS_PUFF) else { return };
    if !pm.can_alloc() {
        return;
    }
    let origin = add(ground, [0.0, 0.0, 10.0]);
    let size = fx.random_float(50.0, 75.0);
    let mut p = Particle::new(Kind::Wind, origin, [0.0; 3], Some(sprite.id), size, 1.0, now);
    p.num_frames = sprite.frames;
    // m_iFrame = RandomLong(n/2, n), then overwritten with 0: only the draw remains.
    fx.random_long(sprite.frames / 2, sprite.frames);
    p.velocity[0] = (f64::from(wind[0]) / f64::from(fx.random_float(1.0, 2.0))) as f32;
    p.velocity[1] = (f64::from(wind[1]) / f64::from(fx.random_float(1.0, 2.0))) as f32;
    p.collision_flags |= 0x20;
    if f64::from(fx.random_float(0.0, 1.0)) < 0.1 {
        p.velocity[0] *= 0.5;
        p.velocity[1] *= 0.5;
    }
    p.rendermode = 4;
    p.render_flags = render::FRUSTUM_SPHERE | render::PVS | render::LIGHT_NONE | render::FACE_PLAYER_ROTATEZ;
    p.avelocity[2] = fx.random_float(-1.0, 1.0);
    p.scale_speed = 0.4;
    p.bounce = 0.0;
    p.color = [128.0; 3];
    p.die_time = later(now, 6.0);
    pm.push(p);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::particleman::FrameInput;
    use crate::particleman::tests::{Floor, input};

    #[test]
    fn wind_angle_helpers_match_the_client() {
        assert_eq!(angle_mod(-90.0), 270.0);
        assert_eq!(angle_mod(725.0), 5.0);
        // 3.1415 instead of π makes 45° come out as 45.0013 → 45 after truncation.
        assert_eq!(vec_to_yaw([1.0, 1.0, 0.0]), 45.0);
        assert_eq!(vec_to_yaw([0.0, -1.0, 0.0]), 270.0);
    }

    #[test]
    fn rain_bursts_only_under_sky_every_0_3_s_and_ends_on_the_floor() {
        let mut fx = Effects::new(&[0; 768], gs_effects::EngineRng::new(1));
        let mut pm = ParticleMan::default();
        let mut w = Environment::default();
        let covered = Floor { sky_above: false };
        w.initialize(&mut fx, &covered, 10.0, 1.0);
        w.receive_w(&[1]);
        let wi = |time| WeatherInput { time, origin: [0.0; 3], observer: (0, 0), cl_weather: 1.0, player_velocity: [0.0; 3] };
        w.update(&mut pm, &mut fx, &covered, &wi(10.0));
        assert!(pm.is_empty(), "nothing spawns under a roof");
        let open = Floor { sky_above: true };
        w.update(&mut pm, &mut fx, &open, &wi(10.1));
        assert!(pm.is_empty(), "the next burst waits 0.3 s");
        w.update(&mut pm, &mut fx, &open, &wi(10.31));
        // 150 drops plus a wind puff on sky hits 16, 31, ..., 136, 151 > 150: 9 puffs.
        assert_eq!(pm.len(), 150 + 9);
        assert_eq!(w.update(&mut pm, &mut fx, &open, &WeatherInput { cl_weather: 5.0, ..wi(10.32) }), Some(3.0), "cl_weather is clamped to 3");
        // Drops fall at 500–1800 u/s from 136–336 units above the floor (origin + 36 + 100..300):
        // after a simulated second every drop has hit the floor or timed out.
        pm.update(&mut fx, &open, &input(10.32));
        for i in 1..=20 {
            pm.update(&mut fx, &open, &input(10.32 + i as f32 * 0.05));
        }
        let kinds = pm.particles().iter().map(|p| std::mem::discriminant(&p.kind));
        assert!(kinds.clone().all(|k| k != std::mem::discriminant(&Kind::RainDrop { touched: false })), "every drop is gone");
        assert!(pm.len() >= 9, "the wind puffs live 6 s");
    }

    #[test]
    fn snowflakes_stop_on_the_floor_and_fade_out() {
        let mut fx = Effects::new(&[0; 768], gs_effects::EngineRng::new(7));
        let mut pm = ParticleMan::default();
        let mut w = Environment::default();
        let open = Floor { sky_above: true };
        w.initialize(&mut fx, &open, 5.0, 0.1);
        w.receive_w(&[2]);
        w.update(&mut pm, &mut fx, &open, &WeatherInput { time: 5.0, origin: [0.0; 3], observer: (0, 0), cl_weather: 0.1, player_velocity: [0.0; 3] });
        assert_eq!(pm.len(), 15, "ceil(150 · 0.1) attempts, all under sky");
        // 136–336 units to fall at 65–200 u/s: the fast ones land within the 3 s life.
        let mut landed = 0;
        for i in 0..=40 {
            let t = 5.0 + i as f32 * 0.1;
            pm.update(&mut fx, &open, &FrameInput { weather_old_time: t - 0.1, ..input(t) });
            landed = landed.max(pm.particles().iter().filter(|p| p.velocity == [0.0; 3] && p.fade_speed == 0.0).count());
            assert!(pm.particles().iter().all(|p| p.origin[2] >= 0.0), "flakes never pass the floor");
        }
        assert!(landed > 0, "some flakes froze on the floor");
        assert!(pm.is_empty(), "3 s of life, or 0.5 s after landing");
    }
}
