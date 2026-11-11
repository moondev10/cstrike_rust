//! `CHudSpectator`'s observer logic (docs/re/client-hud.md §8, docs/re/hltv-client.md §8–§9):
//! observer modes and targets — chosen locally on an HLTV connection, by the server otherwise —
//! the observer buttons and commands, the director messages with the roaming director camera
//! and its waypoint path, and the HLTV per-player FOVs.

use gs_common::token::com_parse;
use gs_draw::titles::TextMessage;

use super::{Hud, HudEngine, MsgReader};
use crate::view::obs;

/// `m_HUDMessages` slots.
const HUD_MESSAGES: usize = 8;
/// `m_CamPath` size.
const MAX_WAYPOINTS: usize = 32;
/// `CHAN_BODY`.
const CHAN_BODY: i32 = 4;

/// `cameraWayPoint_t`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct WayPoint {
    time: f32,
    position: [f32; 3],
    angle: [f32; 3],
    fov: f32,
    flags: i32,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(v: [f32; 3], s: f32) -> [f32; 3] {
    v.map(|c| c * s)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn length(v: [f32; 3]) -> f32 {
    dot(v, v).sqrt()
}

/// `Vector::Normalize`: a zero vector becomes (0, 0, 1).
fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = length(v);
    if l == 0.0 { [0.0, 0.0, 1.0] } else { scale(v, 1.0 / l) }
}

/// `NormalizeAngles`: one wrap step into [-180, 180].
fn normalize_angles(a: [f32; 3]) -> [f32; 3] {
    a.map(|v| {
        if v > 180.0 {
            v - 360.0
        } else if v < -180.0 {
            v + 360.0
        } else {
            v
        }
    })
}

/// `VectorAngles`.
fn vector_angles(f: [f32; 3]) -> [f32; 3] {
    if f[0] == 0.0 && f[1] == 0.0 {
        return [if f[2] > 0.0 { 90.0 } else { 270.0 }, 0.0, 0.0];
    }
    let mut yaw = f[1].atan2(f[0]).to_degrees();
    if yaw < 0.0 {
        yaw += 360.0;
    }
    let mut pitch = f[2].atan2((f[0] * f[0] + f[1] * f[1]).sqrt()).to_degrees();
    if pitch < 0.0 {
        pitch += 360.0;
    }
    [pitch, yaw, 0.0]
}

/// `CInterpolation`: a quadratic Bezier between two waypoints through a centre point derived
/// from their neighbours, with linear angles and FOV.
#[derive(Debug, Clone, Copy, Default)]
struct Interpolation {
    start_point: [f32; 3],
    end_point: [f32; 3],
    center: [f32; 3],
    start_angle: [f32; 3],
    end_angle: [f32; 3],
    start_fov: f32,
    end_fov: f32,
    smooth_start: bool,
    smooth_end: bool,
}

impl Interpolation {
    fn set_waypoints(&mut self, prev: Option<[f32; 3]>, s: [f32; 3], e: [f32; 3], next: Option<[f32; 3]>) {
        self.start_point = s;
        self.end_point = e;
        match (prev, next) {
            (None, None) => self.center = scale(add(s, e), 0.5),
            (Some(p), None) => {
                let a = norm(sub(p, s));
                let b = sub(e, s);
                let d = length(b) * 0.5;
                let c = norm(sub(norm(b), a));
                self.center = add(s, scale(c, d));
            }
            (None, Some(n)) => {
                let a = sub(s, e);
                let d = length(a) * 0.5;
                let b = norm(sub(n, e));
                let c = norm(sub(norm(a), b));
                self.center = add(e, scale(c, d));
            }
            (Some(p), Some(n)) => {
                // The midpoint of the closest points of the lines S + s·c and E + u·d; a
                // degenerate pair keeps the old centre.
                let c = sub(norm(sub(e, s)), norm(sub(p, s)));
                let d = sub(norm(sub(s, e)), norm(sub(n, e)));
                let nn = cross(c, d);
                let m = cross(nn, d);
                let den = dot(m, c);
                if den != 0.0 {
                    let w = sub(e, s);
                    let sv = dot(m, w) / den;
                    let u = -dot(c, cross(nn, w)) / den;
                    self.center = scale(add(add(s, scale(c, sv)), add(e, scale(d, u))), 0.5);
                }
            }
        }
    }

    fn interpolate(&self, t: f32) -> ([f32; 3], [f32; 3], f32) {
        let t = if self.smooth_start && self.smooth_end {
            t * (1.0 - (t - 1.0) * (t - 1.0)) + (1.0 - t) * t * t
        } else if self.smooth_start {
            t * t
        } else if self.smooth_end {
            1.0 - (t - 1.0) * (t - 1.0)
        } else {
            t
        };
        let point = [0, 1, 2].map(|k| {
            (1.0 - t) * (1.0 - t) * self.start_point[k] + 2.0 * t * (1.0 - t) * self.center[k] + t * t * self.end_point[k]
        });
        let angle = [0, 1, 2].map(|k| {
            let mut d = self.end_angle[k] - self.start_angle[k];
            if d > 180.0 {
                d -= 360.0;
            } else if d < -180.0 {
                d += 360.0;
            }
            self.start_angle[k] + d * t
        });
        (point, normalize_angles(angle), self.start_fov + t * (self.end_fov - self.start_fov))
    }
}

