//! hw.dll's console commands that a client runs locally without a local server
//! (docs/re/commands-client.md, commands-host.md, commands-server.md, commands-render-cam.md):
//! the server-administration state a client still carries, the map/save stubs, the file tools
//! and the diagnostics. None of them is ever forwarded except the documented forwarders.

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::time::Instant;

use gs_common::bytestr::{encoded_len, truncate_encoded, truncate_encoded_in_place};
use gs_common::info::InfoString;
use gs_console::cvar::{format_value, q_atof, q_atoi};
use gs_console::{Args, Console, ConsoleHost};
use gs_fs::PathId;

use crate::console::{Host, UiOp};

/// The build number of the hw.dll this client reproduces (`version`, `sv_version`).
pub const BUILD_NUMBER: i32 = 10210;
/// `svs.maxclientslimit` on any normal client.
const MAX_CLIENTS_LIMIT: u32 = 32;
/// `ipfilters[]` / `userfilters[]` capacity.
const MAX_FILTERS: usize = 0x8000;

/// A frame capture a command asked for; the frame writes it after drawing.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureOp {
    /// `snapshot`: `VID_TakeSnapshot(name)` (BMP).
    Snapshot(String),
    /// `screenshot`: `SCR_ScreenShot_f` (TGA, then `Wrote %s`).
    Screenshot(String),
    /// `envmap`: the six cube views into `<base><rt|bk|lf|ft|up|dn>.bmp`.
    Envmap(String),
    /// `timerefresh`: 128 views around the yaw, timed.
    TimeRefresh,
}

/// `USERID_t` with its ban times (`userfilters[]`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct UserFilter {
    /// 1 Steam, 2 Valve.
    idtype: i32,
    /// 0 = pending.
    clientid: u64,
    ban_end: f32,
    ban_time: f32,
}

/// `ipfilters[]`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct IpFilter {
    mask: [u8; 4],
    compare: [u8; 4],
    ban_end: f32,
    ban_time: f32,
}

/// The server-side logging state (`svs.log`, `firstLog`).
#[derive(Debug, Default)]
struct Log {
    active: bool,
    file: Option<PathBuf>,
    net: Option<SocketAddr>,
    list: Vec<SocketAddr>,
    socket: Option<UdpSocket>,
}

/// The engine state the local commands work on.
pub struct EngineState {
    /// `svs.maxclients` (`-maxplayers`, else 1).
    pub svs_maxclients: u32,
    user_filters: Vec<UserFilter>,
    ip_filters: Vec<IpFilter>,
    /// The local server's info string (`\*gamedir\cstrike`).
    serverinfo: InfoString,
    localinfo: InfoString,
    log: Log,
    /// `g_careerState`.
    career: u8,
    /// `r_dointerp`.
    pub r_dointerp: bool,
    /// The first host frame (`stats` uptime), `host_frametime` and `realtime`.
    pub start: Instant,
    pub frametime: f64,
    pub realtime: f64,
    /// `r_refdef.vieworg` / `viewangles` of the last rendered frame.
    pub view: ([f32; 3], [f32; 3]),
    /// The renderer's vendor, renderer, version and extension strings (`gl_dump`).
    pub gl_strings: [String; 4],
    /// Captures for the frame to write.
    pub captures: Vec<CaptureOp>,
    /// The MOTD panel's browser (chromehtml).
    pub motd: crate::motd::MotdBrowser,
    /// `giActive == DLL_RESTART` (`_restart`): the frame ends, then the engine restarts.
    pub restart: bool,
}

impl EngineState {
    /// `Host_Init`: `SV_SetMaxclients` (`-maxplayers N`: N < 1 → 6, N > 32 → 32), the local
    /// server info.
    pub fn new(game: &str) -> Self {
        let maxclients = crate::cmdline::parm_value("-maxplayers").map_or(1, |v| match q_atoi(&v) {
            n if n < 1 => 6,
            n if n > 32 => 32,
            n => n as u32,
        });
        let mut serverinfo = InfoString::new(512);
        let _ = serverinfo.set_star("*gamedir", game);
        Self {
            svs_maxclients: maxclients.min(MAX_CLIENTS_LIMIT),
            user_filters: Vec::new(),
            ip_filters: Vec::new(),
            serverinfo,
            localinfo: InfoString::new(0x8000),
            log: Log::default(),
            career: 0,
            r_dointerp: true,
            start: Instant::now(),
            frametime: 0.0,
            realtime: 0.0,
            view: ([0.0; 3], [0.0; 3]),
            gl_strings: Default::default(),
            captures: Vec::new(),
            // CEF's cookie store and cache live next to the executable (hl.exe's `htmlcookies`,
            // `htmlcache`).
            motd: crate::motd::MotdBrowser::new(),
            restart: false,
        }
    }
}

/// `COM_FileBase`: the file name without directory and extension.
fn file_base(path: &str) -> &str {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    file.rsplit_once('.').map_or(file, |(b, _)| b)
}

/// `COM_DefaultExtension`: appended when no `.` follows the last `/`.
fn default_extension(path: &str, ext: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    if base.contains('.') { path.to_owned() } else { format!("{path}{ext}") }
}

/// `COM_FileExtension`: the text after the last `.` (anywhere).
fn file_extension(path: &str) -> &str {
    path.rsplit_once('.').map_or("", |(_, e)| e)
}

/// `Info_Print`: keys padded to 20 columns, `MISSING VALUE` for a key without value.
fn info_print(raw: &str) -> String {
    let body = raw.strip_prefix('\\').unwrap_or(raw);
    if body.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let mut parts = body.split('\\');
    while let Some(key) = parts.next() {
        let key = truncate_encoded(key, 127);
        // "%-20s" pads to 20 bytes.
        out.push_str(&key);
        out.push_str(&" ".repeat(20usize.saturating_sub(encoded_len(&key))));
        match parts.next() {
            Some(value) => out.push_str(&format!("{value}\n")),
            None => out.push_str("MISSING VALUE\n"),
        }
    }
    out
}

/// The local date and time for log lines (`L %02i/%02i/%04i - %02i:%02i:%02i: `).
fn log_stamp() -> (String, u32, u32) {
    use chrono::{Datelike, Timelike};
    let t = chrono::Local::now();
    (format!("L {:02}/{:02}/{:04} - {:02}:{:02}:{:02}: ", t.month(), t.day(), t.year(), t.hour(), t.minute(), t.second()), t.month(), t.day())
}

/// `SV_GetIDString`.
fn id_string(f: &UserFilter, lan: bool) -> String {
    let (lan_s, pending, prefix) = match f.idtype {
        1 => ("STEAM_ID_LAN", "STEAM_ID_PENDING", "STEAM_0"),
        2 => ("VALVE_ID_LAN", "VALVE_ID_PENDING", "VALVE_0"),
        3 => return "HLTV".to_owned(),
        _ => return "UNKNOWN".to_owned(),
    };
    if lan {
        lan_s.to_owned()
    } else if f.clientid == 0 {
        pending.to_owned()
    } else {
        let lo = f.clientid as u32;
        format!("{prefix}:{}:{}", lo & 1, lo >> 1)
    }
}

