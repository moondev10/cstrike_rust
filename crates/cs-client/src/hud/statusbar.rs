//! `CHudStatusBar` (docs/re/client-hud.md §6.5): the "player ID" line built from the
//! `StatusText` template and the `StatusValue` slots.

use super::health::team_color;
use super::{Hud, HudEngine, MsgReader};

const VALUES: usize = 8;
const MAX_TEXT: usize = 127;
/// `g_ColorYellow`.
const YELLOW: [f32; 3] = [1.0, 0.7, 0.0];

#[derive(Default)]
pub(super) struct StatusBar {
    active: bool,
    template: String,
    /// `m_iStatusValues` (slot 0 is always 1).
    values: [i32; VALUES],
    /// The parsed line and its colour (a `%p` player's team colour).
    line: String,
    color: Option<[f32; 3]>,
    reparse: bool,
}

impl StatusBar {
    pub(super) fn reset(&mut self) {
        self.template.clear();
        self.values = [0; VALUES];
        self.values[0] = 1;
        self.line.clear();
        self.color = None;
    }

    /// `StatusText` (only line 0) and `StatusValue`; `false` for other messages.
    pub(super) fn msg(&mut self, name: &str, r: &mut MsgReader) -> bool {
        match name {
            "StatusText" => {
                if r.byte() == 0 {
                    self.template = gs_common::bytestr::truncate_encoded(&r.string(), MAX_TEXT).into_owned();
                    self.active = true;
                    self.reparse = true;
                }
            }
            "StatusValue" => {
                let index = r.byte();
                let value = r.short();
                if let Some(v) = usize::try_from(index).ok().filter(|&i| (1..VALUES).contains(&i)).and_then(|i| self.values.get_mut(i)) {
                    *v = value;
                    self.active = true;
                    self.reparse = true;
                }
            }
            _ => return false,
        }
        true
    }

    /// `ParseStatusString`: `#` keys through titles.txt, lines prefixed with a value index kept
    /// only when that value is non-zero, `%pN` / `%iN` / `%h` / `%cN` / `%%` substituted. A named
    /// `%p` player becomes `m_iPlayerLastPointedAt`; `%i` stores its value as that player's
    /// health, shown for 5 seconds.
    fn parse(&mut self, hud: &mut Hud, eng: &dyn HudEngine) {
        let template = super::chat::localise_text_string(&self.template, eng);
        let title = |name: &str| eng.text_message(name).map(|t| t.message).unwrap_or_default();
        let mut out = String::new();
        let mut color = None;
        for line in template.split('\n').filter(|l| !l.is_empty()) {
            let digits = line.chars().take_while(char::is_ascii_digit).count();
            let index = line[..digits].parse::<usize>().unwrap_or(0);
            if index >= VALUES || self.values[index] == 0 {
                continue;
            }
            let mut chars = line[digits..].chars().peekable();
            while let Some(c) = chars.next() {
                if c != '%' {
                    out.push(c);
                    continue;
                }
                let Some(kind) = chars.next() else { break };
                if kind == '%' {
                    out.push('%');
                    continue;
                }
                let mut n = String::new();
                while let Some(&d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                    n.push(d);
                    chars.next();
                }
                let value = n.parse::<usize>().ok().filter(|&i| i < VALUES).map(|i| self.values[i]);
                match (kind, value) {
                    ('p', Some(v)) => match eng.player_info(v) {
                        Some(p) => {
                            out.push_str(&p.name);
                            hud.health.last_pointed_at = v;
                            let team = usize::try_from(v).ok().and_then(|i| hud.players.get(i)).map_or(0, |p| p.teamnumber);
                            color = Some(team_color(team).map(|c| c as f32 / 255.0));
                        }
                        None => out.push_str("******"),
                    },
                    ('i', Some(v)) => {
                        let now = hud.time;
                        if let Some(p) = usize::try_from(hud.health.last_pointed_at).ok().and_then(|i| hud.players.get_mut(i)) {
                            p.spec_health = v;
                            p.showhealth = now + 5.0;
                        }
                        out.push_str(&v.to_string());
                    }
                    ('h', _) => out.push_str(&title("Health")),
                    ('c', Some(v)) => out.push_str(&match v {
                        1 => title("Friend"),
                        2 => title("Enemy"),
                        3 => title("Hostage"),
                        _ => String::new(),
                    }),
                    _ => {}
                }
            }
        }
        self.line = gs_common::bytestr::truncate_encoded(&out, MAX_TEXT).into_owned();
        self.color = color;
    }

    pub(super) fn draw(&mut self, hud: &mut Hud, eng: &mut dyn HudEngine) {
        if !self.active || hud.state.user1 != 0 {
            return;
        }
        if self.reparse {
            self.parse(hud, eng);
            self.reparse = false;
        }
        if self.line.is_empty() {
            return;
        }
        let center_id = eng.cvar("hud_centerid");
        let screen = eng.screen();
        let (w, h) = (screen.width as i32, screen.height as i32);
        let font = screen.font("EngineFont");
        let (text_w, text_h) = (screen.text_width(font, &self.line), screen.font_tall(font));
        let (x, y) = if center_id == 0.0 {
            (10, h - (hud.font_height as f32 * 2.75) as i32)
        } else {
            ((w - text_w) / 2, h / 2 + (center_id * text_h as f32) as i32)
        };
        let c = self.color.unwrap_or(YELLOW);
        screen.set_text_color(c[0], c[1], c[2]);
        screen.draw_console_string(x, y, &self.line);
    }
}
