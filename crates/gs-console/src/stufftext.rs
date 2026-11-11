//! `ValidStuffText` (docs/re/engine-core.md §2.8): the deny-list a server's `svc_stufftext` (and
//! director stufftext) must pass before it reaches the filtered command buffer. One violation
//! rejects the whole message.

use gs_common::token;

/// What the validator depends on besides the text.
#[derive(Debug, Clone, Copy, Default)]
pub struct StuffPolicy {
    /// `Host_IsSinglePlayerGame() && !cls.demoplayback`: everything passes.
    pub single_player: bool,
    /// `cl_filterstuffcmd > 0`.
    pub filter: bool,
    /// The game directory is `tfc` (`exec` is then allowed).
    pub tfc: bool,
}

/// Byte offsets of every case-insensitive occurrence of `needle` (`Q_stristr`).
fn find_all_ci<'a>(hay: &'a [u8], needle: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    (0..=hay.len().saturating_sub(needle.len()))
        .filter(move |&i| hay.len() >= needle.len() && hay[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

fn contains_ci(hay: &[u8], needle: &str) -> bool {
    find_all_ci(hay, needle.as_bytes()).next().is_some()
}

/// The first occurrence of `needle` sits at a command boundary: the start, or after one of
/// `before`.
fn first_at_boundary(hay: &[u8], needle: &str, before: &[u8]) -> bool {
    find_all_ci(hay, needle.as_bytes()).next().is_some_and(|i| i == 0 || before.contains(&hay[i - 1]))
}

/// `true` when the server text may be executed.
pub fn valid_stuff_text(text: &str, policy: StuffPolicy) -> bool {
    if policy.single_player {
        return true;
    }
    let s = text.as_bytes();
    let argv0 = token::tokenize(text).arg(0).to_owned();
    let a0 = argv0.as_bytes();
    if a0.len() >= 7 && a0[..7].eq_ignore_ascii_case(b"connect") {
        return false;
    }
    const ARGV0_DENY: [&str; 13] = [
        "bind",
        "_set",
        "unbind",
        "retry",
        "quit",
        "_restart",
        "motd_write",
        "motdfile",
        "kill",
        "exit",
        "writecfg",
        "cl_filterstuffcmd",
        "unbindall",
    ];
    if ARGV0_DENY.iter().any(|d| contains_ci(a0, d)) {
        return false;
    }
    if s.len() >= 6 && s[..6].eq_ignore_ascii_case(b"alias ") {
        return false;
    }
    if find_all_ci(s, b"connect ").any(|i| !(i >= 2 && s[i - 2..i].eq_ignore_ascii_case(b"re"))) {
        return false;
    }
    if ["motd_write", "motdfile", "retry", "_set", "quit", "kill"].iter().any(|n| first_at_boundary(s, n, b" ;\n\r")) {
        return false;
    }
    let anywhere = ["bind ", "unbind ", "_restart", "exit", "writecfg", "cl_filterstuffcmd", "unbindall"];
    if anywhere.iter().any(|n| contains_ci(s, n)) || (!policy.tfc && contains_ci(s, "exec")) {
        return false;
    }
    if policy.filter {
        const FILTERED: [&str; 15] = [
            "ex_interp ",
            "say ",
            "developer",
            "timerefresh",
            "rate",
            "cd ",
            "fps_max",
            "speak_enabled",
            "voice_enable",
            "sensitivity",
            "sys_ticrate",
            "setinfo",
            "removedemo",
            "volume",
            "mp3volume",
        ];
        if FILTERED.iter().any(|n| contains_ci(s, n)) {
            return false;
        }
        if ["cl_", "gl_", "m_", "r_", "hud_"].iter().any(|p| first_at_boundary(s, p, b" ;")) {
            return false;
        }
        // Signed compare: every byte >= 0x80 counts as < 32.
        if s.iter().any(|&b| (b as i8) < 32 && b != b'\n') {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(text: &str) -> bool {
        valid_stuff_text(text, StuffPolicy::default())
    }

    #[test]
    fn stock_server_text_passes() {
        assert!(ok("reconnect\n"));
        assert!(ok("cl_forwardspeed 400\n"));
        assert!(ok("spk \"vox/hello\"\n"));
        assert!(ok("fullserverinfo \"\\\\mapname\\\\de_dust2\"\n"));
    }

    #[test]
    fn dangerous_commands_are_rejected_anywhere_in_the_message() {
        assert!(!ok("connect 1.2.3.4\n"));
        assert!(!ok("echo hi;connect 1.2.3.4\n"), "connect after a separator");
        assert!(!ok("alias x y\n"));
        assert!(!ok("echo a;quit\n"));
        assert!(!ok("bind mouse1 +attack\n"));
        assert!(!ok("echo ;exec evil.cfg\n"));
        assert!(!ok("echo exit\n"), "exit is denied anywhere");
        assert!(ok("echo a_quit\n"), "quit not at a command boundary");
    }

    #[test]
    fn exec_is_allowed_only_for_tfc() {
        let tfc = StuffPolicy { tfc: true, ..StuffPolicy::default() };
        assert!(valid_stuff_text("exec mapdefault.cfg\n", tfc));
    }

    #[test]
    fn cl_filterstuffcmd_adds_prefixes_and_non_ascii() {
        let filter = StuffPolicy { filter: true, ..StuffPolicy::default() };
        assert!(!valid_stuff_text("cl_forwardspeed 400\n", filter));
        assert!(!valid_stuff_text("echo héllo\n", filter));
        assert!(!valid_stuff_text("rate 25000\n", filter));
        assert!(valid_stuff_text("spk hello\n", filter));
        let sp = StuffPolicy { single_player: true, ..StuffPolicy::default() };
        assert!(valid_stuff_text("connect x\n", sp));
    }
}