/// `SV_CompareUserID`.
fn same_user(a: &UserFilter, b: &UserFilter, lan: bool) -> bool {
    a.idtype == b.idtype && matches!(a.idtype, 1 | 2) && id_string(a, lan).eq_ignore_ascii_case(&id_string(b, lan))
}

/// Text → SteamID (`STEAM_` stripped by the caller): `%hu:%u:%u%c`, universe 1 needed with two
/// fields; failure gives 0 (pending).
fn text_to_steam_id(text: &str) -> u64 {
    let text = text.strip_prefix("STEAM_").unwrap_or(text);
    let mut fields = text.split(':');
    let parse = |s: Option<&str>| s.and_then(|s| s.parse::<u32>().ok());
    let (u, y, z) = (parse(fields.next()), parse(fields.next()), fields.next());
    match (u, y, z) {
        (Some(_), Some(y), Some(z)) => match z.parse::<u32>() {
            Ok(z) => (1u64 << 32) | u64::from(y.wrapping_add(z.wrapping_mul(2))),
            Err(_) => 0,
        },
        (Some(1), Some(y), None) => (1u64 << 32) | u64::from(y),
        _ => 0,
    }
}

/// `StringToFilter` (Quake rules); `Err(rest)` is the text from the offending character.
fn string_to_filter(s: &str) -> Result<([u8; 4], [u8; 4]), String> {
    let (mut mask, mut compare) = ([0u8; 4], [0u8; 4]);
    let b = s.as_bytes();
    let mut p = 0;
    for i in 0..4 {
        if !b.get(p).is_some_and(u8::is_ascii_digit) {
            return Err(s[p.min(s.len())..].to_owned());
        }
        let start = p;
        while b.get(p).is_some_and(u8::is_ascii_digit) {
            p += 1;
        }
        compare[i] = q_atoi(&s[start..p]) as u8;
        mask[i] = if compare[i] != 0 { 0xff } else { 0 };
        if p >= b.len() {
            break;
        }
        p += 1;
    }
    Ok((mask, compare))
}

/// Output the original prints with one `Con_Printf` per line: each line is logged
/// (`condebug`, cut at 1023 bytes) and shown in the notify line on its own.
fn print_lines(host: &mut Host, text: &str) {
    for line in text.split_inclusive('\n') {
        host.print(line);
    }
}

