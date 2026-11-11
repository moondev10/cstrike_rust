//! `CHudMessage` (docs/re/client-hud.md §6.1, §5.16): titles.txt messages from `HudText`,
//! `HudTextPro` and `HudTextArgs` in 16 slots with the fade / flicker / scan-out effects, and the
//! `GameTitle` logo.

use std::collections::HashSet;

use gs_draw::FontId;
use gs_draw::titles::TextMessage;

use super::chat::construct;
use super::{Hud, HudEngine, MsgReader};

const SLOTS: usize = 16;

/// `HudText` draws with `Legacy_CreditsFont`, the others with `CreditsFont`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Font {
    Legacy,
    Credits,
}

#[derive(Debug, Clone)]
struct Slot {
    msg: TextMessage,
    /// The text to draw (localised, args filled in); `None` when a `#` key was missing.
    text: Option<String>,
    start: f32,
    font: Font,
}

#[derive(Default)]
pub(super) struct Messages {
    slots: [Option<Slot>; SLOTS],
    /// `m_parms.time`: the HUD time of the last draw (level-transition fix-up).
    last_time: f32,
    /// Titles entries a hint rewrote: the original changes the shared `client_textmessage_t`,
    /// so later uses of the entry keep the hint style.
    hinted: HashSet<String>,
    /// `GameTitle`: the `GAMETITLE` entry and when it started.
    game_title: Option<(TextMessage, f32)>,
    title_half: Option<usize>,
    title_life: Option<usize>,
}

/// `XPosition`: -1 centres the line, negative aligns the block right, else a screen fraction;
/// clamped on screen.
fn x_position(x: f32, width: i32, total_width: i32, screen_w: i32) -> i32 {
    let pos = if x == -1.0 {
        (screen_w - width) / 2
    } else if x < 0.0 {
        ((1.0 + x) * screen_w as f32) as i32 - total_width
    } else {
        (x * screen_w as f32) as i32
    };
    if pos + width > screen_w { screen_w - width } else { pos.max(0) }
}

/// `YPosition`: like [`x_position`] over the block height.
fn y_position(y: f32, height: i32, screen_h: i32) -> i32 {
    let pos = if y == -1.0 {
        ((screen_h - height) as f32 * 0.5) as i32
    } else if y < 0.0 {
        ((1.0 + y) * screen_h as f32) as i32 - height
    } else {
        (y * screen_h as f32) as i32
    };
    if pos + height > screen_h { screen_h - height } else { pos.max(0) }
}

/// `FadeBlend` of the game title: 0..1.
fn fade_blend(fadein: f32, fadeout: f32, hold: f32, t: f32) -> f32 {
    if t < 0.0 {
        0.0
    } else if t < fadein {
        1.0 - (fadein - t) / fadein
    } else if t > fadein + hold {
        if fadeout > 0.0 { 1.0 - (t - fadein - hold) / fadeout } else { 0.0 }
    } else {
        1.0
    }
}

/// The hint style a `#name` hint message gives its titles entry.
fn hint_style(msg: &mut TextMessage, name: &str) {
    msg.effect = 2;
    msg.color1 = [40, 255, 40, 200];
    msg.color2 = [0, 255, 0, 200];
    msg.x = -1.0;
    msg.y = 0.7;
    msg.fadein = 0.01;
    msg.fadeout = 0.7;
    msg.fxtime = 0.07;
    msg.holdtime = if name.eq_ignore_ascii_case("Spec_Duck") { 6.0 } else { ((msg.message.len() / 25) as f32).max(1.0) };
}

impl Messages {
    pub(super) fn vid_init(&mut self, hud: &Hud) {
        self.title_half = hud.sprite_index("title_half");
        self.title_life = hud.sprite_index("title_life");
    }

    /// `HudText`, `HudTextPro`, `HudTextArgs`, `GameTitle`; `false` for other messages.
    pub(super) fn msg(&mut self, name: &str, r: &mut MsgReader, now: f32, eng: &mut dyn HudEngine) -> bool {
        match name {
            "HudText" | "HudTextPro" => {
                let text = r.string();
                let hint = r.byte() != 0;
                let font = if name == "HudText" { Font::Legacy } else { Font::Credits };
                self.add(&text, hint, font, &[], now, eng);
            }
            "HudTextArgs" => {
                let text = r.string();
                let hint = r.byte() != 0;
                let count = r.byte().clamp(0, 4);
                let args: Vec<String> = (0..count).map(|_| r.string().chars().take(63).collect()).collect();
                self.add(&text, hint, Font::Credits, &args, now, eng);
            }
            "GameTitle" => {
                if let Some(t) = eng.text_message("GAMETITLE") {
                    self.game_title = Some((t, now));
                }
            }
            _ => return false,
        }
        true
    }

