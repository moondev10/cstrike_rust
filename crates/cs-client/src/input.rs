//! CS client input (docs/re/client-sim.md Part A): the `kbutton_t` state machine behind the
//! `+x`/`-x` commands, `CL_AdjustAngles`, `CL_CreateMove`, `CL_ButtonBits`, `IN_MouseMove` and
//! `IN_ScaleMouse`, and the pitch drift starts and stops they make. The cvars they read are
//! registered from [`crate::cvars`].
//!
//! Not modelled: joystick input (`IN_JoyMove`; off by default, and no controller API here).
//! The camera commands live in [`crate::cam`].

use gs_console::Args;
use gs_console::cvar::Cvars;
use gs_protocol::structs::UserCmd;

/// `IN_*` button bits.
pub mod in_bits {
    pub const ATTACK: u16 = 0x0001;
    pub const JUMP: u16 = 0x0002;
    pub const DUCK: u16 = 0x0004;
    pub const FORWARD: u16 = 0x0008;
    pub const BACK: u16 = 0x0010;
    pub const USE: u16 = 0x0020;
    pub const LEFT: u16 = 0x0080;
    pub const RIGHT: u16 = 0x0100;
    pub const MOVELEFT: u16 = 0x0200;
    pub const MOVERIGHT: u16 = 0x0400;
    pub const ATTACK2: u16 = 0x0800;
    pub const RELOAD: u16 = 0x2000;
    pub const ALT1: u16 = 0x4000;
    pub const SCORE: u16 = 0x8000;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    Up,
    Down,
    Left,
    Right,
    Forward,
    Back,
    LookUp,
    LookDown,
    Strafe,
    MoveLeft,
    MoveRight,
    Speed,
    Attack,
    Attack2,
    Use,
    Jump,
    KLook,
    MLook,
    JLook,
    Duck,
    Reload,
    Alt1,
    Score,
    Graph,
    Break,
}

const BUTTON_COUNT: usize = Button::Break as usize + 1;

/// `+name`/`-name` commands and their buttons.
const BUTTON_COMMANDS: &[(&str, Button)] = &[
    ("moveup", Button::Up),
    ("movedown", Button::Down),
    ("left", Button::Left),
    ("right", Button::Right),
    ("forward", Button::Forward),
    ("back", Button::Back),
    ("lookup", Button::LookUp),
    ("lookdown", Button::LookDown),
    ("strafe", Button::Strafe),
    ("moveleft", Button::MoveLeft),
    ("moveright", Button::MoveRight),
    ("speed", Button::Speed),
    ("attack", Button::Attack),
    ("attack2", Button::Attack2),
    ("use", Button::Use),
    ("jump", Button::Jump),
    ("klook", Button::KLook),
    ("mlook", Button::MLook),
    ("jlook", Button::JLook),
    ("duck", Button::Duck),
    ("reload", Button::Reload),
    ("alt1", Button::Alt1),
    ("score", Button::Score),
    ("showscores", Button::Score),
    ("graph", Button::Graph),
    ("break", Button::Break),
];

/// `strtol(s, 0, 10)`: optional whitespace and sign, then digits; 0 when none.
pub(crate) fn strtol(s: &str) -> i32 {
    let t = s.trim_start();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let v = digits.bytes().take_while(u8::is_ascii_digit).fold(0i64, |v, d| (v * 10 + i64::from(d - b'0')).min(i64::from(i32::MAX) + 1));
    let v = if neg { -v } else { v };
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// `kbutton_t`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KButton {
    /// Key numbers holding the button; -1 = typed at the console.
    down: [i32; 2],
    /// 1 held, 2 went down, 4 went up since the last consumption.
    state: i32,
}

impl KButton {
    /// `KeyDown`.
    pub(crate) fn key_down(&mut self, arg: &str) {
        let k = if arg.is_empty() { -1 } else { strtol(arg) };
        if k == self.down[0] || k == self.down[1] {
            return;
        }
        if self.down[0] == 0 {
            self.down[0] = k;
        } else if self.down[1] == 0 {
            self.down[1] = k;
        } else {
            log::debug!("Three keys down for a button!");
            return;
        }
        if self.state & 1 == 0 {
            self.state |= 3;
        }
    }