/// The host commands of this module; `false` when `args` names none of them.
pub fn command(host: &mut Host, console: &mut Console, args: &Args) -> bool {
    let name = args.arg(0).to_ascii_lowercase();
    let argc = args.argc();
    let raw_args = args.args.clone().unwrap_or_default();
    match name.as_str() {
        // ------------------------------------------------------------ CL_Init
        "gamedir" => {
            let game = host.client.config.game.clone();
            if argc != 2 {
                host.print(&format!("gamedir is {game}\n"));
                return true;
            }
            host.client.disconnect("gamedir");
            let dir = args.arg(1);
            host.print(&format!("Setting gamedir to {dir}\n"));
            if dir.is_empty() {
                host.print("Server didn't specify a gamedir, assuming no change\n");
            } else if !file_base(&game).eq_ignore_ascii_case(dir) {
                host.print(&format!("Server is running game {dir}.  Restart in that game to connect.\n"));
                host.client.disconnect("gamedir");
            }
        }
        "fullserverinfo" => {
            if argc == 2 {
                let mut s = args.arg(1).to_owned();
                truncate_encoded_in_place(&mut s, 511);
                host.client.serverinfo = s;
            } else {
                host.print("usage: fullserverinfo <complete info string>\n");
            }
        }
        "commentator" => {
            if argc < 2 {
                let on = host.client.config.userinfo.get("*hltv").is_some_and(|v| !v.is_empty() && q_atoi(v) == 3);
                host.print(if on { "HLTV commentator mode enabled.\n" } else { "HLTV commentator mode disabled.\n" });
                return true;
            }
            let value = match q_atoi(&raw_args) {
                0 => "0",
                1 => "3",
                _ => return true,
            };
            if let Err(e) = host.client.config.userinfo.set_star("*hltv", value) {
                host.print(&format!("{e}\n"));
            }
        }
        "waveplaylen" => {
            if argc != 2 {
                host.print("waveplaylen <wave file name>: returns approximate number of milliseconds a wave file will take to play.\n");
                return true;
            }
            let path = args.arg(1);
            let ms = match host.fs.read(path) {
                Ok(data) => {
                    let mut buf = [0u8; 128];
                    let n = data.len().min(128);
                    buf[..n].copy_from_slice(&data[..n]);
                    let tag = |o: usize, t: &[u8]| buf[o..o + 4].eq_ignore_ascii_case(t);
                    if !tag(0, b"RIFF") || !tag(8, b"WAVE") || !tag(12, b"fmt ") {
                        0
                    } else {
                        let avg = u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]);
                        if avg == 0 {
                            host.print("COM_GetApproxWavePlayLength: bad AvgBytesPerSec\n");
                            0
                        } else {
                            let data = (data.len() as u32).wrapping_sub(128);
                            data.checked_div(avg / 1000).unwrap_or_else(|| (data / avg).wrapping_mul(1000))
                        }
                    }
                }
                Err(_) => 0,
            };
            if ms == 0 {
                host.print(&format!("Unable to read {path}, file may be missing or incorrectly formatted.\n"));
            } else {
                host.print(&format!("Play time is approximately {}ms\n", ms as i32));
            }
        }
        "snapshot" => {
            let base = world_base(host).unwrap_or_else(|| "Snapshot".to_owned());
            for i in 0..1000 {
                let name = truncate_encoded(&format!("{base}{i:04}.bmp"), 63).into_owned();
                if host.fs.resolve_in(&name, Some(PathId::GameConfig)).is_err() {
                    host.engine.captures.push(CaptureOp::Snapshot(name));
                    return true;
                }
            }
            host.print("Unable to take a screenshot.\n");
        }
        "entities" => {
            let text = entities_text(host);
            print_lines(host, &text);
        }
        "cl_messages" => {
            let stats = host.client.msg_stats();
            print_lines(host, &cl_messages_text(&stats.count, &stats.bytes));
        }
        "pingservers" | "slist" => {
            if name == "slist" {
                host.client.slist.entries.clear();
            }
            host.print("Searching for local servers...\n");
            host.client.ping_lan_servers(!crate::cmdline::check_parm("-noip"));
        }
        "list" => {
            let text = host.client.slist.text();
            print_lines(host, &text);
        }
        "listen" => {
            match args.args.as_deref().filter(|_| argc >= 2) {
                Some(adr) => host.client.listen_command(adr),
                None => host.print("usage: listen <IP>\n"),
            }
        }
        "spec_pos" => {
            let (mut o, a) = host.engine.view;
            let mut v = [a[0], a[1]];
            if argc == 2 && q_atoi(args.arg(1)) == 2 && host.client.state() == gs_client::State::Active {
                // cl.frames[parsecountmod].playerstate[playernum]: the local player's entity in the
                // latest parsed frame.
                let proto = host.client.protocol();
                let number = i32::from(proto.playernum) + 1;
                if let Some(ps) = proto.current_frame().entities.iter().find(|e| e.number() == number) {
                    o = ps.origin();
                    let angles = ps.angles();
                    v = [angles[0], angles[1]];
                }
            }
            host.print(&format!("spec_pos {:.1} {:.1} {:.1} {:.1} {:.1}\n", o[0], o[1], o[2], v[0], v[1]));
        }
        // ------------------------------------------------------------ Cmd_Init, Con_Init, DELTA_Init
        "stuffcmds" => {
            if argc != 1 {
                host.print("stuffcmds : execute command line parameters\n");
                return true;
            }
            let text = crate::cmdline::argv()[1..].join(" ");
            let b = text.as_bytes();
            let mut out = String::new();
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'+' {
                    let mut j = i + 1;
                    while j < b.len() && b[j] != b'+' && b[j] != b'-' {
                        j += 1;
                    }
                    out.push_str(&text[i + 1..j]);
                    out.push('\n');
                    i = j;
                } else {
                    i += 1;
                }
            }
            if !out.is_empty() {
                console.insert_text(&out);
            }
        }
        "cmdlist" => cmdlist(host, console, args),
        // The disable line is still logged; the enable line is the first one logged.
        "condebug" => {
            if crate::notify::condebug() {
                host.print("condebug disabled\n");
                crate::notify::set_condebug(false);
            } else {
                crate::notify::set_condebug(true);
                host.print("condebug enabled\n");
            }
        }
        "delta_stats" => {
            let text = host.client.delta_stats_text();
            print_lines(host, &text);
        }
        "delta_clear" => {
            host.print("Clearing delta stats\n");
            host.client.clear_delta_stats();
        }
        // ------------------------------------------------------------ HPAK_Init
        "hpklist" | "hpkremove" | "hpkval" | "hpkextract" => {
            let argv: Vec<&str> = (0..argc).map(|i| args.arg(i)).collect();
            let text = host.client.hpak_command(&name, &argv);
            print_lines(host, &text);
        }
        // ------------------------------------------------------------ Host_InitCommands
        "_careeraudio" | "restart" | "reload" | "motd_write" | "heartbeat" | "show_osk" | "resetrcon" | "new" | "sendres" | "dlfile" => {}
        // engine-video.md §7.6: an active connection reconnects after the restart; then
        // CL_Disconnect (no listen server), and the frame ends with giActive = DLL_RESTART.
        "_restart" => {
            host.engine.restart = true;
            if host.client.state() == gs_client::State::Active && !host.client.true_address().is_empty() {
                crate::cmdline::append_post_restart(&format!(" +connect {}", host.client.server_name()));
            }
            host.client.disconnect("disconnect");
        }
        // §7.1: VideoMode_SetVideoMode writes the registry; the running mode stays.
        "_setvideomode" => {
            if argc == 3 || argc == 4 {
                let bpp = if argc == 4 { q_atoi(args.arg(3)) } else { 16 };
                crate::registry::write_int("DisplayWidth", q_atoi(args.arg(1)));
                crate::registry::write_int("DisplayHeight", q_atoi(args.arg(2)));
                crate::registry::write_int("DisplayBPP", bpp);
            }
        }
        // §7.2: VideoMode_SetRenderer (anything but software/d3d is gl; only `windowed` is).
        // The wgpu backend a name stands for (`dx12`/`d3d`, `vulkan`, `gl`) goes to our own
        // `RenderBackend` value, which the next engine run (`_restart`) starts with.
        "_setrenderer" => {
            if argc == 2 || argc == 3 {
                let a = args.arg(1);
                let renderer = if a.eq_ignore_ascii_case("software") {
                    0
                } else if a.eq_ignore_ascii_case("d3d") {
                    2
                } else {
                    1
                };
                let windowed = argc == 3 && args.arg(2).eq_ignore_ascii_case("windowed");
                crate::registry::write_string("EngineDLL", if renderer == 0 { "sw.dll" } else { "hw.dll" });
                crate::registry::write_int("EngineD3D", i32::from(renderer == 2));
                crate::registry::write_int("ScreenWindowed", i32::from(windowed));
                if let Some(backend) = gs_render::gpu::backend_from_name(a) {
                    crate::registry::write_string("RenderBackend", gs_render::gpu::backend_name(backend));
                }
            }
        }
        // §7.3/§7.4: `before` reads the raw registry, `after` honours the command line; a change
        // re-runs COM_SetupDirectories.
        "_sethdmodels" | "_setaddons_folder" => {
            if argc == 2 {
                let hd = name == "_sethdmodels";
                let key = if hd { "hdmodels" } else { "addons_folder" };
                let before = crate::registry::read_int(key, 0) > 0;
                crate::registry::write_int(key, i32::from(args.arg(1).eq_ignore_ascii_case("1")));
                let after = if hd {
                    !crate::cmdline::check_parm("-nohdmodels") && crate::registry::read_int(key, 0) > 0
                } else {
                    crate::cmdline::check_parm("-addons") || crate::registry::read_int(key, 0) > 0
                };
                if before != after
                    && let Some(base) = host.fs.base_dir()
                {
                    host.fs.remount(&crate::dirs::layout(base.to_path_buf()));
                }
            }
        }
        // §7.5: the post-restart command line gets ` -game <dir>` (an absent argument is "").
        "_setgamedir" => {
            let dir = args.arg(1);
            if dir.starts_with('/') || dir.contains(':') || dir.contains("..") || dir.contains('\\') {
                host.print(&format!("Couldn't set gamedir to {dir} (contains illegal characters)\n"));
            } else {
                crate::cmdline::append_post_restart(&format!(" -game {dir}"));
            }
        }
        "shutdownserver" => host.client.disconnect("disconnect"),
        // cmd_soundfade fills cls.soundfade, which nothing in hw.dll reads (audio.md §2.5).
        "soundfade" => {
            if argc != 3 && argc != 5 {
                host.print("soundfade <percent> <hold> [<out> <int>]\n");
            }
        }
        "map" | "career" => map(host, &name, args, &raw_args),
        "maps" => maps(host, args),
        "changelevel" => {
            host.print(if argc < 2 { "changelevel <levelname> : continue game on a new level\n" } else { "Only the server may changelevel\n" });
        }
        "changelevel2" => {
            host.print(if argc < 2 { "changelevel2 <levelname> : continue game on a new level in the unit\n" } else { "Only the server may changelevel\n" });
        }
        "load" => load(host, args),
        "save" | "autosave" => host.print("Not playing a local game.\n"),
        "version" => {
            let product = host
                .fs
                .read("steam.inf")
                .ok()
                .and_then(|d| String::from_utf8_lossy(&d).lines().find_map(|l| l.trim().strip_prefix("ProductName=").map(|v| v.trim().to_owned())))
                .unwrap_or_else(|| "valve".to_owned());
            let version = host.client.version().to_owned();
            host.print(&format!("Protocol version 48\nExe version {version} ({product})\n"));
            host.print(&format!("Exe build: 19:06:31 Oct  7 2024 ({BUILD_NUMBER})\n"));
        }
        "kick" => {
            if argc <= 1 {
                host.print("usage:  kick < name > | < # userid >\n");
            } else {
                gs_console::forward_to_server(host, args);
            }
        }
        "motd" => motd(host, console),
        "stats" => {
            let uptime = (host.engine.start.elapsed().as_secs_f64() as i32) / 60;
            let fps = if host.engine.frametime == 0.0 { f64::INFINITY } else { 1.0 / host.engine.frametime };
            let line = format!("{:5.2} {:5.2} {:5.2} {:7} {:5} {:7.2} {:7}", 0.0, 0.0, 0.0, uptime, 0, fps, 0);
            host.print(&format!("CPU   In    Out   Uptime  Users   FPS    Players\n{line}\n"));
        }
        "writecfg" => writecfg(host, console, args, &raw_args),
        "viewmodel" | "viewframe" | "viewnext" | "viewprev" => host.print("No viewthing on map\n"),
        "mcache" => {
            let mut text = String::from("Cached models:\n");
            for model in host.client.level().models.iter().flatten().filter(|m| !m.is_empty()) {
                text.push_str(&format!("00000000 : {model}\n"));
            }
            print_lines(host, &text);
        }
        "interp" => {
            let old = host.engine.r_dointerp;
            host.engine.r_dointerp = !old;
            host.print(if old { "Frame Interpolation OFF\n" } else { "Frame Interpolation ON\n" });
        }
        "setmaster" => host.print("Usage:\nSetmaster unavailable, start a server first.\n"),
        // ------------------------------------------------------------ NET_Init, SV_Init
        "maxplayers" => {
            if argc != 2 {
                host.print(&format!("\"maxplayers\" is \"{}\"\n", host.engine.svs_maxclients));
                return true;
            }
            let mut n = q_atoi(args.arg(1)).max(1) as u32;
            if n > MAX_CLIENTS_LIMIT {
                n = MAX_CLIENTS_LIMIT;
                host.print(&format!("\"maxplayers\" set to \"{n}\"\n"));
            }
            host.engine.svs_maxclients = n;
            server_cvar_set(host, console, "deathmatch", if n == 1 { "0" } else { "1" });
        }
        "banid" => banid(host, console, args),
        "removeid" => removeid(host, console, args),
        "listid" => {
            let lan = console.cvars.value("sv_lan") != 0.0;
            if host.engine.user_filters.is_empty() {
                host.print("UserID filter list: empty\n");
                return true;
            }
            let mut text = format!("UserID filter list: {} entries\n", host.engine.user_filters.len());
            for (i, f) in host.engine.user_filters.iter().enumerate() {
                if f.ban_time == 0.0 {
                    text.push_str(&format!("{} {} : permanent\n", i + 1, id_string(f, lan)));
                } else {
                    text.push_str(&format!("{} {} : {:.3} min\n", i + 1, id_string(f, lan), f64::from(f.ban_time)));
                }
            }
            print_lines(host, &text);
        }
        "writeid" => {
            let s = console.cvars.string("bannedcfgfile").to_owned();
            if s.is_empty() {
                host.print("Couldn't open banned config file, no file name specified.\n");
                return true;
            }
            if s.starts_with('/') || s.contains(':') || s.contains("..") || s.contains('\\') {
                host.print(&format!("Couldn't open {s} (contains illegal characters).\n"));
                return true;
            }
            let name = default_extension(&truncate_encoded(&s, 255), ".cfg");
            if file_extension(&name) != "cfg" {
                host.print(&format!("Couldn't open {name} (wrong file extension, must be .cfg).\n"));
                return true;
            }
            host.print(&format!("Writing {name}.\n"));
            let lan = console.cvars.value("sv_lan") != 0.0;
            let text: String = host.engine.user_filters.iter().filter(|f| f.ban_time == 0.0).map(|f| format!("banid 0.0 {}\r\n", id_string(f, lan))).collect();
            if host.fs.write(&name, PathId::GameConfig, text.as_bytes()).is_err() {
                host.print(&format!("Couldn't open {name}\n"));
            }
        }
        "logaddress" | "logaddress_add" | "logaddress_del" => logaddress(host, &name, args),
        "log" => {
            if argc != 2 {
                host.print("usage:  log < on | off >\n");
                host.print(if host.engine.log.active { "currently logging\n" } else { "not currently logging\n" });
            } else if args.arg(1).eq_ignore_ascii_case("off") {
                if host.engine.log.active {
                    if host.engine.log.file.is_some() {
                        log_close(host, console);
                    }
                    host.print("Server logging disabled.\n");
                    host.engine.log.active = false;
                }
            } else if args.arg(1).eq_ignore_ascii_case("on") {
                host.engine.log.active = true;
                log_open(host, console);
            } else {
                host.print(&format!("log:  unknown parameter {}, 'on' and 'off' are valid\n", args.arg(1)));
            }
        }
        "serverinfo" | "localinfo" => {
            let server = name == "serverinfo";
            if argc == 1 {
                let raw = if server { host.engine.serverinfo.as_str() } else { host.engine.localinfo.as_str() }.to_owned();
                host.print(if server { "Server info settings:\n" } else { "Local info settings:\n" });
                let text = info_print(&raw);
                if !text.is_empty() {
                    print_lines(host, &text);
                }
                return true;
            }
            if argc != 3 {
                host.print(&format!("usage: {name} [ <key> <value> ]\n"));
                return true;
            }
            if args.arg(1).starts_with('*') {
                host.print("Star variables cannot be changed.\n");
                return true;
            }
            let info = if server { &mut host.engine.serverinfo } else { &mut host.engine.localinfo };
            if let Err(e) = info.set(args.arg(1), args.arg(2)) {
                host.print(&format!("{e}\n"));
            }
            // cmd_serverinfo writes the same-named cvar's string and value directly.
            if server {
                console.cvars.set_raw(args.arg(1), args.arg(2));
            }
        }
        "user" => host.print("Can't 'user', not running a server\n"),
        "users" => host.print("Can't 'users', not running a server\n"),
        "addip" => addip(host, args),
        "removeip" => {
            let filter = match string_to_filter(args.arg(1)) {
                Ok(f) => f,
                Err(rest) => {
                    host.print(&format!("Bad filter address: {rest}\n"));
                    return true;
                }
            };
            match host.engine.ip_filters.iter().position(|f| (f.mask, f.compare) == filter) {
                Some(i) => {
                    host.engine.ip_filters.remove(i);
                    host.print("IP filter removed.\n");
                }
                None => host.print(&format!("removeip: couldn't find {}.\n", args.arg(1))),
            }
        }
        "listip" => {
            if host.engine.ip_filters.is_empty() {
                host.print("IP filter list: empty\n");
                return true;
            }
            let mut text = String::from("IP filter list:\n");
            for f in &host.engine.ip_filters {
                let b = f.compare;
                if f.ban_time == 0.0 {
                    text.push_str(&format!("{:3}.{:3}.{:3}.{:3} : permanent\n", b[0], b[1], b[2], b[3]));
                } else {
                    text.push_str(&format!("{:3}.{:3}.{:3}.{:3} : {:.3} min\n", b[0], b[1], b[2], b[3], f64::from(f.ban_time)));
                }
            }
            print_lines(host, &text);
        }
        "writeip" => {
            host.print("Writing listip.cfg.\n");
            let text: String = host
                .engine
                .ip_filters
                .iter()
                .filter(|f| f.ban_time == 0.0)
                .map(|f| format!("addip 0.0 {}.{}.{}.{}\n", f.compare[0], f.compare[1], f.compare[2], f.compare[3]))
                .collect();
            if host.fs.write("listip.cfg", PathId::GameConfig, text.as_bytes()).is_err() {
                host.print("Couldn't open listip.cfg\n");
            }
        }
        "spawn" => host.print(if argc != 3 { "spawn is not valid\n" } else { "spawn is not valid from the console\n" }),
        // ------------------------------------------------------------ renderer and screen
        "flush" => {
            if host.client.protocol().maxclients > 1 && console.cvars.value("sv_cheats") == 0.0 {
                host.print("Server must enable sv_cheats to activate the flush command in multiplayer games.\n");
            }
        }
        "gl_texels" => host.print(&format!("Current uploaded texels: {}\n", gs_render::texture::uploaded_texels() as i32)),
        "timerefresh" => {
            if host.client.world().is_none() {
                host.print("No map loaded\n");
            } else if console.cvars.value("sv_cheats") == 0.0 {
                host.print("sv_cheats not enabled\n");
            } else {
                host.engine.captures.push(CaptureOp::TimeRefresh);
            }
        }
        "envmap" => {
            if host.client.world().is_none() {
                host.print("No map loaded\n");
            } else {
                let base = world_base(host).unwrap_or_else(|| "Env".to_owned());
                host.engine.captures.push(CaptureOp::Envmap(base));
            }
        }
        "pointfile" => pointfile(host),
        "gl_dump" => {
            let labels = ["GL Vendor", "GL Renderer", "GL Version", "GL Extensions"];
            let text: String = labels.iter().zip(&host.engine.gl_strings).filter(|(_, s)| !s.is_empty()).map(|(l, s)| format!("{l}: {s}\n")).collect();
            if !text.is_empty() {
                print_lines(host, &text);
            }
        }
        // The port has no GLSL world path: R_LoadShaders returns at once, printing nothing.
        "gl_reloadshaders" => {}
        "screenshot" => {
            match (0..100).map(|i| format!("HalfLife{i:02}.tga")).find(|n| !host.fs.exists(n)) {
                Some(name) => host.engine.captures.push(CaptureOp::Screenshot(name)),
                None => host.print("SCR_ScreenShot_f: Couldn't create a TGA file\n"),
            }
        }
        "sizeup" | "sizedown" => {
            let v = console.cvars.value("viewsize");
            let new = if name == "sizeup" { (v < 120.0).then_some(v + 10.0) } else { (v > 30.0).then_some(v - 10.0) };
            if let Some(new) = new {
                console.direct_set(host, "viewsize", &format_value(new));
            }
        }
        // No output device: S_Init still registered the commands, but nothing was loaded.
        "soundinfo" => host.print("sound system not started\n"),
        "soundlist" => host.print("Total resident: 0\n"),
        "play" | "playvol" | "speak" | "spk" | "stopsound" => {}
        _ => return false,
    }
    true
}

