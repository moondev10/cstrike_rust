use std::collections::HashMap;

use gs_common::info::{InfoString, MAX_USERINFO};

use super::*;

#[derive(Default)]
struct Host {
    printed: String,
    sent: Vec<String>,
    connected: bool,
    multiplayer: bool,
    userinfo: Option<InfoString>,
    files: HashMap<String, String>,
    commands: Vec<Vec<String>>,
}

impl ConsoleHost for Host {
    fn print(&mut self, text: &str) {
        self.printed.push_str(text);
    }
    fn connected(&self) -> bool {
        self.connected
    }
    fn multiplayer(&self) -> bool {
        self.multiplayer
    }
    fn send_string_cmd(&mut self, text: &str) {
        self.sent.push(text.to_owned());
    }
    fn forward_string_cmd(&mut self, text: &str) {
        self.sent.push(text.to_owned());
    }
    fn userinfo(&mut self) -> &mut InfoString {
        self.userinfo.get_or_insert_with(|| InfoString::new(MAX_USERINFO))
    }
    fn command(&mut self, console: &mut Console, args: &Args) {
        let mut seen = args.argv.clone();
        seen.push(format!("privileged={}", console.is_privileged()));
        self.commands.push(seen);
    }
    fn read_config(&mut self, name: &str) -> Option<Vec<u8>> {
        self.files.get(name).map(|s| s.clone().into_bytes())
    }
}

fn connected() -> Host {
    Host { connected: true, multiplayer: true, ..Host::default() }
}

fn run(console: &mut Console, host: &mut Host, text: &str) {
    console.add_text(text);
    console.execute(host);
}

#[test]
fn unknown_commands_go_to_the_server_with_their_raw_arguments() {
    let mut c = Console::new(true);
    let mut h = connected();
    run(&mut c, &mut h, "jointeam 2;kill;cmd foo \"bar baz\";cmd dlfile x.wad\n");
    assert_eq!(h.sent, ["jointeam 2", "kill \n", "foo \"bar baz\""]);

    let mut offline = Host::default();
    run(&mut c, &mut offline, "jointeam 2;say hi\n");
    assert!(offline.sent.is_empty());
    assert_eq!(offline.printed, "Can't \"say\", not connected\n", "unknown commands drop silently, forwards complain");
}

#[test]
fn lookup_order_is_command_alias_cvar() {
    let mut c = Console::new(true);
    c.register_command("+attack", cmd_flags::HUD);
    c.register_cvar("sensitivity", "3", cvar_flags::ARCHIVE);
    let mut h = connected();
    run(&mut c, &mut h, "alias go \"+attack;echo fired\";go\nsensitivity 2.5\nsensitivity\n");
    assert_eq!(h.commands, [vec!["+attack".to_owned(), "privileged=true".to_owned()]]);
    assert_eq!(h.printed, "fired \n\"sensitivity\" is \"2.5\"\n");
    assert_eq!(c.cvars.value("sensitivity"), 2.5);
    assert_eq!(c.take_changed(), ["sensitivity"]);
}

#[test]
fn wait_defers_the_rest_to_the_next_frame() {
    let mut c = Console::new(true);
    let mut h = connected();
    run(&mut c, &mut h, "echo 1;wait;echo 2\n");
    assert_eq!(h.printed, "1 \n");
    c.execute(&mut h);
    assert_eq!(h.printed, "1 \n2 \n");
}

#[test]
fn stufftext_runs_one_line_per_frame_without_privileges() {
    let mut c = Console::new(true);
    c.register_cvar("cl_forwardspeed", "400", cvar_flags::ARCHIVE | cvar_flags::CLIENTDLL);
    c.register_cvar("rate", "30000", cvar_flags::ARCHIVE | cvar_flags::USERINFO | cvar_flags::FILTERSTUFFTEXT);
    let mut h = connected();
    c.stuff_text(&mut h, "cl_forwardspeed 300\nkill\nrate 1000\n", StuffPolicy::default());
    assert_eq!(h.printed, "Server tried to send invalid command:\"cl_forwardspeed 300\nkill\nrate 1000\n\"\n");

    c.stuff_text(&mut h, "cl_forwardspeed 300\nsay hi\nrate 1000\n", StuffPolicy::default());
    c.execute(&mut h);
    assert_eq!(c.cvars.value("cl_forwardspeed"), 300.0);
    assert!(h.sent.is_empty(), "only one filtered line per frame");
    c.execute(&mut h);
    assert_eq!(h.sent, ["say hi"]);
    c.execute(&mut h);
    assert_eq!(c.cvars.value("rate"), 1000.0, "FILTERSTUFFTEXT only bites with cl_filterstuffcmd");

    run(&mut c, &mut h, "cl_filterstuffcmd 1\n");
    c.stuff_text(&mut h, "echo x\n", StuffPolicy::default());
    c.execute(&mut h);
    assert!(h.printed.ends_with("x \n"));
}

#[test]
fn filtered_lines_cannot_run_privileged_commands_or_touch_privileged_cvars() {
    let mut c = Console::new(true);
    // CL_Init's connect, as the host registers it.
    c.register_command("connect", cmd_flags::PRIVILEGED);
    let mut h = connected();
    c.execute_string(&mut h, "connect 1.2.3.4", false);
    c.execute_string(&mut h, "cl_filterstuffcmd 1", false);
    assert_eq!(h.printed, "Could not execute privileged command connect 1.2.3.4\ncl_filterstuffcmd is a privileged variable\n");
    assert!(h.commands.is_empty());
    assert_eq!(c.cvars.value("cl_filterstuffcmd"), 0.0);
}

