//! GoldSrc engine client (no rendering, no input): connects to a server, completes sign-on and
//! downloads, keeps the entity snapshot ring current, predicts the local player and sends user
//! commands.

pub mod auth;
mod client;
pub mod cmdclock;
pub mod custom;
pub mod demo;
pub mod hpak;
mod http;
pub mod entities;
pub mod events;
pub mod level;
pub mod postrun;
pub mod predict;
mod predlog;
pub mod resources;
pub mod slist;
pub mod viewfx;
pub mod voice;

pub use client::{Client, ClientConfig, ClientEvent, NetStats, PlayerDecal, State, UiEvent};
pub use cmdclock::{CommandClock, CommandFrame};
pub use entities::{ClEntity, LinkKind};
pub use predict::{LocalState, SimState};