    /// `KeyUp`.
    pub(crate) fn key_up(&mut self, arg: &str) {
        if arg.is_empty() {
            self.down = [0, 0];
            self.state = 4;
            return;
        }
        let k = strtol(arg);
        if self.down[0] == k {
            self.down[0] = 0;
        } else if self.down[1] == k {
            self.down[1] = 0;
        } else {
            return;
        }
        if self.down[0] != 0 || self.down[1] != 0 {
            return;
        }
        if self.state & 1 != 0 {
            self.state = (self.state & !5) | 4;
        }
    }

    /// `CL_KeyState`: how much of the frame the button was down; consumes the impulse bits.
    pub(crate) fn key_state(&mut self) -> f32 {
        let held = self.state & 1 != 0;
        let went_down = self.state & 2 != 0;
        let went_up = self.state & 4 != 0;
        let value = match (held, went_down, went_up) {
            (true, false, false) => 1.0,
            (true, true, false) => 0.5,
            (false, true, true) => 0.25,
            (true, true, true) => 0.75,
            _ => 0.0,
        };
        self.state &= 1;
        value
    }

    fn held(&self) -> bool {
        self.state & 1 != 0
    }

    /// Contributes to `CL_ButtonBits`: held or pressed this frame.
    fn active(&self) -> bool {
        self.state & 3 != 0
    }
}

/// `anglemod`: yaw quantised to 1/65536 of a turn (f64 inside).
pub fn anglemod(a: f32) -> f32 {
    let units = (f64::from(a) * (65536.0 / 360.0)).trunc() as i32 as u16;
    ((360.0 / 65536.0) * f64::from(units)) as f32
}

/// Inputs of one `CL_CreateMove`.
#[derive(Debug, Clone, Copy)]
pub struct MoveParams<'a> {
    pub cvars: &'a Cvars,
    /// `(float)host_frametime`.
    pub frametime: f32,
    /// `cls.signon == 2`.
    pub active: bool,
    /// `cl.maxspeed` from the last clientdata.
    pub maxspeed: f32,
    /// `gHUD.m_Health.m_iHealth`.
    pub health: i32,
    /// `gHUD.m_iIntermission`.
    pub intermission: bool,
    /// `gHUD.m_flMouseSensitivity` (0 = use `sensitivity`).
    pub hud_sensitivity: f32,
    /// `gHUD.m_iFOV`.
    pub hud_fov: i32,
    /// The local player is noclipping (`IsNoClipping`).
    pub noclip: bool,
    /// `GetClientTime()` (`cl.time`; pitch drift stops and starts).
    pub time: f64,
    /// `vgui2::surface()->IsCursorVisible()`: a menu takes the mouse.
    pub cursor_visible: bool,
}

/// The client DLL's input state.
#[derive(Debug, Clone)]
pub struct Input {
    buttons: [KButton; BUTTON_COUNT],
    /// `in_impulse`.
    impulse: i32,
    /// `g_weaponselect` (set by the HUD weapon menu).
    pub weaponselect: i32,
    /// `mx_accum`/`my_accum`: relative mouse motion since the last move.
    accum: (i32, i32),
    /// `old_mouse_x/y` for `m_filter`.
    old_mouse: (i32, i32),
    /// `oldangles`: the angles sent while dead.
    oldangles: [f32; 3],
    /// `mouseactive`.
    pub mouse_active: bool,
    /// The third-person camera (`CAM_*`).
    pub cam: crate::cam::Camera,
    /// `V_DriftPitch`'s state, stopped and started by the look controls.
    pub drift: crate::view::PitchDrift,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            buttons: [KButton::default(); BUTTON_COUNT],
            impulse: 0,
            weaponselect: 0,
            accum: (0, 0),
            old_mouse: (0, 0),
            oldangles: [0.0; 3],
            mouse_active: false,
            cam: crate::cam::Camera::default(),
            drift: crate::view::PitchDrift::default(),
        }
    }
}

