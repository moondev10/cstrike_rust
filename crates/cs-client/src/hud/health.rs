//! `CHudHealth` (health, damage icons, pain arrows, radar), `CHudBattery` and
//! `CHudFlashlight` (docs/re/client-hud.md §5.3, §5.4, §5.12, §5.13).

use gs_draw::SprHandle;

use super::{EMPTY, Hud, HudEngine, MsgReader, RED, YELLOWISH, dhn, hide, scale};

/// Fade pattern: bright after a change, back to `MIN_ALPHA` 100 over `FADE_TIME`.
const MIN_ALPHA: i32 = 100;
const FADE_TIME: f32 = 100.0;

/// `fade -= 20·dt`; the alpha it gives.
fn fade_alpha(fade: &mut f32, dt: f32) -> i32 {
    if *fade == 0.0 {
        return MIN_ALPHA;
    }
    *fade -= dt * 20.0;
    if *fade <= 0.0 {
        *fade = 0.0;
    }
    MIN_ALPHA + (*fade / FADE_TIME * 128.0) as i32
}

/// `giDmgFlags`: damage bits with HUD icons, in icon order.
const DMG_FLAGS: [i32; 12] = [0x20000, 0x100000, 0x400010, 0x4000, 0x200008, 0x10000, 0x40000, 0x100, 0x4000_0000, 0x80_0000, 0x200, i32::MIN];

#[derive(Debug, Clone, Copy, Default)]
struct DamageIcon {
    /// `hud.txt` index; `None` when unused.
    sprite: Option<usize>,
    expire: f32,
    x: i32,
    y: i32,
}

#[derive(Default)]
pub(super) struct Health {
    active: bool,
    health: i32,
    fade: f32,
    cross: Option<usize>,
    dmg_bio: Option<usize>,
    /// `giDmgWidth`, `giDmgHeight`.
    dmg_size: (i32, i32),
    damage: [DamageIcon; 12],
    damage_bits: i32,
    pain: SprHandle,
    /// Front, right, rear, left.
    attack: [f32; 4],
    radar: Option<usize>,
    radar_opaque: Option<usize>,
    /// `m_iPlayerLastPointedAt`: set by the status bar's `%p` and the `SpecHealth` messages.
    pub(super) last_pointed_at: i32,
    /// `m_bTrackArray`: bit p-1 for players marked with `trackplayer`.
    track: u64,
}

/// Team colour of a team number (`GetClientColor`).
pub(super) fn team_color(team: i32) -> [i32; 3] {
    let c: [f32; 3] = match team {
        1 => [1.0, 0.25, 0.25],
        2 => [0.6, 0.8, 1.0],
        3 => [1.0, 0.7, 0.0],
        4 => [0.6, 1.0, 0.6],
        _ => [0.8, 0.8, 0.8],
    };
    c.map(|v| (v * 255.0) as i32)
}

impl Health {
    /// `CHudHealth::Init`: health starts at 100.
    pub(super) fn new() -> Self {
        Self { health: 100, ..Self::default() }
    }

    /// `m_iHealth`.
    pub(super) fn value(&self) -> i32 {
        self.health
    }

    /// `UserCmd_TrackPlayer`: mark the last pointed-at player (1..32) on the radar.
    pub(super) fn track_player(&mut self) {
        if (1..=32).contains(&self.last_pointed_at) {
            self.track |= 1 << (self.last_pointed_at - 1);
        }
    }

    /// `UserCmd_ClearPlayers`.
    pub(super) fn clear_players(&mut self) {
        self.track = 0;
    }

    pub(super) fn vid_init(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        self.cross = hud.sprite_index("cross");
        self.dmg_bio = hud.sprite_index("dmg_bio");
        self.dmg_size = self.dmg_bio.map_or((0, 0), |i| hud.sprite_size(i + 1));
        self.radar = hud.sprite_index("radar");
        self.radar_opaque = hud.sprite_index("radaropaque");
        let res = if eng.screen().width >= 640 { 640 } else { 320 };
        self.pain = eng.spr_load(&format!("sprites/{res}_pain.spr"));
    }

