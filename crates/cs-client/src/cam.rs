//! The third-person camera (`CAM_*`, docs/re/commands-render-cam.md §2, client-sim.md §8):
//! the 19 camera commands, `CAM_Think` and `CL_IsThirdPerson`'s camera part.

use gs_console::Args;
use gs_console::cvar::Cvars;

use crate::input::KButton;

/// The six camera kbuttons, in `CAM_Init` order.
const BUTTONS: [&str; 6] = ["campitchup", "campitchdown", "camyawleft", "camyawright", "camin", "camout"];

/// What a camera command or `CAM_Think` asks of the engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CamEffect {
    None,
    /// `Cvar_SetValue("cam_command", 0)`.
    ResetCommand,
    /// `cam_snapto->value = v` (value only).
    SnapTo(f32),
}

/// The camera state of client.dll.
#[derive(Debug, Clone, Default)]
pub struct Camera {
    /// `cam_thirdperson`.
    pub thirdperson: bool,
    mousemove: bool,
    distancemove: bool,
    /// `iMouseInUse`: mouse look, mouse buttons and `force_centerview` are off.
    pub mouse_in_use: bool,
    /// `cam_ofs`: pitch, yaw, distance.
    pub ofs: [f32; 3],
    buttons: [KButton; 6],
}

/// `(int)` of a float the x87 way: out of range or NaN gives `INT_MIN`.
fn c_int(x: f32) -> i32 {
    if x.is_nan() || !(-2_147_483_648.0..2_147_483_648.0).contains(&x) { i32::MIN } else { x as i32 }
}

/// `MoveToward`'s inline form in `CAM_Think` (x87): the goal is `view + ideal` unrounded; the
/// gap as a float truncated to int decides the ±360 shift past 180; within 1° it snaps, else a
/// quarter of the gap is stored as a float; the result is wrapped into [0, 360).
fn move_toward(cur: f32, goal: f64) -> f32 {
    let c = f64::from(cur);
    let v = if c == goal {
        c
    } else {
        let shifted = if f64::from(c_int((c - goal) as f32).wrapping_abs()) <= 180.0 {
            c
        } else if goal <= c {
            c - 360.0
        } else {
            c + 360.0
        };
        if goal <= shifted {
            if shifted <= goal + 1.0 { goal } else { f64::from((shifted - (shifted - goal) * 0.25) as f32) }
        } else if goal - 1.0 <= shifted {
            goal
        } else {
            f64::from((shifted + (goal - shifted) * 0.25) as f32)
        }
    };
    let v = if v < 0.0 {
        v + 360.0
    } else if v >= 360.0 {
        v - 360.0
    } else {
        v
    };
    v as f32
}

impl Camera {
    /// `CAM_ToThirdPerson`: refused on a multiplayer server (`GetMaxClients() > 1`).
    fn enter_third_person(&mut self, maxclients: i32, viewangles: [f32; 3]) -> CamEffect {
        if maxclients > 1 {
            return CamEffect::None;
        }
        if !self.thirdperson {
            self.thirdperson = true;
            self.ofs = [viewangles[0], viewangles[1], 30.0];
        }
        CamEffect::ResetCommand
    }

    /// `CAM_ToFirstPerson` (also the `ViewMode` user message).
    pub fn enter_first_person(&mut self) -> CamEffect {
        self.thirdperson = false;
        CamEffect::ResetCommand
    }

    /// The camera commands; `None` when `args` is not one. `cvars` gives `cam_snapto`.
    /// (`cam_old_mouse`, which `+cammousemove`/`+camdistance` write from `cam_mouse`, is never
    /// read and not kept.)
    pub fn command(&mut self, args: &Args, maxclients: i32, viewangles: [f32; 3], cvars: &Cvars) -> Option<CamEffect> {
        let name = args.arg(0).to_ascii_lowercase();
        if let Some(rest) = name.strip_prefix(['+', '-'])
            && let Some(i) = BUTTONS.iter().position(|b| *b == rest)
        {
            if name.starts_with('+') { self.buttons[i].key_down(args.arg(1)) } else { self.buttons[i].key_up(args.arg(1)) }
            return Some(CamEffect::None);
        }
        Some(match name.as_str() {
            "thirdperson" => self.enter_third_person(maxclients, viewangles),
            "firstperson" => self.enter_first_person(),
            "+cammousemove" => {
                if !self.thirdperson {
                    self.mousemove = false;
                    self.mouse_in_use = false;
                } else if !self.mousemove {
                    self.mousemove = true;
                    self.mouse_in_use = true;
                }
                CamEffect::None
            }
            "-cammousemove" => {
                self.mousemove = false;
                self.mouse_in_use = false;
                CamEffect::None
            }
            "+camdistance" => {
                if !self.thirdperson {
                    self.distancemove = false;
                    self.mousemove = false;
                    self.mouse_in_use = false;
                } else if !self.distancemove {
                    self.distancemove = true;
                    self.mousemove = true;
                    self.mouse_in_use = true;
                }
                CamEffect::None
            }
            "-camdistance" => {
                self.distancemove = false;
                self.mousemove = false;
                self.mouse_in_use = false;
                CamEffect::None
            }
            "snapto" => CamEffect::SnapTo(if cvars.value("cam_snapto") == 0.0 { 1.0 } else { 0.0 }),
            _ => return None,
        })
    }

