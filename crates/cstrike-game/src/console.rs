//! The engine's console layer for the game window (docs/re/engine-core.md §2–3,
//! docs/re/client-sim.md Part A): engine and client-DLL cvars and commands, the startup config
//! sequence, the console host (connection, userinfo, `exec` files, connect/quit/...), winit →
//! GoldSrc key numbers, the terminal as console input and output, and cvar query replies.

use std::sync::mpsc::{Receiver, channel};

use cs_client::hud::Hud;
use cs_client::input::Input;
use glam::Vec3;
use gs_client::{Client, State};
use gs_common::info::InfoString;
use gs_console::cvar::flags;
use gs_console::{Args, Console, ConsoleHost, cmd_flags, keys};
use gs_fs::{FileSystem, PathId};
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

use crate::audio::Audio;
use crate::hud_host::HudHost;

/// `CL_Init`'s commands, registered (sorted) after the client DLL's `HUD_Init`: name and flags
/// (audit-commands.md §1.2 rows 1–31). Editing demo files and `startmovie` (a BMP per frame,
/// which fills the disk) are privileged here so server text cannot run them; the original
/// gives them flag 0 (`removedemo` FILTERED).
const CL_INIT_COMMANDS: &[(&str, u32)] = &[
    ("gamedir", 0),
    ("connect", cmd_flags::PRIVILEGED),
    ("fullserverinfo", 0),
    ("retry", cmd_flags::PRIVILEGED),
    ("disconnect", 0),
    ("record", 0),
    ("stop", 0),
    ("playdemo", 0),
    ("viewdemo", 0),
    ("timedemo", 0),
    ("gg", 0),
    ("listdemo", 0),
    ("appenddemo", cmd_flags::PRIVILEGED),
    ("removedemo", cmd_flags::PRIVILEGED),
    ("swapdemo", cmd_flags::PRIVILEGED),
    ("setdemoinfo", cmd_flags::PRIVILEGED),
    ("listen", 0),
    ("commentator", 0),
    ("waveplaylen", 0),
    ("snapshot", 0),
    ("startmovie", cmd_flags::PRIVILEGED),
    ("endmovie", 0),
    ("entities", 0),
    ("rcon", cmd_flags::PRIVILEGED),
    ("cl_messages", 0),
    ("pingservers", 0),
    ("slist", 0),
    ("list", 0),
    ("upload", 0),
    ("httpstop", 0),
    ("spec_pos", 0),
];

/// The commands `gs_client::Client::demo_command` runs (demo files, the demo loop, uploads).
const CLIENT_DEMO_COMMANDS: &[&str] = &[
    "record", "stop", "playdemo", "viewdemo", "timedemo", "gg", "listdemo", "appenddemo", "removedemo", "swapdemo", "setdemoinfo", "startdemos",
    "demos", "stopdemo", "upload", "httpstop",
];

/// `startmovie`: the snapshot name prefix and the next frame number (`VID_TakeSnapshot`
/// `"%s%05d.bmp"`).
#[derive(Debug, Clone)]
pub struct Movie {
    pub name: String,
    pub counter: u32,
}

