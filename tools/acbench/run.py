"""cstrike-game AC bench: run our client against the local anti-cheat test server and check it
answers like the original client.

    python tools/acbench/run.py --server-dir <HLDS copy> [--addr 127.0.0.1:27016] [--seconds 95]

The server copy has ReHLDS + Metamod-R + AMX Mod X + ReAPI + ReChecker and two AMXX plugins:
Karaulov's `unreal_cheat_detector` (compiled from source) and our `rs_bench.sma` probe. The expected
cvar set comes from the original binaries (`re/dump/hw_cvars.tsv`, `re/dump/client_cvars.tsv`,
extracted from hw.dll's `Cvar_RegisterVariable` call sites and client.so's `pfnRegisterVariable`
calls). Checks:
  - every original cvar answers a query (no "Bad CVAR request");
  - privileged cvars answer "CVAR is privileged";
  - every other reply equals the original default unless a config file the client executes sets it;
  - stufftext sets apply or are refused exactly as the cvar flags say;
  - the X probes of docs/re/audit-ac-checks.md §6: stufftext semantics (P1), the client command
    log (P2, P8), query edge cases (P4), usercmd accounting (P5), the MOTD browser (P6, served here
    on 127.0.0.1:8088), director stufftext (P9) and byte fidelity (P10);
  - consistency answers for files inside, outside and above the game dir (P3);
  - the cheat detector logged nothing.
Not covered: P7 needs a visible window (config.cfg is only written by a normal quit), P11/P12 need
hl.exe or closed modules; the fake-Steam blocker of P3 fails by design in LAN mode (A4).
Exit status 0 when everything passes.
"""
import argparse
import glob
import http.server
import os
import re
import shutil
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
GAME = r"C:/Program Files (x86)/Steam/steamapps/common/Half-Life"
FCVAR_SERVER = 0x4
FCVAR_PROTECTED = 0x20
FCVAR_PRIVILEGED = 0x400
FCVAR_CLIENTDLL = 0x10

# One cvar per flag class: name, test value.
STUFF_TESTS = [
    ("host_limitlocal", "1"),  # engine, flags 0
    ("cl_dlmax", "1234"),  # engine, ARCHIVE|USERINFO
    ("cl_righthand", "0"),  # client, ARCHIVE
    ("cl_lw", "0"),  # engine, ARCHIVE|USERINFO
    ("volume", "0.35"),  # engine, ARCHIVE|FILTERSTUFFTEXT
    ("sensitivity", "2.5"),  # client, ARCHIVE|FILTERSTUFFTEXT
    ("rcon_password", "bench"),  # engine, PRIVILEGED
    ("cl_filterstuffcmd", "1"),  # engine, ARCHIVE|PRIVILEGED
    ("hud_draw", "0"),  # client, ARCHIVE
]

# Files whose consistency hash ReChecker asks for: outside the game dir, inside it, only in valve/,
# and one that does not exist anywhere. Files the server itself lists as resources (e.g. the map's
# WADs) are not used: the engine then checks them against the server's own resource entry.
FILE_TESTS = ["../demoplayer.dll", "../hw.dll", "liblist.gam", "sprites/hud.txt", "gfx/palette.lmp", "../rs_bench_missing.dll",
              "../../common/Half-Life/valve/delta.lst"]
# Engine search order for the English Steam layout (gs-fs GameLayout; engine-core.md §1).
SEARCH_DIRS = ["cstrike_hd", "cstrike", "cstrike_downloads", "valve_hd", "", "valve", "platform"]
# Client cvars CHud::Init registers only with developer > 0 or in czero (cvars-client.md §0.1).
ABSENT_IN_CSTRIKE = {"cl_fog_density", "cl_fog_r", "cl_fog_g", "cl_fog_b", "tutor_enable", "cl_career_difficulty"}
# Values the engine itself rewrites at run time: the host's address (NET_GetLocalAddress) and
# the DSP's room parameters (SX_RoomFX writes a preset into them on every room_type change).
RUNTIME_VALUES = {"net_address", "room_lp", "room_mod", "room_size", "room_refl", "room_rvblp", "room_delay", "room_feedback",
                  "room_dlylp", "room_left"}
