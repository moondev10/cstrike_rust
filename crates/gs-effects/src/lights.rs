//! `cl_dlights[32]` / `cl_elights[64]` (docs/re/render.md §A.4 and §D.10).

use crate::Effects;
use crate::vec::Vec3;

/// `dlight_t`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DLight {
    pub origin: Vec3,
    pub radius: f32,
    pub color: [u8; 3],
    pub die: f32,
    /// Radius lost per second.
    pub decay: f32,
    pub minlight: f32,
    pub key: i32,
    pub dark: bool,
}

impl DLight {
    /// Lit at `time` (`R_PushDlights`: `cl.time <= die && radius != 0`).
    pub fn is_active(&self, time: f64) -> bool {
        time <= f64::from(self.die) && self.radius != 0.0
    }
}

/// `CL_AllocDlight` / `CL_AllocElight`: the slot keyed `key` (when non-zero), else the first
/// expired slot, else slot 0; always cleared.
fn alloc(lights: &mut [DLight], key: i32, time: f64) -> usize {
    let slot = (key != 0)
        .then(|| lights.iter().position(|l| l.key == key))
        .flatten()
        .or_else(|| lights.iter().position(|l| time > f64::from(l.die)))
        .unwrap_or(0);
    lights[slot] = DLight { key, ..DLight::default() };
    slot
}

/// `CL_DecayLights` for one array.
fn decay(lights: &mut [DLight], time: f64, dt: f32) {
    for l in lights.iter_mut().filter(|l| l.radius != 0.0) {
        if time > f64::from(l.die) {
            l.radius = 0.0;
        } else if l.decay != 0.0 {
            l.radius = (l.radius - l.decay * dt).max(0.0);
        }
    }
}

impl Effects {
    /// `CL_AllocDlight(key)`.
    pub fn alloc_dlight(&mut self, key: i32) -> &mut DLight {
        let i = self.alloc_dlight_slot(key);
        &mut self.dlights[i]
    }

    /// `CL_AllocDlight(key)` as a `cl_dlights` index (for callers that keep the pointer).
    pub fn alloc_dlight_slot(&mut self, key: i32) -> usize {
        alloc(&mut self.dlights, key, self.time)
    }

    /// `CL_AllocElight(key)`.
    pub fn alloc_elight(&mut self, key: i32) -> &mut DLight {
        let i = alloc(&mut self.elights, key, self.time);
        &mut self.elights[i]
    }

    /// `CL_DecayLights`: expire and shrink dlights and elights by this frame's time.
    pub fn decay_lights(&mut self) {
        let dt = (self.time - self.oldtime) as f32;
        decay(&mut self.dlights, self.time, dt);
        decay(&mut self.elights, self.time, dt);
    }

    /// Light at `origin` dying `life` seconds from now.
    pub(crate) fn add_dlight(&mut self, origin: Vec3, radius: f32, color: [u8; 3], life: f32, decay: f32) {
        let die = (self.time + f64::from(life)) as f32;
        let dl = self.alloc_dlight(0);
        *dl = DLight { origin, radius, color, die, decay, ..*dl };
    }
}