/// A console with the engine's and the CS client's commands and cvars registered: every cvar
/// hw.dll and the client DLL register, with their defaults and flags. Commands follow
/// `Host_Init`'s order so the list order is the original's (audit-commands.md §1.4): the engine's
/// sorted inserts, GameUI's `condump` and DemoPlayer's `dem_*` at the head, the client DLL's at
/// the head, then `CL_Init`'s sorted inserts.
pub fn new_console() -> Console {
    let mut console = Console::new(true);
    for &(name, default, f) in crate::engine_cvars::CVARS {
        // The console layer registers `cl_filterstuffcmd` and `developer` itself.
        if console.cvars.find(name).is_none() {
            console.register_cvar(name, default, f);
        }
    }
    // Not in the original: the prediction log (docs/deviations.md "Diagnostics"). Hidden from the
    // server, which neither sees it nor switches it on to fill the disk.
    console.register_cvar("cl_predlog", "0", flags::HIDDEN);
    let parm = crate::cmdline::check_parm;
    // The engine's startup writes (cvars-engine.md, "Startup writes"): Sys_InitGame/Host_Init
    // (`-dev`, `-console`/`-toconsole`), SV_Init (`sv_lan` on a non-dedicated engine), CL_Init
    // (`-novid`, `-nomousegrab`) and R_Init (multitexture turns gl_overbright off).
    let dev = parm("-dev");
    let mut write = |name: &str, value: &str| {
        console.cvars.direct_set(name, value);
    };
    if dev {
        write("developer", "1");
    }
    if dev || parm("-console") || parm("-toconsole") {
        write("console", "1");
    }
    write("sv_lan", "1");
    if dev {
        write("sv_cheats", "1");
    }
    if dev || parm("-novid") {
        write("cl_skipvid", "1");
    }
    if parm("-nomousegrab") {
        write("cl_mousegrab", "0");
    }
    write("gl_overbright", "0");
    // GL_InitCvars: gl_log only with -gl_log.
    if parm("-gl_log") {
        console.register_command("gl_log", 0);
    }
    // S_Init registers the sound commands unless -nosound.
    if !parm("-nosound") {
        for name in crate::audio::COMMANDS {
            console.register_command(name, 0);
        }
    }
    // GameUI's only command (CGameConsole::Initialize, pfnAddCommand).
    console.register_command("condump", cmd_flags::HUD);
    // DemoPlayer_Load: the module's commands (SystemWrapper::RegisterCommand).
    for name in gs_hltv::player::DemoPlayer::COMMANDS {
        console.register_wrapper_command(name);
    }
    for &(name, default, f) in cs_client::cvars::CVARS {
        console.register_cvar(name, default, f | flags::CLIENTDLL);
    }
    // CHud::Init: the fog overrides only with developer > 0 (the czero-only cvars never).
    if console.cvars.value("developer") > 0.0 {
        for &(name, default, f) in cs_client::cvars::DEVELOPER_CVARS {
            console.register_cvar(name, default, f | flags::CLIENTDLL);
        }
    }
    for name in cs_client::COMMANDS {
        console.register_command(name, cmd_flags::HUD);
    }
    for &(name, f) in CL_INIT_COMMANDS {
        console.register_command(name, f);
    }
    console
}

/// `Host_Init` + `ForceReloadProfile`: `GL_Config`'s vendor cfg (`hw/geforce.cfg` for an
/// NVIDIA GeForce/Quadro renderer; the other branches only match pre-2002 hardware) and
/// `hw/opengl.cfg`, then `valve.rc` (via the filesystem fallback; it runs autoexec.cfg),
/// `controller.cfg`, `config.cfg` (which ends with `exec userconfig.cfg`) and `+mlook`; then
/// the forced `~`/`` ` ``/ESC bindings, and a placeholder name becomes `persona` (the Steam
/// persona name in the original).
pub fn startup(console: &mut Console, host: &mut Host, persona: &str, gl_vendor_nvidia: bool, gl_renderer: &str, gameui_cmds: &str) {
    // Host_Init: "<Host_Version>,48,<build>".
    let sv_version = format!("{},48,{}", host.client.version(), crate::engine_cmds::BUILD_NUMBER);
    console.cvars.direct_set("sv_version", &sv_version);
    // NET_GetLocalAddress: the host name's address with the NS_SERVER port ([INFERENCE] 27015:
    // this client opens no server socket, the original binds `port`, default 27015).
    if let Some(address) = local_address() {
        console.cvars.direct_set("net_address", &address);
    }
    let geforce = gl_vendor_nvidia && (gl_renderer.contains("Quadro") || gl_renderer.contains("GeForce"));
    let vendor = if geforce { "exec hw/geforce.cfg\n" } else { "" };
    // ... then CBaseUI::Start: GameUI's music and menu activation commands.
    console.add_text(&format!("{vendor}exec hw/opengl.cfg\nexec valve.rc\nexec controller.cfg\nexec config.cfg\n+mlook\n{gameui_cmds}"));
    console.execute(host);
    for key in [b'~', b'`'] {
        console.bindings.set(key, "toggleconsole");
    }
    console.bindings.set(gs_console::keys::K_ESCAPE, "cancelselect");
    // -nomousegrab beats config.cfg.
    if crate::cmdline::check_parm("-nomousegrab") {
        console.direct_set(host, "cl_mousegrab", "0");
    }
    // HKCU\Software\Valve\Steam\Rate overrides `rate`; a missing value is written from it.
    match crate::registry::steam_string("Rate") {
        Some(rate) => console.direct_set(host, "rate", &rate),
        None => crate::registry::set_steam_string("Rate", console.cvars.string("rate")),
    }
    let name = console.cvars.string("name");
    if name.eq_ignore_ascii_case("player") || name.eq_ignore_ascii_case("unnamed") {
        console.direct_set(host, "name", persona);
    }
}

