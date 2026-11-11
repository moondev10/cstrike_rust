//! The launcher's command line (hl.exe `CCommandLine`) and the engine's `com_argv`
//! (docs/re/engine-video.md §8.3, §2): the engine arguments are the text after `--` on the
//! process command line; `_restart` rebuilds them for the next engine run in this process.

use std::sync::{Mutex, MutexGuard, PoisonError};

struct State {
    /// `CCommandLine`'s string (without the executable).
    line: String,
    /// `szNewCommandParams` (`g_pPostRestartCmdLineArgs`): appended to the line by the restart.
    post_restart: String,
}

static STATE: Mutex<State> = Mutex::new(State { line: String::new(), post_restart: String::new() });

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The process arguments after `--` become the launcher's command line, led by `-game <game>`
/// (the launcher's mod, as Steam starts `hl.exe -game cstrike`) unless they name one.
pub fn init_from_process(game: &str) {
    let args: Vec<String> = std::env::args().collect();
    let engine = args.iter().position(|a| a == "--").map_or(&[][..], |i| &args[i + 1..]);
    let mut line = engine.join(" ");
    if !engine.iter().any(|a| a == "-game") {
        line = if line.is_empty() { format!("-game {game}") } else { format!("-game {game} {line}") };
    }
    state().line = line;
}

/// `CommandLine()->GetCmdLine()` (GameUI's `strstr` checks).
pub fn line() -> String {
    state().line.clone()
}

/// `Sys_InitArgv`: `com_argv[0]` is the program, then the line split on bytes ≤ 0x20 or 0x7F (no
/// quoting), at most 50 arguments in all.
pub fn argv() -> Vec<String> {
    let line = state().line.clone();
    let mut out = vec!["cstrike-game".to_owned()];
    out.extend(line.split(|c: char| c <= ' ' || c == '\x7f').filter(|s| !s.is_empty()).map(str::to_owned));
    out.truncate(50);
    out
}

/// `COM_CheckParm`: the first index in `com_argv[1..]` exactly (case-sensitively) equal to
/// `parm`, or 0.
pub fn check_parm_index(parm: &str) -> usize {
    argv().iter().skip(1).position(|a| a == parm).map_or(0, |i| i + 1)
}

pub fn check_parm(parm: &str) -> bool {
    check_parm_index(parm) != 0
}

/// `com_argv[CheckParm(parm) + 1]` (`" "` past the end, the original's `argvdummy`); `None`
/// when `parm` is absent.
pub fn parm_value(parm: &str) -> Option<String> {
    let i = check_parm_index(parm);
    (i != 0).then(|| argv().get(i + 1).cloned().unwrap_or_else(|| " ".to_owned()))
}

/// `CCommandLine::RemoveParm`: every occurrence of `p` (a substring match) together with its
/// values, up to the next `-` or `+` anywhere after its first byte; trailing spaces stripped.
fn remove_parm(line: &mut String, p: &str) {
    if line.is_empty() || p.is_empty() {
        return;
    }
    while let Some(q) = line.find(p) {
        match line[q + 1..].find(['-', '+']) {
            Some(e) => {
                line.replace_range(q..q + 1 + e, "");
            }
            None => line.truncate(q),
        }
        let trimmed = line.trim_end_matches(' ').len();
        line.truncate(trimmed);
    }
}

/// `CCommandLine::AppendParm(p, v)`: `RemoveParm(p)`, then ` p[ v]` appended.
fn append_parm(line: &mut String, p: &str, v: Option<&str>) {
    remove_parm(line, p);
    if !line.is_empty() {
        line.push(' ');
    }
    line.push_str(p);
    if let Some(v) = v {
        line.push(' ');
        line.push_str(v);
    }
}

/// `_setgamedir`/`_restart`: text for the next run's command line.
pub fn append_post_restart(text: &str) {
    state().post_restart.push_str(text);
}

/// The launcher between two engine runs (hl.exe 0x401679): the overrides removed,
/// `-novid 0`, `-game`/`+load` replaced when the post-restart text brings new ones, then that
/// text appended; the buffer is cleared for the next run.
pub fn rebuild_for_restart() {
    let mut st = state();
    let post = std::mem::take(&mut st.post_restart);
    st.line = rebuilt(&st.line, &post);
}

fn rebuilt(line: &str, post: &str) -> String {
    let mut line = line.to_owned();
    for p in ["-sw", "-startwindowed", "-windowed", "-window", "-full", "-fullscreen", "-soft", "-software", "-gl", "-d3d", "-w", "-width", "-h", "-height", "+connect"] {
        remove_parm(&mut line, p);
    }
    // SetParm("-novid", 0) = RemoveParm + AppendParm("-novid", "0").
    remove_parm(&mut line, "-novid");
    append_parm(&mut line, "-novid", Some("0"));
    if post.contains("-game") {
        remove_parm(&mut line, "-game");
    }
    if post.contains("+load") {
        remove_parm(&mut line, "+load");
    }
    append_parm(&mut line, post, None);
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The launcher's example (engine-video.md §8.3): the overrides and the old `+connect` go,
    /// `-novid 0` and the new connect come in, with the original's double space.
    #[test]
    fn restart_line_drops_overrides_and_appends_the_new_args() {
        let line = rebuilt("-game cstrike -windowed -w 800 -h 600 +connect 1.2.3.4:27015", " +connect 1.2.3.4:27015");
        assert_eq!(line, "-game cstrike -novid 0  +connect 1.2.3.4:27015");
    }

    /// A new `-game` from `_setgamedir` replaces the old one.
    #[test]
    fn restart_line_replaces_the_game() {
        assert_eq!(rebuilt("-game cstrike -dev", " -game valve"), "-dev -novid 0  -game valve");
    }

    /// `RemoveParm` matches substrings: `-h` also takes `-heapsize` and its value.
    #[test]
    fn remove_parm_matches_prefixes() {
        let mut line = "-heapsize 65536 -dev".to_owned();
        remove_parm(&mut line, "-h");
        assert_eq!(line, "-dev");
    }
}