/// The loaded world's base name (`cl_entities[0].model`).
fn world_base(host: &Host) -> Option<String> {
    host.client.world()?;
    host.client.server_info().map(|i| file_base(&i.map_model).to_owned())
}

/// `cmd_entities` with the original's argument mismatch: an int and six doubles read from the
/// seven doubles it pushes (commands-client.md §7).
fn entities_text(host: &Host) -> String {
    let ents = host.client.entities();
    let models = &host.client.level().models;
    let mut out = String::new();
    for i in 0..ents.num_entities() {
        out.push_str(&format!("{i:3}:"));
        let Some(e) = ents.get(i) else { continue };
        let index = e.curstate.modelindex() as usize;
        let Some(model) = models.get(index).and_then(Option::as_deref).filter(|m| index > 0 && !m.is_empty()) else { continue };
        let d = [f64::from(e.curstate.frame()), f64::from(e.origin[0]), f64::from(e.origin[1]), f64::from(e.origin[2]), f64::from(e.angles[0]), f64::from(e.angles[1]), f64::from(e.angles[2])];
        let bytes: Vec<u8> = d.iter().flat_map(|v| v.to_le_bytes()).collect();
        let int = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let dbl = |o: usize| f64::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3], bytes[o + 4], bytes[o + 5], bytes[o + 6], bytes[o + 7]]);
        out.push_str(&format!("{model}:{int:2}  ({:5.1},{:5.1},{:5.1}) [{:5.1} {:5.1} {:5.1}]\n", dbl(4), dbl(12), dbl(20), dbl(28), dbl(36), dbl(44)));
    }
    out
}

