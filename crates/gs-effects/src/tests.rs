use crate::tents::{MAX_TEMP_ENTITIES, flags};
use crate::{Effects, EntityInfo, Env, ModelId, ModelInfo, ModelKind, Trace};

/// A world with a floor at z = 0 and a configurable PVS answer.
struct Floor {
    pvs: bool,
}

const SPRITE: ModelInfo = ModelInfo { id: ModelId(1), kind: ModelKind::Sprite, frames: 4, mins: [-4.0; 3], maxs: [4.0; 3] };
const STUDIO: ModelInfo = ModelInfo { id: ModelId(2), kind: ModelKind::Studio, frames: 1, mins: [0.0; 3], maxs: [0.0; 3] };

impl Env for Floor {
    fn model(&self, index: i32) -> Option<ModelInfo> {
        match index {
            1 => Some(SPRITE),
            2 => Some(STUDIO),
            _ => None,
        }
    }

    fn model_by_name(&self, _: &str) -> Option<ModelInfo> {
        Some(SPRITE)
    }

    fn entity(&self, _: i32) -> Option<EntityInfo> {
        None
    }

    fn trace(&self, start: [f32; 3], end: [f32; 3], _: i32) -> Trace {
        if start[2] >= 0.0 && end[2] < 0.0 {
            let f = start[2] / (start[2] - end[2]);
            let endpos = [0, 1, 2].map(|i| start[i] + (end[i] - start[i]) * f);
            Trace { fraction: f, endpos, normal: [0.0, 0.0, 1.0], ent: 0, ent_info: 0 }
        } else {
            Trace { fraction: 1.0, endpos: end, normal: [0.0; 3], ent: -1, ent_info: 0 }
        }
    }

    fn box_in_pvs(&self, _: [f32; 3], _: [f32; 3]) -> bool {
        self.pvs
    }

    fn max_clients(&self) -> i32 {
        32
    }
}

fn effects() -> Effects {
    let mut fx = Effects::new(&[0u8; 768], crate::EngineRng::new(1234));
    fx.set_time(10.0, 9.99);
    fx
}

/// Advances `fx` by `dt` and runs the temp entity update.
fn step(fx: &mut Effects, env: &dyn Env, dt: f64) {
    let t = fx.time + dt;
    fx.set_time(t, t - dt);
    fx.update_tents(env, 800.0);
}

#[test]
fn falling_model_lands_and_stops() {
    let env = Floor { pvs: true };
    let mut fx = effects();
    let i = fx.temp_model([0.0, 0.0, 50.0], [0.0, 0.0, 0.0], [10.0, 20.0, 30.0], 30.0, Some(STUDIO), 0).unwrap_or(usize::MAX);
    assert!(fx.tent_mut(i).is_some());
    for _ in 0..200 {
        step(&mut fx, &env, 0.01);
    }
    let t = fx.tents().next().map(|t| (t.origin, t.flags, t.angles, t.velocity));
    let Some((origin, f, angles, vel)) = t else { panic!("tent vanished") };
    assert!(origin[2] >= 0.0 && origin[2] < 1.0, "rests on the floor: {origin:?}");
    assert_eq!(f & (flags::GRAVITY | flags::COLLIDEWORLD), 0, "landing clears motion flags");
    assert_eq!((angles[0], angles[2]), (0.0, 0.0), "lands flat");
    assert_eq!(vel, [0.0; 3]);
}

#[test]
fn fadeout_lowers_alpha_after_death_then_frees() {
    let env = Floor { pvs: true };
    let mut fx = effects();
    let i = fx.temp_sprite([0.0, 0.0, 10.0], [0.0; 3], 1.0, Some(SPRITE), 5, 0, 1.0, 0.1, flags::FADEOUT).unwrap_or(usize::MAX);
    assert!(fx.tent_mut(i).is_some());
    step(&mut fx, &env, 0.05);
    assert_eq!(fx.tents().next().map(|t| t.renderamt), Some(255), "alive: untouched");
    // fadeSpeed 0.5: alpha 255·(1 + life·0.5) once past `die`.
    step(&mut fx, &env, 0.55);
    let amt = fx.tents().next().map_or(0, |t| t.renderamt);
    assert!((180..=200).contains(&amt), "half a second late: {amt}");
    step(&mut fx, &env, 2.0);
    assert_eq!(fx.tents().count(), 0, "faded out");
}

#[test]
fn tents_outside_the_pvs_die_unless_persistent() {
    let env = Floor { pvs: false };
    let mut fx = effects();
    fx.temp_sprite([0.0, 0.0, 10.0], [0.0; 3], 1.0, Some(SPRITE), 5, 0, 1.0, 5.0, 0);
    fx.temp_sprite([0.0, 0.0, 10.0], [0.0; 3], 1.0, Some(SPRITE), 5, 0, 1.0, 5.0, flags::PERSIST);
    step(&mut fx, &env, 0.01);
    assert_eq!(fx.visible_tents().count(), 0);
    step(&mut fx, &env, 0.01);
    let left: Vec<u32> = fx.tents().map(|t| t.flags & flags::PERSIST).collect();
    assert_eq!(left, vec![flags::PERSIST]);
}

