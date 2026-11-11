//! Counter-Strike 1.6 client.

mod audio;
mod cmdline;
mod console;
mod demoplayer;
mod dirs;
mod effects;
mod engine_cmds;
mod engine_cvars;
mod event_host;
mod ffi;
mod game;
mod headless;
mod gameui_host;
mod hud_host;
mod launcher;
mod lightstyles;
mod motd;
mod mouse_parms;
mod notify;
mod netgraph;
mod registry;
mod scene;
mod scr;
mod shot;
mod spectate;
mod viewer;
mod video;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use glam::Vec3;
use gs_fs::FileSystem;

#[derive(Parser)]
#[command(about = "Counter-Strike 1.6 client")]
struct Cli {
    /// Half-Life install directory (default: $GS_BASEDIR or the Steam default path).
    #[arg(long, global = true)]
    basedir: Option<PathBuf>,
    /// Mod directory.
    #[arg(long, global = true, default_value = "cstrike")]
    game: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Fly around a map locally (no server).
    View {
        #[command(flatten)]
        camera: CameraArgs,
        /// Window size, `WxH`.
        #[arg(long, default_value = "1280x720", value_parser = parse_size)]
        size: (u32, u32),
    },
    /// Render views of a map offscreen and save a PNG (several views are tiled into one grid).
    Shot {
        #[command(flatten)]
        camera: CameraArgs,
        /// Extra view `"x y z pitch yaw roll"`; repeatable. Replaces --pos/--ang.
        #[arg(long = "view", allow_hyphen_values = true)]
        views: Vec<String>,
        /// Repeat every view rotated by 360/N degrees of yaw.
        #[arg(long, default_value_t = 1)]
        yaws: u32,
        /// Size of each view, `WxH`.
        #[arg(long, default_value = "1280x720", value_parser = parse_size)]
        size: (u32, u32),
        /// Seconds since map start (light style / water animation phase).
        #[arg(long, default_value_t = 0.0)]
        time: f64,
        /// Output PNG path.
        #[arg(long, short)]
        out: PathBuf,
        /// Map fog as the `Fog` message sets it, `R,G,B,DENSITY` (e.g. `128,128,140,0.002`).
        #[arg(long)]
        fog: Option<String>,
        /// Render with the engine's underwater fog (`cl.waterlevel > 2`).
        #[arg(long)]
        underwater: bool,
    },
    /// Connect to a server without a window and log the session (test harness).
    Headless {
        /// Server address `host[:port]`.
        address: String,
        #[arg(long, default_value = "rustplayer")]
        name: String,
        /// Run time in seconds.
        #[arg(long, default_value_t = 30.0)]
        seconds: f64,
        /// Local UDP port (0 = ephemeral).
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Console commands sent to the server once in game, one per second.
        #[arg(long = "cmd")]
        commands: Vec<String>,
    },
    /// Play in a window: connect to a server, or start at the main menu without an address.
    Play {
        /// Server address `host[:port]`.
        address: Option<String>,
        #[arg(long, default_value = "rustplayer")]
        name: String,
        /// Player name typed into the console before connecting (`name` after the configs,
        /// which otherwise set the name from `config.cfg`).
        #[arg(long)]
        nick: Option<String>,
        /// Test harness: a windowed `WxH` window, registry untouched. Without it the video mode
        /// comes from the registry and the engine parameters (`-windowed`, `-w`, `-h`, …).
        #[arg(long, value_parser = parse_size)]
        size: Option<(u32, u32)>,
        #[arg(long, default_value_t = 90.0)]
        fov: f32,
        /// Screenshot `SECONDS:PATH` after entering the game; repeatable.
        #[arg(long = "shot")]
        shots: Vec<String>,
        /// Screenshot `SECONDS:PATH` after launch (menus, connect and loading dialogs);
        /// repeatable.
        #[arg(long = "launch-shot")]
        launch_shots: Vec<String>,
        /// Quit this many seconds after entering the game.
        #[arg(long)]
        quit_after: Option<f64>,
        /// Console command run after entering the game (unknown ones go to the server); repeatable.
        #[arg(long = "cmd")]
        commands: Vec<String>,
        /// Scripted input `KEY:START:END` (seconds after entering the game; keys w a s d e r
        /// space ctrl attack attack2 tab hold the matching `+command`); repeatable.
        #[arg(long = "hold")]
        holds: Vec<String>,
        /// Scripted view angles `SECONDS:PITCH:YAW`; repeatable.
        #[arg(long = "look", allow_hyphen_values = true)]
        looks: Vec<String>,
        /// Scripted key press `KEY:SECONDS` (a bind key name such as `SPACE`, `2`, `KP_ENTER`),
        /// pressed and released through the window key path (open menus first); repeatable.
        #[arg(long = "key")]
        keys: Vec<String>,
        /// Scripted typing `SECONDS:TEXT`: the characters go through the window text-input path,
        /// one every 0.1 s; repeatable.
        #[arg(long = "type")]
        typed: Vec<String>,
        /// Scripted mouse `SECONDS:X:Y[:left|right|move]` (default left): the cursor moves to
        /// the window pixel, then the button is pressed and released through the window mouse
        /// path (`move` only moves); repeatable.
        #[arg(long = "click")]
        clicks: Vec<String>,
        /// Also write the mixed sound output (22050 Hz stereo) to this WAV file at exit.
        #[arg(long)]
        sound_dump: Option<PathBuf>,
        /// Authenticate through the running Steam client (needed by internet servers); without
        /// it the connect ticket only satisfies `sv_lan 1` servers.
        #[arg(long = "steam_auth", alias = "steam-auth")]
        steam_auth: bool,
        /// No visible window (frames render offscreen; screenshots still work). Implied by
        /// `--quit-after`, so scripted runs never open a window.
        #[arg(long)]
        hidden: bool,
        /// Engine command-line parameters after `--` (`-console`, `-steam`, `-demoedit`, …), which
        /// the engine and GameUI test with `COM_CheckParm` on the process command line.
        #[arg(last = true)]
        engine_args: Vec<String>,
    },
}