/// What the observer logic asks of the engine and the view this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ObserverRequests {
    /// `SetViewAngles` (`cl.viewangles`).
    pub set_view_angles: Option<[f32; 3]>,
    /// `V_ResetChaseCam`.
    pub reset_chase_cam: bool,
}

/// `CHudSpectator`'s state.
#[derive(Debug, Clone)]
pub struct Observer {
    pub last_primary: i32,
    pub last_secondary: i32,
    /// `m_iObserverFlags` (director bits for the chase cameras).
    pub observer_flags: i32,
    /// `m_FOV`.
    pub fov: f32,
    chase_entity: i32,
    interpolating: bool,
    waypoints: [WayPoint; MAX_WAYPOINTS],
    num_waypoints: usize,
    waypoint: i32,
    interp: Interpolation,
    last_hud_message: usize,
    /// `m_iSpectatorNumber` (director STATUS).
    pub spectator_number: i32,
    /// `m_cameraOrigin` / `m_cameraAngles` (the map's start position).
    pub camera_origin: [f32; 3],
    pub camera_angles: [f32; 3],
    chat_enabled: bool,
    next_input: f32,
    /// `vJumpOrigin` / `vJumpAngles` and `iJumpSpectator`.
    pub jump_origin: [f32; 3],
    pub jump_angles: [f32; 3],
    pub jump: bool,
    /// `m_lastAutoDirector`: the auto-director value at the last `Draw` (-1 before the first).
    last_autodirector: f32,
    /// `m_mapZoom` / `m_mapOrigin`.
    pub map_zoom: f32,
    pub map_origin: [f32; 3],
    /// `m_zoomDelta` / `m_moveDelta` (map-free zoom and pan per frame).
    pub zoom_delta: f32,
    pub move_delta: f32,
    /// `CHud::m_PlayerFOV` (HLTV in-eye FOV per player).
    pub player_fov: [i32; 64],
    pub requests: ObserverRequests,
}

impl Default for Observer {
    fn default() -> Self {
        Self {
            last_primary: 0,
            last_secondary: 0,
            observer_flags: 0,
            fov: 90.0,
            chase_entity: 0,
            interpolating: false,
            waypoints: [WayPoint::default(); MAX_WAYPOINTS],
            num_waypoints: 0,
            waypoint: 0,
            interp: Interpolation::default(),
            last_hud_message: 0,
            spectator_number: 0,
            camera_origin: [0.0; 3],
            camera_angles: [0.0; 3],
            chat_enabled: true,
            next_input: 0.0,
            jump_origin: [0.0; 3],
            jump_angles: [0.0; 3],
            jump: false,
            last_autodirector: -1.0,
            map_zoom: 1.0,
            map_origin: [0.0; 3],
            zoom_delta: 0.0,
            move_delta: 0.0,
            player_fov: [0; 64],
            requests: ObserverRequests::default(),
        }
    }
}

/// `UTIL_FindEntityInMap(name)`: the first entity of that class in the world's entity lump;
/// `origin`/`angle(s)` keys of earlier entities stay in the outputs when the match lacks them.
pub fn find_entity_in_map(lump: &str, name: &str) -> Option<([f32; 3], [f32; 3])> {
    let vec = |s: &str| {
        let mut v = [0.0f32; 3];
        for (o, t) in v.iter_mut().zip(s.split_whitespace()) {
            *o = t.parse().unwrap_or(0.0);
        }
        v
    };
    let mut data = lump.as_bytes();
    let (mut origin, mut angle) = ([0.0f32; 3], [0.0f32; 3]);
    loop {
        let (token, rest) = com_parse(data, false)?;
        data = rest;
        if token.first() != Some(&b'{') {
            return None;
        }
        let mut found = false;
        loop {
            let (key, rest) = com_parse(data, false)?;
            data = rest;
            if key.first() == Some(&b'}') {
                break;
            }
            let key = String::from_utf8_lossy(&key).trim_end_matches(' ').to_owned();
            let (value, rest) = com_parse(data, false)?;
            data = rest;
            let value = String::from_utf8_lossy(&value).into_owned();
            match key.as_str() {
                "classname" if value == name => found = true,
                "angle" => {
                    let y: f32 = value.trim().parse().unwrap_or(0.0);
                    angle = if y >= 0.0 {
                        [0.0, y, 0.0]
                    } else if y as i32 == -1 {
                        [-90.0, 0.0, 0.0]
                    } else {
                        [90.0, 0.0, 0.0]
                    };
                }
                "angles" => angle = vec(&value),
                "origin" => origin = vec(&value),
                _ => {}
            }
        }
        if found {
            return Some((origin, angle));
        }
    }
}