impl Input {
    fn button(&mut self, b: Button) -> &mut KButton {
        &mut self.buttons[b as usize]
    }

    /// `KB_Find("in_mlook")->state & 1` (`writecfg`'s `+mlook` line).
    pub fn mlook_down(&self) -> bool {
        self.buttons[Button::MLook as usize].held()
    }

    /// `KB_Find("in_jlook")->state & 1`.
    pub fn jlook_down(&self) -> bool {
        self.buttons[Button::JLook as usize].held()
    }

    /// Runs an input command; `false` if `args` is not one. `viewangles` is `cl.viewangles`
    /// (`force_centerview` levels the pitch); `time` is `GetClientTime()` (`-mlook` with
    /// `lookspring` starts the pitch drift).
    pub fn command(&mut self, args: &Args, viewangles: &mut [f32; 3], cvars: &Cvars, time: f64) -> bool {
        let name = args.arg(0);
        if name.eq_ignore_ascii_case("impulse") {
            self.impulse = strtol(args.arg(1));
            return true;
        }
        // Force_CenterView_f: not while the camera has the mouse.
        if name.eq_ignore_ascii_case("force_centerview") {
            if !self.cam.mouse_in_use {
                viewangles[0] = 0.0;
            }
            return true;
        }
        let (down, rest) = match name.as_bytes().first() {
            Some(b'+') => (true, &name[1..]),
            Some(b'-') => (false, &name[1..]),
            _ => return false,
        };
        let Some(&(_, b)) = BUTTON_COMMANDS.iter().find(|(n, _)| n.eq_ignore_ascii_case(rest)) else { return false };
        let arg = args.arg(1);
        if down {
            self.button(b).key_down(arg);
        } else {
            self.button(b).key_up(arg);
            // IN_MLookUp.
            if b == Button::MLook && !self.button(b).held() && cvars.value("lookspring") != 0.0 {
                self.drift.start(time, cvars.value("v_centerspeed"));
            }
        }
        true
    }

    /// `CL_ButtonBits(0) & IN_ATTACK`: held or pressed this frame (no reset).
    pub fn attack_active(&self) -> bool {
        self.buttons[Button::Attack as usize].active()
    }

    /// `ClientDLL_FindKey("in_graph")->state & 1`: `+graph` is held (net_graph level 2).
    pub fn graph_held(&self) -> bool {
        self.buttons[Button::Graph as usize].state & 1 != 0
    }

    /// `CL_ResetButtonBits` without `IN_ATTACK`: the click is released entirely.
    pub fn release_attack(&mut self) {
        self.button(Button::Attack).state &= !7;
    }

    /// `IN_Accumulate`; nothing while the camera has the mouse or the VGUI cursor shows.
    pub fn accumulate_mouse(&mut self, dx: i32, dy: i32, cursor_visible: bool) {
        if self.mouse_active && !self.cam.mouse_in_use && !cursor_visible {
            self.accum.0 += dx;
            self.accum.1 += dy;
        }
    }

    /// `IN_ClearStates`.
    pub fn clear_mouse(&mut self) {
        if self.mouse_active {
            self.accum = (0, 0);
        }
    }