/// `cmd_cl_messages`.
pub fn cl_messages_text(count: &[i32; 64], bytes: &[i32; 64]) -> String {
    let kb = |b: i32| f64::from(b as f32 * (1.0f32 / 1024.0));
    let mut out = String::from("-------- Message Load ---------\n");
    let mut total = 0i32;
    for i in 0..63 {
        if count[i] != 0 {
            let name = if i > 59 { "bogus #" } else { gs_protocol::state::SVC_NAMES[i] };
            out.push_str(&format!("{i}:{name}: {} msgs:{:.2}K\n", count[i], kb(bytes[i])));
            total = total.wrapping_add(count[i]);
        }
    }
    out.push_str(&format!("User messages:  {}:{:.2}K\n", count[63], kb(bytes[63])));
    out.push_str(&format!("------ End:  {} Total----\n", total.wrapping_add(count[63])));
    out
}

/// `cmd_cmdlist`.
fn cmdlist(host: &mut Host, console: &mut Console, args: &Args) {
    let argc = args.argc();
    let mut filter: Option<String> = None;
    let mut log = false;
    if argc >= 2 {
        let a1 = args.arg(1);
        if a1.eq_ignore_ascii_case("?") {
            host.print("CmdList           : List all commands\nCmdList [Partial] : List commands starting with 'Partial'\nCmdList log [Partial] : Logs commands to file \"cmdlist.txt\" in the gamedir.\n");
            return;
        }
        if a1.eq_ignore_ascii_case("log") {
            log = true;
            if argc == 3 {
                filter = Some(args.arg(2).to_owned());
            }
        } else {
            filter = Some(a1.to_owned());
        }
    }
    let names: Vec<String> = console
        .command_names()
        .filter(|n| filter.as_deref().is_none_or(|f| n.len() >= f.len() && n.as_bytes()[..f.len()].eq_ignore_ascii_case(f.as_bytes())))
        .map(|n| format!("{:<16.16}\n", n))
        .collect();
    if log {
        let file: String = names.iter().map(|l| l.replace('\n', "\r\n")).collect();
        if host.fs.write("cmdlist.txt", PathId::GameConfig, file.as_bytes()).is_err() {
            host.print("Couldn't open [cmdlist.txt] for writing!\n");
            return;
        }
    }
    let mut text = String::from("Command List\n--------------\n");
    names.iter().for_each(|l| text.push_str(l));
    match filter.as_deref().filter(|f| argc == 2 && !f.is_empty()) {
        Some(f) => text.push_str(&format!("--------------\n{:3} Commands for [{f}]\nCmdList ? for syntax\n", names.len())),
        None => text.push_str(&format!("--------------\n{:3} Total Commands\nCmdList ? for syntax\n", names.len())),
    }
    print_lines(host, &text);
}