impl Hud {
    /// `IsActivePlayer`: an entity that is a live player other than the local one, with a name.
    fn active_player(&self, i: i32, eng: &dyn HudEngine) -> Option<crate::view::SpecEntity> {
        let e = eng.entity(i)?;
        let local = self.local_index(eng).map_or(-1, |l| l as i32);
        let named = eng.player_info(i).is_some_and(|p| !p.name.is_empty());
        (e.player && e.solid != 0 && i != local && named).then_some(e)
    }

    /// `CHudSpectator::SetModes(main, inset)`.
    pub fn set_modes(&mut self, main: i32, inset: i32, eng: &mut dyn HudEngine) {
        let main = if main == -1 { self.state.user1 } else { main };
        let inset = if inset == -1 { eng.cvar("spec_pip") as i32 } else { inset };
        let pip = if self.viewport.force_camera != 0 { 0.0 } else { inset as f32 };
        eng.cvar_set_value("spec_pip", pip);
        if !(1..=6).contains(&main) {
            eng.console_print("Invalid spectator mode.\n");
            return;
        }
        self.observer.interpolating = false;
        self.observer.chase_entity = 0;
        if main == self.state.user1 {
            self.update_spectator_panel(eng);
            return;
        }
        if !eng.spectate_only() {
            // A normal client asks the server; free chase turns the auto-director off unless
            // the server forces chase cameras.
            if main == obs::CHASE_FREE && self.viewport.force_camera != 2 {
                self.set_autodirector(0.0, eng);
            }
            eng.server_cmd(&format!("specmode {main}"));
            return;
        }
        if self.state.user2 == 0 && main != obs::ROAMING {
            let primary = self.observer.last_primary;
            if self.active_player(primary, eng).is_some() {
                self.state.user2 = primary;
                self.state.user3 = self.observer.last_secondary;
            } else {
                self.find_next_player(false, eng);
            }
        }
        self.state.user1 = main;
        match main {
            obs::CHASE_FREE => self.set_autodirector(0.0, eng),
            obs::ROAMING => {
                if self.state.user2 != 0
                    && let Some(e) = eng.entity(self.state.user2)
                {
                    // V_GetInEyePos.
                    let mut origin = e.origin;
                    let mut angles = [e.angles[0] * -3.0, e.angles[1], e.angles[2]];
                    if e.solid != 0 {
                        origin[2] += if e.usehull == 1 { 12.0 } else { 17.0 };
                    } else {
                        origin[2] -= 8.0;
                        angles[2] = 80.0;
                    }
                    self.observer.jump_origin = origin;
                    self.observer.jump_angles = angles;
                    self.observer.requests.set_view_angles = Some(angles);
                    self.observer.jump = true;
                }
            }
            obs::MAP_FREE | obs::MAP_CHASE => {
                self.observer.map_zoom = self.overview.zoom;
                self.observer.map_origin = self.overview.origin;
            }
            _ => {}
        }
        self.update_spectator_panel(eng);
    }

    /// The raw `m_autoDirector->value` write (the cvar string keeps its old text).
    fn set_autodirector(&mut self, value: f32, eng: &mut dyn HudEngine) {
        eng.cvar_set_value("spec_autodirector_internal", value);
    }

    /// `FindNextPlayer(bReverse)`: the server's `follownext`, or the local cycle over 1..64.
    pub fn find_next_player(&mut self, reverse: bool, eng: &mut dyn HudEngine) {
        if !eng.spectate_only() {
            eng.server_cmd(&format!("follownext {}", u8::from(reverse)));
            return;
        }
        let start = if self.state.user2 != 0 { self.state.user2 } else { 1 };
        self.state.user2 = 0;
        let mut i = start;
        let mut found = None;
        loop {
            i += if reverse { -1 } else { 1 };
            if i > 64 {
                i = 1;
            }
            if i <= 0 {
                i = 64;
            }
            if let Some(e) = self.active_player(i, eng) {
                self.state.user2 = i;
                found = Some(e);
                break;
            }
            if i == start {
                break;
            }
        }
        self.jump_to_target(found, eng);
    }

    /// `FindPlayer(name)`: the server's `follow`, or the first live player 1..63 with that name.
    pub fn find_player(&mut self, name: &str, eng: &mut dyn HudEngine) {
        if !eng.spectate_only() {
            // snprintf(127): at most 126 bytes, no newline.
            let mut text = format!("follow \"{name}\"");
            gs_common::bytestr::truncate_encoded_in_place(&mut text, 126);
            eng.server_cmd(&text);
            return;
        }
        self.state.user2 = 0;
        let mut found = None;
        for i in 1..64 {
            if let Some(e) = self.active_player(i, eng)
                && eng.player_info(i).is_some_and(|p| p.name.eq_ignore_ascii_case(name))
            {
                self.state.user2 = i;
                found = Some(e);
                break;
            }
        }
        self.jump_to_target(found, eng);
    }