    /// `CL_ButtonBits(bResetState)`.
    pub fn button_bits(&mut self, reset: bool, health: i32, intermission: bool) -> u16 {
        let map: [(Button, u16); 14] = [
            (Button::Attack, in_bits::ATTACK),
            (Button::Attack2, in_bits::ATTACK2),
            (Button::Duck, in_bits::DUCK),
            (Button::MoveRight, in_bits::MOVERIGHT),
            (Button::Reload, in_bits::RELOAD),
            (Button::Alt1, in_bits::ALT1),
            (Button::Jump, in_bits::JUMP),
            (Button::Score, in_bits::SCORE),
            (Button::Forward, in_bits::FORWARD),
            (Button::Back, in_bits::BACK),
            (Button::Use, in_bits::USE),
            (Button::Left, in_bits::LEFT),
            (Button::Right, in_bits::RIGHT),
            (Button::MoveLeft, in_bits::MOVELEFT),
        ];
        let mut bits = map.iter().filter(|(b, _)| self.buttons[*b as usize].active()).fold(0, |acc, (_, bit)| acc | bit);
        if health <= 0 || intermission {
            bits |= in_bits::SCORE;
        }
        if reset {
            for (b, _) in map {
                self.button(b).state &= !2;
            }
        }
        bits
    }

    /// `CL_AdjustAngles`: keyboard turning; yaw is anglemodded every frame.
    fn adjust_angles(&mut self, cvars: &Cvars, mut frametime: f32, time: f64, va: &mut [f32; 3]) {
        if self.button(Button::Speed).held() {
            frametime *= cvars.value("cl_anglespeedkey");
        }
        let yawspeed = cvars.value("cl_yawspeed").clamp(50.0, 250.0);
        let pitchspeed = cvars.value("cl_pitchspeed").clamp(50.0, 250.0);
        if !self.button(Button::Strafe).held() {
            let s = yawspeed * frametime;
            va[1] -= s * self.button(Button::Right).key_state();
            let left = self.button(Button::Left).key_state();
            va[1] = (f64::from(left) * f64::from(s) + f64::from(va[1])) as f32;
            va[1] = anglemod(va[1]);
        }
        let p = pitchspeed * frametime;
        if self.button(Button::KLook).held() {
            self.drift.stop(time);
            va[0] -= p * self.button(Button::Forward).key_state();
            // client.dll adds this term on the x87 stack: one rounding, as for +left above.
            let back = self.button(Button::Back).key_state();
            va[0] = (f64::from(back) * f64::from(p) + f64::from(va[0])) as f32;
        }
        let up = self.button(Button::LookUp).key_state();
        let down = self.button(Button::LookDown).key_state();
        va[0] = va[0] - p * up + p * down;
        if up != 0.0 || down != 0.0 {
            self.drift.stop(time);
        }
        va[0] = va[0].min(cvars.value("cl_pitchdown")).max(-cvars.value("cl_pitchup"));
        va[2] = va[2].clamp(-50.0, 50.0);
    }

    /// `IN_ScaleMouse`.
    fn scale_mouse(cvars: &Cvars, p: &MoveParams, mx: &mut f32, my: &mut f32) {
        let base = if p.hud_sensitivity != 0.0 { p.hud_sensitivity } else { cvars.value("sensitivity") };
        let accel = cvars.value("m_customaccel");
        if accel == 0.0 {
            *mx *= base;
            *my *= base;
            return;
        }
        let raw = f64::from(*mx * *mx + *my * *my).sqrt() as f32;
        let mut a = f64::from(raw).powf(f64::from(cvars.value("m_customaccel_exponent"))) as f32 * cvars.value("m_customaccel_scale") + base;
        let max = cvars.value("m_customaccel_max");
        if max > 0.0001 {
            a = a.min(max);
        }
        let default_fov = cvars.value("default_fov");
        if p.hud_fov as f32 != default_fov {
            a = a * (p.hud_fov as f32 / default_fov) * cvars.value("zoom_sensitivity_ratio");
        }
        *mx *= a;
        *my *= a;
        if accel == 2.0 {
            *mx *= cvars.value("m_yaw");
            *my *= cvars.value("m_pitch");
        }
    }