#[test]
fn aliases_from_stufftext_stay_unprivileged() {
    let mut c = Console::new(true);
    let mut h = connected();
    run(&mut c, &mut h, "alias boom \"quit\"\n");
    c.stuff_text(&mut h, "boom\n", StuffPolicy::default());
    c.execute(&mut h);
    c.execute(&mut h);
    // The whole refused line: the alias value is "quit " (Cmd_Alias_f's argument join).
    assert_eq!(h.printed, "Could not execute privileged command quit \n");
    assert!(h.commands.is_empty());
}

#[test]
fn userinfo_cvars_update_userinfo_and_send_setinfo_when_changed() {
    let mut c = Console::new(true);
    c.register_cvar("name", "unnamed", cvar_flags::USERINFO | cvar_flags::ARCHIVE | cvar_flags::PRINTABLEONLY | cvar_flags::NOEXTRAWHITESPACE);
    let mut h = connected();
    run(&mut c, &mut h, "name \"  Rusty \"\nname Rusty\n");
    assert_eq!(h.userinfo().get("name"), Some("Rusty"));
    assert_eq!(h.sent, ["setinfo \"name\" \"Rusty\"\n"], "the unchanged second set sends nothing");
}

#[test]
fn sponly_cvars_are_locked_in_multiplayer() {
    let mut c = Console::new(true);
    c.register_cvar("r_speeds", "0", cvar_flags::SPONLY);
    let mut h = connected();
    run(&mut c, &mut h, "r_speeds 1\n");
    assert_eq!(h.printed, "Can't set r_speeds in multiplayer\n");
    assert_eq!(c.cvars.value("r_speeds"), 0.0);
}

#[test]
fn exec_inserts_files_in_front_and_checks_names() {
    let mut c = Console::new(true);
    let mut h = connected();
    h.files.insert("config.cfg".into(), "bind \"w\" \"+forward\"\n// comment\nalias x \"echo cfg\"\n".into());
    run(&mut c, &mut h, "exec config.cfg;x\nexec ../evil.cfg\nexec a.b.cfg\nexec notes.txt\nexec missing.cfg\nexec autoexec.cfg\n");
    assert_eq!(c.bindings.get(b'w'), "+forward");
    assert_eq!(
        h.printed,
        "cfg \nexec ../evil.cfg: invalid path.\nexec a.b.cfg: invalid filename.\nexec notes.txt: not a .cfg or .rc file\ncouldn't exec missing.cfg\n"
    );
}

#[test]
fn alias_names_are_validated() {
    let mut c = Console::new(true);
    c.register_cvar("sensitivity", "3", 0);
    let mut h = Host::default();
    run(&mut c, &mut h, &format!("alias sensitivity x\nalias cl_autobuy x\nalias {} x\nalias\n", "a".repeat(32)));
    assert_eq!(h.printed, "Alias name is invalid\nAlias name is invalid\nAlias name is too long\nCurrent alias commands:\n");
}

#[test]
fn bind_reports_and_stores_bindings() {
    let mut c = Console::new(true);
    let mut h = Host::default();
    run(&mut c, &mut h, "bind mouse1 +attack\nbind MOUSE1\nbind f2\nbind nokey x\nunbind mouse1\nbind mouse1\n");
    assert_eq!(
        h.printed,
        "\"MOUSE1\" = \"+attack\"\n\"f2\" is not bound\n\"nokey\" isn't a valid key\n\"mouse1\" is not bound\n"
    );
}

#[test]
fn config_lists_bindings_archived_cvars_and_extra_userinfo() {
    let mut c = Console::new(true);
    c.register_cvar("name", "unnamed", cvar_flags::USERINFO | cvar_flags::ARCHIVE);
    c.register_cvar("volume", "0.8", cvar_flags::ARCHIVE);
    c.register_cvar("cl_showfps", "0", 0);
    let mut info = InfoString::new(MAX_USERINFO);
    assert_eq!(info.set("name", "Rusty"), Ok(()));
    assert_eq!(info.set("_vgui_menus", "1"), Ok(()));
    assert_eq!(c.config_text(&info), None, "fewer than two bindings");
    c.bindings.set(b'w', "+forward");
    c.bindings.set(keys::K_SPACE, "+jump");
    let text = c.config_text(&info).unwrap_or_default();
    let body: Vec<&str> = text.lines().skip(3).collect();
    assert_eq!(
        body,
        [
            "unbindall",
            "bind \"SPACE\" \"+jump\"",
            "bind \"w\" \"+forward\"",
            "cl_filterstuffcmd \"0\"",
            "name \"unnamed\"",
            "volume \"0.8\"",
            "setinfo \"_vgui_menus\" \"1\"",
            "+mlook",
            "+jlook",
            "exec controller.cfg",
            "exec userconfig.cfg",
        ]
    );
}

/// A value a server managed to set with quotes or a line break must stay one quoted value in
/// config.cfg, not commands run with a local config's privileges at the next start.
#[test]
fn config_values_cannot_close_their_quotes() {
    let mut c = Console::new(true);
    c.register_cvar("name", "unnamed", cvar_flags::ARCHIVE);
    c.bindings.set(b'w', "+forward");
    c.bindings.set(keys::K_SPACE, "+jump");
    c.cvars.direct_set("name", "x\";connect 6.6.6.6\n\"");
    let text = c.config_text(&InfoString::new(MAX_USERINFO)).unwrap_or_default();
    assert!(text.lines().any(|l| l == "name \"x;connect 6.6.6.6\""), "{text}");
    assert!(!text.lines().any(|l| l.starts_with("connect")), "{text}");
}