RECHECKER_MARK = "; cstrike-game bench"
# Client commands a stock client sends to the game DLL during this run (P2 allow-list), plus
# the probe's own unknown command.
ALLOWED_COMMANDS = {"vmodenable", "vban", "specmode", "spec_set_ad", "menuselect", "jointeam", "joinclass", "say",
                    "say_team", "rsb_unknown"}
# Engine commands the probe stuffs that must stay on the client (P2), and the allow_autoaim tail (P1).
LOCAL_ONLY = {"fullserverinfo", "snapshot", "screenshot", "cmdlist", "version", "contimes", "clear", "rsb_marker"}
MOTD_PORT = 8088


class MotdHandler(http.server.BaseHTTPRequestHandler):
    """P6: the page sets a cookie and references an image the browser must request with it."""
    requests = []

    def do_GET(self):
        MotdHandler.requests.append((self.path, self.headers.get("User-Agent") or "", self.headers.get("Cookie") or ""))
        if self.path.startswith("/rsb"):
            body = b'<html><body>bench<img src="/img"></body></html>'
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Set-Cookie", "rsbid=bench1; Path=/")
        else:
            body = b"GIF89a"
            self.send_response(200)
            self.send_header("Content-Type", "image/gif")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def expected_md5(name):
    """MD5 hex of the file FileSystem_Stdio opens for `name` (first search path where
    `<dir>/<name>` exists), or None when it is missing."""
    import hashlib
    for d in SEARCH_DIRS:
        p = os.path.normpath(os.path.join(GAME, d, name))
        if os.path.isfile(p):
            with open(p, "rb") as f:
                return hashlib.md5(f.read()).hexdigest()
    return None


def install_rechecker_rules(server_dir):
    path = os.path.join(server_dir, "cstrike", "addons", "rechecker", "resources.ini")
    with open(path, encoding="latin-1") as f:
        text = f.read()
    text = text.split("\n" + RECHECKER_MARK)[0].rstrip("\n") + "\n"
    text += RECHECKER_MARK + "\n"
    for name in FILE_TESTS:
        text += f'"{name}"\tUNKNOWN\t"echo RSB|[file_name]|[file_md5hash]"\n'
        if expected_md5(name) is None:
            text += f'"{name}"\tMISSING\t"echo RSB|[file_name]|MISSING"\n'
    with open(path, "w", encoding="latin-1") as f:
        f.write(text)


def read_table(path, client):
    rows = {}
    with open(path, encoding="latin-1") as f:
        for line in f:
            parts = line.rstrip("\n").split("\t")
            name, default, flags = parts[0], parts[1], int(parts[2], 16)
            rows[name.lower()] = (name, default, flags | (FCVAR_CLIENTDLL if client else 0))
    return rows


def config_names():
    """Cvars a config file the client executes may set (their values are the user's)."""
    names = set()
    for rel in ("cstrike/config.cfg", "cstrike/userconfig.cfg", "cstrike/autoexec.cfg", "valve/config.cfg",
                "valve/autoexec.cfg", "cstrike/valve.rc", "valve/valve.rc", "valve/hw/opengl.cfg", "cstrike/hw/opengl.cfg",
                "valve/controller.cfg", "cstrike/controller.cfg", "valve/joystick.cfg", "cstrike/joystick.cfg"):
        p = os.path.join(GAME, rel)
        if os.path.exists(p):
            with open(p, encoding="latin-1") as f:
                for line in f:
                    tok = line.strip().split()
                    if tok:
                        names.add(tok[0].strip('"').lower())
    return names


def startup_values():
    """Cvars the engine writes during startup (cvars-engine.md "Startup writes"), for a client
    run without -dev/-console/-novid/-nomousegrab."""
    patch = "1.0.1.4"
    for d in SEARCH_DIRS:
        p = os.path.join(GAME, d, "steam.inf")
        if os.path.isfile(p):
            with open(p, encoding="latin-1") as f:
                for line in f:
                    if line.strip().startswith("PatchVersion="):
                        patch = line.strip()[len("PatchVersion="):].strip()
            break
    return {"sv_lan": "1", "gl_overbright": "0", "sv_version": f"{patch}/Stdio,48,10210"}