    /// `IN_MouseMove`.
    fn mouse_move(&mut self, p: &MoveParams, va: &mut [f32; 3], cmd: &mut UserCmd) {
        if self.button(Button::MLook).held() {
            self.drift.stop(p.time);
        }
        // The mouse is ignored (and the accumulators kept) in intermission or under the VGUI
        // cursor; +mlook has already stopped the pitch drift.
        if p.intermission || p.cursor_visible {
            return;
        }
        let cvars = p.cvars;
        let (mx, my) = std::mem::take(&mut self.accum);
        let (mut mouse_x, mut mouse_y) = if cvars.value("m_filter") != 0.0 {
            ((f64::from(mx + self.old_mouse.0) * 0.5) as f32, (f64::from(my + self.old_mouse.1) * 0.5) as f32)
        } else {
            (mx as f32, my as f32)
        };
        self.old_mouse = (mx, my);
        Self::scale_mouse(cvars, p, &mut mouse_x, &mut mouse_y);
        let strafe = self.button(Button::Strafe).held();
        let mlook = self.button(Button::MLook).held();
        if !strafe && (cvars.value("lookstrafe") == 0.0 || !mlook) {
            va[1] -= cvars.value("m_yaw") * mouse_x;
        } else {
            cmd.sidemove += cvars.value("m_side") * mouse_x;
        }
        if mlook && !strafe {
            va[0] += cvars.value("m_pitch") * mouse_y;
            va[0] = va[0].min(cvars.value("cl_pitchdown")).max(-cvars.value("cl_pitchup"));
        } else if strafe && p.noclip {
            cmd.upmove -= cvars.value("m_forward") * mouse_y;
        } else {
            cmd.forwardmove -= cvars.value("m_forward") * mouse_y;
        }
    }

    /// `CL_CreateMove`: `viewangles` is `cl.viewangles`, updated by keyboard and mouse.
    pub fn create_move(&mut self, p: &MoveParams, viewangles: &mut [f32; 3]) -> UserCmd {
        let cvars = p.cvars;
        let mut cmd = UserCmd::default();
        if p.active {
            self.adjust_angles(cvars, p.frametime, p.time, viewangles);
            if self.button(Button::Strafe).held() {
                cmd.sidemove += cvars.value("cl_sidespeed") * self.button(Button::Right).key_state();
                cmd.sidemove -= cvars.value("cl_sidespeed") * self.button(Button::Left).key_state();
            }
            cmd.sidemove += cvars.value("cl_sidespeed") * self.button(Button::MoveRight).key_state();
            cmd.sidemove -= cvars.value("cl_sidespeed") * self.button(Button::MoveLeft).key_state();
            cmd.upmove += cvars.value("cl_upspeed") * self.button(Button::Up).key_state();
            cmd.upmove -= cvars.value("cl_upspeed") * self.button(Button::Down).key_state();
            if !self.button(Button::KLook).held() {
                cmd.forwardmove += cvars.value("cl_forwardspeed") * self.button(Button::Forward).key_state();
                cmd.forwardmove -= cvars.value("cl_backspeed") * self.button(Button::Back).key_state();
            }
            // IN_Move: the mouse only while the camera does not hold it.
            if self.mouse_active && !self.cam.mouse_in_use {
                self.mouse_move(p, viewangles, &mut cmd);
            }
            if p.maxspeed != 0.0 {
                let len = f64::from(cmd.forwardmove * cmd.forwardmove + cmd.sidemove * cmd.sidemove + cmd.upmove * cmd.upmove).sqrt() as f32;
                if len > p.maxspeed {
                    let r = p.maxspeed / len;
                    cmd.forwardmove *= r;
                    cmd.sidemove *= r;
                    cmd.upmove *= r;
                }
            }
            if self.button(Button::Speed).held() {
                let k = cvars.value("cl_movespeedkey").min(0.52);
                cmd.forwardmove *= k;
                cmd.sidemove *= k;
                cmd.upmove *= k;
            }
        }
        cmd.impulse = std::mem::take(&mut self.impulse) as u8;
        cmd.weaponselect = std::mem::take(&mut self.weaponselect) as u8;
        cmd.buttons = self.button_bits(true, p.health, p.intermission);
        // The squelch-mode IN_ATTACK clear never fires (StartSquelchMode has no callers).
        // `joystick` on: the forward move's sign sets IN_FORWARD / IN_BACK.
        if cvars.value("joystick") != 0.0 {
            if cmd.forwardmove > 0.0 {
                cmd.buttons |= in_bits::FORWARD;
            } else if cmd.forwardmove < 0.0 {
                cmd.buttons |= in_bits::BACK;
            }
        }
        if p.health > 0 {
            cmd.viewangles = *viewangles;
            self.oldangles = *viewangles;
        } else {
            cmd.viewangles = self.oldangles;
        }
        cmd
    }
}

