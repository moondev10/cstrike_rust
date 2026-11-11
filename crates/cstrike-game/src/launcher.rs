//! hl.exe's registry work around each engine run (docs/re/engine-video.md §8.3, §8.5). Its
//! message boxes are logged; rendering is wgpu whichever engine DLL the registry names.

use crate::{cmdline, registry};

/// The crash recovery at process start (§8.5): a video initialisation that never finished
/// switches the renderer and resets the resolution to the desktop's (`desktop` = the desktop
/// window's size, 800×600 when unknown).
pub fn crash_recovery(desktop: Option<(i32, i32)>) {
    if registry::read_int("CrashInitializingVideoMode", 0) == 0 {
        return;
    }
    registry::write_int("CrashInitializingVideoMode", 0);
    let (mut w, mut h) = desktop.unwrap_or((800, 600));
    if registry::read_string("EngineDLL", "").eq_ignore_ascii_case("hw.dll") {
        if registry::read_int("EngineD3D", 0) == 1 {
            registry::write_int("EngineD3D", 0);
            log::warn!("Video mode change failure: The game has detected that the previous attempt to start in D3D video mode failed.\nThe game will now run attempt to run in openGL mode.");
        } else {
            registry::write_string("EngineDLL", "sw.dll");
            log::warn!("Video mode change failure: The game has detected that the previous attempt to start in OpenGL video mode failed.\nThe game will now run in software mode.");
            (w, h) = (1024, 764);
        }
    } else {
        registry::write_string("EngineDLL", "hw.dll");
        log::warn!("Video mode change failure: The game has detected that the previous attempt to start in software video mode failed.\nThe game will now run in OpenGL mode.");
    }
    registry::write_int("DisplayBPP", 32);
    registry::write_int("DisplayHeight", h);
    registry::write_int("DisplayWidth", w);
}

/// The engine DLL a launcher iteration loads (§8.3): the registry's `hw.dll`/`sw.dll`
/// (anything else is `hw.dll`), `-soft`/`-software` or `-gl`/`-d3d` overriding, written back.
pub fn resolve_engine_dll() {
    let e = registry::read_string("EngineDLL", "hw.dll");
    let mut dll = if e.eq_ignore_ascii_case("sw.dll") { "sw.dll" } else { "hw.dll" };
    if cmdline::check_parm("-soft") || cmdline::check_parm("-software") {
        dll = "sw.dll";
    } else if cmdline::check_parm("-gl") || cmdline::check_parm("-d3d") {
        dll = "hw.dll";
    }
    registry::write_string("EngineDLL", dll);
}