/// `cmd_map` / `cmd_career` up to the point a listen server would start.
fn map(host: &mut Host, name: &str, args: &Args, raw_args: &str) {
    let saved = if name == "career" { 1 } else { host.engine.career };
    let argc = args.argc();
    if argc > 1 && encoded_len(raw_args) > 54 {
        host.print("map change failed: command string is too long.\n");
        host.engine.career = 0;
        return;
    }
    if argc < 2 {
        host.print("map <levelname> : changes server to specified map\n");
        host.engine.career = 0;
        return;
    }
    host.client.disconnect("map");
    host.engine.career = if saved == 1 { 1 } else { 0 };
    let mut level = truncate_encoded(args.arg(1), 63).into_owned();
    if encoded_len(&level) > 4 && level.get(level.len() - 4..).is_some_and(|t| t.eq_ignore_ascii_case(".bsp")) {
        level.truncate(level.len() - 4);
    }
    host.ui_ops.push(UiOp::LoadingStarted);
    let check = truncate_encoded(&level, 32);
    if !host.fs.exists(&format!("maps/{check}.bsp")) {
        host.print(&format!("map change failed: '{level}' not found on server.\n"));
        return;
    }
    // A listen server starts here in the original; this client has none.
    host.ui_ops.push(UiOp::LoadingFinished);
    host.print(&format!("map: can't start '{level}', this client has no local server.\n"));
}

/// `cmd_maps` → `COM_ListMaps`.
fn maps(host: &mut Host, args: &Args) {
    if args.argc() != 2 {
        host.print("Usage:  maps <substring>\nmaps * for full listing\n");
        return;
    }
    let s = args.arg(1);
    if s.is_empty() {
        return;
    }
    let prefix = if s.starts_with('*') { None } else { Some(s) };
    let gamedir = host.client.config.game.clone();
    let found = host.fs.find("maps/*.bsp", None);
    let mut text = String::from("-------------\n");
    for show_outdated in [true, false] {
        for (f, dir) in &found {
            if *dir {
                continue;
            }
            if encoded_len(&format!("maps/{f}")) >= 260 {
                text.push_str(&format!("Map name too long: {f}\n"));
                continue;
            }
            let local = host.fs.resolve_in(&format!("maps/{f}"), None).map(|p| p.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if !local.contains(&gamedir) || prefix.is_some_and(|p| f.len() < p.len() || !f.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes())) {
                continue;
            }
            let rel = format!("maps/{f}");
            if encoded_len(&rel) >= 64 {
                text.push_str(&format!("Map name too long: {f}\n"));
                continue;
            }
            let version = match host.fs.read(&rel) {
                Ok(d) => d.get(..4).map_or(0, |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                Err(_) => {
                    text.push_str(&format!("Could not open map {rel}\n"));
                    0
                }
            };
            if version == 30 {
                if !show_outdated {
                    text.push_str(&format!("{f}\n"));
                }
            } else if show_outdated {
                text.push_str(&format!("OUTDATED:  {f}\n"));
            }
        }
    }
    print_lines(host, &text);
}

/// `cmd_load` → `Host_Load` up to the point a local game would start.
fn load(host: &mut Host, args: &Args) {
    if args.argc() != 2 {
        host.print("load <savename> : load a game\n");
        return;
    }
    let name = args.arg(1);
    let path = (|| {
        if name.is_empty() {
            return None;
        }
        if name.contains("..") {
            host.print("Relative pathnames are not allowed.\n");
            return None;
        }
        let path = if let Some(rest) = name.strip_prefix('_') {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            // Five or more digits overrun the original's buffer (a fast-fail); refused here.
            if digits.len() >= 5 {
                return None;
            }
            let n = q_atoi(&digits);
            if !(1..=12).contains(&n) {
                return None;
            }
            format!("SAVE/Half-Life-{n}")
        } else {
            truncate_encoded(&format!("SAVE/{name}"), 251).into_owned()
        };
        let path = default_extension(&path, ".sav");
        if !file_extension(&path).eq_ignore_ascii_case("sav") {
            return None;
        }
        Some(path.replace('/', "\\"))
    })();
    let Some(path) = path else {
        host.print("Error loading saved game\n");
        return;
    };
    host.print(&format!("Loading game from {path}...\n"));
    if host.fs.resolve_in(&path.replace('\\', "/"), Some(PathId::GameConfig)).is_err() {
        host.print("Error loading saved game\n");
        return;
    }
    host.print("load: this client has no local server to load a saved game.\n");
}

/// `cmd_motd`: the local `motdfile`.
fn motd(host: &mut Host, console: &Console) {
    let f = console.cvars.string("motdfile").to_owned();
    if f.starts_with('/') || f.contains(':') || f.contains("..") || f.contains('\\') {
        host.print(&format!("Unable to open {f} (contains illegal characters)\n"));
        return;
    }
    if !file_extension(&f).eq_ignore_ascii_case("txt") {
        host.print(&format!("Invalid motdfile name {f} (wrong file extension, must be .txt)\n"));
        return;
    }
    let Ok(data) = host.fs.read(&f) else {
        host.print(&format!("Unable to open {f}\n"));
        return;
    };
    if data.is_empty() {
        return;
    }
    let text = gs_common::bytestr::decode(&data);
    let text = text.split('\0').next().unwrap_or("");
    let mut out = String::from("motd:");
    for line in text.split('\n') {
        out.push_str(line);
        out.push('\n');
    }
    print_lines(host, &out);
}

/// `cmd_writecfg`.
fn writecfg(host: &mut Host, console: &Console, args: &Args, raw_args: &str) {
    if args.argc() == 1 || raw_args.is_empty() {
        host.print("writecfg: writecfg <filename>\n");
        return;
    }
    let name = truncate_encoded(raw_args, 256).into_owned();
    if name.is_empty() || name.starts_with('/') || name.contains("..") || name.contains('\\') || name.contains(':') || name.contains("./") {
        host.print(&format!("skipping writecfg output, invalid filename {name} given (invalid characters in filename)\n"));
        return;
    }
    if console.bindings.count() <= 1 {
        host.print("skipping config.cfg output, no keys bound\n");
        return;
    }
    let name = default_extension(&name, ".cfg");
    if !file_extension(&name).eq_ignore_ascii_case("cfg") {
        host.print(&format!("skipping writecfg output, invalid filename {name} given (wrong file extension, must be .cfg)\n"));
        return;
    }
    const RESERVED: [&str; 12] = [
        "config.cfg",
        "autoexec.cfg",
        "listenserver.cfg",
        "server.cfg",
        "userconfig.cfg",
        "language.cfg",
        "violence.cfg",
        "banned.cfg",
        "listip.cfg",
        "skill.cfg",
        "game.cfg",
        "joystick.cfg",
    ];
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(&name)) || name.len() >= 3 && name[..3].eq_ignore_ascii_case("hw/") {
        host.print(&format!("skipping writecfg output, invalid filename {name} given\n"));
        return;
    }
    let text = console.writecfg_text(&host.client.config.userinfo, host.input.mlook_down(), host.input.jlook_down());
    if host.fs.write(&name, PathId::GameConfig, &gs_common::bytestr::encode(&text)).is_err() {
        host.print(&format!("Couldn't write {name}.\n"));
        return;
    }
    host.print(&format!("{name} successfully created!\n"));
}