def check_probes(probes, replies, commands, windows, req_states, http_requests, server_console, failures):
    """The X-phase probes, the command log and the usercmd windows (audit-ac-checks.md §6)."""
    def expect(key, want, what):
        got = probes.get(key)
        if got != want:
            failures.append(f"{what}: {key} = {got!r}, original {want!r}")

    # P1: each message whole, one line per frame, quotes honoured, a refused message refused whole.
    expect("host_limitlocal", "1", "P1 two stuffs in one frame")
    expect("cl_dlmax", "1001", "P1 multi-command line")
    expect("cl_lw", "0", "P1 multi-command line")
    expect("cl_lc", replies.get("cl_lc"), "P1 message with a refused command")
    expect("_rsb", "a;b", "P1 quoted ;")
    # P2 / P8: the game DLL sees only what a stock client sends.
    seen = [a.lower() for a, _ in commands]
    for argv0 in sorted(set(seen) & LOCAL_ONLY):
        failures.append(f"P2 local command reached the server: {argv0}")
    for argv0 in sorted(set(seen) - ALLOWED_COMMANDS - LOCAL_ONLY):
        failures.append(f"P2 unexpected client command: {argv0}")
    if ("rsb_unknown", '1 "2 3"') not in commands:
        failures.append(f"P2 unknown command not forwarded verbatim: {[c for c in commands if c[0] == 'rsb_unknown']}")
    # P8: VModEnable on the first voice update; vban only when forced by a ReqState (the masks
    # agree otherwise), and mp.dll sends ReqState only until VModEnable has arrived.
    if "vmodenable" not in seen:
        failures.append("P8 VModEnable never sent")
    if req_states and "vban" not in seen:
        failures.append(f"P8 vban not sent after {req_states} ReqState message(s)")
    if not req_states and "vban" in seen:
        failures.append("P8 vban without a ReqState (the masks agree)")
    # P4: answered during sign-on (`team` is an engine cvar, default ""), long names (ReHLDS cuts
    # the echoed 255-character name to 254), case-insensitive lookup.
    expect("signon_team", "", "P4 sign-on query")
    expect("len254", "Bad CVAR request", "P4 254-character name")
    expect("len255", "Bad CVAR request", "P4 255-character name")
    expect("CL_LW", "0", "P4 mixed-case name")
    # P5: usercmd time accounting and move/button consistency.
    if not windows:
        failures.append("P5 no usercmd windows logged")
    for ratio, cmds, lerp_min, lerp_max, bad in windows:
        if not 0.95 <= float(ratio) <= 1.05:
            failures.append(f"P5 msec/time ratio {ratio} over {cmds} commands")
        if int(lerp_max) > 100 or int(lerp_min) < 0:
            failures.append(f"P5 lerp_msec {lerp_min}..{lerp_max}")
        if int(bad) != 0:
            failures.append(f"P5 {bad} moves without their button bits")
    # P6: the MOTD URL fetched by a Chrome 18 UA, its image with the page's cookie.
    page = [r for r in http_requests if r[0].startswith("/rsb")]
    image = [r for r in http_requests if r[0].startswith("/img")]
    if not page or "Chrome/18.0.989.0" not in page[0][1]:
        failures.append(f"P6 MOTD page request: {page}")
    if not image or "rsbid=bench1" not in image[0][2]:
        failures.append(f"P6 MOTD image request with the page's cookie: {image}")
    # P9: director stufftext; P10: bytes through setinfo and a string cvar.
    expect("director_cl_dlmax", "1003", "P9 director stufftext")
    expect("_rsb8", "000000", "P10 non-UTF-8 setinfo refused by ReHLDS")
    if "Keys and values must be valid utf8 text" not in server_console:
        failures.append("P10 the cp1251 setinfo did not reach the server as raw bytes")
    expect("_rsb9", "d098d0b3", "P10 UTF-8 setinfo bytes")
    expect("bytes_cl_logofile", "c8e3", "P10 cvar bytes")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--server-dir", required=True, help="the anti-cheat test server's HLDS directory")
    ap.add_argument("--addr", default="127.0.0.1:27016")
    ap.add_argument("--seconds", type=int, default=95)
    ap.add_argument("--client", default=os.path.join(ROOT, "target", "debug", "cstrike-game.exe"))
    args = ap.parse_args()

    expected = read_table(os.path.join(ROOT, "re", "dump", "hw_cvars.tsv"), False)
    expected.update(read_table(os.path.join(ROOT, "re", "dump", "client_cvars.tsv"), True))
    # client.dll (Windows) registers one more than client.so.
    expected["m_mousethread_sleep"] = ("m_mousethread_sleep", "10", 0x1 | FCVAR_CLIENTDLL)
    amxx = os.path.join(args.server_dir, "cstrike", "addons", "amxmodx")
    with open(os.path.join(amxx, "configs", "rs_bench_cvars.ini"), "w") as f:
        f.write("".join(n + "\n" for n, _, _ in sorted(expected.values())))
    with open(os.path.join(amxx, "configs", "rs_bench_stuff.ini"), "w") as f:
        f.write("".join(f'{n} "{v}"\n' for n, v in STUFF_TESTS))

    # Build and enable the probe.
    sma = os.path.join(amxx, "scripting", "rs_bench.sma")
    shutil.copyfile(os.path.join(ROOT, "tools", "acbench", "rs_bench.sma"), sma)
    out = subprocess.run([os.path.join(amxx, "scripting", "amxxpc.exe"), sma, "-i" + os.path.join(amxx, "scripting", "include"),
                          "-o" + os.path.join(amxx, "plugins", "rs_bench.amxx")], capture_output=True, text=True)
    if "Done." not in out.stdout:
        print(out.stdout, out.stderr)
        return 2
    # The rcon `restart` below reloads plugins.ini and the freshly built plugin.
    plugins = os.path.join(amxx, "configs", "plugins.ini")
    with open(plugins, encoding="latin-1") as f:
        enabled = "rs_bench.amxx" in f.read()
    if not enabled:
        with open(plugins, "a") as w:
            w.write("rs_bench.amxx\n")

    logs = os.path.join(amxx, "logs")
    for name in ("rs_bench.log", "unreal_cheat_detect.log"):
        p = os.path.join(logs, name)
        if os.path.exists(p):
            os.remove(p)
    install_rechecker_rules(args.server_dir)
    qconsole = os.path.join(args.server_dir, "qconsole.log")
    qstart = os.path.getsize(qconsole) if os.path.exists(qconsole) else 0
    subprocess.run([sys.executable, os.path.join(ROOT, "tools", "rcon.py"), "--addr", args.addr, "restart"], capture_output=True)
    time.sleep(3)
    env = dict(os.environ, RUST_LOG="warn")
    failures = []
    # P6's page server; P2's snapshot/screenshot write into the game directory (removed after).
    motd_server = http.server.ThreadingHTTPServer(("127.0.0.1", MOTD_PORT), MotdHandler)
    threading.Thread(target=motd_server.serve_forever, daemon=True).start()
    captures = os.path.join(GAME, "cstrike")
    before_files = set(glob.glob(os.path.join(captures, "*.bmp")) + glob.glob(os.path.join(captures, "*.tga")))
    # Close the server's MOTD (it heads the menu queue), team and class auto-select (no menu left
    # open for the probe's MOTD), then movement for P5.
    cmd = [args.client, "play", args.addr, "--size", "640x480", "--key", "SPACE:3", "--key", "5:5", "--key", "5:6",
           "--hold", "w:40:52", "--hold", "a:44:48", "--quit-after", str(args.seconds)]
    try:
        run = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", env=env, timeout=args.seconds + 60)
        client_out = run.stdout + run.stderr
    except subprocess.TimeoutExpired as e:
        def text(b):
            return b.decode("utf-8", "replace") if isinstance(b, bytes) else (b or "")
        client_out = text(e.stdout) + text(e.stderr)
        failures.append(f"client did not quit within {args.seconds + 60} s (never entered the game?)")
        print(client_out[-3000:])
    motd_server.shutdown()
    for path in set(glob.glob(os.path.join(captures, "*.bmp")) + glob.glob(os.path.join(captures, "*.tga"))) - before_files:
        os.remove(path)
    bench = os.path.join(logs, "rs_bench.log")
    lines = open(bench, encoding="latin-1").read().splitlines() if os.path.exists(bench) else []
    server_console = ""
    if os.path.exists(qconsole):
        with open(qconsole, "rb") as f:
            f.seek(qstart)
            server_console = f.read().decode("latin-1")
    replies, stuffs, done, xdone = {}, [], False, False
    probes, commands, windows, req_states = {}, [], [], 0
    for line in lines:
        m = re.search(r": (XDONE|DONE|LOADED|Q|S|X|C|P|R)\|?(.*)$", line)
        if not m:
            continue
        kind, rest = m.group(1), m.group(2)
        if kind == "Q":
            name, _, value = rest.partition("|")
            replies[name.lower()] = value
        elif kind == "S":
            name, before, after = rest.split("|", 2)
            stuffs.append((name, before, after))
        elif kind == "X":
            name, _, value = rest.partition("|")
            probes[name] = value
        elif kind == "C":
            argv0, _, cmd_args = rest.partition("|")
            commands.append((argv0, cmd_args))
        elif kind == "P":
            windows.append(rest.split("|"))
        elif kind == "R":
            req_states += 1
        elif kind == "DONE":
            done = True
        elif kind == "XDONE":
            xdone = True
    if not done:
        failures.append("probe did not finish (client dropped or too short a run)")
    if not xdone:
        failures.append("X probes did not finish")
    check_probes(probes, replies, commands, windows, req_states, MotdHandler.requests, server_console, failures)

    user_set = config_names()
    startup = startup_values()
    for key, (name, default, flags) in sorted(expected.items()):
        got = replies.get(key)
        if got is None:
            if done:
                failures.append(f"{name}: no reply")
            continue
        if key in ABSENT_IN_CSTRIKE:
            if got != "Bad CVAR request":
                failures.append(f"{name}: answered {got!r}, but the original registers it only conditionally")
            continue
        if got == "Bad CVAR request":
            failures.append(f"{name}: missing (Bad CVAR request)")
            continue
        # CL_ParseCvarValue's reply rules (net-messages.md §3.3), in order.
        want = None
        if flags & FCVAR_PRIVILEGED:
            want = "CVAR is privileged"
        elif flags & FCVAR_SERVER:
            want = "CVAR is server-only"
        elif flags & FCVAR_PROTECTED:
            want = "CVAR is protected"
        elif key == "r_decals" and replies.get("mp_decals") is not None:
            # CL_ParseServerInfo: Cvar_SetValue("r_decals", mp_decals.value) in multiplayer
            # ("%d" for a whole number, else "%f").
            v = float(replies["mp_decals"])
            want = f"{int(v)}" if abs(v - int(v)) < 0.000001 else f"{v:f}"
        elif key in startup:
            want = startup[key]
        elif key in RUNTIME_VALUES:
            want = None
        elif key not in user_set:
            want = default
        if want is not None and got != want:
            failures.append(f"{name}: {got!r}, original {want!r}")

    for name, before, after in stuffs:
        _, _, flags = expected[name.lower()]
        value = dict(STUFF_TESTS)[name]
        if flags & FCVAR_PRIVILEGED:
            if after != before:
                failures.append(f"stufftext {name}: privileged cvar changed {before!r} -> {after!r}")
        elif after != value:
            failures.append(f"stufftext {name}: {before!r} -> {after!r}, expected {value!r}")

    detector = os.path.join(logs, "unreal_cheat_detect.log")
    if os.path.exists(detector):
        for line in open(detector, encoding="latin-1"):
            if "CHEAT DETECTOR" in line:
                failures.append("detector: " + line.strip())
    for line in client_out.splitlines():
        if "Cheat Detected" in line or "panicked" in line or "Disconnected:" in line and "quit" not in line:
            failures.append("client: " + line.strip())

    # ReChecker's consistency answers (server console, -condebug).
    hashes = {}
    for line in server_console.splitlines():
        m = re.match(r"^RSB\|([^|]+)\|(\S+)", line.strip())
        if m:
            hashes[m.group(1).lower()] = m.group(2).lower()
    for name in FILE_TESTS:
        want = expected_md5(name)
        got = hashes.get(name.lower())
        if got is None:
            failures.append(f"file {name}: no consistency answer (server must run with -condebug)")
        elif want is None and got != "missing":
            failures.append(f"file {name}: answered {got}, original: missing")
        elif want is not None and int(got, 16) != int(want[:8], 16):
            # ReChecker prints the first four digest bytes with %x (no leading zeros).
            failures.append(f"file {name}: answered {got}, original md5 {want}")

    print(f"queried {len(replies)}/{len(expected)} cvars, {len(stuffs)} stufftext checks")
    for f in failures:
        print("FAIL", f)
    print("PASS" if not failures else f"{len(failures)} failure(s)")
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
