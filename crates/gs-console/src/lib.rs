//! The GoldSrc console core (docs/re/engine-core.md §2–3): the privileged and filtered command
//! buffers, the command/alias/cvar registry with its privilege filtering, the engine's built-in
//! console commands, server stufftext, key bindings and `config.cfg` output.
//!
//! The console owns no game state. Whatever lives elsewhere (printing, the server connection,
//! `cls.userinfo`, config files, game commands) is reached through [`ConsoleHost`].

pub mod cbuf;
pub mod cvar;
pub mod keys;
pub mod stufftext;

use gs_common::info::InfoString;
pub use gs_common::token::Args;
use gs_common::token::{self, com_parse};

use crate::cbuf::{CBUF_SIZE, Cbuf};
use crate::cvar::{Cvars, SetResult, flags as cvar_flags};
use crate::keys::Bindings;
use crate::stufftext::{StuffPolicy, valid_stuff_text};

/// `cmd_function_t` flags.
pub mod cmd_flags {
    /// Registered by the client DLL.
    pub const HUD: u32 = 0x01;
    pub const GAME: u32 = 0x02;
    pub const WRAPPER: u32 = 0x04;
    /// Never runs from the filtered buffer.
    pub const PRIVILEGED: u32 = 0x08;
    /// Blocked from the filtered buffer when `cl_filterstuffcmd > 0`.
    pub const FILTERED: u32 = 0x10;
    /// Not in the original: a command the original client lacks. Server text passes over it as
    /// an unknown name, so it goes to the server as the original sends it.
    pub const HIDDEN: u32 = 0x4000_0000;
}

/// Name prefixes the filtered buffer may not touch when `cl_filterstuffcmd > 0`.
const FILTERED_PREFIXES: [&str; 5] = ["cl_", "gl_", "m_", "r_", "hud_"];