    pub(super) fn reset(&mut self) {
        self.attack = [0.0; 4];
        self.damage_bits = 0;
        self.damage = [DamageIcon::default(); 12];
    }

    /// `MsgFunc_Health`.
    pub(super) fn msg_health(&mut self, r: &mut MsgReader) {
        let x = r.byte();
        self.active = true;
        if x != self.health {
            self.fade = FADE_TIME;
            self.health = x;
        }
    }

    /// `MsgFunc_Damage`: icons for new damage bits and the pain direction.
    pub(super) fn msg_damage(&mut self, r: &mut MsgReader, origin: [f32; 3], angles: [f32; 3], now: f32) {
        let armor = r.byte();
        let taken = r.byte();
        let bits = r.long();
        let from = [r.coord(), r.coord(), r.coord()];
        self.update_tiles(now, bits);
        if armor > 0 || taken > 0 {
            self.calc_direction(from, origin, angles);
        }
    }

    /// `UpdateTiles`: new bits get an icon at the bottom, older ones move up.
    fn update_tiles(&mut self, now: f32, bits: i32) {
        let new = bits & !self.damage_bits;
        for (i, &flag) in DMG_FLAGS.iter().enumerate() {
            if new & flag == 0 {
                continue;
            }
            for icon in self.damage.iter_mut().filter(|d| d.sprite.is_some()) {
                icon.y -= self.dmg_size.1;
            }
            let sprite = self.dmg_bio.map(|b| b + 1 + i);
            self.damage[i] = DamageIcon { sprite, expire: now + 2.0, x: self.dmg_size.0 / 8, y: 0 };
            // y is relative to H - 2·dmgHeight, applied when drawing.
        }
        self.damage_bits |= bits;
    }

    /// `CalcDamageDirection`.
    fn calc_direction(&mut self, from: [f32; 3], origin: [f32; 3], angles: [f32; 3]) {
        if from == [0.0; 3] {
            self.attack = [0.0; 4];
            return;
        }
        let d = [from[0] - origin[0], from[1] - origin[1], from[2] - origin[2]];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if dist <= 50.0 {
            self.attack = [1.0; 4];
            return;
        }
        let n = d.map(|v| v / dist);
        let (forward, right, _) = crate::view::angle_vectors(angles);
        let f = forward[0] * n[0] + forward[1] * n[1] + forward[2] * n[2];
        let s = right[0] * n[0] + right[1] * n[1] + right[2] * n[2];
        if f > 0.3 {
            self.attack[0] = self.attack[0].max(f);
        } else if f < -0.3 {
            self.attack[2] = self.attack[2].max(-f);
        }
        if s > 0.3 {
            self.attack[1] = self.attack[1].max(s);
        } else if s < -0.3 {
            self.attack[3] = self.attack[3].max(-s);
        }
    }

    fn color(&self) -> [i32; 3] {
        if self.health <= 25 { RED } else { YELLOWISH }
    }

    /// `HUD_ACTIVE` (set by the first `Health` message).
    pub(super) fn active(&self) -> bool {
        self.active
    }

    pub(super) fn draw(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        if !self.active || hud.hide & hide::HEALTH != 0 || hud.state.user1 != 0 {
            return;
        }
        if hud.has_suit() && hud.draw_radar {
            self.draw_radar(hud, eng);
        }
        let mut a = fade_alpha(&mut self.fade, hud.delta);
        if self.health <= 15 {
            a = 255;
        }
        let c = scale(self.color(), a);
        let screen = eng.screen();
        let fh = hud.font_height;
        if hud.has_suit()
            && let Some(cross) = self.cross
        {
            let cross_w = hud.sprite_size(cross).0;
            let y = screen.height as i32 - fh - fh / 2;
            hud.draw_sprite(screen, cross, cross_w / 2, y, c);
            hud.draw_number(screen, cross_w + hud.digit_width() / 2, y, dhn::THREE_DIGITS | dhn::DRAWZERO, self.health, c);
        }
        self.draw_damage(hud, eng);
        self.draw_pain(hud, eng);
    }

