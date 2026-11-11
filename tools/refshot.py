"""Reference screenshots from the original client, for visual comparison with ours.

Runs a private copy of the game (`--game-dir`, a copy of the Steam Half-Life folder) on the
invisible desktop, connects to a server, optionally joins a team, takes `screenshot`s at given
times and converts them to PNG.

    python tools/refshot.py --server 127.0.0.1:27015 --join 2 --shot 8:screenshots/ref_ct.png

Times are seconds after the `connect` command was issued (100 `wait`s per second at fps_max 100).
Needs Steam running and logged in (the original client authenticates).
"""
import argparse
import glob
import os
import subprocess
import sys

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
FPS = 100


def build_cfg(server, join, shots, settle, viewmodel, commands=()):
    lines = [
        'alias w10 "' + ";".join(["wait"] * 10) + '"',
        'alias w100 "' + ";".join(["w10"] * 10) + '"',
        "setinfo _vgui_menus 0",
        f"r_drawviewmodel {1 if viewmodel else 0}",
        "crosshair 0",
        "hud_draw 0",
        "net_graph 0",
        f"fps_max {FPS}",
        "name refclient",
        f"connect {server}",
    ]
    elapsed = 0.0

    def wait_until(t):
        nonlocal elapsed
        steps = max(0, round((t - elapsed) * FPS / 100))
        lines.extend(["w100"] * steps)
        elapsed += steps

    if join:
        wait_until(settle)
        lines.append(f"jointeam {join}")
        wait_until(settle + 2)
        lines.append("joinclass 5")
    timeline = [(t, "screenshot") for t, _ in shots] + list(commands)
    for t, line in sorted(timeline, key=lambda e: e[0]):
        wait_until(t)
        lines.append(line)
    wait_until(elapsed + 1)
    lines.append("quit")
    return "\n".join(lines) + "\n"


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--game-dir", required=True, help="a copy of the Steam Half-Life folder")
    p.add_argument("--server", default="127.0.0.1:27015")
    p.add_argument("--join", type=int, choices=[1, 2], help="team to join (1 T, 2 CT)")
    p.add_argument("--settle", type=float, default=20.0, help="seconds to wait for the connection before joining")
    p.add_argument("--cmd", action="append", default=[], help="SECONDS:COMMAND console command at a time")
    p.add_argument("--shot", action="append", default=[], help="SECONDS:OUT.png")
    p.add_argument("--size", default="640x480")
    p.add_argument("--viewmodel", action="store_true", help="draw the view model")
    args = p.parse_args()
    shots = []
    for s in args.shot:
        t, out = s.split(":", 1)
        shots.append((float(t), out))
    shots.sort()
    commands = []
    for c in args.cmd:
        t, line = c.split(":", 1)
        commands.append((float(t), line))
    cstrike = os.path.join(args.game_dir, "cstrike")
    for old in glob.glob(os.path.join(cstrike, "HalfLife*.tga")):
        os.remove(old)
    with open(os.path.join(cstrike, "refshot.cfg"), "w") as f:
        f.write(build_cfg(args.server, args.join, shots, args.settle, args.viewmodel, commands))
    w, h = args.size.split("x")
    cmd = [sys.executable, os.path.join(HERE, "hidden_run.py"), "--cwd", args.game_dir, "--",
           os.path.join(args.game_dir, "hl.exe"), "-game", "cstrike", "-windowed", "-w", w, "-h", h,
           "-novid", "-nojoy", "+exec", "refshot.cfg"]
    timeout = (shots[-1][0] if shots else args.settle) + 60
    subprocess.run(cmd, timeout=timeout, check=False)
    produced = sorted(glob.glob(os.path.join(cstrike, "HalfLife*.tga")))
    if len(produced) != len(shots):
        sys.exit(f"expected {len(shots)} screenshots, got {len(produced)}")
    for (_, out), tga in zip(shots, produced):
        os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
        Image.open(tga).convert("RGB").save(out)
        os.remove(tga)
        print(out)


if __name__ == "__main__":
    main()