/// What the console needs from the rest of the client.
pub trait ConsoleHost {
    /// `Con_Printf`.
    fn print(&mut self, text: &str);
    /// `cls.state` is connected, uninitialised or active (3..=5).
    fn connected(&self) -> bool;
    /// SPONLY cvars are locked: `cls.state >= 2 && cl.maxclients > 1`.
    fn multiplayer(&self) -> bool;
    /// Appends a reliable `clc_stringcmd` (byte 3 + `text`) the engine writes itself.
    fn send_string_cmd(&mut self, text: &str);
    /// `Cmd_ForwardToServerInternal`'s reliable write (dropped in a demo playback or when it does
    /// not fit the reliable buffer).
    fn forward_string_cmd(&mut self, text: &str);
    /// `cls.userinfo`.
    fn userinfo(&mut self) -> &mut InfoString;
    /// A command registered with [`Console::register_command`] ran.
    fn command(&mut self, console: &mut Console, args: &Args);
    /// A config file for `exec`: path ID GAMECONFIG, then GAME, then any.
    fn read_config(&mut self, name: &str) -> Option<Vec<u8>>;
    /// A client-DLL command's handler returned (`CL_RecordHUDCommand` with its registered
    /// name).
    fn hud_command_ran(&mut self, _name: &str) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Builtin {
    Alias,
    Echo,
    Wait,
    Exec,
    Cmd,
    CvarList,
    Bind,
    Unbind,
    UnbindAll,
    SetInfo,
    FullInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handler {
    Builtin(Builtin),
    /// `Cmd_ForwardToServer` (e.g. `kill`, `say`).
    Forward,
    Host,
}

#[derive(Debug, Clone)]
struct Command {
    name: String,
    flags: u32,
    handler: Handler,
}

/// Engine commands registered before the client DLL (`Host_InitCommands`, `Cmd_Init`, `Con_Init`,
/// `HPAK_Init`, `Key_Init`, `NET_Init`, `DELTA_Init`, `SV_Init`, `Memory_Init`, `Draw_Init`,
/// `R_Init`, `SCR_Init`; audit-commands.md §1.2): name, flags, handler. `CL_Init`'s and
/// `S_Init`'s commands are registered by the host at their own points.
const ENGINE_COMMANDS: &[(&str, u32, Handler)] = &[
    // Host_InitCommands.
    ("cd", cmd_flags::FILTERED, Handler::Host),
    ("mp3", 0, Handler::Host),
    ("_careeraudio", 0, Handler::Host),
    ("shutdownserver", 0, Handler::Host),
    ("soundfade", 0, Handler::Host),
    // status / stat: forwarded without a local server.
    ("status", 0, Handler::Forward),
    ("stat", 0, Handler::Forward),
    ("quit", cmd_flags::PRIVILEGED, Handler::Host),
    ("_restart", cmd_flags::PRIVILEGED, Handler::Host),
    ("_setrenderer", cmd_flags::PRIVILEGED, Handler::Host),
    ("_setvideomode", cmd_flags::PRIVILEGED, Handler::Host),
    ("_setgamedir", cmd_flags::PRIVILEGED, Handler::Host),
    ("_sethdmodels", cmd_flags::PRIVILEGED, Handler::Host),
    ("_setaddons_folder", cmd_flags::PRIVILEGED, Handler::Host),
    ("exit", 0, Handler::Host),
    ("map", 0, Handler::Host),
    ("career", 0, Handler::Host),
    ("maps", 0, Handler::Host),
    ("restart", 0, Handler::Host),
    ("reload", 0, Handler::Host),
    ("changelevel", 0, Handler::Host),
    ("changelevel2", 0, Handler::Host),
    ("reconnect", 0, Handler::Host),
    ("version", 0, Handler::Host),
    ("say", cmd_flags::FILTERED, Handler::Forward),
    ("say_team", 0, Handler::Forward),
    ("tell", 0, Handler::Forward),
    ("kill", cmd_flags::PRIVILEGED, Handler::Forward),
    ("pause", 0, Handler::Forward),
    ("setpause", 0, Handler::Host),
    ("unpause", 0, Handler::Host),
    ("kick", 0, Handler::Host),
    ("ping", 0, Handler::Forward),
    ("motd", cmd_flags::PRIVILEGED, Handler::Host),
    ("motd_write", cmd_flags::PRIVILEGED, Handler::Host),
    ("stats", 0, Handler::Host),
    ("inputset_menu", 0, Handler::Host),
    ("inputset_game", 0, Handler::Host),
    ("richpresence_override", 0, Handler::Host),
    ("richpresence_nicemap", 0, Handler::Host),
    ("richpresence_update", 0, Handler::Host),
    ("richpresence_gamemode", 0, Handler::Host),
    ("show_osk", 0, Handler::Host),
    ("load", 0, Handler::Host),
    ("save", 0, Handler::Host),
    ("autosave", 0, Handler::Host),
    ("writecfg", cmd_flags::PRIVILEGED, Handler::Host),
    ("+voicerecord", 0, Handler::Host),
    ("-voicerecord", 0, Handler::Host),
    ("startdemos", 0, Handler::Host),
    ("demos", 0, Handler::Host),
    ("stopdemo", 0, Handler::Host),
    ("setinfo", cmd_flags::FILTERED, Handler::Builtin(Builtin::SetInfo)),
    ("fullinfo", 0, Handler::Builtin(Builtin::FullInfo)),
    ("god", 0, Handler::Forward),
    ("notarget", 0, Handler::Forward),
    ("fly", 0, Handler::Forward),
    ("noclip", 0, Handler::Forward),
    ("viewmodel", 0, Handler::Host),
    ("viewframe", 0, Handler::Host),
    ("viewnext", 0, Handler::Host),
    ("viewprev", 0, Handler::Host),
    ("mcache", 0, Handler::Host),
    ("interp", 0, Handler::Host),
    ("setmaster", 0, Handler::Host),
    ("heartbeat", 0, Handler::Host),
    // Cmd_Init.
    ("stuffcmds", 0, Handler::Host),
    ("exec", cmd_flags::PRIVILEGED, Handler::Builtin(Builtin::Exec)),
    ("echo", 0, Handler::Builtin(Builtin::Echo)),
    ("alias", cmd_flags::PRIVILEGED, Handler::Builtin(Builtin::Alias)),
    ("cmd", 0, Handler::Builtin(Builtin::Cmd)),
    ("wait", 0, Handler::Builtin(Builtin::Wait)),
    ("cmdlist", 0, Handler::Host),
    // Cvar_CmdInit.
    ("cvarlist", 0, Handler::Builtin(Builtin::CvarList)),
    // HPAK_Init. Server text cannot edit or extract HPAKs (flag 0 in the original).
    ("hpklist", 0, Handler::Host),
    ("hpkremove", cmd_flags::PRIVILEGED, Handler::Host),
    ("hpkval", 0, Handler::Host),
    ("hpkextract", cmd_flags::PRIVILEGED, Handler::Host),
    // Key_Init.
    ("bind", cmd_flags::PRIVILEGED, Handler::Builtin(Builtin::Bind)),
    ("unbind", cmd_flags::PRIVILEGED, Handler::Builtin(Builtin::Unbind)),
    ("unbindall", cmd_flags::PRIVILEGED, Handler::Builtin(Builtin::UnbindAll)),
    ("escape", 0, Handler::Host),
    // Con_Init.
    ("contimes", 0, Handler::Host),
    ("toggleconsole", 0, Handler::Host),
    ("hideconsole", 0, Handler::Host),
    ("messagemode", 0, Handler::Host),
    ("messagemode2", 0, Handler::Host),
    ("clear", 0, Handler::Host),
    ("condebug", cmd_flags::PRIVILEGED, Handler::Host),
    // NET_Init, DELTA_Init.
    ("maxplayers", cmd_flags::PRIVILEGED, Handler::Host),
    ("delta_stats", 0, Handler::Host),
    ("delta_clear", 0, Handler::Host),
    // SV_Init (server commands; on a client most do nothing or forward). The ones that change
    // state, write files or send logs are privileged here (flag 0 in the original): server text
    // could otherwise overwrite config.cfg (`serverinfo bannedcfgfile config.cfg; writeid`),
    // set privileged cvars through `serverinfo`, or point the logs at any address.
    ("banid", cmd_flags::PRIVILEGED, Handler::Host),
    ("removeid", cmd_flags::PRIVILEGED, Handler::Host),
    ("listid", 0, Handler::Host),
    ("writeid", cmd_flags::PRIVILEGED, Handler::Host),
    ("resetrcon", 0, Handler::Host),
    ("logaddress", cmd_flags::PRIVILEGED, Handler::Host),
    ("logaddress_add", cmd_flags::PRIVILEGED, Handler::Host),
    ("logaddress_del", cmd_flags::PRIVILEGED, Handler::Host),
    ("log", cmd_flags::PRIVILEGED, Handler::Host),
    ("serverinfo", cmd_flags::PRIVILEGED, Handler::Host),
    ("localinfo", cmd_flags::PRIVILEGED, Handler::Host),
    ("showinfo", 0, Handler::Forward),
    ("user", 0, Handler::Host),
    ("users", 0, Handler::Host),
    ("dlfile", 0, Handler::Host),
    ("addip", cmd_flags::PRIVILEGED, Handler::Host),
    ("removeip", cmd_flags::PRIVILEGED, Handler::Host),
    ("listip", 0, Handler::Host),
    ("writeip", cmd_flags::PRIVILEGED, Handler::Host),
    ("dropclient", 0, Handler::Forward),
    ("spawn", 0, Handler::Host),
    ("new", 0, Handler::Host),
    ("sendres", 0, Handler::Host),
    ("sendents", 0, Handler::Forward),
    ("fullupdate", 0, Handler::Forward),
    // Memory_Init, Draw_Init, R_Init, SCR_Init.
    ("flush", 0, Handler::Host),
    ("gl_texels", 0, Handler::Host),
    ("timerefresh", cmd_flags::FILTERED, Handler::Host),
    ("envmap", 0, Handler::Host),
    ("pointfile", 0, Handler::Host),
    ("gl_dump", 0, Handler::Host),
    ("gl_reloadshaders", 0, Handler::Host),
    ("screenshot", 0, Handler::Host),
    ("sizeup", 0, Handler::Host),
    ("sizedown", 0, Handler::Host),
];

/// Files `exec` skips silently when missing (substring match).
const SILENT_EXECS: [&str; 5] = ["autoexec.cfg", "userconfig.cfg", "hw/opengl.cfg", "joystick.cfg", "game.cfg"];

/// Alias names refused in cstrike/czero besides cvar names (`SetCStrikeFlags`).
const CSTRIKE_ALIAS_DENY: [&str; 5] = ["cl_autobuy", "cl_rebuy", "gl_ztrick", "gl_ztrick_old", "gl_d3dflip"];

pub struct Console {
    pub cvars: Cvars,
    pub bindings: Bindings,
    commands: Vec<Command>,
    /// `cmd_alias`, newest first.
    aliases: Vec<(String, String)>,
    /// `cmd_text` (privileged).
    text: Cbuf,
    /// `filteredcmd_text` (server stufftext).
    filtered: Cbuf,
    /// `cmd_wait`.
    wait: bool,
    /// `cmd_privileged` while a handler runs.
    privileged: bool,
    /// The game is cstrike/czero (alias name blacklist).
    cstrike: bool,
    /// Cvars whose value changed since [`Console::take_changed`].
    changed: Vec<String>,
    /// `keydown[K_SHIFT]` as `cle_Key_Event` tracks it.
    shift_down: bool,
}

impl Console {
    /// The engine commands plus `cl_filterstuffcmd` and `developer`.
    pub fn new(cstrike: bool) -> Self {
        let mut console = Self {
            cvars: Cvars::default(),
            bindings: Bindings::default(),
            commands: Vec::new(),
            aliases: Vec::new(),
            text: Cbuf::default(),
            filtered: Cbuf::default(),
            wait: false,
            privileged: true,
            cstrike,
            changed: Vec::new(),
            shift_down: false,
        };
        console.cvars.register("cl_filterstuffcmd", "0", cvar_flags::ARCHIVE | cvar_flags::PRIVILEGED);
        console.cvars.register("developer", "0", cvar_flags::FILTERSTUFFTEXT);
        for &(name, flags, handler) in ENGINE_COMMANDS {
            console.add_command(name, flags, handler, false);
        }
        console
    }

    /// Command names in list order (GameUI's autocomplete walks the engine list).
    pub fn command_names(&self) -> impl Iterator<Item = &str> {
        self.commands.iter().map(|c| c.name.as_str())
    }

    /// `Cmd_AddCommandWithFlags` (sorted by `stricmp`) for a host-handled command, or
    /// `Cmd_AddHUDCommand` (list head) when `flags` has [`cmd_flags::HUD`].
    pub fn register_command(&mut self, name: &str, flags: u32) -> bool {
        self.add_command(name, flags, Handler::Host, flags & cmd_flags::HUD != 0)
    }

    /// `Cmd_AddWrapperCommand`: a module command (flag 4) at the list head.
    pub fn register_wrapper_command(&mut self, name: &str) -> bool {
        self.add_command(name, cmd_flags::WRAPPER, Handler::Host, true)
    }

    /// The registrars' common part: names of cvars with a non-empty value
    /// (`Cvar_VariableString(name)[0]`) and exact duplicates are refused; a head insert goes
    /// first, a sorted one before the first entry `stricmp` puts after it.
    fn add_command(&mut self, name: &str, flags: u32, handler: Handler, head: bool) -> bool {
        if !self.cvars.string(name).is_empty() {
            log::warn!("Cmd_AddCommand: {name} already defined as a var");
            return false;
        }
        if self.commands.iter().any(|c| c.name == name) {
            log::warn!("Cmd_AddCommand: {name} already defined");
            return false;
        }
        let at = if head {
            0
        } else {
            let key = name.to_ascii_lowercase();
            self.commands.iter().position(|c| c.name.to_ascii_lowercase() > key).unwrap_or(self.commands.len())
        };
        self.commands.insert(at, Command { name: name.to_owned(), flags, handler });
        true
    }

    /// `Cvar_RegisterVariable`, refusing command names.
    pub fn register_cvar(&mut self, name: &str, default: &str, flags: u32) -> bool {
        if self.find_command(name).is_some() {
            log::warn!("Cvar_RegisterVariable: {name} is a command");
            return false;
        }
        self.cvars.register(name, default, flags)
    }

    fn find_command(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// `Cmd_IsPrivileged`: whether the running handler came from the privileged buffer.
    pub fn is_privileged(&self) -> bool {
        self.privileged
    }

    /// `Cbuf_AddText` into the privileged buffer.
    pub fn add_text(&mut self, text: &str) {
        if !self.text.add(text) {
            log::warn!("Cbuf_AddTextToBuffer: overflow");
        }
    }

    /// `Cbuf_InsertText` into the privileged buffer.
    pub fn insert_text(&mut self, text: &str) {
        if !self.text.insert(text) {
            log::warn!("Cbuf_InsertText: overflow");
        }
    }

    /// `Cbuf_AddFilteredText`: into the filtered buffer (text that must not run privileged).
    pub fn add_filtered_text(&mut self, text: &str) {
        if !self.filtered.add(text) {
            log::warn!("Cbuf_AddTextToBuffer: overflow");
        }
    }

    /// `cle_Key_Event`'s binding dispatch for one key transition; with SHIFT held a binding
    /// starting with `togg` runs only with `con_shifttoggleconsole` on.
    pub fn key_event(&mut self, key: u8, down: bool, repeat: bool) {
        if repeat && !keys::repeats(key) {
            return;
        }
        if key == keys::K_SHIFT {
            self.shift_down = down;
        }
        let Some(text) = self.bindings.event_text(key, down, repeat) else { return };
        let toggle = !text.starts_with('+') && text.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("togg"));
        if down && toggle && self.shift_down && self.cvars.value("con_shifttoggleconsole") == 0.0 {
            return;
        }
        self.add_text(&text);
    }

    /// Names of cvars changed since the last call (the `Cvar_HookVariable` callbacks).
    pub fn take_changed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.changed)
    }