    /// `DrawDamage`: pulsing icons; expired ones drop out once dim.
    fn draw_damage(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        if self.damage_bits == 0 {
            return;
        }
        let a = ((hud.time * 2.0).sin().abs() * 256.0) as i32;
        let c = scale(YELLOWISH, a);
        let screen = eng.screen();
        let base = screen.height as i32 - 2 * self.dmg_size.1;
        for icon in self.damage.iter().filter(|d| d.sprite.is_some()) {
            if let Some(s) = icon.sprite {
                hud.draw_sprite(screen, s, icon.x, base + icon.y, c);
            }
        }
        for (i, flag) in DMG_FLAGS.iter().enumerate() {
            let icon = self.damage[i];
            if icon.sprite.is_some() && icon.expire <= hud.time && a < 40 {
                self.damage_bits &= !flag;
                self.damage[i].sprite = None;
                for other in self.damage.iter_mut().filter(|d| d.sprite.is_some() && d.y < icon.y) {
                    other.y += self.dmg_size.1;
                }
            }
        }
    }

    /// `DrawPain`: the four direction arrows from `sprites/<res>_pain.spr`.
    fn draw_pain(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        if self.attack.iter().all(|&v| v == 0.0) {
            return;
        }
        let fade = hud.delta * 2.0;
        let color = self.color();
        let screen = eng.screen();
        let (w, h) = (screen.width as i32, screen.height as i32);
        for (frame, value) in self.attack.iter_mut().enumerate() {
            if *value > 0.4 {
                let a = if *value < 0.5 { 127 } else { (*value * 255.0) as i32 };
                let c = scale(color, a);
                let (sw, sh) = screen.spr_size(self.pain, frame as i32);
                let (x, y) = match frame {
                    0 => (w / 2 - sw / 2, h / 2 - 3 * sh),
                    1 => (w / 2 + 2 * sw, h / 2 - sh / 2),
                    2 => (w / 2 - sw / 2, h / 2 + 2 * sh),
                    _ => (w / 2 - 3 * sw, h / 2 - sh / 2),
                };
                screen.spr_set(self.pain, c[0], c[1], c[2]);
                screen.spr_draw_additive(frame as i32, x, y, None);
                *value = (*value - fade).max(0.0);
            } else {
                *value = 0.0;
            }
        }
    }