    /// The tail of `FindNextPlayer`/`FindPlayer`: the jump vectors at the target (raw entity
    /// angles) or the map start, then the panel.
    fn jump_to_target(&mut self, found: Option<crate::view::SpecEntity>, eng: &mut dyn HudEngine) {
        match found {
            Some(e) => {
                self.observer.jump_origin = e.origin;
                self.observer.jump_angles = e.angles;
            }
            None => {
                if eng.cvar("developer") != 0.0 {
                    eng.console_print("No observer targets.\n");
                }
                self.observer.jump_origin = self.observer.camera_origin;
                self.observer.jump_angles = self.observer.camera_angles;
            }
        }
        self.update_spectator_panel(eng);
        self.observer.jump = true;
    }

    /// `CHudSpectator::HandleButtonsDown(bit)` (from `IN_*Down` after the key state changed).
    pub fn handle_buttons_down(&mut self, buttons: i32, eng: &mut dyn HudEngine) {
        let t = eng.client_time();
        if self.intermission || self.state.user1 == 0 {
            return;
        }
        let pip = eng.cvar("spec_pip");
        if eng.demo_playing_back() && !eng.spectate_only() {
            return;
        }
        if self.observer.next_input > t {
            return;
        }
        if buttons & 4 != 0 && !self.spectator_gui.bar {
            self.show_spectator_gui_bar(eng);
        }
        let inset = if buttons & 0x20 != 0 {
            let mut n = (eng.cvar("spec_pip") + 1.0) as i32;
            if self.state.user1 > 4 {
                if n >= 3 {
                    n = 0;
                }
            } else if n > 4 {
                n = 0;
            } else if n == 1 {
                n = 3;
            }
            eng.cvar_set_value("spec_pip", n as f32);
            self.spectator_inset(n != 0);
            n
        } else {
            pip as i32
        };
        let mut main = -1;
        if eng.spectate_only() {
            if buttons & 2 != 0 {
                main = match self.state.user1 {
                    1 => 2,
                    2 => 4,
                    4 => 3,
                    3 => 5,
                    5 => 6,
                    _ => 2,
                };
            }
            if buttons & 0x801 != 0 {
                self.find_next_player(buttons & 0x800 != 0, eng);
                if self.state.user1 == obs::ROAMING {
                    self.observer.requests.set_view_angles = Some(self.observer.jump_angles);
                    self.observer.jump = true;
                    self.health.last_pointed_at = self.state.user2;
                    if let Some(p) = usize::try_from(self.state.user2).ok().and_then(|i| self.players.get_mut(i)) {
                        p.showhealth = self.time + 3.0;
                    }
                }
                self.set_autodirector(0.0, eng);
            }
        }
        self.set_modes(main, inset, eng);
        if self.state.user1 == obs::MAP_FREE {
            if buttons & 8 != 0 {
                self.observer.zoom_delta = 0.01;
            }
            if buttons & 0x10 != 0 {
                self.observer.zoom_delta = -0.01;
            }
            if buttons & 0x200 != 0 {
                self.observer.move_delta = -12.0;
            }
            if buttons & 0x400 != 0 {
                self.observer.move_delta = 12.0;
            }
        }
        self.observer.next_input = t + 0.2;
    }

    /// `CHudSpectator::HandleButtonsUp(bit)`: the map-free zoom/pan stops.
    pub fn handle_buttons_up(&mut self, buttons: i32) {
        if !self.spectator_gui_visible() {
            return;
        }
        if buttons & 0x18 != 0 {
            self.observer.zoom_delta = 0.0;
        }
        if buttons & 0x600 != 0 {
            self.observer.move_delta = 0.0;
        }
    }

    /// `CheckSettings`: the inset mode for the main mode, the chat filter for HLTV, and no PIP
    /// for T/CT players watching in-eye.
    pub(super) fn check_settings(&mut self, eng: &mut dyn HudEngine) {
        let mut pip = eng.cvar("spec_pip") as i32;
        if self.state.user1 <= 4 {
            if pip == 1 || pip == 2 {
                pip = 3;
            }
        } else if pip >= 3 {
            pip = 1;
        }
        if self.intermission {
            pip = 0;
        }
        let mut pip = pip as f32;
        let chat = eng.cvar("hud_saytext_internal") != 0.0;
        if chat != self.observer.chat_enabled {
            self.observer.chat_enabled = chat;
            if eng.spectate_only() {
                eng.server_cmd(&format!("ignoremsg {}", i32::from(!chat)));
            }
        }
        let team = self.local_index(eng).map_or(0, |i| self.players[i].teamnumber);
        if (team == 1 || team == 2) && self.state.user1 == obs::IN_EYE && pip != 0.0 {
            let y = (eng.screen().height as f32 * 0.35) as i32;
            let now = self.time;
            self.centre.print("#Spec_No_PIP", &[], y, [255, 180, 30], now, eng);
            pip = 0.0;
        }
        eng.cvar_set_value("spec_pip", pip);
        // SpectatorGUIEnableInsetView: no outline while watching ourselves.
        let local = self.local_index(eng).map_or(-1, |l| l as i32);
        self.spectator_inset(local != self.state.user2 && pip != 0.0);
    }

