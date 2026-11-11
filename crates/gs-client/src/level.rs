//! Per-level client state (`cl`): precache lists, light styles, movevars, player infos, clock.

use std::sync::Arc;

use gs_common::info::InfoString;
use gs_fs::FileSystem;
use gs_protocol::messages::{MoveVars, ResourceList, resource_flags};
use gs_protocol::structs::EntityState;
use gs_studio::StudioModel;

use crate::resources::kind;

pub const MAX_CLIENTS: usize = 32;
pub const MAX_LIGHTSTYLES: usize = 64;

#[derive(Debug, Clone, Default)]
pub struct PlayerInfo {
    pub userid: i32,
    pub info: String,
    pub name: String,
    pub model: String,
    pub topcolor: i32,
    pub bottomcolor: i32,
    /// `svc_pings`: latency in ms and packet loss.
    pub ping: i32,
    pub packet_loss: i32,
    /// `hashedcdkey`: the 16 bytes of `svc_updateuserinfo` (`GetPlayerUniqueID`).
    pub hashed_cdkey: [u8; 16],
}

impl PlayerInfo {
    fn from_info(userid: i32, info: &str) -> Self {
        let parsed = InfoString::from_raw(info, 512);
        let int = |k: &str| parsed.get(k).and_then(|v| v.parse().ok()).unwrap_or(0);
        Self {
            userid,
            info: info.to_owned(),
            name: parsed.get("name").unwrap_or("").to_owned(),
            model: parsed.get("model").unwrap_or("").to_owned(),
            topcolor: int("topcolor"),
            bottomcolor: int("bottomcolor"),
            ping: 0,
            packet_loss: 0,
            hashed_cdkey: [0; 16],
        }
    }
}

#[derive(Debug, Clone)]
pub struct Level {
    /// `cl.intermission`: 0, 1 `svc_intermission`, 2 `svc_finale`, 3 `svc_cutscene`
    /// (`CL_ClearState` puts it back to 0).
    pub intermission: u8,
    /// `cl.model_precache`: index → name (`maps/x.bsp`, `*1`, `models/...`); index 0 unused.
    pub models: Vec<Option<String>>,
    pub sounds: Vec<Option<String>>,
    pub events: Vec<Option<String>>,
    pub decals: Vec<Option<String>>,
    /// `svc_lightstyle` patterns.
    pub lightstyles: Vec<String>,
    pub movevars: MoveVars,
    pub players: Vec<PlayerInfo>,
    /// `svc_setview` entity (0 = own player).
    pub view_entity: i16,
    /// Pending `svc_setangle` / `svc_addangle` for the view (consumed by the input layer).
    pub set_angles: Option<[f32; 3]>,
    pub add_yaw: f32,
    /// Loaded studio models by precache index (`Mod_ForName` at registration).
    pub studio: Vec<Option<Arc<StudioModel>>>,
    /// `cl.weaponsequence` / `cl.weaponstarttime` (0 = restart at the next view model draw) /
    /// `cl.viewent.curstate.body`, from `svc_weaponanim` and `pfnWeaponAnim`.
    pub weapon_sequence: i32,
    pub weapon_starttime: f64,
    pub weapon_body: i32,
    /// `cl.sf`: the screen fade (inside `cl`, so `CL_ClearState` clears it).
    pub fade: crate::viewfx::ScreenFade,
    /// `cl.crosshairangle` from `svc_crosshairangle` (pitch, yaw; roll never written).
    pub crosshair_angle: [f32; 3],
    /// The world model's entity lump (`model->entities`).
    pub entity_lump: String,
}

impl Default for Level {
    fn default() -> Self {
        Self {
            intermission: 0,
            models: Vec::new(),
            sounds: Vec::new(),
            events: Vec::new(),
            decals: vec![None; 512],
            lightstyles: vec![String::new(); MAX_LIGHTSTYLES],
            movevars: MoveVars::default(),
            players: vec![PlayerInfo::default(); MAX_CLIENTS],
            view_entity: 0,
            set_angles: None,
            add_yaw: 0.0,
            studio: Vec::new(),
            weapon_sequence: 0,
            weapon_starttime: 0.0,
            weapon_body: 0,
            fade: crate::viewfx::ScreenFade::default(),
            crosshair_angle: [0.0; 3],
            entity_lump: String::new(),
        }
    }
}

fn place(list: &mut Vec<Option<String>>, index: u16, name: &str) {
    let i = usize::from(index);
    if list.len() <= i {
        list.resize(i + 1, None);
    }
    list[i] = Some(name.to_owned());
}

impl Level {
    pub fn set_resources(&mut self, list: &ResourceList) {
        self.models.clear();
        self.sounds.clear();
        self.events.clear();
        for r in &list.resources {
            match r.kind {
                kind::MODEL => place(&mut self.models, r.index, &r.name),
                kind::SOUND => place(&mut self.sounds, r.index, &r.name),
                kind::EVENTSCRIPT => place(&mut self.events, r.index, &r.name),
                // Server decal names (`Draw_DecalSetName(nIndex, name)`); custom sprays carry
                // RES_CUSTOM and stay out of the table.
                kind::DECAL if r.flags & resource_flags::CUSTOM == 0 => place(&mut self.decals, r.index, &r.name),
                _ => {}
            }
        }
    }