/// `Cvar_DirectSet` for an FCVAR_SERVER cvar: the log line and the broadcast print (a developer
/// line on a client) when the string changes.
fn server_cvar_set(host: &mut Host, console: &mut Console, name: &str, value: &str) {
    let changed = console.cvars.string(name) != value;
    console.direct_set(host, name, value);
    if changed {
        log_printf(host, console, &format!("Server cvar \"{name}\" = \"{value}\"\n"));
        if console.cvars.value("developer") != 0.0 {
            crate::notify::con_dprintf(&format!("\"{name}\" changed to \"{value}\"\n"));
        }
    }
}

/// `SV_BanId_f`.
fn banid(host: &mut Host, console: &mut Console, args: &Args) {
    let argc = args.argc();
    if !(3..=8).contains(&argc) {
        host.print("Usage:  banid <minutes> <uniqueid or #userid> { kick }\nUse 0 minutes for permanent\n");
        return;
    }
    let mut ban_time = q_atof(args.arg(1));
    if f64::from(ban_time) < 0.01 {
        ban_time = 0.0;
    }
    let mut idstring = truncate_encoded(args.arg(2), 63).into_owned();
    let kick = argc >= 4 && args.arg(argc - 1).eq_ignore_ascii_case("kick");
    if let Some(rest) = idstring.strip_prefix('#') {
        let search = if rest.is_empty() { q_atoi(args.arg(3)) } else { q_atoi(rest) };
        host.print(&format!("SV_BanId_f:  Couldn't find #userid {}\n", search as u32));
        return;
    }
    let prefixed = |s: &str, p: &str| s.get(..6).is_some_and(|h| h.eq_ignore_ascii_case(p));
    let steam = prefixed(&idstring, "STEAM_");
    if steam || prefixed(&idstring, "VALVE_") {
        idstring = truncate_encoded(&format!("{}:{}:{}", args.arg(2), args.arg(4), args.arg(6)), 62).into_owned();
    }
    let mut id = UserFilter { idtype: 0, clientid: 0, ban_end: 0.0, ban_time: 0.0 };
    if !idstring.is_empty() {
        id.idtype = if steam { 1 } else { 2 };
        id.clientid = text_to_steam_id(&idstring[6.min(idstring.len())..]);
    }
    let lan = console.cvars.value("sv_lan") != 0.0;
    let at = match host.engine.user_filters.iter().position(|f| same_user(f, &id, lan)) {
        Some(i) => i,
        None => {
            if host.engine.user_filters.len() >= MAX_FILTERS {
                host.print("SV_BanId_f:  User filter list is full\n");
                return;
            }
            host.engine.user_filters.push(id);
            host.engine.user_filters.len() - 1
        }
    };
    let end = if ban_time != 0.0 { (host.engine.realtime + f64::from(ban_time) * 60.0) as f32 } else { 0.0 };
    host.engine.user_filters[at] = UserFilter { ban_time, ban_end: end, ..id };
    if kick {
        return;
    }
    if console.cvars.value("sv_logbans") != 0.0 {
        let reason = if ban_time == 0.0 { "permanently".to_owned() } else { format!("for {:.2} minutes", f64::from(ban_time)) };
        let who = console.cvars.string("name").to_owned();
        let line = format!("Ban: \"<><{}><>\" was banned \"{reason}\" by \"{who}\"\n", id_string(&host.engine.user_filters[at], lan));
        log_printf(host, console, &line);
    }
}

/// `SV_RemoveId_f`.
fn removeid(host: &mut Host, console: &Console, args: &Args) {
    let argc = args.argc();
    if argc != 2 && argc != 6 {
        host.print("Usage:  removeid <uniqueid | #slotnumber>\n");
        return;
    }
    let mut id = truncate_encoded(args.arg(1), 63).into_owned();
    if id.is_empty() {
        host.print("SV_RemoveId_f:  Id string is empty!\n");
        return;
    }
    let lan = console.cvars.value("sv_lan") != 0.0;
    if let Some(slot) = id.strip_prefix('#') {
        let slot = q_atoi(slot);
        if slot <= 0 || slot as usize > host.engine.user_filters.len() {
            host.print(&format!("SV_RemoveId_f:  invalid slot #{slot}\n"));
            return;
        }
        let removed = host.engine.user_filters.remove(slot as usize - 1);
        host.print(&format!("UserID filter removed for {id}, id {}\n", id_string(&removed, lan)));
        return;
    }
    if id.get(..6).is_some_and(|h| h.eq_ignore_ascii_case("STEAM_") || h.eq_ignore_ascii_case("VALVE_")) {
        id = truncate_encoded(&format!("{}:{}:{}", args.arg(1), args.arg(3), args.arg(5)), 62).into_owned();
    }
    match host.engine.user_filters.iter().position(|f| id_string(f, lan).eq_ignore_ascii_case(&id)) {
        Some(i) => {
            host.engine.user_filters.remove(i);
            host.print(&format!("UserID filter removed for {id}\n"));
        }
        None => host.print(&format!("removeid: couldn't find {id}\n")),
    }
}

/// `cmd_addip`.
fn addip(host: &mut Host, args: &Args) {
    const USAGE: &str = "Usage: addip <minutes> <ipaddress>\nUse 0 minutes for permanent\n";
    if args.argc() != 3 {
        host.print(USAGE);
        return;
    }
    let mut minutes = q_atof(args.arg(1));
    let (mask, compare) = match string_to_filter(args.arg(2)) {
        Ok(f) => f,
        Err(rest) => {
            host.print(&format!("Bad filter address: {rest}\n"));
            host.print(&format!("Invalid IP address!\n{USAGE}"));
            return;
        }
    };
    let realtime = host.engine.realtime;
    let end = |m: f32| if m != 0.0 { (realtime + f64::from(m) * 60.0) as f32 } else { 0.0 };
    if let Some(f) = host.engine.ip_filters.iter_mut().find(|f| f.mask == mask && f.compare == compare) {
        f.ban_time = minutes;
        f.ban_end = end(minutes);
        return;
    }
    if host.engine.ip_filters.len() >= MAX_FILTERS {
        host.print("IP filter list is full\n");
        return;
    }
    if minutes < 0.01 {
        minutes = 0.0;
    }
    host.engine.ip_filters.push(IpFilter { mask, compare, ban_end: end(minutes), ban_time: minutes });
}

