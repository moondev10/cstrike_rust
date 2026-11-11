//! `CHudAccountBalance` and `CHudRoundTimer` (docs/re/client-hud.md §5.5, §5.6).

use super::{EMPTY, Hud, HudEngine, MsgReader, YELLOWISH, dhn, hide, scale};

/// `CHudAccountBalance`.
#[derive(Default)]
pub(super) struct Money {
    amount: i32,
    delta: i32,
    show_delta: bool,
    /// 5 s colour fade and 2.5 s delta fade after a change.
    fade: f32,
    fade_fast: f32,
    blink_count: i32,
    blink_time: f32,
}

fn lerp(from: [i32; 3], to: [i32; 3], f: f32) -> [i32; 3] {
    [0, 1, 2].map(|i| (to[i] as f32 + (from[i] - to[i]) as f32 * f) as i32)
}

impl Money {
    pub(super) fn reset(&mut self) {
        self.blink_count = 0;
    }

    /// `GetBalance`.
    pub(super) fn balance(&self) -> i32 {
        self.amount
    }

    /// `MsgFunc_Money`.
    pub(super) fn msg_money(&mut self, r: &mut MsgReader) {
        let amount = r.long();
        self.show_delta = r.byte() != 0;
        if amount != self.amount {
            // The client DLL's int subtraction wraps; a server's extreme amounts must not panic.
            self.delta = amount.wrapping_sub(self.amount);
            self.amount = amount;
            self.fade = 5.0;
            self.fade_fast = 2.5;
        } else if self.show_delta {
            self.show_delta = false;
        }
    }

    /// `MsgFunc_BlinkAcct`.
    pub(super) fn blink(&mut self, n: i32) {
        self.blink_count = 2 * n;
        self.blink_time = 0.0;
    }

    pub(super) fn draw(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        if hud.state.user1 != 0 || hud.hide & hide::MONEY != 0 || eng.spectate_only() {
            return;
        }
        let mut color = if self.fade == 0.0 {
            scale(YELLOWISH, 100)
        } else {
            self.fade = (self.fade - hud.delta).max(0.0);
            let f = self.fade / 5.0;
            let a = (128.0 * f + 100.0) as i32;
            let base = match self.delta {
                d if d > 0 => lerp([0, 255, 0], YELLOWISH, f),
                d if d < 0 => lerp([255, 0, 0], YELLOWISH, f),
                _ => YELLOWISH,
            };
            scale(base, a)
        };
        let delta_color = if self.fade_fast > 0.0 {
            self.fade_fast = (self.fade_fast - hud.delta).max(0.0);
            let k = self.fade_fast / 2.5 * 256.0;
            Some(color.map(|c| (c as f32 * k / 255.0) as i32))
        } else {
            None
        };
        if self.blink_count > 0 {
            if hud.time > self.blink_time {
                self.blink_time = hud.time + 0.1;
                self.blink_count -= 1;
            }
            if self.blink_count % 2 == 1 {
                color = YELLOWISH;
            }
        }
        let screen = eng.screen();
        let (w, h) = (screen.width as i32, screen.height as i32);
        let fh = hud.font_height;
        let dw = hud.digit_width();
        let x = w - if w > 639 { 16 } else { 12 } - 5 * dw;
        let y = if hud.has_suit() { h - 3 * fh } else { h - 5 * fh };
        if let Some(dollar) = hud.sprite_index("dollar") {
            hud.draw_sprite(screen, dollar, x - hud.sprite_size(dollar).0, y, color);
        }
        hud.draw_number5(screen, x, y, self.amount, color);
        if let Some(dc) = delta_color.filter(|_| self.show_delta && self.delta != 0) {
            let dy = y - (fh + fh / 2);
            if let Some(sign) = hud.sprite_index(if self.delta > 0 { "plus" } else { "minus" }) {
                hud.draw_sprite(screen, sign, x - hud.sprite_size(sign).0, dy, dc);
            }
            hud.draw_number5(screen, x, dy, self.delta.abs(), dc);
        }
    }
}

/// `CHudRoundTimer`.
#[derive(Default)]
pub(super) struct RoundTimer {
    new_period: Option<f32>,
    time_end: f32,
    panic: bool,
    next_toggle: f32,
    /// `m_closestRight`: where the scenario icon goes.
    pub(super) closest_right: i32,
}

impl RoundTimer {
    /// `MsgFunc_RoundTime`.
    pub(super) fn msg_round_time(&mut self, r: &mut MsgReader) {
        self.new_period = Some(r.short() as f32);
        self.panic = false;
    }

    /// `m_flTimeEnd`.
    pub(super) fn time_end(&self) -> f32 {
        self.time_end
    }

    pub(super) fn draw(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        if let Some(period) = self.new_period.take() {
            self.time_end = hud.time + period;
        }
        if hud.hide & hide::TIMER != 0 || !hud.show_timer || hud.spectator_gui_visible() {
            return;
        }
        let t = ((self.time_end - hud.time) as i32).max(0);
        let interval = match t {
            ..=2 => Some(0.05),
            3..=5 => Some(0.1),
            6..=10 => Some(0.2),
            11..=20 => Some(0.4),
            21..=30 => Some(0.8),
            _ => None,
        };
        if let Some(i) = interval
            && hud.time > self.next_toggle
        {
            self.panic = !self.panic;
            self.next_toggle = hud.time + i;
        }
        let color = if t == 0 || self.panic { EMPTY } else { scale(YELLOWISH, 100) };
        let legacy_tall = {
            let screen = eng.screen();
            let font = screen.font("Legacy_CreditsFont");
            screen.font_tall(font)
        };
        let screen = eng.screen();
        let (w, h) = (screen.width as i32, screen.height as i32);
        let fh = hud.font_height;
        let dw = hud.digit_width();
        let mut x = (w - dw * if w <= 639 { 5 } else { 4 }) / 2;
        let y = if hud.has_suit() { h - fh - fh / 2 } else { 42 };
        if w > 399
            && let Some(watch) = hud.sprite_index("stopwatch")
        {
            hud.draw_sprite(screen, watch, x - hud.sprite_size(watch).0, y, color);
        }
        x = hud.draw_number(screen, x, y, dhn::DRAWZERO | dhn::TWO_DIGITS, t / 60, color);
        x += screen.draw_string(x, y - (legacy_tall - fh) / 2 - 1, ":", color[0], color[1], color[2]);
        x = hud.draw_number(screen, x, y, dhn::DRAWZERO | dhn::TWO_DIGITS | dhn::FILLZERO, t % 60, color);
        self.closest_right = (1.5 * dw as f32) as i32 + x;
    }
}
