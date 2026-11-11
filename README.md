# cstrike-game

A Counter-Strike 1.6 client written in Rust. It is a reimplementation of the current Steam client
(HL25, build 10210, protocol 48) that plays on ordinary CS 1.6 servers using the game files from
your Half-Life install.

## Why

- **Same behaviour as the original.** The network protocol, movement and prediction, weapons, HUD,
  VGUI menus, console, cvars, configs and demos follow the original binaries. Servers, plugins and
  anti-cheats see a normal client, and the game plays the same. A behaviour difference that is not
  listed under [Differences](#differences-from-the-original) is a bug.
- **Safe against hostile servers.** The original client has holes a server can use: commands that
  write or delete files, downloads of `.lnk`/`.url` files, a 2012 Chromium for the MOTD, a stack
  overflow in the voice decoder, maps that crash or hang the client. They are closed here, and the
  code is memory-safe Rust.
- **Current renderer.** The renderer is built on wgpu (Direct3D 12, Vulkan or OpenGL) instead of the
  original's fixed-function OpenGL.

## Requirements

- Windows 10/11 x64. This is the platform the client is developed and tested on.
- Counter-Strike 1.6 installed through Steam. All game data is read from that install; this
  repository contains none.
- Rust 1.96 or newer.
- Optional: Steam running, with `steam_api64.dll` next to the executable (or its path in
  `CSTRIKE_GAME_STEAM_API`). It is needed for Steam authentication and Steam voice. Without it the
  client still runs.
- Optional: the WebView2 runtime (part of Windows 11) to show HTML MOTDs. Without it, MOTDs are
  shown as plain text.

## Build and run

```sh
cargo build --release

target/release/cstrike-game play                         # main menu
target/release/cstrike-game play 1.2.3.4:27015           # connect to a server
target/release/cstrike-game play 1.2.3.4:27015 --steam_auth
target/release/cstrike-game play -- -console -windowed   # engine parameters go after --
```

The Half-Life directory is found from `--basedir`, then `GS_BASEDIR`, then the default Steam path.

The client shares its settings with the original game:
- `cstrike/config.cfg` in the Half-Life directory;
- the registry key `HKCU\Software\Valve\Half-Life\Settings` (video mode, renderer).

### Authentication

| Mode | Ticket | Servers it works on |
|---|---|---|
| Default | placeholder ticket | `sv_lan 1` servers and servers that admit non-Steam clients (e.g. ReUnion) |
| `--steam_auth` | real ticket from the running Steam client (the account must own the game) | any server, including ones that require Steam |

### Other commands

| Command | What it does |
|---|---|
| `cstrike-game view --map de_dust2` | Fly around a map locally, without a server. |
| `cstrike-game shot --map de_dust2 --out x.png` | Render views of a map offscreen to a PNG. |
| `cstrike-game headless <addr>` | Connect without a window and log the session (test harness). |
| `gstool <command>` | Inspect and validate game content: search paths, BSP, WAD, MDL, sprites. |

`play` also has options for scripted, windowless test runs: `--hidden`, `--shot`, `--cmd`,
`--key`, `--click`, `--quit-after`. See `cstrike-game play --help`.

## Differences from the original

These are deliberate. Everything else is meant to match the original.

**Security**
- Server-sent commands cannot run commands that write or delete files, set privileged cvars or
  redirect logs (`writeid`, `condebug`, `logaddress`, `hpkremove`, …).
- Values written to `config.cfg` cannot break out of their quotes.
- Downloads are limited to the formats the game reads (`bsp mdl spr wav mp3 wad tga bmp txt res sc`).
  Nothing goes under `resource/`, Windows device names are refused, and so are transfers over 64 MiB.
- File-consistency checks cannot probe paths outside the game (`..`).
- MOTD pages and HTTP downloads cannot reach this machine or its LAN unless the game server is on it.
- The Steam API library is loaded only from `CSTRIKE_GAME_STEAM_API` or the executable's directory,
  never from the working directory.
- Malformed voice packets, and maps that would overflow the stack, loop forever or exceed GPU
  limits, no longer crash or hang the client.

**MOTD.** HTML MOTDs are shown in a locked-down WebView2, not CEF 18:
- no JavaScript and no sound;
- `http`/`https` only;
- no popups, downloads, permission prompts or external protocols such as `steam://`;
- all traffic goes through a filtering proxy inside the client.

**Not supported**
- The listen server: `map`, `career` and `load` print a refusal. Connect to a dedicated server
  (HLDS) instead.
- The software renderer and the joystick.

**Voice without Steam.** Without a Steam client, the Steam voice format is encoded and decoded
with the bundled Opus and SILK code.

## Repository layout

| Crate | Purpose |
|---|---|
| `cstrike-game` | The executable: window, main loop, engine glue, MOTD browser |
| `cs-client` | The CS client DLL: HUD, VGUI menus, view, weapons prediction, events |
| `gs-client` | Engine client: connection, sign-on, snapshots, prediction, downloads, demos |
| `gs-net`, `gs-protocol` | Protocol 48: UDP transport, netchan, bit buffers, delta codec, messages |
| `gs-pmove`, `gs-world` | Player movement and BSP collision |
| `gs-render`, `gs-studio`, `gs-effects` | wgpu renderer, studio models, temp entities and particles |
| `gs-draw`, `gs-vgui`, `gs-gameui` | 2D drawing and fonts, vgui2, GameUI (main menu, options, console) |
| `gs-sound`, `gs-voicecodec`, `gs-steam` | Sound and music, voice codecs, Steamworks API |
| `gs-console`, `gs-fs`, `gs-formats`, `gs-assets` | Commands and cvars, game filesystem, file formats, asset loading |
| `gs-serverquery`, `gs-hltv` | Server browser queries, HLTV demo player |
| `gs-common`, `gs-tool` | Shared helpers, the `gstool` CLI |

`tools/` holds test scripts:
- `hidden_run.py` runs a program on an invisible desktop;
- `rcon.py` sends rcon commands;
- `refshot.py` takes screenshots from the original client;
- `acbench/` runs the client against a server with anti-cheat plugins.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

- Workspace lints deny `unsafe` except in the three FFI modules (`cstrike-game`, `gs-steam`,
  `gs-voicecodec`).
- Workspace lints warn on `unwrap`, `expect` and `panic`.
- Code comments cite `docs/re/*.md`. These are reverse-engineering notes on the original binaries
  and are not part of this repository.

## Legal

This project is not affiliated with or endorsed by Valve. It contains no game data and no Valve
binaries; you need your own copy of Counter-Strike 1.6. The application icon is based on the
Counter-Strike 1.6 icon.

Bundled third-party code:
- libopus, BSD licence: `crates/gs-voicecodec/opus/COPYING`;
- SILK SDK, Skype licence: `crates/gs-voicecodec/silk/LICENSE`.