/// The first IPv4 address the computer name resolves to, with port 27015.
fn local_address() -> Option<String> {
    use std::net::ToSocketAddrs;
    let name = std::env::var("COMPUTERNAME").ok()?;
    let ip = (name.as_str(), 0).to_socket_addrs().ok()?.find_map(|a| match a {
        std::net::SocketAddr::V4(v4) => Some(*v4.ip()),
        std::net::SocketAddr::V6(_) => None,
    })?;
    Some(format!("{ip}:27015"))
}

/// A camera command's or `CAM_Think`'s cvar write: `Cvar_SetValue("cam_command", 0)` or the
/// value-only `cam_snapto` toggle.
pub fn apply_cam_effect(console: &mut Console, effect: cs_client::cam::CamEffect) {
    use cs_client::cam::CamEffect;
    match effect {
        CamEffect::None => {}
        CamEffect::ResetCommand => {
            console.cvars.direct_set("cam_command", "0");
        }
        CamEffect::SnapTo(v) => console.cvars.set_value_only("cam_snapto", v),
    }
}

/// Copies the cvars `CL_Move`, the connection and the netchan read into the client.
pub fn apply_client_cvars(console: &Console, client: &mut Client) {
    let v = |name| console.cvars.value(name);
    client.config.cmdrate = v("cl_cmdrate");
    client.config.updaterate = v("cl_updaterate");
    client.config.cmdbackup = v("cl_cmdbackup").clamp(0.0, 8.0) as u8;
    client.config.resend = f64::from(v("cl_resend"));
    client.config.timeout = f64::from(v("cl_timeout"));
    client.config.dlmax = v("cl_dlmax") as i32;
    client.config.allow_download = v("cl_allowdownload") != 0.0;
    client.config.download_ingame = v("cl_download_ingame") != 0.0;
    client.config.allow_upload = v("cl_allowupload") != 0.0;
    // HPAK_CheckSize resets a negative hpk_maxsize to 0.
    client.config.hpk_maxsize = v("hpk_maxsize").max(0.0);
    client.ex_interp = f64::from(v("ex_interp"));
    console.cvars.string("rcon_password").clone_into(&mut client.config.rcon_password);
    client.config.slist = v("cl_slist");
    // UDP_OpenSocket: port -1 binds an ephemeral port.
    let port = v("multicastport") as i32;
    client.config.multicast_port = if port == -1 { 0 } else { port as u16 };
    client.config.clock_reset = f64::from(v("cl_clockreset"));
    client.config.fix_time_rate = f64::from(v("cl_fixtimerate"));
    client.config.no_smooth = v("cl_nosmooth") != 0.0;
    client.config.smooth_time = v("cl_smoothtime");
    client.config.ideal_pitch_scale = v("cl_idealpitchscale");
    client.config.show_error = v("cl_showerror") != 0.0;
    client.config.cheats = v("sv_cheats") != 0.0;
    client.config.fake_lag = v("fakelag");
    client.config.fake_loss = v("fakeloss");
    client.config.fix_interpolation_artifacts = v("cl_fixmodelinterpolationartifacts") >= 1.0;
    client.config.bmodel_interp = v("r_bmodelinterp") != 0.0;
    client.config.solid_players = v("cl_solid_players") != 0.0;
    client.config.nosound = v("nosound") != 0.0;
    client.config.show_net = v("cl_shownet");
    client.config.show_messages = v("cl_showmessages") != 0.0;
    client.config.need_instanced = v("cl_needinstanced") != 0.0;
    client.config.show_packets = v("net_showpackets");
    client.config.show_drop = v("net_showdrop") != 0.0;
    client.config.startup_timings = v("fs_startup_timings") != 0.0;
    client.config.pred_log = v("cl_predlog") != 0.0;
}