/// `CHud`'s `m_flMouseSensitivity` for a `SetFOV` value: 0 at the default FOV, else the
/// zoom-scaled sensitivity.
pub fn hud_mouse_sensitivity(cvars: &Cvars, fov: i32) -> f32 {
    let default_fov = cvars.value("default_fov");
    let ifov = if fov != 0 { fov } else { default_fov as i32 };
    if ifov as f32 == default_fov {
        0.0
    } else {
        (ifov as f32 / default_fov) * cvars.value("sensitivity") * cvars.value("zoom_sensitivity_ratio")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cvars() -> Cvars {
        let mut c = Cvars::default();
        for &(n, v, f) in crate::cvars::CVARS {
            c.register(n, v, f);
        }
        c
    }

    fn run(input: &mut Input, line: &str) {
        let mut va = [0.0; 3];
        assert!(input.command(&gs_common::token::tokenize(line), &mut va, &cvars(), 0.0), "{line}");
    }

    fn params(c: &Cvars) -> MoveParams<'_> {
        MoveParams {
            cvars: c,
            frametime: 0.01,
            active: true,
            maxspeed: 250.0,
            health: 100,
            intermission: false,
            hud_sensitivity: 0.0,
            hud_fov: 90,
            noclip: false,
            time: 0.0,
            cursor_visible: false,
        }
    }

    #[test]
    fn key_state_reports_partial_frames_and_consumes_impulses() {
        let mut b = KButton::default();
        b.key_down("87");
        assert_eq!(b.key_state(), 0.5);
        assert_eq!(b.key_state(), 1.0);
        b.key_up("87");
        assert_eq!(b.key_state(), 0.0);
        b.key_down("87");
        b.key_up("87");
        assert_eq!(b.key_state(), 0.25, "tap inside one frame");
    }

    #[test]
    fn a_button_stays_down_until_every_key_releases_it() {
        let mut b = KButton::default();
        b.key_down("119");
        b.key_down("241");
        b.key_up("119");
        assert!(b.held());
        b.key_up("241");
        assert!(!b.held());
        b.key_down("");
        b.key_up("5");
        assert!(b.held(), "a key that never went down cannot release it");
        b.key_up("");
        assert!(!b.held(), "an argument-less release forces it up");
    }

    #[test]
    fn create_move_scales_keys_clips_to_maxspeed_and_walks() {
        let c = cvars();
        let mut input = Input::default();
        let mut va = [0.0; 3];
        run(&mut input, "+forward 119");
        run(&mut input, "+moveright 100");
        let cmd = input.create_move(&params(&c), &mut va);
        // Pressed this frame: 0.5 · 400 each, length 282.8 > 250 → scaled.
        let r = 250.0 / (f64::from(200.0f32 * 200.0 + 200.0 * 200.0)).sqrt() as f32;
        assert_eq!((cmd.forwardmove, cmd.sidemove), (200.0 * r, 200.0 * r));
        assert_eq!(cmd.buttons, in_bits::FORWARD | in_bits::MOVERIGHT, "held keys keep their bits after CL_KeyState");
        run(&mut input, "+speed 304");
        let cmd = input.create_move(&params(&c), &mut va);
        let r = 250.0 / (f64::from(400.0f32 * 400.0 + 400.0 * 400.0)).sqrt() as f32;
        assert_eq!(cmd.forwardmove, 400.0 * r * 0.52);
        assert_eq!(cmd.buttons, in_bits::FORWARD | in_bits::MOVERIGHT);
    }

    #[test]
    fn sub_frame_taps_still_send_attack_and_jump() {
        let c = cvars();
        let mut input = Input::default();
        let mut va = [0.0; 3];
        run(&mut input, "+attack 241");
        run(&mut input, "-attack 241");
        run(&mut input, "impulse 201");
        let cmd = input.create_move(&params(&c), &mut va);
        assert_eq!(cmd.buttons, in_bits::ATTACK);
        assert_eq!(cmd.impulse, 201);
        let cmd = input.create_move(&params(&c), &mut va);
        assert_eq!((cmd.buttons, cmd.impulse), (0, 0));
    }

    #[test]
    fn mouse_turns_with_sensitivity_and_pitch_is_clamped() {
        let c = cvars();
        let mut input = Input { mouse_active: true, ..Input::default() };
        run(&mut input, "+mlook");
        let mut va = [80.0, 10.0, 0.0];
        input.accumulate_mouse(100, 1000, false);
        let cmd = input.create_move(&params(&c), &mut va);
        assert_eq!(va[0], 89.0);
        assert_eq!(va[1], anglemod(10.0) - 0.022 * 300.0, "yaw is anglemodded before the mouse turns it");
        assert_eq!(cmd.viewangles, va);
        input.accumulate_mouse(0, -10, false);
        let cmd = input.create_move(&params(&c), &mut va);
        assert_eq!(cmd.viewangles[0], 89.0 - 0.022 * 30.0);
    }

    #[test]
    fn dead_players_send_their_last_living_angles_and_scores() {
        let c = cvars();
        let mut input = Input { mouse_active: true, ..Input::default() };
        run(&mut input, "+mlook");
        let mut va = [5.0, 90.0, 0.0];
        input.create_move(&params(&c), &mut va);
        input.accumulate_mouse(50, 0, false);
        let dead = MoveParams { health: 0, ..params(&c) };
        let cmd = input.create_move(&dead, &mut va);
        assert_ne!(va[1], 90.0);
        assert_eq!(cmd.viewangles, [5.0, anglemod(90.0), 0.0]);
        assert_eq!(cmd.buttons, in_bits::SCORE);
    }

    #[test]
    fn menu_cursor_keeps_the_view_and_the_pitch_drift_stopped() {
        let c = cvars();
        let mut input = Input { mouse_active: true, ..Input::default() };
        run(&mut input, "+mlook");
        let mut va = [10.0, 0.0, 0.0];
        input.accumulate_mouse(40, 40, false);
        let menu = MoveParams { cursor_visible: true, time: 5.0, ..params(&c) };
        input.accumulate_mouse(1000, 1000, true);
        input.create_move(&menu, &mut va);
        assert_eq!(va, [10.0, 0.0, 0.0], "no mouse look under the cursor");
        // V_StartPitchDrift refuses in the frame +mlook stopped it, cursor or not.
        input.drift.start(5.0, 20.0);
        let mut stopped = crate::view::PitchDrift::default();
        stopped.stop(5.0);
        assert_eq!(format!("{:?}", input.drift), format!("{stopped:?}"));
        input.create_move(&params(&c), &mut va);
        assert_eq!(va[0], 10.0 + 0.022 * 120.0, "motion from before the menu is kept, none from under it");
    }

    #[test]
    fn anglemod_quantises_to_sixteen_bits() {
        assert_eq!(anglemod(90.0), 90.0);
        assert_eq!(anglemod(-90.0), 270.0);
        assert_eq!(anglemod(0.001), 0.0);
    }

    #[test]
    fn zoom_scales_sensitivity_by_fov_and_ratio() {
        let c = cvars();
        assert_eq!(hud_mouse_sensitivity(&c, 90), 0.0);
        assert_eq!(hud_mouse_sensitivity(&c, 0), 0.0);
        assert_eq!(hud_mouse_sensitivity(&c, 10), (10.0 / 90.0) * 3.0 * 1.2);
    }
}
