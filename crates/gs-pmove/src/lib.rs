//! Counter-Strike player movement: the `pm_shared` code shared by client prediction and the
//! server (docs/re/movement.md Part A) on top of the engine's pmove trace services (Part B).

pub mod math;
mod pm;
pub mod studio_hull;
pub mod textures;
pub mod trace;

pub use pm::{PlayerMove, PmContext, PmSound, StuckState, buttons, clip_velocity, flags, movetype, pm_move};
pub use textures::TextureTypes;
pub use studio_hull::StudioPhys;
pub use trace::{PLAYER_MAXS, PLAYER_MINS, Physent, PmTrace};

#[cfg(test)]
mod tests;