/// `cmd_logaddress`, `cmd_logaddress_add`, `cmd_logaddress_del`.
fn logaddress(host: &mut Host, name: &str, args: &Args) {
    if args.argc() != 3 {
        let mut text = format!("{name}:  usage\n{name} ip port\n");
        if name == "logaddress" {
            if host.engine.log.active {
                let cur = host.engine.log.net.map_or_else(|| "0.0.0.0:0".to_owned(), |a| a.to_string());
                text.push_str(&format!("current:  {cur}\n"));
            }
        } else {
            host.engine.log.list.iter().for_each(|a| text.push_str(&format!("current:  {a}\n")));
        }
        print_lines(host, &text);
        return;
    }
    let (ip, port) = (args.arg(1), q_atoi(args.arg(2)));
    if port == 0 {
        host.print(&format!("{name}:  must specify a valid port\n"));
        return;
    }
    if ip.is_empty() {
        host.print(&format!("{name}:  unparseable address\n"));
        return;
    }
    let text = format!("{ip}:{port}");
    let Some(adr) = text.to_socket_addrs().ok().and_then(|mut a| a.find(SocketAddr::is_ipv4)) else {
        host.print(&format!("{name}:  unable to resolve {text}\n"));
        return;
    };
    match name {
        "logaddress" => {
            host.engine.log.net = Some(adr);
            host.print(&format!("logaddress:  {adr}\n"));
        }
        "logaddress_add" => {
            // The duplicate test skips the tail of a list of two or more.
            let list = &host.engine.log.list;
            let tested = if list.len() >= 2 { list.len() - 1 } else { list.len() };
            if list[..tested].contains(&adr) {
                host.print("logaddress_add:  address already in list\n");
                return;
            }
            host.engine.log.list.push(adr);
            host.print(&format!("logaddress_add:  {adr}\n"));
        }
        _ => {
            if host.engine.log.list.is_empty() {
                host.print("logaddress_del:  No addresses added yet\n");
                return;
            }
            match host.engine.log.list.iter().position(|a| *a == adr) {
                // Deleting the head drops the whole list.
                Some(0) => host.engine.log.list.clear(),
                Some(i) => {
                    host.engine.log.list.remove(i);
                }
                None => {
                    host.print("logaddress_del:  Couldn't find address in list\n");
                    return;
                }
            }
            host.print(&format!("deleting:  {adr}\n"));
        }
    }
}

/// `Log_Printf`: the time stamp, then the network targets and (with `log on` and a
/// multiplayer server or `sv_log_singleplayer`) the console echo and the file.
fn log_printf(host: &mut Host, console: &Console, text: &str) {
    let log = &mut host.engine.log;
    if log.net.is_none() && log.list.is_empty() && !log.active {
        return;
    }
    let (stamp, _, _) = log_stamp();
    let line = truncate_encoded(&format!("{stamp}{text}"), 1023).into_owned();
    let secret = console.cvars.value("sv_logsecret");
    let mut packets = Vec::new();
    if let Some(net) = log.net {
        packets.push((net, format!("log {line}")));
    }
    for a in &log.list {
        let body = if secret != 0.0 { format!("S{}{line}", console.cvars.string("sv_logsecret")) } else { format!("log {line}") };
        packets.push((*a, body));
    }
    if !packets.is_empty() {
        if log.socket.is_none() {
            let port = console.cvars.value("hostport") as u16;
            let port = if port == 0 { 27015 } else { port };
            log.socket = UdpSocket::bind(("0.0.0.0", port)).or_else(|_| UdpSocket::bind(("0.0.0.0", 0))).ok();
        }
        if let Some(sock) = &log.socket {
            for (to, body) in packets {
                let mut pkt = vec![0xFF; 4];
                pkt.extend_from_slice(&gs_common::bytestr::encode(&body));
                pkt.push(0);
                if let Err(e) = sock.send_to(&pkt, to) {
                    log::debug!("log packet to {to}: {e}");
                }
            }
        }
    }
    if log.active && (host.engine.svs_maxclients > 1 || console.cvars.value("sv_log_singleplayer") != 0.0) {
        if console.cvars.value("mp_logecho") != 0.0 {
            host.print(&line);
        }
        if let Some(file) = host.engine.log.file.clone()
            && console.cvars.value("mp_logfile") != 0.0
        {
            use std::io::Write;
            let res = std::fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .and_then(|mut f| f.write_all(&gs_common::bytestr::encode(&line.replace('\n', "\r\n"))));
            if let Err(e) = res {
                log::debug!("{}: {e}", file.display());
            }
        }
    }
}

/// `Log_Close`.
fn log_close(host: &mut Host, console: &Console) {
    if host.engine.log.file.is_some() {
        log_printf(host, console, "Log file closed\n");
    }
    host.engine.log.file = None;
}

/// `Log_Open`: `<logsdir>/LMMDDnnn.log` in the mod directory (first free index).
fn log_open(host: &mut Host, console: &Console) {
    if !host.engine.log.active || (console.cvars.value("sv_log_onefile") != 0.0 && host.engine.log.file.is_some()) {
        return;
    }
    if console.cvars.value("mp_logfile") == 0.0 {
        host.print("Server logging data to console.\n");
        return;
    }
    log_close(host, console);
    let dir = console.cvars.string("logsdir");
    let dir = if dir.is_empty() || dir.contains(':') || dir.contains("..") { "logs" } else { dir };
    let (_, month, day) = log_stamp();
    let base = format!("{dir}/L{month:02}{day:02}");
    let name = (0..1000).map(|n| format!("{base}{n:03}.log")).find(|n| host.fs.resolve_in(n, Some(PathId::GameConfig)).is_err());
    let opened = name.as_ref().and_then(|n| {
        let path = host.fs.write_path(n, PathId::GameConfig).ok()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        std::fs::write(&path, b"").ok()?;
        Some((n.replace('/', "\\"), path))
    });
    match opened {
        Some((shown, path)) => {
            host.print(&format!("Server logging data to file {shown}\n"));
            host.engine.log.file = Some(path);
            let version = host.client.version().to_owned();
            let game = host.engine.serverinfo.get("*gamedir").unwrap_or("").to_owned();
            log_printf(host, console, &format!("Log file started (file \"{shown}\") (game \"{game}\") (version \"48/{version}/10210\")\n"));
        }
        None => {
            host.print(&format!("Unable to open logfiles under {base}\nLogging disabled\n"));
            host.engine.log.active = false;
        }
    }
}

/// `cmd_pointfile`: `maps\<map>.pts` relative to the install root (hl.exe's working directory).
fn pointfile(host: &mut Host) {
    if host.client.world().is_none() {
        return;
    }
    let Some(model) = host.client.server_info().map(|i| i.map_model.clone()) else { return };
    let base = model.rsplit_once('.').map_or(model.as_str(), |(b, _)| b);
    let path = truncate_encoded(&format!("{base}.pts"), 259).replace('/', "\\");
    let Some(base_dir) = host.fs.base_dir() else { return };
    let full = base_dir.join(path.replace('\\', "/"));
    let Ok(text) = std::fs::read_to_string(&full) else {
        host.print(&format!("couldn't open {path}\n"));
        return;
    };
    host.print(&format!("Reading {path}...\n"));
    let mut count = 0;
    let mut nums = text.split_ascii_whitespace().map(|t| t.parse::<f32>());
    while let (Some(Ok(x)), Some(Ok(y)), Some(Ok(z))) = (nums.next(), nums.next(), nums.next()) {
        count += 1;
        if !host.effects.as_deref_mut().is_some_and(|e| e.point_particle([x, y, z], (-count) & 15)) {
            host.print("Not enough free particles\n");
            break;
        }
    }
    host.print(&format!("{count} points read\n"));
}