/// `svc_sendcvarvalue(2)`'s answer for `name`; a hidden cvar answers as no cvar.
pub fn cvar_reply(console: &Console, name: &str) -> String {
    let var = (gs_common::bytestr::encoded_len(name) < 255).then(|| console.cvars.find(name)).flatten().filter(|v| v.flags & flags::HIDDEN == 0);
    match var {
        None => "Bad CVAR request".to_owned(),
        Some(v) if v.flags & flags::PRIVILEGED != 0 => "CVAR is privileged".to_owned(),
        Some(v) if v.flags & flags::SERVER != 0 => "CVAR is server-only".to_owned(),
        Some(v) if v.flags & flags::PROTECTED != 0 => "CVAR is protected".to_owned(),
        Some(v) => v.string.clone(),
    }
}

/// Lines typed into the terminal, read on a background thread.
pub fn terminal_input() -> Receiver<String> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// A console command acting on GameUI, run by the frame (which has the surface).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiOp {
    ToggleConsole,
    HideConsole,
    Clear,
    Condump,
    Escape,
    /// `map`: `VGuiWrap2_LoadingStarted("level", name)`.
    LoadingStarted,
    /// `VGuiWrap2_LoadingFinished`.
    LoadingFinished,
}

/// The console's view of the running client.
pub struct Host<'a> {
    pub client: &'a mut Client,
    pub fs: &'a FileSystem,
    pub input: &'a mut Input,
    /// The client DLL's HUD and the engine pieces its commands reach.
    pub hud: &'a mut Hud,
    pub screen: &'a mut gs_draw::Screen,
    pub renderer: &'a mut gs_render::Renderer,
    pub titles: &'a gs_draw::titles::Titles,
    /// `cl.viewangles`.
    pub viewangles: &'a mut Vec3,
    /// `quit` / `exit` ran.
    pub quit: &'a mut bool,
    /// The engine-side command state (`engine_cmds`).
    pub engine: &'a mut crate::engine_cmds::EngineState,
    /// The particle system (`pointfile`); absent before the first frame.
    pub effects: Option<&'a mut gs_effects::Effects>,
    /// The sound system (`None` without an output device).
    pub audio: Option<&'a mut Audio>,
    /// The `messagemode` line.
    pub chat: &'a mut crate::notify::Chat,
    /// `startmovie`'s state; a frame is captured after each update while it is set.
    pub movie: &'a mut Option<Movie>,
    /// GameUI work for the frame (`toggleconsole`, `hideconsole`, `clear`, `condump`, `escape`).
    pub ui_ops: &'a mut Vec<UiOp>,
}