    /// `CHudMessage::MessageAdd` into the first free slot. A name missing from titles.txt is
    /// shown as a plain scan-out message (the HLSDK `g_pCustomMessage`).
    fn add(&mut self, name: &str, hint: bool, font: Font, args: &[String], now: f32, eng: &mut dyn HudEngine) {
        if self.slots.iter().all(Option::is_some) {
            return;
        }
        let key = name.strip_prefix('#').unwrap_or(name);
        let msg = match eng.text_message(key) {
            Some(mut m) => {
                if (hint && name.starts_with('#')) || self.hinted.contains(key) {
                    hint_style(&mut m, key);
                    self.hinted.insert(key.to_owned());
                }
                m
            }
            None => TextMessage {
                effect: 2,
                color1: [100, 100, 100, 100],
                color2: [240, 110, 0, 0],
                x: -1.0,
                y: 0.7,
                fadein: 0.01,
                fadeout: 1.5,
                fxtime: 0.25,
                holdtime: 5.0,
                name: "Custom".to_owned(),
                message: name.to_owned(),
            },
        };
        self.insert(msg, font, args, now, eng);
    }

    /// `CHudMessage::MessageAdd(client_textmessage_t*)` (director messages, the duck hint).
    pub(super) fn add_message(&mut self, msg: TextMessage, now: f32, eng: &mut dyn HudEngine) {
        self.insert(msg, Font::Credits, &[], now, eng);
    }

    /// The shared tail of `MessageAdd`: the localised text, one message per text and screen
    /// position, into the first free slot.
    fn insert(&mut self, mut msg: TextMessage, font: Font, args: &[String], now: f32, eng: &mut dyn HudEngine) {
        let Some(free) = self.slots.iter().position(Option::is_none) else { return };
        msg.message = msg.message.chars().take(255).collect();
        // MessageDrawScan's text: `#` keys through the VGUI2 localisation, args by position.
        let text = if msg.message.starts_with('#') {
            let token = msg.message.trim_end_matches(['\r', '\n']);
            match eng.screen().localize.find(token) {
                Some(t) => Some(t.to_owned()),
                None => {
                    eng.console_print(&format!("ERROR: Missing {token} from the czero_english.txt file!\n"));
                    None
                }
            }
        } else {
            Some(msg.message.clone())
        };
        let text = text.map(|t| if args.is_empty() { t } else { construct(&t, args) });
        for slot in self.slots.iter_mut() {
            let Some(s) = slot else { continue };
            if s.msg.message == msg.message {
                return;
            }
            // One message per screen position.
            if (s.msg.y - msg.y).abs() < 0.0001 && (s.msg.x - msg.x).abs() < 0.0001 {
                *slot = None;
            }
        }
        self.slots[free] = Some(Slot { msg, text, start: now, font });
    }

    pub(super) fn draw(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        let now = hud.time;
        if let Some((title, start)) = &mut self.game_title {
            *start = start.min(now);
            let t = now - *start;
            if t > title.fadein + title.holdtime + title.fadeout {
                self.game_title = None;
            } else if let (Some(half), Some(life)) = (self.title_half, self.title_life) {
                let b = fade_blend(title.fadein, title.fadeout, title.holdtime, t);
                let screen = eng.screen();
                let (hw, hh) = hud.sprite_size(half);
                let full = hw + hud.sprite_size(life).0;
                let x = x_position(title.x, full, full, screen.width as i32);
                let y = y_position(title.y, hh, screen.height as i32);
                let c = [0, 1, 2].map(|i| (b * f32::from(title.color1[i])) as i32);
                hud.draw_sprite(screen, half, x, y, c);
                hud.draw_sprite(screen, life, x + hw, y, c);
            }
        }
        for s in self.slots.iter_mut().flatten() {
            if s.start > now || s.start == 1.0 {
                s.start = now + self.last_time - s.start + 0.2;
            }
        }
        for slot in &mut self.slots {
            let Some(s) = slot else { continue };
            let m = &s.msg;
            let len = s.text.as_ref().map_or(0, |t| t.chars().count()) as f32;
            let end = match m.effect {
                0 | 1 => s.start + m.fadein + m.fadeout + m.holdtime,
                2 => s.start + m.fadein * len + m.fadeout + m.holdtime,
                _ => 0.0,
            };
            if now > end {
                *slot = None;
                continue;
            }
            if let Some(text) = &s.text {
                let font = match s.font {
                    Font::Legacy => font_or_credits(eng, "Legacy_CreditsFont"),
                    Font::Credits => eng.screen().font("CreditsFont"),
                };
                draw_scan(m, text, now - s.start, font, hud, eng);
            }
        }
        self.last_time = now;
    }
}

