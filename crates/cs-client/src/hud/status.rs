//! `CHudStatusIcons`, `CHudProgressBar`, `CHudScenarioStatus` and `CHudNightVision`
//! (docs/re/client-hud.md §5.7, §5.8, §5.10, §5.14).

use gs_draw::Screen;

use super::{Hud, HudEngine, MsgReader, YELLOWISH, hide, scale};

#[derive(Debug, Clone)]
struct Icon {
    name: String,
    /// `hud.txt` index.
    sprite: Option<usize>,
    color: [i32; 3],
    flash: bool,
}

/// `CHudStatusIcons`: four slots stacked upward from mid-screen.
#[derive(Default, Clone)]
pub(super) struct StatusIcons {
    icons: [Option<Icon>; 4],
    flash_on: bool,
    next_flash: f32,
}

impl StatusIcons {
    pub(super) fn reset(&mut self) {
        self.icons = Default::default();
    }

    /// Whether `name` is shown (`InBuyZone` asks for "buyzone").
    pub(super) fn has(&self, name: &str) -> bool {
        self.icons.iter().flatten().any(|i| i.name.eq_ignore_ascii_case(name))
    }

    /// `MsgFunc_StatusIcon`.
    pub(super) fn msg_status_icon(&mut self, r: &mut MsgReader, hud: &Hud, eng: &mut dyn HudEngine) {
        let status = r.byte();
        let name = r.string();
        if status == 0 {
            self.disable(&name);
            return;
        }
        let color = [r.byte(), r.byte(), r.byte()];
        self.enable(name, color, status == 2, hud, eng);
    }

    /// `EnableIcon`: the slot with this name, else the first free one, else slot 0.
    pub(super) fn enable(&mut self, name: String, color: [i32; 3], flash: bool, hud: &Hud, eng: &mut dyn HudEngine) {
        let index = self
            .icons
            .iter()
            .position(|s| s.as_ref().is_some_and(|i| i.name.eq_ignore_ascii_case(&name)))
            .or_else(|| self.icons.iter().position(Option::is_none))
            .unwrap_or(0);
        if name.contains("grenade") {
            eng.play_sound("weapons/timer.wav", 1.0);
        }
        let sprite = hud.sprite_index(&name);
        self.icons[index] = Some(Icon { name, sprite, color, flash });
    }

    /// `DisableIcon`.
    pub(super) fn disable(&mut self, name: &str) {
        for slot in &mut self.icons {
            if slot.as_ref().is_some_and(|i| i.name.eq_ignore_ascii_case(name)) {
                *slot = None;
            }
        }
    }

    pub(super) fn draw(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        // HLTV shows no status icons.
        if eng.spectate_only() || self.icons.iter().all(Option::is_none) {
            return;
        }
        if hud.time >= self.next_flash {
            self.flash_on = !self.flash_on;
            self.next_flash = hud.time + 0.1;
        }
        let x = hud.sprite_index("cross").map_or(0, |c| hud.sprite_size(c).0 / 2);
        let screen = eng.screen();
        let mut y = screen.height as i32 / 2;
        for icon in self.icons.iter().flatten() {
            let Some(s) = icon.sprite else { continue };
            y -= hud.sprite_size(s).1 + 5;
            let c = if icon.flash && !self.flash_on { [250, 0, 0] } else { icon.color };
            hud.draw_sprite(screen, s, x, y, c);
        }
    }
}

/// `CHudProgressBar`: bomb plant/defuse and hostage use timers.
#[derive(Default, Clone)]
pub(super) struct ProgressBar {
    width: i32,
    x0: i32,
    x1: i32,
    y0: i32,
    y1: i32,
    start: f32,
    end: f32,
}

impl ProgressBar {
    /// Geometry from the screen size; no bar.
    pub(super) fn reset(&mut self, screen: &Screen) {
        self.width = (screen.width as f32 * 0.5) as i32;
        self.x0 = self.width / 2;
        self.x1 = self.x0 + self.width - 1;
        self.y0 = (screen.height as f32 * 0.5 + 64.0) as i32;
        self.y1 = self.y0 + 9;
        self.start = 0.0;
        self.end = -1.0;
    }

    fn set(&mut self, period: f32, pre_elapsed: f32, now: f32) {
        if period <= 0.0 {
            self.end = -1.0;
            return;
        }
        self.start = now - pre_elapsed;
        self.end = now + period - pre_elapsed;
    }