impl ConsoleHost for Host<'_> {
    fn print(&mut self, text: &str) {
        crate::notify::con_printf(text);
    }

    fn connected(&self) -> bool {
        self.client.state() >= State::Connected
    }

    fn multiplayer(&self) -> bool {
        self.client.state() >= State::Connecting && self.client.protocol().maxclients > 1
    }

    fn send_string_cmd(&mut self, text: &str) {
        self.client.send_string_cmd(text);
    }

    fn forward_string_cmd(&mut self, text: &str) {
        self.client.forward_string_cmd(text);
    }

    fn userinfo(&mut self) -> &mut InfoString {
        &mut self.client.config.userinfo
    }

    fn command(&mut self, console: &mut Console, args: &Args) {
        let name = args.arg(0).to_ascii_lowercase();
        if let Some(audio) = self.audio.as_deref_mut()
            && audio.command(console, args)
        {
            return;
        }
        let mut va = self.viewangles.to_array();
        // CAM_Init's commands (client.dll, flag HUD).
        let maxclients = i32::from(self.client.protocol().maxclients);
        if let Some(effect) = self.input.cam.command(args, maxclients, va, &console.cvars) {
            apply_cam_effect(console, effect);
            return;
        }
        let input = self.input.command(args, &mut va, &console.cvars, self.client.time());
        if input {
            *self.viewangles = Vec3::from(va);
        }
        let mut eng = HudHost {
            screen: self.screen,
            renderer: self.renderer,
            fs: self.fs,
            console,
            client: self.client,
            titles: self.titles,
            audio: self.audio.as_deref_mut(),
            effects: None,
            motd: Some(&mut self.engine.motd),
        };
        // IN_ScoreDown / IN_ScoreUp: the kbutton plus the viewport's scoreboard.
        match name.as_str() {
            "+score" | "+showscores" => self.hud.score(true, &mut eng),
            "-score" | "-showscores" => self.hud.score(false, &mut eng),
            _ => {}
        }
        // IN_*Down / IN_*Up: the observer buttons after the key state changed.
        if input {
            let down = match name.as_str() {
                "+attack" => 0x1,
                "+jump" => 0x2,
                "+duck" => 0x4,
                "+forward" => 0x8,
                "+back" => 0x10,
                "+use" => 0x20,
                "+moveleft" => 0x200,
                "+moveright" => 0x400,
                "+attack2" => 0x800,
                _ => 0,
            };
            let up = match name.as_str() {
                "-forward" => 0x8,
                "-back" => 0x10,
                "-moveleft" => 0x200,
                "-moveright" => 0x400,
                _ => 0,
            };
            if down != 0 {
                self.hud.handle_buttons_down(down, &mut eng);
            }
            if up != 0 {
                self.hud.handle_buttons_up(up);
            }
        }
        if input || self.hud.command(args, &mut eng) {
            return;
        }
        match name.as_str() {
            "quit" | "exit" => *self.quit = true,
            "disconnect" => self.client.disconnect("disconnect"),
            "toggleconsole" => self.ui_ops.push(UiOp::ToggleConsole),
            "hideconsole" => self.ui_ops.push(UiOp::HideConsole),
            "clear" => self.ui_ops.push(UiOp::Clear),
            "condump" => self.ui_ops.push(UiOp::Condump),
            "escape" => self.ui_ops.push(UiOp::Escape),
            // Steam Input action sets and Steam friends rich presence: this client opens neither
            // interface, which is the case where hw.dll's handlers do nothing.
            "inputset_menu" | "inputset_game" | "richpresence_override" | "richpresence_nicemap" | "richpresence_update" | "richpresence_gamemode" => {}
            "reconnect" => self.client.reconnect(),
            // cmd_plus_voicerecord / cmd_minus_voicerecord: in game only.
            "+voicerecord" if self.client.state() == State::Active => {
                if let Some(audio) = self.audio.as_deref_mut() {
                    audio.voice_record_start(console.cvars.value("voice_recordtofile") as i32 != 0);
                }
            }
            "-voicerecord" if self.client.state() == State::Active => {
                if let Some(audio) = self.audio.as_deref_mut()
                    && audio.voice_recording()
                {
                    // CL_AddVoiceToDatagram(bFinal = 1): sent with the next clc_move.
                    let loopback = console.cvars.value("voice_loopback") != 0.0;
                    self.client.add_voice_to_datagram(&mut crate::audio::VoiceSendHost { audio: Some(&mut *audio), loopback });
                    audio.voice_record_stop();
                }
            }
            "+voicerecord" | "-voicerecord" => {}
            "cd" | "mp3" => {
                let argv: Vec<&str> = (0..args.argc()).map(|i| args.arg(i)).collect();
                let lines = self.audio.as_deref_mut().map(|a| a.music_command(&console.cvars, &argv)).unwrap_or_default();
                lines.iter().for_each(|l| self.print(l));
            }
            // cmd_setpause / cmd_unpause: forwarded only with a level loaded (cl_levelname).
            "setpause" | "unpause" => {
                if self.client.server_info().is_some_and(|i| !i.map_model.is_empty()) {
                    gs_console::forward_to_server(self, args);
                }
            }
            // cmd_messagemode / cmd_messagemode2: open the engine chat line.
            "messagemode" | "messagemode2" => {
                let target = if name == "messagemode2" {
                    Some("say_team")
                } else {
                    (args.argc() == 2).then(|| args.arg(1))
                };
                self.chat.open(target, console.is_privileged());
            }
            // Joy_AdvancedUpdate_f: without a game controller only the print is observable.
            "joyadvancedupdate" => {
                let joyname = console.cvars.string("joyname").to_owned();
                if console.cvars.value("joyadvanced") != 0.0 && joyname != "joystick" {
                    self.print(&format!("\n{joyname} configured\n\n"));
                }
            }
            "contimes" => {
                let argv: Vec<&str> = (0..args.argc()).map(|i| args.arg(i)).collect();
                self.print(&crate::notify::contimes(&argv));
            }
            "rcon" => {
                let address = console.cvars.string("rcon_address").to_owned();
                let port = console.cvars.value("rcon_port");
                self.client.rcon(args.args.as_deref(), &address, port);
            }
            "connect" => match args.args.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
                Some(address) => self.client.connect(address),
                None => self.print("usage: connect <server>\n"),
            },
            "retry" => self.client.retry(),
            "startmovie" => {
                if args.argc() != 3 {
                    self.print("startmovie <filename> <fps>\n");
                } else if !self.client.demo_playing() {
                    self.print("startmovie only allowed during demo playback.\n");
                } else if args.arg(1).starts_with('/') || [":", "..", "\\"].iter().any(|s| args.arg(1).contains(s)) {
                    self.print(&format!("Couldn't open {} (contains illegal characters).\n", args.arg(1)));
                } else {
                    // The value only: host_framerate's string keeps what it was.
                    let fps: f64 = args.arg(2).trim().parse().unwrap_or(0.0);
                    console.cvars.set_value_only("host_framerate", (1.0 / fps) as f32);
                    *self.movie = Some(Movie { name: args.arg(1).to_owned(), counter: 1 });
                    self.print("Started recording movie...\n");
                }
            }
            "endmovie" => {
                if self.movie.take().is_some() {
                    console.cvars.set_value_only("host_framerate", 0.0);
                    self.print("Stopped recording movie...\n");
                } else {
                    self.print("No movie started.\n");
                }
            }
            n if CLIENT_DEMO_COMMANDS.contains(&n) => {
                let argv: Vec<&str> = (0..args.argc()).map(|i| args.arg(i)).collect();
                self.client.demo_command(&argv);
            }
            n if gs_hltv::player::DemoPlayer::COMMANDS.contains(&n) => self.module_command(console, args),
            _ if crate::engine_cmds::command(self, console, args) => {}
            _ => log::debug!("unhandled command {name}"),
        }
    }

    fn read_config(&mut self, name: &str) -> Option<Vec<u8>> {
        [Some(PathId::GameConfig), Some(PathId::Game), None]
            .into_iter()
            .find_map(|id| self.fs.resolve_in(name, id).ok())
            .and_then(|path| std::fs::read(path).ok())
    }

    fn hud_command_ran(&mut self, name: &str) {
        self.client.demo_record_hud_command(name);
    }
}