    /// `CHudSpectator::Draw` while observing: an auto-director change goes to the server
    /// (`spec_set_ad %f`) and swaps the chase modes; the map-free zoom and the map pan (per HUD
    /// frame); `spec_mode_internal` follows the mode; the player names on a map view.
    pub(super) fn observer_think(&mut self, eng: &mut dyn HudEngine) {
        if self.state.user1 == 0 {
            return;
        }
        let ad = eng.cvar("spec_autodirector_internal");
        if ad != self.observer.last_autodirector {
            self.observer.last_autodirector = ad;
            eng.client_cmd(&format!("spec_set_ad {ad:.6}\n"));
            if ad == 0.0 && self.state.user1 == obs::CHASE_LOCKED {
                self.set_modes(obs::CHASE_FREE, -1, eng);
            } else if ad != 0.0 && self.state.user1 == obs::CHASE_FREE {
                self.set_modes(obs::CHASE_LOCKED, -1, eng);
            }
        }
        if self.observer.zoom_delta != 0.0 && self.state.user1 == obs::MAP_FREE {
            let o = &mut self.observer;
            o.map_zoom = (o.map_zoom + o.zoom_delta).clamp(0.5, 3.0);
        }
        if self.observer.move_delta != 0.0 && self.state.user1 != obs::ROAMING {
            // AngleVectors(v_angles, NULL, right, NULL), normalised.
            let (sy, cy) = self.v_angles[1].to_radians().sin_cos();
            let (sp, cp) = self.v_angles[0].to_radians().sin_cos();
            let (sr, cr) = self.v_angles[2].to_radians().sin_cos();
            let right = [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp];
            let len = (right[0] * right[0] + right[1] * right[1] + right[2] * right[2]).sqrt();
            if len > 0.0 {
                let o = &mut self.observer;
                for (m, r) in o.map_origin.iter_mut().zip(right) {
                    *m += r / len * o.move_delta;
                }
            }
        }
        if eng.cvar("spec_mode_internal") as i32 != self.state.user1 {
            eng.cvar_set("spec_mode_internal", &self.state.user1.to_string());
        }
        self.overview_names(eng);
    }

    /// `spec_mode <main> [inset]`.
    pub(super) fn cmd_spec_mode(&mut self, args: &gs_common::token::Args, eng: &mut dyn HudEngine) {
        let atoi = |s: &str| s.trim().parse::<i32>().unwrap_or(0);
        match args.argc() {
            0 | 1 => eng.console_print("usage:  spec_mode <Main Mode> [<Inset Mode>]\n"),
            2 => self.set_modes(atoi(args.arg(1)), -1, eng),
            3 if atoi(args.arg(2)) == -1 => {
                let inset = i32::from(eng.cvar("spec_pip") == 0.0);
                self.set_modes(-1, inset, eng);
            }
            3 => self.set_modes(atoi(args.arg(1)), atoi(args.arg(2)), eng),
            _ => {}
        }
    }

    /// `spec_toggleinset`: never turns the inset off (map 1↔2, world 3↔4).
    pub(super) fn cmd_spec_toggleinset(&mut self, eng: &mut dyn HudEngine) {
        let mut n = (eng.cvar("spec_pip") + 1.0) as i32;
        if self.state.user1 >= obs::MAP_FREE {
            if n > 2 {
                n = 1;
            }
        } else if n > 4 || n == 1 {
            n = 3;
        }
        self.set_modes(-1, n, eng);
    }

    /// `spec_decal` (HLTV only): `drc_spray` at what the view looks at within 128 units.
    pub(super) fn cmd_spec_decal(&mut self, eng: &mut dyn HudEngine, v_origin: [f32; 3], v_angles: [f32; 3]) {
        if !eng.spectate_only() {
            return;
        }
        let (sy, cy) = v_angles[1].to_radians().sin_cos();
        let (sp, cp) = v_angles[0].to_radians().sin_cos();
        let fwd = [cp * cy * 128.0, cp * sy * 128.0, -sp * 128.0];
        let end = add(v_origin, fwd);
        let tr = eng.trace_line(v_origin, end, -1);
        if tr.fraction != 1.0 {
            let p = tr.endpos;
            eng.server_cmd(&format!("drc_spray {:.2} {:.2} {:.2} {}", p[0], p[1], p[2], tr.ent));
        }
    }

    /// `InitHUDData` (from `InitHUD`) and `Reset` (from `ResetHUD`).
    pub(super) fn observer_init_hud_data(&mut self, eng: &mut dyn HudEngine) {
        let o = &mut self.observer;
        o.last_primary = 0;
        o.last_secondary = 0;
        o.last_hud_message = 0;
        o.spectator_number = 0;
        o.jump = false;
        o.next_input = 0.0;
        self.state.user2 = 0;
        self.state.user1 = 0;
        self.observer_reset(eng);
        self.state.user2 = 0;
        self.fov = eng.cvar("default_fov") as i32;
    }

