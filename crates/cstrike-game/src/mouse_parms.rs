//! client.dll's Windows mouse parameters (`IN_StartupMouse` client.dll@0x10035958,
//! `IN_ActivateMouse` @0x100368d0, `IN_DeactivateMouse` @0x10036940): while the client has the
//! mouse, Windows' pointer acceleration is set to `newmouseparms` {0, 0, 1}, which turns
//! "Enhance pointer precision" on for the whole system, unless `-noforcemspd`,
//! `-noforcemaccel` or `-noforcemparms` keep the user's values; giving the mouse up puts the
//! user's values back. It matters with `m_rawinput 0`, where the view turns by the cursor.

use crate::{cmdline, ffi};

pub struct MouseParms {
    /// `mouseinitialized`: no `-nomouse`.
    initialized: bool,
    /// `originalmouseparms`; `None` when `SPI_GETMOUSE` failed (`mouseparmsvalid` 0).
    original: Option<[i32; 3]>,
    /// `newmouseparms`.
    new: [i32; 3],
    /// `restore_spi`: the last `SPI_SETMOUSE` of the new values succeeded.
    restore: bool,
}

impl MouseParms {
    /// `IN_StartupMouse`'s mouse part; `system` false leaves the system settings alone (the
    /// windowless test harness never has the mouse).
    pub fn startup(system: bool) -> Self {
        let initialized = system && !cmdline::check_parm("-nomouse");
        let original = if initialized { ffi::mouse_params() } else { None };
        let mut new = [0, 0, 1];
        if let Some(orig) = original {
            if cmdline::check_parm("-noforcemspd") {
                new[2] = orig[2];
            }
            if cmdline::check_parm("-noforcemaccel") {
                new[0] = orig[0];
                new[1] = orig[1];
            }
            if cmdline::check_parm("-noforcemparms") {
                new = orig;
            }
        }
        Self { initialized, original, new, restore: false }
    }

    /// `IN_ActivateMouse`'s `SPI_SETMOUSE` of the new values.
    pub fn activate(&mut self) {
        if self.initialized && self.original.is_some() {
            self.restore = ffi::set_mouse_params(self.new);
        }
    }

    /// `IN_DeactivateMouse`'s `SPI_SETMOUSE` of the user's values (`restore_spi` stays set).
    pub fn deactivate(&mut self) {
        if self.initialized
            && self.restore
            && let Some(orig) = self.original
            && !ffi::set_mouse_params(orig)
        {
            log::warn!("SPI_SETMOUSE: the mouse parameters were not restored");
        }
    }
}

impl Drop for MouseParms {
    /// The user's values back when the game ends without `Sys_ShutdownGame` (a panic).
    fn drop(&mut self) {
        self.deactivate();
    }
}