impl Host<'_> {
    /// `SystemWrapper_CommandForwarder` → `SystemWrapper::ExecuteString`: module commands by
    /// name prefix with the whole token, everything else back to `Cmd_ExecuteString`.
    fn module_command(&mut self, console: &mut Console, args: &Args) {
        use crate::demoplayer::Exec;
        let Some(p) = self.client.demo_player.clone() else { return };
        let line = crate::demoplayer::forwarded_line(args.arg(0), args.args.as_deref(), args.argc());
        for exec in crate::demoplayer::execute_string(&line, &gs_hltv::player::DemoPlayer::COMMANDS) {
            match exec {
                Exec::Module { id, line } => {
                    p.borrow_mut().execute_command(id, &line);
                    let sys = p.borrow().sys.clone();
                    crate::demoplayer::drain_log(&sys, console.cvars.value("developer") != 0.0);
                }
                Exec::Engine(s) => console.execute_string(self, &s, true),
                Exec::TooLong => self.print("WARNING! System::ExecuteString: Command token too long.\n"),
            }
        }
    }
}

/// The GoldSrc key number of a physical key (`IN_TranslateKeyCode` equivalent).
pub fn key_number(code: KeyCode) -> Option<u8> {
    use KeyCode as K;
    Some(match code {
        K::KeyA => b'a',
        K::KeyB => b'b',
        K::KeyC => b'c',
        K::KeyD => b'd',
        K::KeyE => b'e',
        K::KeyF => b'f',
        K::KeyG => b'g',
        K::KeyH => b'h',
        K::KeyI => b'i',
        K::KeyJ => b'j',
        K::KeyK => b'k',
        K::KeyL => b'l',
        K::KeyM => b'm',
        K::KeyN => b'n',
        K::KeyO => b'o',
        K::KeyP => b'p',
        K::KeyQ => b'q',
        K::KeyR => b'r',
        K::KeyS => b's',
        K::KeyT => b't',
        K::KeyU => b'u',
        K::KeyV => b'v',
        K::KeyW => b'w',
        K::KeyX => b'x',
        K::KeyY => b'y',
        K::KeyZ => b'z',
        K::Digit0 => b'0',
        K::Digit1 => b'1',
        K::Digit2 => b'2',
        K::Digit3 => b'3',
        K::Digit4 => b'4',
        K::Digit5 => b'5',
        K::Digit6 => b'6',
        K::Digit7 => b'7',
        K::Digit8 => b'8',
        K::Digit9 => b'9',
        K::Minus => b'-',
        K::Equal => b'=',
        K::BracketLeft => b'[',
        K::BracketRight => b']',
        K::Backslash => b'\\',
        K::Semicolon => b';',
        K::Quote => b'\'',
        K::Backquote => b'`',
        K::Comma => b',',
        K::Period => b'.',
        K::Slash => b'/',
        K::Space => keys::K_SPACE,
        K::Enter => keys::K_ENTER,
        K::Tab => keys::K_TAB,
        K::Escape => keys::K_ESCAPE,
        K::Backspace => keys::K_BACKSPACE,
        K::ArrowUp => keys::K_UPARROW,
        K::ArrowDown => keys::K_DOWNARROW,
        K::ArrowLeft => keys::K_LEFTARROW,
        K::ArrowRight => keys::K_RIGHTARROW,
        K::AltLeft | K::AltRight => keys::K_ALT,
        K::ControlLeft | K::ControlRight => keys::K_CTRL,
        K::ShiftLeft | K::ShiftRight => keys::K_SHIFT,
        K::F1 => keys::K_F1,
        K::F2 => keys::K_F1 + 1,
        K::F3 => keys::K_F1 + 2,
        K::F4 => keys::K_F1 + 3,
        K::F5 => keys::K_F1 + 4,
        K::F6 => keys::K_F1 + 5,
        K::F7 => keys::K_F1 + 6,
        K::F8 => keys::K_F1 + 7,
        K::F9 => keys::K_F1 + 8,
        K::F10 => keys::K_F1 + 9,
        K::F11 => keys::K_F1 + 10,
        K::F12 => keys::K_F1 + 11,
        K::Insert => keys::K_INS,
        K::Delete => keys::K_DEL,
        K::PageDown => keys::K_PGDN,
        K::PageUp => keys::K_PGUP,
        K::Home => keys::K_HOME,
        K::End => keys::K_END,
        K::Numpad7 => keys::K_KP_HOME,
        K::Numpad8 => keys::K_KP_UPARROW,
        K::Numpad9 => keys::K_KP_PGUP,
        K::Numpad4 => keys::K_KP_LEFTARROW,
        K::Numpad5 => keys::K_KP_5,
        K::Numpad6 => keys::K_KP_RIGHTARROW,
        K::Numpad1 => keys::K_KP_END,
        K::Numpad2 => keys::K_KP_DOWNARROW,
        K::Numpad3 => keys::K_KP_PGDN,
        K::NumpadEnter => keys::K_KP_ENTER,
        K::Numpad0 => keys::K_KP_INS,
        K::NumpadDecimal => keys::K_KP_DEL,
        K::NumpadDivide => keys::K_KP_SLASH,
        K::NumpadSubtract => keys::K_KP_MINUS,
        K::NumpadAdd => keys::K_KP_PLUS,
        K::CapsLock => keys::K_CAPSLOCK,
        K::NumpadMultiply => keys::K_KP_MUL,
        K::SuperLeft | K::SuperRight => keys::K_WIN,
        K::Pause => keys::K_PAUSE,
        _ => return None,
    })
}

