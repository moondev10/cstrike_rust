//! hw.dll's DemoPlayer host (docs/re/demoplayer-engine.md): `DemoPlayer_Load` with the Rust
//! modules in place of `LoadLibrary`, the module file system, `Printf`/`DPrintf` into the console
//! and `SystemWrapper::ExecuteString` (the `dem_*` command forwarder).

use std::cell::RefCell;
use std::rc::Rc;

use gs_fs::{FileSystem, PathId};
use gs_hltv::player::DemoPlayer;
use gs_hltv::{Files, LogLine, Sys};

use crate::notify;

/// `IBaseSystem::GetFileSystem`: the engine's file system (no path id; writes go to the game
/// directory's writable path).
struct ModuleFiles(FileSystem);

impl Files for ModuleFiles {
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        self.0.read(name).ok()
    }

    fn write(&self, name: &str, data: &[u8]) -> bool {
        self.0.write(name, PathId::GameConfig, data).map_err(|e| log::warn!("{name}: {e}")).is_ok()
    }
}

/// `DemoPlayer_Load` (§3.1): `GetModule("demoplayer001", "demoplayer")`, whose Init loads
/// `core.dll` twice for the server and world, then `SetEditMode(-demoedit)`. The developer lines
/// of the library loads come before the module's own.
pub fn load(fs: &FileSystem, edit_mode: bool) -> Rc<RefCell<DemoPlayer>> {
    let sys = Sys::new(Box::new(ModuleFiles(fs.clone())));
    sys.dprint("Loaded library demoplayer.dll.\n");
    sys.dprint("Loaded library core.dll.\n");
    sys.dprint("Loaded library core.dll.\n");
    let mut player = DemoPlayer::new(sys);
    player.core.edit_mode = edit_mode;
    Rc::new(RefCell::new(player))
}

/// The modules' `Printf` (`Con_Printf("%s")`) and `DPrintf` (`Con_DPrintf`, developer only)
/// since the last call.
pub fn drain_log(sys: &Sys, developer: bool) {
    for line in sys.take_log() {
        match line {
            LogLine::Print(text) => notify::con_printf(&text),
            LogLine::DPrint(text) => {
                if developer {
                    notify::con_dprintf(&text);
                }
            }
        }
    }
}

/// `SystemWrapper_CommandForwarder`: `Cmd_Argv(0)`, then `" " + Cmd_Args()` with more than one
/// argument, cut to 1023 bytes.
pub fn forwarded_line(argv0: &str, args: Option<&str>, argc: usize) -> Vec<u8> {
    let mut line = gs_common::bytestr::encode(argv0);
    if argc > 1 {
        line.push(b' ');
        line.extend_from_slice(&gs_common::bytestr::encode(args.unwrap_or("")));
    }
    line.truncate(1023);
    line
}

/// One command `SystemWrapper::ExecuteString` dispatches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exec {
    /// `module->ExecuteCommand(id, token)`: the registered name is a prefix of the token.
    Module { id: i32, line: String },
    /// `Cmd_ExecuteString(token, src_command)`.
    Engine(String),
    /// `"WARNING! System::ExecuteString: Command token too long.\n"`; nothing after it runs.
    TooLong,
}

/// `SystemWrapper::ExecuteString` (§4.7): `%`, control bytes and bytes ≥ 0x80 become spaces;
/// tokens split at `;` outside quotes (quotes stay); leading spaces dropped; empty tokens skipped;
/// a token reaching 255 characters ends the string with the warning.
pub fn execute_string(commands: &[u8], registered: &[&str]) -> Vec<Exec> {
    let mut out = Vec::new();
    if commands.is_empty() || commands[0] == 0 {
        return out;
    }
    let text: Vec<u8> = commands.iter().map(|&c| if c == b'%' || (c as i8) < 0x20 && c != 0 { b' ' } else { c }).collect();
    let at = |i: usize| text.get(i).copied().unwrap_or(0);
    let mut in_quote = false;
    let mut p = 0usize;
    let mut tok = vec![at(0)];
    loop {
        let c = tok.last().copied().unwrap_or(0);
        if c == b'"' {
            in_quote = !in_quote;
        } else if (c == b';' && !in_quote) || c == 0 {
            tok.pop();
            let start = tok.iter().position(|&b| b != b' ').unwrap_or(tok.len());
            let s = &tok[start..];
            if !s.is_empty() {
                let line = String::from_utf8_lossy(s).into_owned();
                let module = registered.iter().zip(1..).find(|(name, _)| s.len() >= name.len() && s[..name.len()].eq_ignore_ascii_case(name.as_bytes()));
                out.push(match module {
                    Some((_, id)) => Exec::Module { id, line },
                    None => Exec::Engine(line),
                });
            }
            if at(p) == 0 {
                return out;
            }
            p += 1;
            tok.clear();
            tok.push(at(p));
            continue;
        }
        if at(p) == 0 {
            return out;
        }
        p += 1;
        tok.push(at(p));
        if tok.len() >= 256 {
            out.push(Exec::TooLong);
            return out;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMDS: [&str; 6] = DemoPlayer::COMMANDS;

    #[test]
    fn prefix_match_and_split() {
        let r = execute_string(b"dem_speedx 2;  echo hi;;dem_jump 10", &CMDS);
        assert_eq!(
            r,
            vec![
                Exec::Module { id: 4, line: "dem_speedx 2".into() },
                Exec::Engine("echo hi".into()),
                Exec::Module { id: 1, line: "dem_jump 10".into() },
            ]
        );
    }

    #[test]
    fn quoted_semicolon_and_control_bytes() {
        let r = execute_string(b"DEM_SAVE \"a;b\"\t%x", &CMDS);
        assert_eq!(r, vec![Exec::Module { id: 6, line: "DEM_SAVE \"a;b\"  x".into() }]);
    }

    #[test]
    fn token_length_limit() {
        let ok = format!("echo {};x", "a".repeat(249));
        assert_eq!(execute_string(ok.as_bytes(), &CMDS)[0], Exec::Engine(format!("echo {}", "a".repeat(249))));
        let long = format!("x;echo {}", "a".repeat(250));
        assert_eq!(execute_string(long.as_bytes(), &CMDS), vec![Exec::Engine("x".into()), Exec::TooLong]);
    }
}