    /// `Parse_svc_stufftext`: `allow_autoaim`/`allow_shaders` set their cvars directly; other
    /// text must pass `ValidStuffText` and then goes, as is, to the filtered buffer.
    pub fn stuff_text(&mut self, host: &mut dyn ConsoleHost, text: &str, mut policy: StuffPolicy) {
        let args = token::tokenize(text);
        for (command, var) in [("allow_autoaim", "sv_allow_autoaim"), ("allow_shaders", "sv_allow_shaders")] {
            if args.arg(0).eq_ignore_ascii_case(command) {
                let on = if args.arg(1).starts_with('0') { "0" } else { "1" };
                self.direct_set(host, var, on);
                return;
            }
        }
        policy.filter = self.cvars.value("cl_filterstuffcmd") > 0.0;
        if valid_stuff_text(text, policy) {
            if !self.filtered.add(text) {
                log::warn!("Cbuf_AddTextToBuffer: overflow");
            }
        } else {
            host.print(&format!("Server tried to send invalid command:\"{text}\"\n"));
        }
    }

    /// `Cbuf_Execute`: the privileged buffer until it is empty or a `wait`, then one line of
    /// the filtered buffer.
    pub fn execute(&mut self, host: &mut dyn ConsoleHost) {
        while let Some(line) = self.text.next_line() {
            self.execute_string(host, &line, true);
            if std::mem::take(&mut self.wait) {
                break;
            }
        }
        if let Some(line) = self.filtered.next_line() {
            self.execute_string(host, &line, false);
            self.wait = false;
        }
    }