/// A scheme font, `CreditsFont` when the scheme lacks it.
fn font_or_credits(eng: &mut dyn HudEngine, name: &str) -> FontId {
    let screen = eng.screen();
    let font = screen.font(name);
    if screen.font_tall(font) > 0 { font } else { screen.font("CreditsFont") }
}

/// `MessageDrawScan` + `MessageScanStart` / `MessageScanNextChar`: lines positioned per the
/// entry's x/y, each character coloured by the effect and drawn additively.
fn draw_scan(m: &TextMessage, text: &str, time: f32, font: FontId, hud: &Hud, eng: &mut dyn HudEngine) {
    let screen = eng.screen();
    let (sw, sh) = (screen.width as i32, screen.height as i32);
    let tall = screen.font_tall(font);
    let lines: Vec<&str> = text.split('\n').collect();
    let widths: Vec<i32> = lines.iter().map(|l| l.chars().map(|c| screen.char_width(font, c)).sum()).collect();
    let total_width = widths.iter().copied().max().unwrap_or(0);
    let length = text.chars().count() as f32;
    let mut y = y_position(m.y, lines.len() as i32 * tall, sh);
    // MessageScanStart.
    let (fade_time, fade_blend, flicker) = match m.effect {
        0 | 1 => {
            let fade_time = m.fadein + m.holdtime;
            let blend = if time < m.fadein {
                ((m.fadein - time) * (1.0 / m.fadein) * 255.0) as i32
            } else if time > fade_time {
                if m.fadeout > 0.0 { ((time - fade_time) / m.fadeout * 255.0) as i32 } else { 255 }
            } else {
                0
            };
            (fade_time, blend, m.effect == 1 && hud.rand() % 100 < 10)
        }
        _ => {
            let fade_time = m.fadein * length + m.holdtime;
            let blend = if time > fade_time && m.fadeout > 0.0 { ((time - fade_time) / m.fadeout * 255.0) as i32 } else { 0 };
            (fade_time, blend, false)
        }
    };
    let src = [m.color1[0], m.color1[1], m.color1[2]].map(i32::from);
    let fx = [m.color2[0], m.color2[1], m.color2[2]].map(i32::from);
    let mut char_time = 0.0;
    for (line, width) in lines.iter().zip(&widths) {
        let mut x = x_position(m.x, *width, total_width, sw);
        for ch in line.chars() {
            let next = x + screen.char_width(font, ch);
            // MessageScanNextChar.
            let (mut s, mut dest, mut blend) = (src, [0; 3], 0);
            match m.effect {
                0 | 1 => blend = fade_blend,
                2 => {
                    char_time += m.fadein;
                    if char_time > time {
                        s = [0; 3];
                    } else {
                        let delta = time - char_time;
                        if time > fade_time {
                            blend = fade_blend;
                        } else if delta <= m.fxtime {
                            dest = fx;
                            blend = 255 - (delta * (1.0 / m.fxtime) * 255.0 + 0.5) as i32;
                        }
                    }
                }
                _ => {}
            }
            let blend = blend.clamp(0, 255);
            let c = [0, 1, 2].map(|i| (s[i] * (255 - blend) + dest[i] * blend) >> 8);
            let on_screen = x >= 0 && y >= 0 && next <= sw;
            if flicker && on_screen {
                screen.vgui2_draw_character_add(x, y, ch, fx[0], fx[1], fx[2], font);
            }
            if on_screen {
                screen.vgui2_draw_character_add(x, y, ch, c[0], c[1], c[2], font);
            }
            x = next;
        }
        y += tall;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_centre_align_and_clamp() {
        assert_eq!(x_position(-1.0, 100, 200, 640), 270);
        // Negative x right-aligns the whole block.
        assert_eq!(x_position(-0.1, 100, 200, 640), 376);
        assert_eq!(x_position(0.95, 100, 100, 640), 540);
        assert_eq!(y_position(-1.0, 40, 480), 220);
        assert_eq!(y_position(0.7, 40, 480), 336);
        assert_eq!(y_position(-0.05, 40, 480), 416);
    }
}