    /// `CAM_Think` (each host frame from `cls.state` 2 on). `window_center` is the window's
    /// centre in screen coordinates; the ideal angles and distance go back as values only
    /// (`[pitch, yaw, dist]`).
    pub fn think(&mut self, cvars: &Cvars, maxclients: i32, viewangles: [f32; 3], window_center: (i32, i32)) -> (CamEffect, Option<[f32; 3]>) {
        let effect = match cvars.value("cam_command") as i32 {
            1 => self.enter_third_person(maxclients, viewangles),
            2 => self.enter_first_person(),
            _ => CamEffect::None,
        };
        if !self.thirdperson {
            return (effect, None);
        }
        let v = |name: &str| cvars.value(name);
        let (cx, cy) = window_center;
        let mut pitch = v("cam_idealpitch");
        let mut yaw = v("cam_idealyaw");
        // cam_mouse stays (0, 0), left of and above any positive centre.
        let (mx, my) = (0, 0);
        if self.mousemove && !self.distancemove {
            if mx > cx {
                if v("c_maxyaw") > yaw {
                    yaw += ((mx - cx) / 2) as f32 * 0.5;
                }
                yaw = yaw.min(v("c_maxyaw"));
            } else if mx < cx {
                if yaw > v("c_minyaw") {
                    yaw -= ((cx - mx) / 2) as f32 * 0.5;
                }
                yaw = if v("c_minyaw") <= yaw { yaw } else { v("c_minyaw") };
            }
            if my > cy {
                if v("c_maxpitch") > pitch {
                    pitch += ((my - cy) / 2) as f32 * 0.5;
                }
                pitch = pitch.min(v("c_maxpitch"));
            } else if my < cy {
                if pitch > v("c_minpitch") {
                    pitch -= ((cy - my) / 2) as f32 * 0.5;
                }
                pitch = if v("c_minpitch") <= pitch { pitch } else { v("c_minpitch") };
            }
        }
        if self.buttons[0].key_state() != 0.0 {
            pitch += 2.5;
        } else if self.buttons[1].key_state() != 0.0 {
            pitch -= 2.5;
        }
        if self.buttons[2].key_state() != 0.0 {
            yaw -= 2.5;
        } else if self.buttons[3].key_state() != 0.0 {
            yaw += 2.5;
        }
        let mut dist = if self.buttons[4].key_state() != 0.0 {
            149.0
        } else if self.buttons[5].key_state() != 0.0 {
            151.0
        } else {
            150.0
        };
        if self.distancemove {
            if my > cy {
                if v("c_maxdistance") > dist {
                    dist += ((my - cy) / 2) as f32;
                }
                dist = dist.min(v("c_maxdistance"));
            } else if my < cy {
                if dist > v("c_mindistance") {
                    dist -= ((cy - my) / 2) as f32;
                }
                dist = if v("c_mindistance") <= dist { dist } else { v("c_mindistance") };
            }
        }
        let ideal = [pitch, yaw, 150.0];
        // The gaps and goals are x87 sums of floats (exact in f64); the snap path rounds once.
        let (mut ofs_pitch, mut ofs_yaw) = (self.ofs[0], self.ofs[1]);
        let (va_pitch, va_yaw) = (f64::from(viewangles[0]), f64::from(viewangles[1]));
        if v("cam_snapto") == 0.0 {
            if f64::from(ofs_yaw) - va_yaw != f64::from(yaw) {
                ofs_yaw = move_toward(ofs_yaw, va_yaw + f64::from(yaw));
            }
            if f64::from(ofs_pitch) - va_pitch != f64::from(pitch) {
                ofs_pitch = move_toward(ofs_pitch, va_pitch + f64::from(pitch));
            }
        } else {
            ofs_yaw = yaw + viewangles[1];
            ofs_pitch = pitch + viewangles[0];
        }
        self.ofs = [ofs_pitch, ofs_yaw, dist];
        (effect, Some(ideal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_toward_wraps_snaps_and_eases() {
        assert_eq!(move_toward(10.0, 10.5), 10.5, "within a degree snaps");
        assert_eq!(move_toward(0.0, 100.0), 25.0, "a quarter of the gap");
        // 350 → 10 crosses zero: shifted to -10, a quarter of 20 toward 10, then wrapped.
        assert_eq!(move_toward(350.0, 10.0), 355.0);
        assert_eq!(move_toward(-5.0, -5.0), 355.0, "equal values still wrap");
    }

    #[test]
    fn thirdperson_is_refused_on_a_multiplayer_server() {
        let mut cam = Camera::default();
        assert_eq!(cam.enter_third_person(16, [1.0, 2.0, 0.0]), CamEffect::None);
        assert!(!cam.thirdperson);
        assert_eq!(cam.enter_third_person(0, [1.0, 2.0, 0.0]), CamEffect::ResetCommand);
        assert_eq!(cam.ofs, [1.0, 2.0, 30.0]);
    }
}