    /// `CHudSpectator::Reset`: a new level's overview, the start position as the jump target,
    /// then the configured mode.
    pub(super) fn observer_reset(&mut self, eng: &mut dyn HudEngine) {
        self.overview_reset(eng);
        let o = &mut self.observer;
        o.fov = 90.0;
        o.interpolating = false;
        o.chase_entity = 0;
        let lump = eng.map_entities().unwrap_or_default();
        let start = ["trigger_camera", "info_player_start", "info_player_deathmatch", "info_player_coop"]
            .iter()
            .find_map(|c| find_entity_in_map(&lump, c))
            .unwrap_or(([0.0; 3], [0.0; 3]));
        o.camera_origin = start.0;
        o.camera_angles = start.1;
        o.jump_origin = start.0;
        o.jump_angles = start.1;
        o.jump = true;
        let mode = eng.cvar("spec_mode_internal") as i32;
        let pip = eng.cvar("spec_pip") as i32;
        self.set_modes(mode, pip, eng);
    }

    /// `DuckMessage("#Spec_Duck")`: the hint text through `MessageAdd`.
    pub(super) fn duck_message(&mut self, text: &str, eng: &mut dyn HudEngine) {
        let msg = TextMessage {
            effect: 2,
            color1: [40, 255, 40, 200],
            color2: [0, 255, 0, 200],
            x: -1.0,
            y: 0.7,
            fadein: 0.01,
            fadeout: 0.7,
            holdtime: 6.0,
            fxtime: 0.07,
            name: "Spec_Duck".into(),
            message: gs_common::bytestr::truncate_encoded(text, 127).into_owned(),
        };
        self.message_add(msg, eng);
    }

    /// The Newfont `CHudMessage::MessageAdd(client_textmessage_t*)`: `#Spec_Duck` is not shown
    /// to T/CT players on a normal server.
    fn message_add(&mut self, msg: TextMessage, eng: &mut dyn HudEngine) {
        let team = self.local_index(eng).map_or(0, |i| self.players[i].teamnumber);
        if msg.message == "#Spec_Duck" && (team == 1 || team == 2) && !eng.spectate_only() {
            return;
        }
        let now = self.time;
        self.messages.add_message(msg, now, eng);
    }

    /// `CHudSpectator::DirectorMessage` (svc_director's payload).
    pub fn director_message(&mut self, data: &[u8], eng: &mut dyn HudEngine) {
        let mut r = MsgReader::new(data);
        let cmd = r.byte();
        log::debug!("director command {cmd} ({} bytes)", data.len());
        let ad = eng.cvar("spec_autodirector_internal") != 0.0;
        match cmd {
            // START: a fresh HUD as on a new level.
            1 => {
                self.init_hud(eng);
                self.reset_hud(eng);
                self.duck_message("#Spec_Duck", eng);
            }
            // EVENT: the auto-director's targets.
            2 => {
                let o = &mut self.observer;
                o.last_primary = r.short();
                o.last_secondary = r.short();
                o.observer_flags = r.long();
                if ad {
                    if o.last_primary != self.state.user2 || o.last_secondary != self.state.user3 {
                        o.requests.reset_chase_cam = true;
                    }
                    self.state.user2 = o.last_primary;
                    self.state.user3 = o.last_secondary;
                    o.interpolating = false;
                    o.chase_entity = 0;
                }
            }
            // MODE: its byte is only read with the auto-director on.
            3 => {
                if ad {
                    let mode = r.byte();
                    self.set_modes(mode, -1, eng);
                }
            }
            4 => {
                let pos = [r.coord(), r.coord(), r.coord()];
                let ang = [r.coord(), r.coord(), r.coord()];
                let fov = r.byte();
                let target = r.short();
                if ad {
                    self.set_modes(obs::ROAMING, -1, eng);
                    self.camera_cut(pos, ang, fov as f32);
                    self.observer.chase_entity = target;
                }
            }
            // TIMESCALE: the proxy sends svc_timescale itself.
            5 => {
                let _ = r.float();
            }
            6 => {
                let effect = r.byte();
                let c = r.long();
                let rgb = [(c >> 16) as u8, (c >> 8) as u8, c as u8];
                let (x, y, fadein, fadeout, holdtime, fxtime) = (r.float(), r.float(), r.float(), r.float(), r.float(), r.float());
                let text = gs_common::bytestr::truncate_encoded(&r.string(), 127).into_owned();
                let msg = TextMessage {
                    effect,
                    color1: [rgb[0], rgb[1], rgb[2], 255],
                    color2: [rgb[0], rgb[1], rgb[2], 255],
                    x,
                    y,
                    fadein,
                    fadeout,
                    holdtime,
                    fxtime,
                    name: "HUD_MESSAGE".into(),
                    message: text,
                };
                self.observer.last_hud_message = (self.observer.last_hud_message + 1) % HUD_MESSAGES;
                self.message_add(msg, eng);
            }
            7 => {
                let sample = r.string();
                let volume = r.float();
                eng.ev_play_sound(0, self.v_origin, CHAN_BODY, &sample, volume, 0.8, 0, 100);
            }
            8 => {
                let _slots = r.long();
                self.observer.spectator_number = r.long();
                let _proxies = r.short();
                self.update_spectator_panel(eng);
            }
            9 => {
                self.spectator_gui.set_banner(&r.string());
                self.update_spectator_panel(eng);
            }
            10 => {
                let text = r.string();
                eng.filtered_client_cmd(&text);
            }
            14 => {
                let pos = [r.coord(), r.coord(), r.coord()];
                let ang = [r.coord(), r.coord(), r.coord()];
                let fov = r.byte();
                let _flags = r.byte();
                if ad {
                    self.set_modes(obs::ROAMING, -1, eng);
                    self.camera_cut(pos, ang, fov as f32);
                }
            }
            15 => {
                let n = r.byte();
                self.observer.num_waypoints = 0;
                self.observer.waypoint = 0;
                for _ in 0..n.max(0) {
                    let t = self.time + r.short() as f32 / 100.0;
                    let pos = [r.coord(), r.coord(), r.coord()];
                    let ang = [r.coord(), r.coord(), r.coord()];
                    let fov = r.byte();
                    let flags = r.byte();
                    self.add_waypoint(t, pos, ang, fov as f32, flags, eng);
                }
                if !ad {
                    self.observer.num_waypoints = 0;
                } else {
                    self.set_modes(obs::ROAMING, -1, eng);
                    let o = &mut self.observer;
                    o.interpolating = true;
                    let (a, b) = (o.waypoints[0], o.waypoints[1]);
                    let next = (o.num_waypoints > 2).then_some(o.waypoints[2]);
                    Self::set_way_interpolation(&mut o.interp, None, &a, &b, next.as_ref());
                }
            }
            other => {
                if eng.cvar("developer") != 0.0 {
                    eng.console_print(&format!("CHudSpectator::DirectorMessage: unknown command {other}.\n"));
                }
            }
        }
    }