/// `IN_MouseEvent`: buttons 1..5 are key numbers 241..245.
pub fn mouse_key_number(button: MouseButton) -> Option<u8> {
    Some(match button {
        MouseButton::Left => keys::K_MOUSE1,
        MouseButton::Right => keys::K_MOUSE1 + 1,
        MouseButton::Middle => keys::K_MOUSE1 + 2,
        MouseButton::Back => keys::K_MOUSE1 + 3,
        MouseButton::Forward => keys::K_MOUSE1 + 4,
        MouseButton::Other(_) => return None,
    })
}

/// The `+command` a scripted `--hold` key stands for.
pub fn hold_command(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "w" => "forward",
        "s" => "back",
        "a" => "moveleft",
        "d" => "moveright",
        "e" => "use",
        "r" => "reload",
        "space" | "jump" => "jump",
        "ctrl" | "duck" => "duck",
        "attack" | "enter" | "mouse1" => "attack",
        "attack2" | "mouse2" => "attack2",
        "tab" | "score" => "showscores",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cvar_queries_hide_privileged_and_unknown_values() {
        let console = new_console();
        assert_eq!(cvar_reply(&console, "cl_forwardspeed"), "400");
        assert_eq!(cvar_reply(&console, "CL_FORWARDSPEED"), "400");
        assert_eq!(cvar_reply(&console, "cl_filterstuffcmd"), "CVAR is privileged");
        assert_eq!(cvar_reply(&console, "nope"), "Bad CVAR request");
        assert_eq!(cvar_reply(&console, "cl_predlog"), "Bad CVAR request", "ours, hidden: as in the original, which has none");
        assert_eq!(cvar_reply(&console, &"x".repeat(300)), "Bad CVAR request");
    }

    /// The engine list as `cmdlist` and GameUI's autocomplete walk it (audit-commands.md §1.4,
    /// plus DemoPlayer's `dem_*` pushed at the head after GameUI's `condump`).
    #[test]
    fn command_list_follows_the_original_registration_order() {
        let expected = "appenddemo cl_messages commentator connect disconnect endmovie entities fullserverinfo gamedir gg httpstop list \
            listdemo listen pingservers playdemo rcon record removedemo retry setdemoinfo slist snapshot spec_pos \
            startmovie stop swapdemo timedemo upload viewdemo voice_showbanned spec_autodirector spec_drawstatus spec_drawcone spec_drawnames togglescores \
            spec_menu spec_help spec_decal spec_toggleinset spec_mode hud_saytext hideradar drawradar clearplayers trackplayer adjust_crosshair invprev \
            invnext cancelselect slot10 slot9 slot8 slot7 slot6 slot5 slot4 slot3 slot2 slot1 \
            -nvgadjust +nvgadjust buy rebuy autobuy gunsmoke -commandmenu +commandmenu joyadvancedupdate force_centerview snapto -camdistance \
            +camdistance -cammousemove +cammousemove firstperson thirdperson -camout +camout -camin +camin -camyawright +camyawright -camyawleft \
            +camyawleft -campitchdown +campitchdown -campitchup +campitchup -break +break -graph +graph -showscores +showscores -score \
            +score -alt1 +alt1 -reload +reload -duck +duck -jlook +jlook -mlook +mlook -klook \
            +klook impulse -jump +jump -use +use -attack2 +attack2 -attack +attack -speed +speed \
            -moveright +moveright -moveleft +moveleft -strafe +strafe -lookdown +lookdown -lookup +lookup -back +back \
            -forward +forward -right +right -left +left -movedown +movedown -moveup +moveup \
            dem_save dem_start dem_speed dem_pause dem_forcehltv dem_jump condump +voicerecord \
            -voicerecord _careeraudio _restart _setaddons_folder _setgamedir _sethdmodels _setrenderer _setvideomode addip alias autosave banid \
            bind career cd changelevel changelevel2 clear cmd cmdlist condebug contimes cvarlist delta_clear \
            delta_stats demos dlfile dropclient echo envmap escape exec exit flush fly fullinfo \
            fullupdate gl_dump gl_reloadshaders gl_texels god heartbeat hideconsole hpkextract hpklist hpkremove hpkval inputset_game \
            inputset_menu interp kick kill listid listip load localinfo log logaddress logaddress_add logaddress_del \
            map maps maxplayers mcache messagemode messagemode2 motd motd_write mp3 new noclip notarget \
            pause ping play playvol pointfile quit reconnect reload removeid removeip resetrcon restart \
            richpresence_gamemode richpresence_nicemap richpresence_override richpresence_update save say say_team screenshot sendents sendres serverinfo setinfo \
            setmaster setpause show_osk showinfo shutdownserver sizedown sizeup soundfade soundinfo soundlist spawn speak \
            spk startdemos stat stats status stopdemo stopsound stuffcmds tell timerefresh toggleconsole unbind \
            unbindall unpause user users version viewframe viewmodel viewnext viewprev wait waveplaylen writecfg \
            writeid writeip";
        let console = new_console();
        let names: Vec<&str> = console.command_names().collect();
        assert_eq!(names, expected.split_whitespace().collect::<Vec<_>>());
    }
}