    pub fn model_name(&self, index: i32) -> Option<&str> {
        usize::try_from(index).ok().and_then(|i| self.models.get(i)?.as_deref())
    }

    pub fn set_player(&mut self, slot: u8, userid: i32, info: &str, hashed_cdkey: [u8; 16]) {
        if let Some(p) = self.players.get_mut(usize::from(slot)) {
            *p = if info.is_empty() { PlayerInfo::default() } else { PlayerInfo::from_info(userid, info) };
            p.hashed_cdkey = hashed_cdkey;
        }
    }

    /// `cle_GetPlayerUniqueID(i)`: the hashed cdkey of player `i` (1-based) when it has a
    /// userinfo and a name.
    pub fn player_unique_id(&self, i: i32) -> Option<[u8; 16]> {
        let p = usize::try_from(i).ok().filter(|&i| i >= 1).and_then(|i| self.players.get(i - 1))?;
        (!p.info.is_empty() && !p.name.is_empty()).then_some(p.hashed_cdkey)
    }

    pub fn set_decal(&mut self, index: u8, name: &str) {
        place(&mut self.decals, u16::from(index), name);
    }

    /// `svc_weaponanim` / `EV_WeaponAnimation`: restart the view model on `sequence`.
    pub fn set_weapon_anim(&mut self, sequence: i32, body: i32) {
        self.weapon_sequence = sequence;
        self.weapon_starttime = 0.0;
        self.weapon_body = body;
    }

    /// Loads every precached `.mdl` (with `T.mdl` textures and sequence groups). The engine
    /// turns a non-v10 file into an empty model without an error; unreadable files stay `None`.
    pub fn load_studio_models(&mut self, fs: &FileSystem) {
        self.studio = self
            .models
            .iter()
            .map(|name| {
                let name = name.as_deref().filter(|n| n.to_ascii_lowercase().ends_with(".mdl"))?;
                match gs_assets::load_studio(fs, name) {
                    Ok(m) => Some(Arc::new(m)),
                    Err(e) => {
                        log::warn!("{name}: {e}");
                        None
                    }
                }
            })
            .collect();
    }

    pub fn studio_model(&self, index: i32) -> Option<&Arc<StudioModel>> {
        usize::try_from(index).ok().and_then(|i| self.studio.get(i)?.as_ref())
    }

    /// `R_StudioEstimateFrame` for the entity's current sequence at `time` (0 for non-studio).
    pub fn estimate_frame(&self, s: &EntityState, time: f64) -> f32 {
        let Some(model) = self.studio_model(s.modelindex()) else { return 0.0 };
        let Some(seq) = usize::try_from(s.sequence()).ok().and_then(|i| model.sequences.get(i)) else { return 0.0 };
        let animtime = f64::from(s.animtime());
        let dfdt = if animtime <= time { ((time - animtime) as f32) * s.framerate() * seq.fps } else { 0.0 };
        gs_studio::anim::estimate_frame(seq, s.frame(), dfdt)
    }
}

/// `cl.time` management: advances with frame time and is pulled toward server time
/// (docs/re/engine-core.md §4, `CL_AdjustClock`).
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub time: f64,
    pub old_time: f64,
    delta: f64,
    /// `cl_clockreset`.
    pub reset: f64,
    /// `cl_fixtimerate` in ms.
    pub fix_rate: f64,
}

impl Default for Clock {
    fn default() -> Self {
        Self { time: 0.0, old_time: 0.0, delta: 0.0, reset: 0.1, fix_rate: 7.5 }
    }
}

impl Clock {
    pub fn advance(&mut self, frametime: f64) {
        self.old_time = self.time;
        self.time += frametime;
    }

    /// `svc_time` arrived with server time `mtime`.
    pub fn server_time(&mut self, mtime: f64, singleplayer: bool) {
        if singleplayer {
            self.time = mtime;
        }
        if (self.time - mtime).abs() > self.reset {
            self.time = mtime;
            self.delta = 0.0;
        } else if self.time != mtime {
            self.delta = self.time - mtime;
        }
        self.old_time = self.old_time.min(self.time);
    }

    /// `CL_AdjustClock`: at most `cl_fixtimerate` ms of correction per frame (the caller has put
    /// a negative rate back to 7.5).
    pub fn adjust(&mut self) {
        let rate = self.fix_rate;
        if self.delta != 0.0 && rate != 0.0 && self.delta.abs() >= 0.001 {
            let ms = (self.delta * 1000.0) as i32;
            let sign = if ms < 0 { -1.0 } else { 1.0 };
            let adjust = -sign * rate.min(f64::from(ms.unsigned_abs())) / 1000.0;
            if adjust.abs() < self.delta.abs() {
                self.delta += adjust;
                self.time += adjust;
            }
        }
        self.old_time = self.old_time.min(self.time);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_resets_on_large_error_and_slews_small_error() {
        let mut c = Clock::default();
        c.advance(5.0);
        c.server_time(10.0, false);
        assert_eq!(c.time, 10.0, "more than cl_clockreset apart: snap");
        c.advance(0.05);
        c.server_time(10.0, false);
        assert!((c.time - 10.05).abs() < 1e-9, "small error is kept and slewed");
        c.adjust();
        assert!((c.time - 10.0425).abs() < 1e-9, "7.5 ms per frame toward server time: {}", c.time);
    }
}