    /// `Cmd_ExecuteStringWithPrivilegeCheck`: command, then alias, then cvar, then forwarding
    /// to the server (dropped silently while disconnected).
    pub fn execute_string(&mut self, host: &mut dyn ConsoleHost, text: &str, privileged: bool) {
        let args = token::tokenize(text);
        if args.argc() == 0 {
            return;
        }
        let name = args.arg(0);
        let filter = self.cvars.value("cl_filterstuffcmd") > 0.0;
        // Server text passes over a hidden command as over an unknown name.
        if let Some(cmd) = self.find_command(name).filter(|c| privileged || c.flags & cmd_flags::HIDDEN == 0).cloned() {
            if !privileged
                && (cmd.flags & cmd_flags::PRIVILEGED != 0
                    || (filter && (cmd.flags & cmd_flags::FILTERED != 0 || has_filtered_prefix(name))))
            {
                host.print(&format!("Could not execute privileged command {text}\n"));
                return;
            }
            self.privileged = privileged;
            match cmd.handler {
                Handler::Builtin(b) => self.builtin(host, b, &args),
                Handler::Forward => forward_to_server(host, &args),
                Handler::Host => {
                    host.command(self, &args);
                    if cmd.flags & cmd_flags::HUD != 0 {
                        host.hud_command_ran(&cmd.name);
                    }
                }
            }
            self.privileged = true;
            return;
        }
        if let Some((_, value)) = self.aliases.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            let value = value.clone();
            let (buf, label) = if privileged { (&mut self.text, "Cbuf_InsertText") } else { (&mut self.filtered, "Cbuf_InsertTextToBuffer") };
            if !buf.insert(&value) {
                log::warn!("{label}: overflow");
            }
            return;
        }
        if self.cvar_command(host, &args, privileged, filter) {
            return;
        }
        if host.connected() {
            forward_to_server(host, &args);
        }
    }

    /// `Cvar_Command`: `name` prints, `name value` sets (only argv1 is used).
    fn cvar_command(&mut self, host: &mut dyn ConsoleHost, args: &Args, privileged: bool, filter: bool) -> bool {
        let name = args.arg(0);
        let sub: Vec<Vec<u8>> =
            std::iter::successors(com_parse(&gs_common::bytestr::encode(name), false), |(_, rest)| com_parse(rest, false))
                .map(|(t, _)| t)
                .take(2)
                .collect();
        if sub.len() > 1 {
            let first = gs_common::bytestr::decode(&sub[0]);
            // Server text passes over a hidden cvar as over an unknown name.
            let Some(var) = self.cvars.find(&first).filter(|v| privileged || v.flags & cvar_flags::HIDDEN == 0) else { return false };
            host.print(&format!("\"{}\" is \"{}\"\n", var.name, var.string));
            return true;
        }
        let Some(var) = self.cvars.find(name).filter(|v| privileged || v.flags & cvar_flags::HIDDEN == 0).cloned() else { return false };
        if !privileged
            && (var.flags & cvar_flags::PRIVILEGED != 0
                || (filter && (var.flags & cvar_flags::FILTERSTUFFTEXT != 0 || has_filtered_prefix(&var.name))))
        {
            host.print(&format!("{} is a privileged variable\n", var.name));
            return true;
        }
        if args.argc() == 1 {
            host.print(&format!("\"{}\" is \"{}\"\n", var.name, var.string));
            return true;
        }
        if var.flags & cvar_flags::SPONLY != 0 && host.multiplayer() {
            host.print(&format!("Can't set {} in multiplayer\n", var.name));
            return true;
        }
        self.direct_set(host, &var.name, args.arg(1));
        true
    }

    /// `Cvar_Set` / `Cvar_DirectSet` with their side effects: USERINFO cvars are mirrored into
    /// `cls.userinfo` and, when changed while connected, sent as `setinfo`.
    pub fn direct_set(&mut self, host: &mut dyn ConsoleHost, name: &str, value: &str) {
        match self.cvars.direct_set(name, value) {
            SetResult::Missing => log::debug!("Cvar_Set: variable {name} not found"),
            SetResult::Refused => host.print(&format!("Refusing to set {name} to {value} (invalid path)\n")),
            SetResult::Set { changed, value, flags } => {
                let name = self.cvars.find(name).map_or_else(|| name.to_owned(), |v| v.name.clone());
                if flags & cvar_flags::USERINFO != 0 {
                    if let Err(e) = host.userinfo().set(&name, &value) {
                        host.print(&format!("{e}\n"));
                    }
                    if changed && host.connected() {
                        host.send_string_cmd(&format!("setinfo \"{name}\" \"{value}\"\n"));
                    }
                }
                if changed {
                    self.changed.push(name);
                }
            }
        }
    }

    fn builtin(&mut self, host: &mut dyn ConsoleHost, which: Builtin, args: &Args) {
        match which {
            Builtin::Alias => self.alias(host, args),
            Builtin::Echo => {
                let mut line: String = (1..args.argc()).map(|i| format!("{} ", args.arg(i))).collect();
                line.push('\n');
                host.print(&line);
            }
            Builtin::Wait => self.wait = true,
            Builtin::Exec => self.exec(host, args),
            Builtin::Cmd => forward_to_server(host, args),
            Builtin::CvarList => self.cvarlist(host, args),
            Builtin::Bind => self.bind(host, args),
            Builtin::Unbind => {
                if args.argc() != 2 {
                    host.print("unbind <key> : remove commands from a key\n");
                    return;
                }
                match keys::key_number(args.arg(1)) {
                    Some(k) => self.bindings.set(k, ""),
                    None => host.print(&format!("\"{}\" isn't a valid key\n", args.arg(1))),
                }
            }
            Builtin::UnbindAll => self.bindings.clear_all(),
            Builtin::SetInfo => match args.argc() {
                1 => {
                    let lines: String = host.userinfo().pairs().map(|(k, v)| format!("{k:<20}{v}\n")).collect();
                    host.print(&lines);
                }
                3 => {
                    if let Err(e) = host.userinfo().set(args.arg(1), args.arg(2)) {
                        host.print(&format!("{e}\n"));
                    }
                    forward_to_server(host, args);
                }
                _ => host.print("usage: setinfo [ <key> <value> ]\n"),
            },
            Builtin::FullInfo => {
                // Applies the first pair only, then forwards the whole line (original bug).
                let mut parts = args.arg(1).split('\\').skip(1);
                if let Some(key) = parts.next() {
                    match parts.next() {
                        Some(value) => {
                            if let Err(e) = host.userinfo().set(key, value) {
                                host.print(&format!("{e}\n"));
                            }
                        }
                        None => host.print("MISSING VALUE\n"),
                    }
                }
                forward_to_server(host, args);
            }
        }
    }

    /// `cmd_alias`.
    fn alias(&mut self, host: &mut dyn ConsoleHost, args: &Args) {
        if args.argc() == 1 {
            let mut out = String::from("Current alias commands:\n");
            for (name, value) in &self.aliases {
                out.push_str(&format!("{name} : {value}\n"));
            }
            host.print(&out);
            return;
        }
        let name = args.arg(1);
        if gs_common::bytestr::encoded_len(name) >= 32 {
            host.print("Alias name is too long\n");
            return;
        }
        let denied = self.cvars.find(name).is_some()
            || name.eq_ignore_ascii_case("_special")
            || name.eq_ignore_ascii_case("special")
            || (self.cstrike && CSTRIKE_ALIAS_DENY.iter().any(|d| d.eq_ignore_ascii_case(name)));
        if denied {
            host.print("Alias name is invalid\n");
            return;
        }
        let mut value = String::new();
        let mut used = 0;
        for i in 2..args.argc() {
            let piece = format!("{} ", args.arg(i));
            let n = gs_common::bytestr::encoded_len(&piece);
            if used + n > 1022 {
                break;
            }
            used += n;
            value.push_str(&piece);
        }
        value.push('\n');
        match self.aliases.iter_mut().find(|(n, _)| n == name) {
            Some((_, old)) => *old = value,
            None => self.aliases.insert(0, (name.to_owned(), value)),
        }
    }

    /// `cmd_exec`: the file's text is inserted in front of the privileged buffer (wrapped in
    /// newlines) and so runs privileged.
    fn exec(&mut self, host: &mut dyn ConsoleHost, args: &Args) {
        if args.argc() != 2 {
            host.print("exec <filename> : execute a script file\n");
            return;
        }
        let name = args.arg(1);
        if name.contains(['\\', ':', '~']) || name.contains("..") || name.starts_with('/') {
            host.print(&format!("exec {name}: invalid path.\n"));
            return;
        }
        if name.matches('.').count() > 1 {
            host.print(&format!("exec {name}: invalid filename.\n"));
            return;
        }
        if !matches!(name.rsplit_once('.').map(|(_, ext)| ext), Some("cfg" | "rc")) {
            host.print(&format!("exec {name}: not a .cfg or .rc file\n"));
            return;
        }
        let Some(data) = host.read_config(name) else {
            if !SILENT_EXECS.iter().any(|s| name.contains(s)) {
                host.print(&format!("couldn't exec {name}\n"));
            }
            return;
        };
        log::debug!("execing {name}");
        let text = gs_common::bytestr::decode(&data);
        let text = text.split('\0').next().unwrap_or_default();
        if text.len() + self.text.len() + 2 < CBUF_SIZE {
            self.insert_text(&format!("\n{text}\n"));
            return;
        }
        // Too big for the buffer: run what is queued, then the file line by line.
        self.execute(host);
        for line in text.split(|c: char| (c as u32) < 32).filter(|l| !l.is_empty()) {
            self.insert_text(&format!("{line}\n"));
            self.execute(host);
        }
    }

    /// `cvarlist [-a | -s | <prefix>]`.
    fn cvarlist(&mut self, host: &mut dyn ConsoleHost, args: &Args) {
        let mode = args.arg(1);
        let mut out = String::new();
        let mut count = 0;
        for var in self.cvars.iter() {
            let show = match mode {
                "" => true,
                "-a" => var.flags & cvar_flags::ARCHIVE != 0,
                "-s" => var.flags & cvar_flags::SERVER != 0,
                prefix => var.name.get(..prefix.len()).is_some_and(|p| p.eq_ignore_ascii_case(prefix)),
            };
            if !show {
                continue;
            }
            count += 1;
            if (var.value - var.value.trunc()).abs() < f32::EPSILON {
                out.push_str(&format!("{:<15} : {:>8}", var.name, var.value as i32));
            } else {
                out.push_str(&format!("{:<15} : {:>8.3}", var.name, var.value));
            }
            for (flag, suffix) in [(cvar_flags::ARCHIVE, ", a"), (cvar_flags::SERVER, ", sv"), (cvar_flags::USERINFO, ", i")] {
                if var.flags & flag != 0 {
                    out.push_str(suffix);
                }
            }
            out.push('\n');
        }
        if mode.is_empty() {
            out.push_str(&format!("--------------\n{count:3} Total CVars\nCvarList ? for syntax\n"));
        } else {
            out.push_str(&format!("--------------\n{count:3} CVars for [{mode}]\nCvarList ? for syntax\n"));
        }
        host.print(&out);
    }

    /// `Key_Bind_f`.
    fn bind(&mut self, host: &mut dyn ConsoleHost, args: &Args) {
        if !(2..=3).contains(&args.argc()) {
            host.print("bind <key> [command] : attach a command to a key\n");
            return;
        }
        let Some(key) = keys::key_number(args.arg(1)) else {
            host.print(&format!("\"{}\" isn't a valid key\n", args.arg(1)));
            return;
        };
        if args.argc() == 2 {
            let binding = self.bindings.get(key);
            host.print(&if binding.is_empty() {
                format!("\"{}\" is not bound\n", args.arg(1))
            } else {
                format!("\"{}\" = \"{binding}\"\n", args.arg(1))
            });
            return;
        }
        let command = gs_common::bytestr::truncate_encoded(args.arg(2), 1023);
        self.bindings.set(key, &command);
    }

    /// `Host_WriteConfiguration`'s `config.cfg` text; `None` with fewer than two bound keys
    /// ("skipping config.cfg output, no keys bound").
    pub fn config_text(&self, userinfo: &InfoString) -> Option<String> {
        if self.bindings.count() < 2 {
            return None;
        }
        let mut out = String::from(
            "// This file is overwritten whenever you change your user settings in the game.\n\
             // Add custom configurations to the file \"userconfig.cfg\".\n\nunbindall\n",
        );
        self.bindings.write(&mut out);
        self.write_archived(&mut out);
        for (key, value) in userinfo.pairs() {
            if self.cvars.find(key).is_none() && !key.starts_with('*') {
                out.push_str(&format!("setinfo \"{key}\" \"{value}\"\n"));
            }
        }
        out.push_str("+mlook\n+jlook\nexec controller.cfg\nexec userconfig.cfg\n");
        Some(out)
    }

    /// The archived cvars as `name "value"` lines. The original writes `%s "%s"` as is: a value
    /// a server set with a `"` or a line break in it would close the quotes and run as commands
    /// at the next start, with a local config's privileges. Those characters are left out.
    /// docs/deviations.md "Security".
    fn write_archived(&self, out: &mut String) {
        for var in self.cvars.iter().filter(|v| v.flags & cvar_flags::ARCHIVE != 0) {
            let value: String = var.string.chars().filter(|c| !matches!(c, '"' | '\r' | '\n')).collect();
            out.push_str(&format!("{} \"{value}\"\n", var.name));
        }
    }

    /// `cmd_writecfg`'s file: bindings, archived cvars, user-only userinfo keys, then
    /// `+mlook`/`+jlook` only when those buttons are held.
    pub fn writecfg_text(&self, userinfo: &InfoString, mlook: bool, jlook: bool) -> String {
        let mut out = String::from("unbindall\n");
        self.bindings.write(&mut out);
        self.write_archived(&mut out);
        for (key, value) in userinfo.pairs() {
            if self.cvars.find(key).is_none() && !key.starts_with('*') {
                out.push_str(&format!("setinfo \"{key}\" \"{value}\"\n"));
            }
        }
        if mlook {
            out.push_str("+mlook\n");
        }
        if jlook {
            out.push_str("+jlook\n");
        }
        out
    }
}