#[test]
fn high_priority_recycles_the_newest_low_priority_tent() {
    let mut fx = effects();
    for k in 0..MAX_TEMP_ENTITIES {
        assert!(fx.tent_alloc([k as f32, 0.0, 0.0], Some(SPRITE)).is_some());
    }
    assert!(fx.tent_alloc([0.0; 3], Some(SPRITE)).is_none(), "pool full");
    let i = fx.tent_alloc_high([0.0, 0.0, 99.0], Some(SPRITE));
    assert_eq!(i, Some(MAX_TEMP_ENTITIES - 1));
    assert_eq!(fx.tents().filter(|t| t.priority == 1).count(), 1);
    assert_eq!(fx.tents().count(), MAX_TEMP_ENTITIES);
}

#[test]
fn particle_pool_is_shared_and_bounded() {
    let mut fx = effects();
    fx.particle_explosion([0.0; 3]);
    fx.particle_explosion([0.0; 3]);
    fx.particle_explosion([0.0; 3]);
    assert_eq!(fx.particles.len(), crate::DEFAULT_PARTICLES);
    fx.spark_streaks([0.0; 3], 10, -10, 10);
    assert!(fx.tracers.is_empty(), "tracers share the exhausted pool");
}

#[test]
fn fire_particles_burn_out() {
    let mut fx = effects();
    fx.rocket_trail([0.0; 3], [30.0, 0.0, 0.0], crate::particles::trail::ROCKET);
    assert_eq!(fx.particles.len(), 10);
    let view = crate::ViewParams {
        origin: [0.0; 3],
        forward: [1.0, 0.0, 0.0],
        right: [0.0, -1.0, 0.0],
        up: [0.0, 0.0, 1.0],
        world_to_screen: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        gravity: 800.0,
    };
    let mut out = Vec::new();
    // ramp += 5·ft per frame; dead (die = -1) once ramp reaches 6, freed the frame after.
    for _ in 0..20 {
        let t = fx.time + 0.1;
        fx.set_time(t, t - 0.1);
        fx.draw_particles(&view, &mut out);
    }
    assert!(fx.particles.is_empty());
}

/// TE_DLIGHT as a KZ server plugin sends it every packet (radius 12, life 1, decay 192 on the
/// wire): radius 120, gone after 0.1 s, shrinking 1920 units a second until then.
#[test]
fn te_dlight_shrinks_by_its_decay_and_goes_dark_at_zero_or_after_its_life() {
    let env = Floor { pvs: true };
    let parse = |wire: &[u8]| gs_protocol::tempent::parse(&mut gs_protocol::MsgReader::new(wire)).expect("TE_DLIGHT");
    let mut fx = effects();
    fx.temp_entity(&env, &parse(&[27, 0, 0, 0, 0, 0, 0, 12, 255, 255, 255, 1, 192]));
    let dl = fx.dlights[0];
    assert_eq!((dl.radius, dl.decay, dl.die), (120.0, 1920.0, (10.0f64 + 0.1) as f32));
    assert!(dl.is_active(10.0));
    fx.set_time(10.05, 10.0);
    fx.decay_lights();
    assert!((fx.dlights[0].radius - 24.0).abs() < 1e-3, "{}", fx.dlights[0].radius);
    fx.set_time(10.07, 10.05);
    fx.decay_lights();
    assert_eq!(fx.dlights[0].radius, 0.0, "clamped at zero");
    assert!(!fx.dlights[0].is_active(10.07), "dark before its die time");

    // Without decay the radius holds until cl.time passes die.
    let mut fx = effects();
    fx.temp_entity(&env, &parse(&[27, 0, 0, 0, 0, 0, 0, 30, 255, 0, 0, 1, 0]));
    fx.set_time(10.1, 10.05);
    fx.decay_lights();
    assert_eq!(fx.dlights[0].radius, 300.0);
    fx.set_time(10.11, 10.1);
    fx.decay_lights();
    assert_eq!(fx.dlights[0].radius, 0.0);
}

/// `CL_AllocDlight`: a keyed light takes (and clears) its key's slot, an unkeyed one the first
/// expired slot, and with every slot lit slot 0 is overwritten.
#[test]
fn alloc_dlight_reuses_the_key_then_an_expired_slot_then_slot_zero() {
    let mut fx = effects();
    for (i, dl) in fx.dlights.iter_mut().enumerate() {
        if i != 3 {
            *dl = crate::DLight { radius: 50.0, die: 20.0, ..crate::DLight::default() };
        }
    }
    assert_eq!(fx.alloc_dlight_slot(0), 3, "the only expired slot");
    fx.dlights[3].die = 20.0;
    assert_eq!(fx.alloc_dlight_slot(0), 0, "every slot lit");
    assert_eq!(fx.dlights[0].radius, 0.0, "the slot is cleared");
    fx.dlights[7].key = 42;
    assert_eq!(fx.alloc_dlight_slot(42), 7);
    assert_eq!(fx.dlights[7], crate::DLight { key: 42, ..crate::DLight::default() });
}