    /// `MsgFunc_BarTime`.
    pub(super) fn msg_bar_time(&mut self, r: &mut MsgReader, now: f32) {
        let seconds = r.short() as f32;
        self.set(seconds, 0.0, now);
    }

    /// `MsgFunc_BarTime2`.
    pub(super) fn msg_bar_time2(&mut self, r: &mut MsgReader, now: f32) {
        let seconds = r.short() as f32;
        let percent = r.short() as f32;
        self.set(seconds, seconds * percent / 100.0, now);
    }

    pub(super) fn draw(&self, hud: &Hud, eng: &mut dyn HudEngine) {
        if !(self.start <= hud.time && hud.time <= self.end) {
            return;
        }
        let screen = eng.screen();
        let [r, g, b] = YELLOWISH;
        screen.fill_rgba(self.x0, self.y0, 1, 10, r, g, b, 80);
        screen.fill_rgba(self.x1, self.y0, 1, 10, r, g, b, 80);
        screen.fill_rgba(self.x0 + 1, self.y0, self.width - 2, 1, r, g, b, 80);
        screen.fill_rgba(self.x0 + 1, self.y1, self.width - 2, 1, r, g, b, 80);
        let f = (hud.time - self.start) / (self.end - self.start);
        let fill = ((((self.width - 4) as f32) * f) as i32).min(self.width - 4);
        screen.fill_rgba(self.x0 + 2, self.y0 + 2, fill, 6, r, g, b, 160);
    }
}

/// `CHudScenarioStatus`: the objective icon next to the round timer.
#[derive(Default, Clone)]
pub(super) struct Scenario {
    active: bool,
    sprite: Option<usize>,
    alpha: f32,
    flash_alpha: f32,
    interval: f32,
    next_flash: f32,
}

impl Scenario {
    pub(super) fn reset(&mut self) {
        self.active = false;
    }

    /// `MsgFunc_Scenario`.
    pub(super) fn msg_scenario(&mut self, r: &mut MsgReader, hud: &Hud, now: f32) {
        if r.byte() == 0 {
            self.active = false;
            return;
        }
        let name = r.string();
        let alpha = r.byte() as f32;
        self.sprite = hud.sprite_index(&name);
        self.flash_alpha = alpha.max(100.0);
        if !self.active {
            self.alpha = alpha;
        }
        self.active = true;
        if self.alpha != 100.0 {
            self.interval = r.short() as f32 / 100.0;
            if self.next_flash == 0.0 {
                self.next_flash = now;
            }
            self.next_flash += r.short() as f32 / 100.0;
        }
    }

    pub(super) fn draw(&mut self, hud: &Hud, eng: &mut dyn HudEngine, x: i32) {
        if !self.active || hud.hide & hide::HEALTH != 0 || hud.state.user1 != 0 || eng.spectate_only() {
            return;
        }
        let Some(s) = self.sprite else { return };
        let screen = eng.screen();
        let fh = hud.font_height;
        let y = screen.height as i32 - 3 * fh / 2 - (hud.sprite_size(s).1 - fh) / 2;
        hud.draw_sprite(screen, s, x, y, scale(YELLOWISH, self.alpha as i32));
        if self.alpha > 100.0 {
            self.alpha *= 0.9;
        }
        if self.interval != 0.0 && hud.time >= self.next_flash {
            self.next_flash = hud.time + self.interval;
            self.alpha = self.flash_alpha;
        }
    }
}

/// `CHudNightVision`: the green full-screen tint.
#[derive(Clone)]
pub(super) struct NightVision {
    pub(super) on: bool,
    pub(super) alpha: i32,
}

impl Default for NightVision {
    fn default() -> Self {
        Self { on: false, alpha: 110 }
    }
}

impl NightVision {
    /// `CHudNightVision::Draw`: the tint, then `EV_HLDM_CreateLight(radius 750..800, (1,20,1),
    /// 0.1 s)` at `gHUD.m_vecOrigin` (a new unkeyed slot every frame).
    pub(super) fn draw(&self, eng: &mut dyn HudEngine, origin: [f32; 3], time: f32) {
        if !self.on || eng.spectate_only() {
            return;
        }
        let screen = eng.screen();
        let (w, h) = (screen.width as i32, screen.height as i32);
        screen.fill_rgba(0, 0, w, h, 50, 225, 50, self.alpha);
        let radius = (eng.random_long(0, 50) + 750) as f32;
        eng.create_light(origin, radius, [1, 20, 1], time + 0.1);
    }
}