fn has_filtered_prefix(name: &str) -> bool {
    FILTERED_PREFIXES.iter().any(|p| name.get(..p.len()).is_some_and(|s| s.eq_ignore_ascii_case(p)))
}

/// `Cmd_ForwardToServer`: `argv0 + " " + Cmd_Args()` (or `"\n"` without arguments) as a
/// reliable string command; `cmd` forwards only its arguments, and `cmd dlfile` never.
pub fn forward_to_server(host: &mut dyn ConsoleHost, args: &Args) {
    let Some(text) = forward_text(args) else { return };
    if !host.connected() {
        let argv0 = args.arg(0);
        if !argv0.eq_ignore_ascii_case("setinfo") {
            host.print(&format!("Can't \"{argv0}\", not connected\n"));
        }
        return;
    }
    host.forward_string_cmd(&text);
}

/// `Cmd_ForwardToServerInternal`'s text: `argv0 + " "` (not for `cmd`), then `Cmd_Args`, or
/// `"\n"` without arguments; `None` for `cmd dlfile`, which `Cmd_ForwardToServer` never sends.
pub fn forward_text(args: &Args) -> Option<String> {
    let argv0 = args.arg(0);
    let is_cmd = argv0.eq_ignore_ascii_case("cmd");
    if is_cmd && args.arg(1).eq_ignore_ascii_case("dlfile") {
        return None;
    }
    let mut text = if is_cmd { String::new() } else { format!("{argv0} ") };
    match &args.args {
        Some(rest) => text.push_str(rest),
        None => text.push('\n'),
    }
    Some(text)
}

#[cfg(test)]
mod tests;
