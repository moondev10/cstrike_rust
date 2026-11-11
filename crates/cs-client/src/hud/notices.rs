//! `CHudDeathNotice` (docs/re/client-hud.md §5.9): the kill feed and its console lines.

use super::health::team_color;
use super::{Hud, HudEngine, MsgReader, PlayerExtra};

const MAX_NOTICES: usize = 4;

#[derive(Debug, Clone)]
struct Notice {
    killer: String,
    killer_team: i32,
    victim: String,
    victim_team: i32,
    /// `hud.txt` index of `d_<weapon>` (or `d_skull`).
    sprite: Option<usize>,
    headshot: bool,
    suicide: bool,
    teamkill: bool,
    /// Non-player victim: the weapon name is the victim text.
    non_player: bool,
    expire: f32,
}

#[derive(Default)]
pub(super) struct DeathNotice {
    notices: Vec<Notice>,
}

/// `GetProportionalScaledValue` for a 480-tall base layout.
fn proportional(v: i32, screen_height: u32) -> i32 {
    (v as f32 * screen_height as f32 / 480.0) as i32
}

impl DeathNotice {
    pub(super) fn clear(&mut self) {
        self.notices.clear();
    }

    /// `MsgFunc_DeathMsg`.
    pub(super) fn msg_death(&mut self, r: &mut MsgReader, players: &[PlayerExtra], hud: &Hud, eng: &mut dyn HudEngine) {
        let killer = r.byte();
        let victim = r.byte();
        let headshot = r.byte() != 0;
        let weapon = r.string();
        let name = |eng: &dyn HudEngine, i: i32| eng.player_info(i).map(|p| p.name).unwrap_or_default();
        let team = |i: i32| usize::try_from(i).ok().and_then(|i| players.get(i)).map_or(0, |p| p.teamnumber);
        let non_player = victim == 255;
        let suicide = !non_player && (killer == victim || killer == 0);
        let teamkill = weapon == "teammate";
        let sprite = hud.sprite_index(&format!("d_{weapon}")).or_else(|| hud.sprite_index("d_skull"));
        let killer_name = name(eng, killer);
        let victim_name = if non_player { weapon.clone() } else { name(eng, victim) };
        let line = if non_player {
            format!("{killer_name} killed a {weapon}\n")
        } else if suicide {
            if weapon == "world" { format!("{victim_name} died\n") } else { format!("{victim_name} killed self\n") }
        } else if teamkill {
            format!("{killer_name} killed his teammate {victim_name}\n")
        } else if headshot {
            format!("*** {killer_name} killed {victim_name} with a headshot from {weapon} ***\n")
        } else {
            format!("{killer_name} killed {victim_name} with {weapon}\n")
        };
        eng.console_print(&line);
        if self.notices.len() == MAX_NOTICES {
            self.notices.remove(0);
        }
        let time = eng.cvar("hud_deathnotice_time") as i32;
        self.notices.push(Notice {
            killer: killer_name,
            killer_team: team(killer),
            victim: victim_name,
            victim_team: team(victim),
            sprite,
            headshot,
            suicide,
            teamkill,
            non_player,
            expire: hud.time + time as f32,
        });
    }

    pub(super) fn draw(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        self.notices.retain(|n| n.expire > hud.time);
        if self.notices.is_empty() {
            return;
        }
        let headshot = hud.sprite_index("d_headshot");
        let observing = hud.state.user1 != 0 || eng.spectate_only();
        let screen = eng.screen();
        let (w, h) = (screen.width as i32, screen.height);
        let (gap, right, text_gap) = (proportional(16, h), proportional(12, h), proportional(5, h));
        // GetDeathNoticeY: below the spectator top bar while observing or on HLTV.
        let top = if observing { proportional(64, h) } else { proportional(32, h) };
        for (i, n) in self.notices.iter().enumerate() {
            let y = top + i as i32 * gap;
            let (victim_w, _) = screen.draw_console_string_len(&n.victim);
            let weapon_w = n.sprite.map_or(0, |s| hud.sprite_size(s).0);
            let head_w = headshot.filter(|_| n.headshot).map_or(0, |s| hud.sprite_size(s).0 + text_gap);
            let mut x = w - victim_w - weapon_w - right - text_gap - head_w;
            if !n.suicide && !n.non_player {
                let (killer_w, _) = screen.draw_console_string_len(&n.killer);
                let c = team_color(n.killer_team);
                screen.set_text_color(c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0);
                screen.draw_console_string(x - text_gap - killer_w, y, &n.killer);
            }
            if let Some(s) = n.sprite {
                let c = if n.teamkill { [10, 240, 10] } else { [255, 80, 0] };
                hud.draw_sprite(screen, s, x, y, c);
                x += weapon_w;
            }
            if let Some(s) = headshot.filter(|_| n.headshot) {
                x += text_gap;
                hud.draw_sprite(screen, s, x, y, [255, 80, 0]);
                x += hud.sprite_size(s).0;
            }
            x += text_gap;
            let c = team_color(if n.non_player { 0 } else { n.victim_team });
            screen.set_text_color(c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0);
            screen.draw_console_string(x, y, &n.victim);
        }
    }
}