    /// The CAMERA / CAMPATH cut: the jump vectors, `m_FOV`, the view angles.
    fn camera_cut(&mut self, pos: [f32; 3], ang: [f32; 3], fov: f32) {
        let o = &mut self.observer;
        o.jump_origin = pos;
        o.jump_angles = ang;
        o.fov = fov;
        o.requests.set_view_angles = Some(ang);
        o.jump = true;
    }

    /// `AddWaypoint`.
    fn add_waypoint(&mut self, time: f32, pos: [f32; 3], ang: [f32; 3], fov: f32, flags: i32, eng: &mut dyn HudEngine) {
        if flags != 0 && time == 0.0 {
            self.camera_cut(pos, ang, fov);
            return;
        }
        let o = &mut self.observer;
        if o.num_waypoints > MAX_WAYPOINTS - 1 {
            eng.console_print("Too many camera waypoints!\n");
            return;
        }
        o.waypoints[o.num_waypoints] = WayPoint { time, position: pos, angle: ang, fov, flags };
        if eng.cvar("developer") != 0.0 {
            eng.console_print(&format!("Added waypoint {}\n", o.num_waypoints));
        }
        o.num_waypoints += 1;
    }

    /// `SetWayInterpolation(prev, start, end, next)`.
    fn set_way_interpolation(interp: &mut Interpolation, prev: Option<&WayPoint>, start: &WayPoint, end: &WayPoint, next: Option<&WayPoint>) {
        interp.start_angle = normalize_angles(start.angle);
        interp.end_angle = normalize_angles(end.angle);
        interp.start_fov = start.fov;
        interp.end_fov = end.fov;
        interp.smooth_start = start.flags & 2 != 0;
        interp.smooth_end = start.flags & 4 != 0;
        interp.set_waypoints(prev.map(|p| p.position), start.position, end.position, next.map(|n| n.position));
    }

    /// `CHudSpectator::GetDirectorCamera(position, angle)` for the roaming view: looking at the
    /// CAMERA target, or following the WAYPOINTS path (which also re-snaps the camera pmove).
    pub fn director_camera(&mut self, position: &mut [f32; 3], angle: &mut [f32; 3], eng: &mut dyn HudEngine) -> bool {
        let t = self.time;
        let o = &mut self.observer;
        if o.chase_entity != 0 {
            let Some(e) = eng.entity(o.chase_entity) else { return false };
            let mut p = e.cur_origin;
            if o.chase_entity <= eng.max_clients() {
                p[2] += if e.solid != 0 { if e.usehull == 1 { 12.0 } else { 17.0 } } else { -8.0 };
            }
            let mut a = vector_angles(sub(p, *position));
            a[0] = -a[0];
            *angle = a;
            return true;
        }
        if !o.interpolating {
            return false;
        }
        let n = o.num_waypoints as i32;
        if o.waypoint < 0 || o.waypoint >= n - 1 || o.waypoints[o.waypoint as usize].time > t {
            return false;
        }
        let mut start = o.waypoints[o.waypoint as usize];
        let mut end = o.waypoints[o.waypoint as usize + 1];
        while t > end.time {
            let w = o.waypoint as usize;
            o.waypoint += 1;
            if o.waypoint >= n - 1 {
                o.interpolating = false;
                return false;
            }
            start = end;
            end = o.waypoints[w + 2];
            // The next point may lie past the path (a stale slot).
            let (prev, next) = (o.waypoints[w], o.waypoints[(w + 3).min(MAX_WAYPOINTS - 1)]);
            Self::set_way_interpolation(&mut o.interp, Some(&prev), &start, &end, Some(&next));
        }
        if start.time >= end.time {
            return false;
        }
        let f = ((t - start.time) / (end.time - start.time)).clamp(0.0, 1.0);
        let (p, a, fov) = o.interp.interpolate(f);
        *position = p;
        *angle = a;
        eng.console_print(&format!(
            "Interpolate time: {:.2}, fraction {:.2}, point : {:.2},{:.2},{:.2}\n",
            f64::from(t),
            f64::from(f),
            f64::from(p[0]),
            f64::from(p[1]),
            f64::from(p[2])
        ));
        o.jump_origin = p;
        o.jump_angles = a;
        o.fov = fov;
        o.requests.set_view_angles = Some(a);
        o.jump = true;
        true
    }