#[derive(Args)]
struct CameraArgs {
    /// Map name, e.g. `de_dust2`.
    #[arg(long)]
    map: String,
    /// Eye position `x y z` (default: first spawn point).
    #[arg(long, num_args = 3, allow_negative_numbers = true)]
    pos: Option<Vec<f32>>,
    /// View angles `pitch yaw roll` in degrees.
    #[arg(long, num_args = 3, allow_negative_numbers = true)]
    ang: Option<Vec<f32>>,
    /// Horizontal field of view at 4:3.
    #[arg(long, default_value_t = 90.0)]
    fov: f32,
}

fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s.split_once('x').ok_or("expected WxH")?;
    let w: u32 = w.parse().map_err(|_| "bad width")?;
    let h: u32 = h.parse().map_err(|_| "bad height")?;
    if w == 0 || h == 0 {
        return Err("size must be positive".into());
    }
    Ok((w, h))
}

fn vec3(v: Option<Vec<f32>>) -> Option<Vec3> {
    v.map(|v| Vec3::new(v[0], v[1], v[2]))
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"))
        .init();
    let cli = Cli::parse();
    cmdline::init_from_process(&cli.game);
    let base = cli.basedir.or_else(gs_fs::default_base_dir).context("Half-Life directory not found; pass --basedir")?;
    let layout = dirs::layout(base);
    let fs = FileSystem::from_layout(&layout);
    if !layout.base_dir.join(&layout.game).is_dir() {
        bail!("game directory {} not found under {}", layout.game, layout.base_dir.display());
    }
    match cli.cmd {
        Cmd::View { camera, size } => viewer::run(
            fs,
            viewer::ViewerOptions { map: camera.map, pos: vec3(camera.pos), angles: vec3(camera.ang), fov: camera.fov, size },
        ),
        Cmd::Shot { camera, views, yaws, size, time, out, fog, underwater } => {
            let mut specs = views.iter().map(|v| shot::ViewSpec::parse(v)).collect::<Result<Vec<_>>>()?;
            if specs.is_empty() {
                specs.push(shot::ViewSpec { pos: vec3(camera.pos), angles: vec3(camera.ang) });
            }
            let user = match fog {
                Some(f) => {
                    let v: Vec<f32> = f.split(',').map(|s| s.trim().parse::<f32>()).collect::<Result<_, _>>().context("--fog R,G,B,DENSITY")?;
                    let [r, g, b, density] = v[..] else { bail!("--fog expects R,G,B,DENSITY, got {f}") };
                    Some(([r / 255.0, g / 255.0, b / 255.0], density))
                }
                None => None,
            };
            let fog = gs_render::Fog { user, underwater };
            let req = shot::ShotRequest { map: camera.map, views: specs, yaws, fov: camera.fov, size, time, out, fog };
            shot::run(&fs, &req)
        }
        Cmd::Headless { address, name, seconds, port, commands } => {
            headless::run(fs, &headless::HeadlessOptions { address, name, seconds, port, commands })
        }
        Cmd::Play { address, name, nick, size, fov, shots, launch_shots, quit_after, commands, holds, looks, keys, typed, clicks, sound_dump, steam_auth, hidden, engine_args: _ } => {
            let parse_shots = |list: &[String], flag: &str| {
                let mut out = list
                    .iter()
                    .map(|s| {
                        let (secs, path) = s.split_once(':').with_context(|| format!("{flag} expects SECONDS:PATH"))?;
                        Ok((secs.parse::<f64>().with_context(|| format!("{flag} seconds"))?, PathBuf::from(path)))
                    })
                    .collect::<Result<Vec<_>>>()?;
                out.sort_by(|a, b| a.0.total_cmp(&b.0));
                Ok::<_, anyhow::Error>(out)
            };
            let shots = parse_shots(&shots, "--shot")?;
            let launch_shots = parse_shots(&launch_shots, "--launch-shot")?;
            let holds = holds
                .iter()
                .map(|h| {
                    let mut parts = h.split(':');
                    let (Some(key), Some(from), Some(to)) = (parts.next(), parts.next(), parts.next()) else {
                        anyhow::bail!("--hold expects KEY:START:END, got {h}");
                    };
                    let key = console::hold_command(key).with_context(|| format!("--hold: unknown key {key}"))?;
                    Ok((key, from.parse::<f64>().context("--hold start")?, to.parse::<f64>().context("--hold end")?))
                })
                .collect::<Result<Vec<_>>>()?;
            let mut looks = looks
                .iter()
                .map(|l| {
                    let v: Vec<&str> = l.split(':').collect();
                    let [at, pitch, yaw] = v[..] else { anyhow::bail!("--look expects SECONDS:PITCH:YAW, got {l}") };
                    Ok((at.parse::<f64>()?, pitch.parse::<f32>()?, yaw.parse::<f32>()?))
                })
                .collect::<Result<Vec<_>>>()?;
            looks.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut keys = keys
                .iter()
                .map(|k| {
                    let (key, at) = k.rsplit_once(':').with_context(|| format!("--key expects KEY:SECONDS, got {k}"))?;
                    let key = gs_console::keys::key_number(key).with_context(|| format!("--key: unknown key {key}"))?;
                    Ok((at.parse::<f64>().context("--key seconds")?, key))
                })
                .collect::<Result<Vec<_>>>()?;
            keys.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut typed_chars = Vec::new();
            for t in &typed {
                let (at, text) = t.split_once(':').with_context(|| format!("--type expects SECONDS:TEXT, got {t}"))?;
                let at = at.parse::<f64>().context("--type seconds")?;
                typed_chars.extend(text.chars().enumerate().map(|(i, ch)| (at + i as f64 * 0.1, ch)));
            }
            typed_chars.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut clicks = clicks
                .iter()
                .map(|c| {
                    let v: Vec<&str> = c.split(':').collect();
                    let (at, x, y, button) = match v[..] {
                        [at, x, y] => (at, x, y, "left"),
                        [at, x, y, b] => (at, x, y, b),
                        _ => anyhow::bail!("--click expects SECONDS:X:Y[:BUTTON], got {c}"),
                    };
                    let button = match button {
                        "left" => game::ScriptedMouse::Left,
                        "right" => game::ScriptedMouse::Right,
                        "move" => game::ScriptedMouse::Move,
                        other => anyhow::bail!("--click: unknown button {other}"),
                    };
                    Ok((at.parse::<f64>().context("--click seconds")?, x.parse::<i32>().context("--click x")?, y.parse::<i32>().context("--click y")?, button))
                })
                .collect::<Result<Vec<_>>>()?;
            clicks.sort_by(|a, b| a.0.total_cmp(&b.0));
            let hidden = hidden || quit_after.is_some();
            game::run(fs, game::GameOptions { address, name, nick, size, fov, shots, launch_shots, quit_after, commands, holds, looks, keys, typed: typed_chars, clicks, sound_dump, steam_auth, hidden })
        }
    }
}