    /// `DrawRadar`: teammates, the bomb and flashing speakers around the local player.
    fn draw_radar(&self, hud: &Hud, eng: &mut dyn HudEngine) {
        let Some(local) = hud.local_index(eng) else { return };
        let me = &hud.players[local];
        if me.dead {
            return;
        }
        let opaque = eng.cvar("cl_radartype") != 0.0;
        let screen = eng.screen();
        let big = screen.width > 639;
        let background = if opaque { self.radar_opaque } else { self.radar };
        if let Some(bg) = background {
            let (h, mut r) = hud.sprite(bg);
            r.right = r.left + 128;
            r.bottom = r.top + 128;
            if opaque {
                screen.spr_set(h, 200, 200, 200);
                screen.spr_draw_holes(0, 0, 0, Some(r));
            } else {
                screen.spr_set(h, 25, 75, 25);
                screen.spr_draw_additive(0, 0, 0, Some(r));
            }
        }
        let (centre, range, dot) = if big { (64, 2048.0, 2) } else { (32, 1024.0, 1) };
        let yaw = hud.state.angles[1];
        let origin = hud.state.origin;
        let project = |target: [f32; 3]| -> Option<(i32, i32, f32)> {
            let d = [target[0] - origin[0], target[1] - origin[1]];
            let dist = (d[0] * d[0] + d[1] * d[1]).sqrt();
            let angle = d[1].atan2(d[0]).to_degrees();
            let r = (yaw - angle).to_radians();
            let (mut x, mut y) = (dist * r.sin(), -dist * r.cos());
            if dist > range {
                x *= range / dist;
                y *= range / dist;
            }
            let (px, py) = (centre + (x / 32.0) as i32, centre + (y / 32.0) as i32);
            let limit = if big { 128 } else { 64 };
            ((0..limit).contains(&px) && (0..limit).contains(&py)).then_some((px, py, target[2] - origin[2]))
        };
        let my_team = me.teamnumber;
        for (i, p) in hud.players.iter().enumerate().take(33) {
            if i == local || p.dead || p.teamname != me.teamname || i == 0 {
                continue;
            }
            let Some((x, y, z)) = project(p.origin) else { continue };
            let tracked = (1..=32).contains(&i) && self.track & 1 << (i - 1) != 0;
            let (color, size) = if (p.has_c4 && my_team <= 1) || (p.vip && (my_team == 0 || my_team == 2)) {
                ([250, 0, 0], dot)
            } else if tracked {
                ([185, 20, 20], dot * 2)
            } else {
                ([150, 75, 250], dot)
            };
            radar_dot(screen, x, y, z, size, 0, color, 235);
            if p.radarflashon && p.radarflashes > 0 {
                radar_dot(screen, x, y, z, size, 4, [230, 110, 25], 245);
            }
        }
        let bomb = &hud.players[33];
        if my_team == 1 && bomb.radarflashon && bomb.radarflashes > 0
            && let Some((x, y, z)) = project(bomb.origin)
        {
            let planted = !hud.show_timer;
            radar_dot(screen, x, y, z, dot, 1 | if planted { 8 } else { 0 }, [250, 0, 0], 235);
        }
        // Hostages: CT viewers only, and only while they flash (dead ones need a flash time).
        if my_team == 2 {
            for h in hud.hostages.iter().filter(|h| h.radarflashon && h.radarflashes > 0 && (!h.dead || h.radarflash != -1.0)) {
                let Some((x, y, z)) = project(h.origin) else { continue };
                let color = if h.dead { [250, 25, 25] } else { [50, 50, 255] };
                radar_dot(screen, x, y, z, dot, 2, color, 245);
            }
        }
    }
}

/// `DrawRadarDot(x, y, z, size, flags, r, g, b, a)`: height cues above/below, else a square
/// by `flags`.
#[allow(clippy::too_many_arguments)]
fn radar_dot(screen: &mut gs_draw::Screen, x: i32, y: i32, z: f32, s: i32, flags: i32, c: [i32; 3], a: i32) {
    let fill = |screen: &mut gs_draw::Screen, x, y, w, h| screen.fill_rgba(x, y, w, h, c[0], c[1], c[2], a);
    if z < -128.0 {
        let k = ((-z / 400.0) as i32).min(7);
        fill(screen, x, y, 1, 2 * s + k);
        fill(screen, x - 2, y + 2 * s + k, 5, 1);
    } else if z > 128.0 {
        let k = ((z / 400.0) as i32).min(7);
        fill(screen, x - 2, y, 5, 1);
        fill(screen, x, y + 1, 1, k + 2 * s - 1);
    } else if flags & 4 != 0 {
        fill(screen, x - 3, y - 3, s + 3, s + 3);
    } else if flags & 2 != 0 || (flags & 1 != 0 && flags & 8 == 0) {
        fill(screen, x - 1, y - 1, s + 1, s + 1);
    } else if flags & 9 == 9 {
        for (dx, dy) in [(0, 0), (-2, -2), (-2, 2), (2, -2)] {
            fill(screen, x + dx, y + dy, s, s);
        }
        fill(screen, x + 2, y + 2, s, s);
    } else {
        fill(screen, x, y, s, s);
    }
}

/// `CHudBattery`.
#[derive(Default)]
pub(super) struct Battery {
    active: bool,
    armor: i32,
    pub(super) armor_type: i32,
    fade: f32,
}

impl Battery {
    pub(super) fn reset(&mut self) {
        self.fade = 0.0;
    }