    /// `CHud::MsgFunc_HLTV`: per-player health (bit 7) or FOV, index 0 = everyone.
    pub(super) fn msg_hltv(&mut self, r: &mut MsgReader, eng: &mut dyn HudEngine) {
        let index = r.byte();
        let value = r.byte();
        if !eng.spectate_only() {
            if eng.cvar("developer") != 0.0 {
                eng.console_print("Warning! Received unexpected HLTV user message.\n");
            }
            return;
        }
        if value & 0x80 != 0 {
            let h = value & 0x7F;
            if index == 0 {
                self.players.iter_mut().for_each(|p| p.spec_health = h);
            } else if let Some(p) = usize::try_from(index).ok().and_then(|i| self.players.get_mut(i)) {
                p.spec_health = h;
            }
        } else {
            let mut f = value;
            if f == 0 {
                f = eng.cvar("default_fov") as i32;
                if f <= 89 {
                    f = 90;
                }
            }
            if index == 0 {
                self.observer.player_fov = [f; 64];
            } else if let Some(slot) = usize::try_from(index).ok().and_then(|i| self.observer.player_fov.get_mut(i)) {
                *slot = f;
            }
        }
    }

    /// `CHud::Think`'s HLTV FOV: the watched player's in-eye, the director's otherwise.
    pub(super) fn hltv_fov(&self, eng: &dyn HudEngine) -> Option<i32> {
        if !eng.spectate_only() {
            return None;
        }
        let u2 = self.state.user2;
        Some(if self.state.user1 == obs::IN_EYE && u2 != 0 {
            if u2 <= eng.max_clients() {
                usize::try_from(u2).ok().and_then(|i| self.observer.player_fov.get(i)).copied().unwrap_or(90)
            } else {
                (eng.cvar("default_fov") as i32).max(90)
            }
        } else {
            self.observer.fov as i32
        })
    }

    /// `HUD_ProcessPlayerState` for the local player: `g_iUser1..3` from its state (the
    /// clientdata's values, `HUD_TxferLocalOverrides`).
    pub fn process_player_state(&mut self, users: [i32; 3]) {
        self.state.user1 = users[0];
        self.state.user2 = users[1];
        self.state.user3 = users[2];
    }

    /// Takes this frame's engine/view requests.
    pub fn take_observer_requests(&mut self) -> ObserverRequests {
        std::mem::take(&mut self.observer.requests)
    }

    /// Whether the HLTV camera pmove should snap to the jump vectors (`iJumpSpectator`).
    pub fn take_spectator_jump(&mut self) -> Option<([f32; 3], [f32; 3])> {
        std::mem::take(&mut self.observer.jump).then_some((self.observer.jump_origin, self.observer.jump_angles))
    }

    /// The engine pmove did not consume a jump: it stays pending.
    pub fn restore_spectator_jump(&mut self, jump: Option<([f32; 3], [f32; 3])>) {
        if jump.is_some() {
            self.observer.jump = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_position_keeps_earlier_keys_and_takes_the_first_match() {
        let lump = "{ \"classname\" \"worldspawn\" \"origin\" \"1 2 3\" }\n{ \"angle\" \"-1\" \"classname\" \"info_player_start\" }\n{ \"classname\" \"info_player_start\" \"origin\" \"9 9 9\" }";
        assert_eq!(find_entity_in_map(lump, "info_player_start"), Some(([1.0, 2.0, 3.0], [-90.0, 0.0, 0.0])));
        assert_eq!(find_entity_in_map(lump, "trigger_camera"), None);
    }

    #[test]
    fn waypoint_path_is_a_bezier_through_the_centre() {
        let mut i = Interpolation::default();
        i.set_waypoints(None, [0.0; 3], [100.0, 0.0, 0.0], None);
        i.end_angle = [0.0, 350.0, 0.0];
        i.start_angle = [0.0, 10.0, 0.0];
        let (p, a, _) = i.interpolate(0.5);
        assert_eq!(p, [50.0, 0.0, 0.0]);
        // 10 → 350 the short way: through 0.
        assert!((a[1] - 0.0).abs() < 1e-4, "{a:?}");
    }
}