    /// `MsgFunc_Battery`.
    pub(super) fn msg_battery(&mut self, r: &mut MsgReader) {
        let x = r.short();
        self.active = true;
        if x != self.armor {
            self.fade = FADE_TIME;
            self.armor = x;
            if x <= 0 {
                self.armor_type = 0;
            }
        }
    }

    pub(super) fn draw(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        if !self.active || hud.hide & hide::HEALTH != 0 || hud.state.user1 != 0 || !hud.has_suit() {
            return;
        }
        let (empty, full) = if self.armor_type == 1 { ("suithelmet_empty", "suithelmet_full") } else { ("suit_empty", "suit_full") };
        let (Some(empty), Some(full)) = (hud.sprite_index(empty), hud.sprite_index(full)) else { return };
        self.fade = self.fade.min(FADE_TIME);
        let a = fade_alpha(&mut self.fade, hud.delta);
        let c = scale(YELLOWISH, a);
        let screen = eng.screen();
        let (w, h) = (screen.width as i32, screen.height as i32);
        let fh = hud.font_height;
        let x = w / 5;
        let y = h - fh - fh / 2 + if w > 639 { 3 } else { 0 } + 1;
        let (ew, eh) = hud.sprite_size(empty);
        let offset = eh / 6;
        hud.draw_sprite(screen, empty, x, y - offset, c);
        let (hf, mut rect) = hud.sprite(full);
        let height = rect.bottom - rect.top;
        let top = rect.top;
        rect.top += if self.armor > 99 { 0 } else { ((100 - self.armor) as f32 / 100.0 * height as f32) as i32 };
        // An empty rect would draw the whole sheet; the original skips it.
        if rect.bottom > rect.top {
            screen.spr_set(hf, c[0], c[1], c[2]);
            screen.spr_draw_additive(0, x, y - offset + (rect.top - top), Some(rect));
        }
        hud.draw_number(screen, x + ew, if w > 639 { y - 4 } else { y - 1 }, dhn::THREE_DIGITS | dhn::DRAWZERO, self.armor, c);
    }
}

/// `CHudFlashlight`.
#[derive(Default)]
pub(super) struct Flashlight {
    on: bool,
    /// Battery 0..1.
    battery: f32,
}

impl Flashlight {
    pub(super) fn reset(&mut self) {
        self.on = false;
        self.battery = 1.0;
    }

    pub(super) fn msg_flashlight(&mut self, r: &mut MsgReader) {
        self.on = r.byte() != 0;
        self.battery = r.byte() as f32 / 100.0;
    }

    pub(super) fn msg_flashbat(&mut self, r: &mut MsgReader) {
        self.battery = r.byte() as f32 / 100.0;
    }

    pub(super) fn draw(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        if hud.hide & (hide::FLASHLIGHT | hide::ALL) != 0 || !hud.has_suit() || hud.state.user1 != 0 {
            return;
        }
        let (Some(empty), Some(full), Some(beam)) =
            (hud.sprite_index("flash_empty"), hud.sprite_index("flash_full"), hud.sprite_index("flash_beam"))
        else {
            return;
        };
        let color = if self.battery < 0.2 { EMPTY } else { YELLOWISH };
        let c = scale(color, if self.on { 225 } else { 100 });
        let screen = eng.screen();
        let width = screen.width as i32;
        let (fw, _) = hud.sprite_size(full);
        let (_, er) = hud.sprite(empty);
        let (_, fr) = hud.sprite(full);
        let x = width - fw - fw / 2;
        let y = (er.bottom - fr.top) / 2;
        hud.draw_sprite(screen, empty, x, y, c);
        if self.on {
            hud.draw_sprite(screen, beam, width - fw / 2, y, c);
        }
        let (h, mut rect) = hud.sprite(full);
        let cut = ((1.0 - self.battery) * fw as f32) as i32;
        rect.left += cut;
        if rect.left < rect.right {
            screen.spr_set(h, c[0], c[1], c[2]);
            screen.spr_draw_additive(0, x + cut, y, Some(rect));
        }
    }
}
