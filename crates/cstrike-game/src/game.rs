//! Playing on a server: window, input → console → usercmds, world and entity presentation.
//!
//! Keys and mouse buttons run their `config.cfg` bindings through the console; the terminal is
//! the console (type commands, read output). Click captures the mouse, Esc releases it (or
//! quits when released), F12 takes a screenshot.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use cs_client::events::ClientEvents;
use cs_client::particleman;
use cs_client::input::{Input, MoveParams, hud_mouse_sensitivity};
use cs_client::hud::{ClientState, Hud, OverviewPass, OverviewTexture, decode_map_bmp, map_tiles};
use cs_client::models::{FrameTimes, LightSource, StudioCull, StudioRenderer, ViewModelInput};
use cs_client::weather::WeatherInput;
use cs_client::view::{DriftParams, RefParams, SpecEnv, SpecParams, ViewState, angle_vectors};
use cs_client::weapons::{ClientWeapons, WeaponInputs};
use glam::Vec3;
use gs_assets::MapAssets;
use gs_client::auth::{Authenticator, LanAuthenticator};
use gs_client::demo::{DemoClientData, DemoInfo, DemoItem, RefParamsRaw};
use gs_client::{Client, ClientConfig, ClientEvent, LinkKind, State};
use gs_effects::{Effects, ModelKind, ViewParams};
use gs_fs::{FileSystem, PathId};
use gs_console::Console;
use gs_console::stufftext::StuffPolicy;
use gs_pmove::PmSound;
use gs_protocol::ServerMessage;
use gs_render::lightmap::animate_styles;
use gs_render::studio::{GlowShell, LightTarget, StudioDraw, StudioHandle, StudioLight, StudioVertex, select_local_lights};
use gs_render::{BrushInstance, EffectBatch, EffectBlend, Gpu, RenderMode, Renderer, Scene, SpriteDraw, SpriteHandle, TextureMode, View, capture};
use gs_sound::Listener;
use gs_sound::voice::VoiceCodec;
use gs_studio::StudioModel;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, CursorIcon, CustomCursor, Fullscreen, Window, WindowButtons, WindowId};

use crate::audio::{Audio, SoundRequest, VoiceSendHost};
use crate::console::{self, Host, UiOp};
use crate::gameui_host;
use crate::hud_host::{self, HudHost};
use crate::effects::{self, ClientEnv, ParticleHost, PendingSprite, Registry};
use crate::event_host::EventHost;
use crate::shot::{save_bmp, save_png};
use crate::notify;
use crate::scr;
use crate::spectate::{SpecView, ViewHost};

mod events;
mod frame;
mod hud;
mod input;
mod scene;

/// `FL_ONGROUND`.
const FL_ONGROUND: i32 = 1 << 9;
/// `MOVETYPE_NOCLIP`.
const MOVETYPE_NOCLIP: i32 = 8;
/// `kRenderFxNoDissipation`.
const RENDERFX_NO_DISSIPATION: i32 = 14;
/// `kRenderFxGlowShell`.
const RENDERFX_GLOW_SHELL: i32 = 19;
/// `EF_MUZZLEFLASH`.
const EF_MUZZLEFLASH: i32 = 2;

pub struct GameOptions {
    /// `connect` target after startup; `None` starts at the main menu.
    pub address: Option<String>,
    pub name: String,
    /// `--nick`: `name "<nick>"` run after the configs, right before `connect`.
    pub nick: Option<String>,
    /// `--size`: the harness's windowed size instead of the registry's video mode.
    pub size: Option<(u32, u32)>,
    pub fov: f32,
    /// Screenshot paths to capture `(seconds after entering the game, path)`.
    pub shots: Vec<(f64, PathBuf)>,
    /// Screenshot paths to capture `(seconds after launch, path)`.
    pub launch_shots: Vec<(f64, PathBuf)>,
    /// Quit this many seconds after entering the game.
    pub quit_after: Option<f64>,
    /// Console commands run once in game (e.g. `jointeam 2`), one per second.
    pub commands: Vec<String>,
    /// Scripted input: `+command` held from `start` to `end` seconds after entering the game.
    pub holds: Vec<(&'static str, f64, f64)>,
    /// Scripted view angles `(seconds after entering the game, pitch, yaw)`.
    pub looks: Vec<(f64, f32, f32)>,
    /// Scripted key presses `(seconds after entering the game, key number)`, sorted.
    pub keys: Vec<(f64, u8)>,
    /// Scripted typed characters `(seconds after entering the game, char)`, sorted.
    pub typed: Vec<(f64, char)>,
    /// Scripted mouse input `(seconds after entering the game, x, y, button)`, sorted.
    pub clicks: Vec<(f64, i32, i32, ScriptedMouse)>,
    /// WAV file receiving the mixed sound output at exit.
    pub sound_dump: Option<PathBuf>,
    /// `--steam_auth`: tickets from the running Steam client instead of the LAN placeholder.
    pub steam_auth: bool,
    /// No visible window: frames render into an offscreen texture (automated runs).
    pub hidden: bool,
}

/// A scripted mouse action (`--click`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptedMouse {
    Left,
    Right,
    Move,
}

pub fn run(fs: FileSystem, opts: GameOptions) -> Result<()> {
    // hl.exe is not DPI-aware: on a scaled display Windows stretches its window.
    let mut builder = EventLoop::builder();
    #[cfg(windows)]
    winit::platform::windows::EventLoopBuilderExtWindows::with_dpi_aware(&mut builder, false);
    let event_loop = builder.build().context("creating event loop")?;
    let mut app = Game { fs, opts, state: None, error: None, first_run: true };
    event_loop.run_app(&mut app).context("event loop")?;
    app.error.map_or(Ok(()), Err)
}

struct Game {
    fs: FileSystem,
    opts: GameOptions,
    state: Option<Running>,
    error: Option<anyhow::Error>,
    /// The process's first engine run (hl.exe's crash recovery runs before it only).
    first_run: bool,
}

struct Running {
    window: Arc<Window>,
    /// A transparent cursor: `SDL_ShowCursor(0)` while the grab still lets it move.
    blank_cursor: Option<CustomCursor>,
    /// The grab is SDL's relative mode (`m_rawinput != 0` when it was taken).
    relative_grab: bool,
    /// Windowless run: the window stays invisible and frames go to `offscreen`.
    hidden: bool,
    offscreen: Option<wgpu::Texture>,
    /// The engine run's video modes (IGameUIFuncs) and windowed flag.
    video: crate::video::Video,
    /// `vid.width × vid.height`: what the engine, the HUD and VGUI draw at (the mode).
    vid: (u32, u32),
    /// Fullscreen with the window larger than the mode: the mode-sized FBO, its presenter,
    /// `gl_keep_aspect` and the letterbox scale factors the cursor mapping reads.
    fbo: Option<(wgpu::Texture, gs_render::present::Present)>,
    keep_aspect: bool,
    letterbox_scale: (f32, f32),
    surface: wgpu::Surface<'static>,
    gpu: Gpu,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    fs: FileSystem,
    client: Client,
    map: Option<MapAssets>,
    /// `cl.viewangles`.
    angles: Vec3,
    /// The OS cursor is hidden and grabbed: the window is active and no popup takes mouse input.
    grabbed: bool,
    /// client.dll `m_bRawInput` and the realtime it was last re-read from `m_rawinput`.
    raw_input: bool,
    raw_input_checked: f64,
    /// Window cursor position in pixels.
    cursor: (i32, i32),
    console: Console,
    /// The client DLL's input (`kbutton_t`s, mouse accumulation).
    input: Input,
    /// client.dll's Windows mouse parameters (the acceleration `IN_ActivateMouse` forces).
    mouse_parms: crate::mouse_parms::MouseParms,
    /// The client DLL's HUD and the engine 2D screen it draws on.
    hud: Hud,
    screen: gs_draw::Screen,
    /// `titles.txt`.
    titles: gs_draw::titles::Titles,
    /// `g_lastFOV`: the last `SetFOV`, or the demo zoom in a playback.
    last_fov: f32,
    /// `g_demozoom` (type-9 demo buffers).
    demo_zoom: f32,
    /// `scr_fov_value`: `m_iFOV` as `HUD_UpdateClientData` returned it.
    scr_fov: f32,
    /// A playback's replayed refdef (copied back into `cl.simorg` & co.) and view model.
    replay: Option<(RefParamsRaw, i32)>,
    /// `startmovie`.
    movie: Option<console::Movie>,
    /// `realtime` / `oldrealtime` (`Host_FilterTime`).
    realtime: f64,
    oldrealtime: f64,
    /// `cl.paused` (`svc_setpause`).
    paused: bool,
    /// `SCR_CenterPrint` state, the PAUSED plaque and the `cl_showfps` average.
    center: scr::CenterPrint,
    paused_pic: Option<gs_draw::Image>,
    fps: scr::Fps,
    /// The `messagemode` line (`key_dest == key_message` while open) and `Con_NPrintf` slots.
    chat: notify::Chat,
    nprint: notify::NPrint,
    /// `net_graph`'s statics.
    net_graph: crate::netgraph::NetGraph,
    /// `quit` ran.
    quit: bool,
    /// The engine-side command state (`engine_cmds`).
    engine: crate::engine_cmds::EngineState,
    /// Lines typed into the terminal.
    terminal: std::sync::mpsc::Receiver<String>,
    start: Instant,
    /// `CEngine::Frame`'s previous `Sys_FloatTime` (every loop iteration, frame or not).
    last_loop: Instant,
    /// `host_speeds`' end stamp of the previous host frame.
    speeds_prev_end: Instant,
    /// `host_profile`'s call count and summed seconds since its last print.
    profile: (u32, f64),
    /// `CGame::IsActiveApp`: the window is shown and focused (an offscreen run counts as
    /// active). An inactive app waits up to 20 ms for input before each loop iteration.
    active_app: bool,
    /// The keyboard focus is in the MOTD page, a child of the window: no app switch for
    /// `CGame::AppActivate` (chromehtml renders offscreen, so the original's focus never moves).
    page_focus: bool,
    /// `GL_BeginRendering`'s last applied `(int)gl_vsync`.
    swap_interval: i32,
    /// `gl_lowlatency`'s post-swap sleep (ms, never reset) and the end of the last swap (ms on
    /// the engine clock).
    throttle_ms: f64,
    last_swap_end_ms: f64,
    /// `gl_texturemode` and `r_prefertexturefiltering` as their change hooks last saw them.
    applied_texture_mode: String,
    applied_prefer_filtering: f32,
    frametime: f64,
    in_game_since: Option<f64>,
    shots: Vec<(f64, PathBuf)>,
    launch_shots: Vec<(f64, PathBuf)>,
    /// Screenshots to take from the next drawn frame.
    pending_shots: Vec<PathBuf>,
    quit_after: Option<f64>,
    commands: Vec<String>,
    next_command_at: f64,
    /// Scripted `+command`s: command, start, end, and how far each got (0 idle, 1 down, 2 up).
    holds: Vec<(&'static str, f64, f64, u8)>,
    looks: Vec<(f64, f32, f32)>,
    keys: Vec<(f64, u8)>,
    typed: Vec<(f64, char)>,
    clicks: Vec<(f64, i32, i32, ScriptedMouse)>,
    /// The overview's map sprite (`m_MapSprite`), uploaded as 128×128 tiles.
    overview_sprite: Option<SpriteHandle>,
    /// Seconds since entering the game (scripted input clock).
    in_game_time: Option<f64>,
    studio: StudioRenderer,
    view_state: ViewState,
    /// Uploaded studio models by `Arc` identity (the `Arc` is kept so the address stays unique).
    studio_handles: HashMap<usize, (Arc<StudioModel>, StudioHandle)>,
    /// Loaded models (`mod_known`) and the precache table.
    registry: Registry,
    /// Temp entities, particles, beams and effect lights.
    effects: Effects,
    /// PVS of the last rendered view leaf.
    pvs: Vec<u8>,
    /// `StudioSetShadowSprite` from the `ShadowIdx` user message (a model index).
    shadow_sprite: i32,
    /// The CS client DLL's event state (`ev_cs`).
    client_events: ClientEvents,
    /// The sound system; `None` without an output device.
    audio: Option<Audio>,
    /// The client DLL's weapon prediction (`HUD_PostRunCmd`).
    weapons: ClientWeapons,
    /// `v_angles`: the angles of the last rendered view.
    last_view_angles: [f32; 3],
    /// The vgui2 tree with GameUI (`CBaseUI`) and its surface state.
    gameui: gs_gameui::GameUi,
    vgui: gameui_host::SurfaceState,
    vgui_input: gameui_host::InputState,
    /// GameUI work queued by console commands.
    ui_ops: Vec<console::UiOp>,
    /// `clientSwallowedDown`: key-downs VGUI consumed (their release is offered again).
    swallowed: [bool; 256],
    /// The client's GameUI calls, run by the frame (which has the surface).
    ui_events: Vec<gs_client::UiEvent>,
    /// `g_bDisconnectExplained` + `g_szDisconnectReason`, and the extended reason.
    disconnect_reason: Option<String>,
    disconnect_ext: String,
}

/// Everything one frame draws.
struct FrameScene {
    view: View,
    scene: Scene,
    /// The PIP inset pass: its view, scene and refdef viewport.
    inset: Option<(View, Scene, [i32; 4])>,
}

impl FrameScene {
    fn inset(&self) -> Option<(&View, &Scene, [i32; 4])> {
        self.inset.as_ref().map(|(v, s, r)| (v, s, *r))
    }
}

impl Game {
    /// `_restart` (engine-video.md §8): Host_Frame's extra `Cbuf_Execute`, the engine's
    /// shutdown (config.cfg written), then hl.exe's next iteration in this process: the command
    /// line rebuilt, the directories set up for its `-game`, and a new engine run.
    fn restart(&mut self, event_loop: &ActiveEventLoop) {
        let Some(mut state) = self.state.take() else { return };
        state.run_console();
        state.shutdown();
        drop(state);
        crate::cmdline::rebuild_for_restart();
        if let Some(base) = self.fs.base_dir().map(std::path::Path::to_path_buf) {
            let layout = crate::dirs::layout(base);
            if !layout.base_dir.join(&layout.game).is_dir() {
                self.error = Some(anyhow!("game directory {} not found under {}", layout.game, layout.base_dir.display()));
                event_loop.exit();
                return;
            }
            self.fs = FileSystem::from_layout(&layout);
        }
        // The post-restart `+connect` and config.cfg carry the server and the name over.
        self.opts.address = None;
        self.opts.nick = None;
        self.resumed(event_loop);
    }

    fn start(&mut self, event_loop: &ActiveEventLoop) -> Result<Running> {
        // hl.exe's iteration (crash recovery once per process, the engine DLL), then
        // VideoMode_Create / CVideoMode_Common::Init on display 0 between the
        // CrashInitializingVideoMode marks; CGame::CreateGameWindow (title = liblist.gam's
        // `game`, a fixed-size hidden window), AdjustWindowForMode. The harness keeps the
        // registry out.
        let monitor = event_loop.primary_monitor().or_else(|| event_loop.available_monitors().next());
        let display = crate::video::Display::new(monitor.as_ref());
        let persist = self.opts.size.is_none() && !self.opts.hidden;
        if persist {
            if std::mem::take(&mut self.first_run) {
                crate::launcher::crash_recovery(Some(display.desktop));
            }
            crate::launcher::resolve_engine_dll();
            crate::registry::write_int("CrashInitializingVideoMode", 1);
        }
        let video = match self.opts.size {
            Some(size) => crate::video::Video::harness(&display, size),
            None => crate::video::Video::init(&display, persist),
        };
        let title = gs_gameui::options::ModInfo::load(&gameui_host::HostFs(self.fs.clone())).game_description().to_owned();
        let ((width, height), pos) = video.windowed_rect(&display);
        // Fullscreen: a desktop-sized borderless window showing the mode-sized FBO, or with
        // -nofbo an exclusive switch to the closest display mode (§5.2, §5.3.2).
        let fullscreen = !video.windowed && !self.opts.hidden;
        let fbo_enabled = !crate::cmdline::check_parm("-nofbo");
        // SDL2's fixed-size window style: caption, system menu, minimize (no thick frame, no
        // maximize button).
        let mut attrs = crate::video::with_app_icon(
            Window::default_attributes()
                .with_title(title)
                .with_resizable(false)
                .with_enabled_buttons(WindowButtons::CLOSE | WindowButtons::MINIMIZE)
                .with_inner_size(PhysicalSize::new(width, height))
                .with_active(false)
                .with_visible(false),
        );
        if fullscreen {
            attrs = attrs.with_fullscreen(Some(match (fbo_enabled, monitor.as_ref()) {
                (false, Some(m)) => match crate::video::closest_display_mode(m, (width, height), crate::cmdline::parm_value("-freq").map(|v| crate::video::crt_atoi(&v))) {
                    Some(mode) => Fullscreen::Exclusive(mode),
                    None => Fullscreen::Borderless(monitor.clone()),
                },
                _ => Fullscreen::Borderless(monitor.clone()),
            }));
        }
        let window = Arc::new(event_loop.create_window(attrs).context("creating window")?);
        let blank_cursor = CustomCursor::from_rgba(vec![0u8; 4], 1, 1, 0, 0)
            .map(|source| event_loop.create_custom_cursor(source))
            .map_err(|err| log::warn!("blank cursor: {err}"))
            .ok();
        // SDL_SetWindowPosition places the client area.
        if !fullscreen && let (Ok(inner), Ok(outer)) = (window.inner_position(), window.outer_position()) {
            window.set_outer_position(winit::dpi::PhysicalPosition::new(pos.0 - (inner.x - outer.x), pos.1 - (inner.y - outer.y)));
        }
        window.set_visible(!self.opts.hidden);
        if persist {
            crate::registry::write_int("CrashInitializingVideoMode", 0);
        }
        let instance = Gpu::new_instance();
        let surface = instance.create_surface(window.clone()).context("creating surface")?;
        // The backend chosen in the video options (`_setrenderer`); OpenGL first otherwise.
        let preferred = gs_render::gpu::backend_from_name(&crate::registry::read_string("RenderBackend", ""));
        let gpu = pollster::block_on(Gpu::new(instance, Some(&surface), preferred))?;
        let renderers = if self.opts.hidden { Vec::new() } else { pollster::block_on(gpu.available_backends(&surface)) };
        // vid: the mode size the engine and VGUI draw at; the surface is the window.
        let vid = (width, height);
        let size = if fullscreen { window.inner_size() } else { PhysicalSize::new(width, height) };
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| anyhow!("surface is not supported by the adapter"))?;
        let formats = surface.get_capabilities(&gpu.adapter).formats;
        config.format = formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
            .ok_or_else(|| anyhow!("no linear 8-bit surface format in {formats:?}"))?;
        config.present_mode = wgpu::PresentMode::AutoNoVsync;
        surface.configure(&gpu.device, &config);
        let offscreen = self.opts.hidden.then(|| frame_texture(&gpu.device, vid, config.format, "offscreen frame", wgpu::TextureUsages::RENDER_ATTACHMENT));
        // The FBO path whenever the fullscreen window is not the mode's size.
        let fbo = (fullscreen && (config.width, config.height) != vid).then(|| new_fbo(&gpu.device, vid, config.format));
        let keep_aspect = !crate::cmdline::check_parm("-stretchaspect");
        // 4× MSAA windowed (always) and in the fullscreen FBO unless -nomsaa; none in the
        // exclusive -nofbo mode (engine-video.md §5.3.9).
        let samples = if !fullscreen || (fbo_enabled && !crate::cmdline::check_parm("-nomsaa")) { 4 } else { 1 };
        let mut renderer = Renderer::new(&gpu.device, &gpu.queue, config.format, samples);
        renderer.set_decal_wad(load_decal_wad(&self.fs)?);
        // SCR_Init: the PAUSED plaque.
        let paused_pic = scr::load_paused(&self.fs, &mut renderer);

        // hl.exe always runs under Steam: its ISteamUser carries voice (g_bUsingSteamVoice) and
        // the server browser's matchmaking. The connect ticket is Steam's only with
        // --steam-auth (where a Steam failure ends startup); otherwise the LAN ticket.
        let steam = match gs_steam::SteamClient::init() {
            Ok(s) => Some(s),
            Err(e) if self.opts.steam_auth => return Err(anyhow!("Steam authentication: {e}")),
            Err(e) => {
                log::warn!("Steam: {e}");
                None
            }
        };
        let auth: Box<dyn Authenticator> = match steam.as_ref().filter(|_| self.opts.steam_auth) {
            Some(s) => Box::new(s.authenticator()),
            None => Box::new(LanAuthenticator),
        };
        // hl.exe never runs without Steam. Without a Steam client here, Steam's voice wire format
        // is spoken by the vendored codecs instead (docs/deviations.md).
        let voice_codec: Option<Box<dyn VoiceCodec>> = match steam.as_ref() {
            Some(s) => Some(Box::new(s.voice())),
            None => match gs_voicecodec::SteamFormatVoice::new() {
                Ok(v) => Some(Box::new(v)),
                Err(e) => {
                    log::warn!("voice: {e}");
                    None
                }
            },
        };
        // Host_Init: HPAK_CheckIntegrity("custom").
        gs_client::hpak::Hpak::check_integrity(&self.fs);
        // DemoPlayer_Load (after Voice_Init, before CL_Init): the module and its dem_* commands.
        let demo_player = crate::demoplayer::load(&self.fs, crate::cmdline::check_parm("-demoedit"));
        let mut client_config = ClientConfig::new(&self.opts.name);
        client_config.multicast_loop = crate::cmdline::check_parm("-loopback");
        let mut client = Client::new(self.fs.clone(), client_config, auth)?;
        client.demo_player = Some(demo_player.clone());
        let mut console = console::new_console();
        // Con_Init: `-condebug` turns the log on; FS_RemoveFile("qconsole.log") clears the mod
        // directory's copy (not the working directory's file that is appended to).
        notify::set_condebug(crate::cmdline::check_parm("-condebug"));
        self.fs.remove("qconsole.log", Some(gs_fs::PathId::GameConfig));
        crate::demoplayer::drain_log(&demo_player.borrow().sys, console.cvars.value("developer") != 0.0);
        let mut screen = hud_host::new_screen(&self.fs);
        screen.set_size(vid.0, vid.1);
        let titles = hud_host::load_titles(&self.fs);
        let mut hud = Hud::default();
        let (mut input, mut angles, mut quit) = (Input::default(), Vec3::ZERO, false);
        // The game window starts active: Sys_InitGame's IN_ActivateMouse.
        input.mouse_active = true;
        let mut mouse_parms = crate::mouse_parms::MouseParms::startup(!self.opts.hidden);
        mouse_parms.activate();
        let mut engine = crate::engine_cmds::EngineState::new(&client.config.game);
        if !self.opts.hidden {
            engine.motd.attach(&window);
        }
        let mut chat = notify::Chat::default();
        let mut movie = None;
        let mut ui_ops = Vec::new();
        // VGuiWrap2_Startup → CBaseUI::Start: the vgui2 tree with GameUI; `-console` is a raw
        // substring test of the command line.
        let mut vgui = gameui_host::SurfaceState::default();
        let mut gameui = {
            let ui = gameui_host::new_ui(&self.fs);
            let tracker_text = gameui_host::read_text(&self.fs, "resource/TrackerScheme.res").unwrap_or_default();
            let tracker = gs_vgui::scheme::Scheme::parse("resource/TrackerScheme.res", "BaseUI", &tracker_text);
            let menu = gameui_host::read_text(&self.fs, "resource/GameMenu.res");
            let layout = gameui_host::read_text(&self.fs, "resource/BackgroundLayout.txt");
            let console_flag = crate::cmdline::line().contains("-console");
            let mut loader = hud_host::Loader { renderer: &mut renderer, fs: &self.fs };
            let mut s = gameui_host::VguiSurface::new(&mut vgui, &mut screen, &mut loader);
            let steam = crate::cmdline::argv().iter().any(|a| a.eq_ignore_ascii_case("-steam") || a.eq_ignore_ascii_case("-showplatform"));
            let fs: gs_gameui::Fs = std::rc::Rc::new(gameui_host::HostFs(self.fs.clone()));
            gs_gameui::GameUi::start(ui, tracker, menu.as_deref(), layout.as_deref(), console_flag, steam, fs, &mut s)
        };
        // CGameUI::Start(…, &g_SystemWrapper): the demo dialogs reach the loaded DemoPlayer.
        gameui.host.borrow_mut().demo_player = gs_gameui::DemoModule(Some(demo_player.clone()));
        // ServerBrowser.dll's Steam: the engine's client context; without Steam the browser
        // queries community masters and the servers itself (gs-serverquery).
        let browser_steam = steam.clone();
        match browser_steam.and_then(|s| s.matchmaking().map_err(|e| log::warn!("server browser: {e}")).ok()) {
            Some(mm) => gameui.set_matchmaking(Box::new(mm)),
            None => gameui.set_matchmaking(Box::new(gs_serverquery::ServerQueries::new(std::rc::Rc::new(gameui_host::HostFs(self.fs.clone()))))),
        }
        // IGameUIFuncs: GetVideoModes / GetCurrentVideoMode / GetCurrentRenderer's windowed flag
        // (the engine run's values), and the registry values GameUI reads.
        {
            let mut h = gameui.host.borrow_mut();
            let vm = |m: crate::video::Mode| gs_gameui::VideoMode { width: m.width, height: m.height, bpp: m.bpp };
            h.video_modes = video.modes().iter().copied().map(vm).collect();
            h.current_mode = vm(video.current_mode());
            h.windowed = video.windowed;
            // GetCurrentRenderer's name: the running backend; the video options offer the others.
            h.renderer = gs_render::gpu::backend_name(gpu.adapter.get_info().backend).to_owned();
            h.renderers = renderers.iter().map(|&b| (gs_render::gpu::backend_name(b).to_owned(), gs_render::gpu::backend_label(b).to_owned())).collect();
            // The Steam persona: the configured player name stands for it (console::startup).
            h.persona_name = self.opts.name.clone();
            h.steam_available = true;
            h.app_id = gs_steam::CSTRIKE_APP_ID as i32;
            h.steam_offline_mode = crate::registry::steam_int("OfflineAFS") != Some(1) && crate::registry::steam_int("Offline") == Some(1);
            h.hd_models = crate::registry::read_int("hdmodels", 0) > 0;
            h.addons_folder = crate::registry::read_int("addons_folder", 0) > 0;
            h.registry.insert(gs_gameui::options_content::GORE_KEY.to_owned(), crate::registry::read_string("User Token 2", ""));
        }
        let gameui_cmds: String = gameui.take_requests().into_iter().filter_map(|r| match r {
            gs_gameui::HostRequest::ClientCmd(c) => Some(format!("{c}\n")),
            _ => None,
        }).collect();
        let rng = client.rng().clone();
        let mut audio = Audio::open(&self.fs, rng.clone(), self.opts.sound_dump.take(), voice_codec, console.cvars.value("MP3Volume"));
        // Host_Init → Voice_Init, before any config runs (voice_enable still "1").
        if let Some(audio) = &mut audio {
            audio.voice_init(&console);
        }
        {
            let mut host = Host {
                client: &mut client,
                fs: &self.fs,
                input: &mut input,
                hud: &mut hud,
                screen: &mut screen,
                renderer: &mut renderer,
                titles: &titles,
                viewangles: &mut angles,
                quit: &mut quit,
                engine: &mut engine,
                effects: None,
                audio: audio.as_mut(),
                chat: &mut chat,
                movie: &mut movie,
                ui_ops: &mut ui_ops,
            };
            let adapter = gpu.adapter.get_info();
            // gl_dump's glGetString values: the adapter's vendor, name and driver (no extension
            // string on this renderer).
            let vendor = match adapter.vendor {
                0x10DE => "NVIDIA Corporation".to_owned(),
                0x1002 => "ATI Technologies Inc.".to_owned(),
                0x8086 => "Intel".to_owned(),
                _ => adapter.driver.clone(),
            };
            host.engine.gl_strings = [vendor, adapter.name.clone(), adapter.driver_info.clone(), String::new()];
            console::startup(&mut console, &mut host, &self.opts.name, adapter.vendor == 0x10DE, &adapter.name, &gameui_cmds);
            if let Some(nick) = &self.opts.nick {
                console.execute_string(&mut host, &format!("name \"{nick}\""), true);
            }
            if let Some(address) = &self.opts.address {
                console.execute_string(&mut host, &format!("connect {address}"), true);
            }
        }
        console::apply_client_cvars(&console, &mut client);
        client.config.sound_started = audio.is_some();
        // ForceReloadProfile: swap interval -1 (adaptive) with gl_vsync > 0, else 0; the first
        // GL_BeginRendering then applies (int)gl_vsync.
        config.present_mode = present_mode_for(&surface, &gpu.adapter, if console.cvars.value("gl_vsync") > 0.0 { -1 } else { 0 });
        surface.configure(&gpu.device, &config);
        let palette = self.fs.read("gfx/palette.lmp").context("gfx/palette.lmp")?;
        let now = Instant::now();
        Ok(Running {
            window,
            blank_cursor,
            relative_grab: false,
            hidden: self.opts.hidden,
            offscreen,
            video,
            vid,
            fbo,
            keep_aspect,
            letterbox_scale: (1.0, 1.0),
            surface,
            gpu,
            config,
            renderer,
            fs: self.fs.clone(),
            client,
            map: None,
            angles,
            grabbed: false,
            raw_input: false,
            raw_input_checked: 0.0,
            cursor: (0, 0),
            console,
            input,
            mouse_parms,
            hud,
            screen,
            titles,
            last_fov: 0.0,
            demo_zoom: 0.0,
            scr_fov: self.opts.fov,
            replay: None,
            movie,
            realtime: 0.0,
            oldrealtime: 0.0,
            paused: false,
            center: scr::CenterPrint::default(),
            paused_pic,
            fps: scr::Fps::default(),
            chat,
            nprint: notify::NPrint::default(),
            net_graph: crate::netgraph::NetGraph::default(),
            quit,
            engine,
            terminal: console::terminal_input(),
            start: now,
            last_loop: now,
            // Sys_FloatTime counts from start-up, so the first frame's `host` spans it.
            speeds_prev_end: now,
            profile: (0, 0.0),
            active_app: true,
            page_focus: false,
            swap_interval: 0,
            throttle_ms: 0.0,
            last_swap_end_ms: 0.0,
            // Empty: the first frame applies gl_texturemode with the configured gl_ansio, as
            // config.cfg's Cvar_Set of it does.
            applied_texture_mode: String::new(),
            applied_prefer_filtering: 1.0,
            frametime: 0.0,
            in_game_since: None,
            shots: std::mem::take(&mut self.opts.shots),
            launch_shots: std::mem::take(&mut self.opts.launch_shots),
            pending_shots: Vec::new(),
            quit_after: self.opts.quit_after,
            commands: std::mem::take(&mut self.opts.commands),
            next_command_at: 0.0,
            holds: std::mem::take(&mut self.opts.holds).into_iter().map(|(c, from, to)| (c, from, to, 0)).collect(),
            looks: std::mem::take(&mut self.opts.looks),
            keys: std::mem::take(&mut self.opts.keys),
            typed: std::mem::take(&mut self.opts.typed),
            clicks: std::mem::take(&mut self.opts.clicks),
            overview_sprite: None,
            in_game_time: None,
            studio: StudioRenderer::new(self.fs.clone(), rng.clone()),
            view_state: ViewState::default(),
            studio_handles: HashMap::new(),
            registry: Registry::default(),
            effects: Effects::new(&palette, rng.clone()),
            pvs: Vec::new(),
            shadow_sprite: 0,
            client_events: ClientEvents::default(),
            audio,
            weapons: ClientWeapons::new(rng),
            last_view_angles: [0.0; 3],
            gameui,
            vgui,
            vgui_input: gameui_host::InputState::default(),
            ui_ops,
            swallowed: [false; 256],
            ui_events: Vec::new(),
            disconnect_reason: None,
            disconnect_ext: String::new(),
        })
    }
}

impl ApplicationHandler for Game {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match self.start(event_loop) {
            Ok(mut state) => {
                // CHud::Init's one-time work.
                let (hud, mut eng) = state.hud_host();
                hud.init(&mut eng);
                self.state = Some(state);
            }
            Err(err) => {
                self.error = Some(err);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => {
                state.shutdown();
                event_loop.exit();
            }
            // SDL_WINDOWEVENT_RESIZED: the window rect follows; the mode (vid, fonts, HUD) stays.
            // The window equals the mode while windowed; fullscreen shows the mode's FBO.
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                state.config.width = size.width;
                state.config.height = size.height;
                state.surface.configure(&state.gpu.device, &state.config);
                if state.video.windowed {
                    if (size.width, size.height) != state.vid {
                        let _ = state.window.request_inner_size(PhysicalSize::new(state.vid.0, state.vid.1));
                    }
                } else if state.fbo.is_none() && (size.width, size.height) != state.vid {
                    state.fbo = Some(new_fbo(&state.gpu.device, state.vid, state.config.format));
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                // IN_Accumulate without m_rawinput: GetCursorPos − the window centre, then
                // IN_ResetMouse puts the cursor back in the centre.
                if state.grabbed && !state.raw_input() {
                    let size = state.window.inner_size();
                    let (dx, dy) = (position.x as i32 - (size.width / 2) as i32, position.y as i32 - (size.height / 2) as i32);
                    if dx != 0 || dy != 0 {
                        let visible = state.vgui_cursor_visible();
                        state.input.accumulate_mouse(dx, dy, visible);
                        state.center_cursor();
                    }
                }
                state.cursor = state.window_to_vid(position.x as i32, position.y as i32);
                state.hud.set_cursor(state.cursor.0, state.cursor.1);
                // BaseUISurface::AppHandler: vgui2 sees every event.
                state.gameui.ui.cursor_moved(state.cursor.0, state.cursor.1);
            }
            WindowEvent::MouseInput { state: pressed, button, .. } => {
                let down = pressed == ElementState::Pressed;
                let code = match button {
                    MouseButton::Left => Some(gs_vgui::keys::MOUSE_LEFT),
                    MouseButton::Right => Some(gs_vgui::keys::MOUSE_RIGHT),
                    MouseButton::Middle => Some(2),
                    MouseButton::Back => Some(3),
                    MouseButton::Forward => Some(4),
                    MouseButton::Other(_) => None,
                };
                if let Some(code) = code {
                    if down {
                        // A click outside the MOTD page takes the keyboard back from it.
                        if state.page_focus {
                            state.engine.motd.focus_game();
                        }
                        let (t, s) = (state.console.cvars.value("sdl_double_click_time"), state.console.cvars.value("sdl_double_click_size"));
                        let cursor = state.cursor;
                        state.vgui_input.mouse_down(&mut state.gameui.ui, code, cursor, t, s);
                    } else {
                        state.gameui.ui.mouse_released(code);
                    }
                }
                // The client's IN_MouseEvent ignores buttons while the VGUI cursor shows.
                if state.gameui.is_game_ui_active() {
                    return;
                }
                if state.hud.cursor_visible() {
                    // The menu gets the click; mouse buttons are not key events meanwhile, but
                    // a release still ends a `+command` held from before.
                    if down {
                        let (x, y) = state.cursor;
                        let (hud, mut eng) = state.hud_host();
                        hud.click(x, y, button == MouseButton::Left, &mut eng);
                    } else {
                        if button == MouseButton::Left {
                            let (x, y) = state.cursor;
                            let (hud, mut eng) = state.hud_host();
                            hud.release(x, y, &mut eng);
                        }
                        if let Some(key) = console::mouse_key_number(button) {
                            state.console.key_event(key, false, false);
                        }
                    }
                    return;
                }
                if !state.grabbed {
                    return;
                }
                // IN_MouseEvent: no button key events while the camera holds the mouse.
                if let Some(key) = console::mouse_key_number(button)
                    && !state.input.cam.mouse_in_use
                {
                    state.key_input(key, down, false);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y.round() as i32,
                    MouseScrollDelta::PixelDelta(p) => p.y.signum() as i32,
                };
                if notches == 0 {
                    return;
                }
                state.gameui.ui.mouse_wheeled(notches);
                if state.hud.cursor_visible() && !state.gameui.is_game_ui_active() {
                    let (hud, mut eng) = state.hud_host();
                    hud.wheel(notches, &mut eng);
                } else if state.grabbed || state.gameui.is_game_ui_active() {
                    // Key_Event(MWHEELUP / MWHEELDOWN) press and release, through the filter.
                    let key = if notches > 0 { gs_console::keys::K_MWHEELUP } else { gs_console::keys::K_MWHEELDOWN };
                    state.key_input(key, true, false);
                    state.key_input(key, false, false);
                }
            }
            // CGame::AppActivate (SDL focus events): an inactive app sleeps up to 20 ms for input
            // before each frame; music pauses while the window is in the background. The focus
            // moving into the MOTD page (the window stays in front) and back is no switch.
            WindowEvent::Focused(active) => {
                if !active && state.engine.motd.shows_pages() && crate::ffi::is_foreground(&state.window) {
                    state.page_focus = true;
                } else if !(active && std::mem::take(&mut state.page_focus)) {
                    state.app_activate(active);
                }
            }
            WindowEvent::KeyboardInput { event, is_synthetic, .. } => {
                use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
                let PhysicalKey::Code(code) = event.physical_key else { return };
                let down = event.state == ElementState::Pressed;
                // winit presses the keys held when the window gains focus; SDL does not (its
                // releases on focus loss, SDL_ResetKeyboard, are kept).
                if is_synthetic && down {
                    return;
                }
                if code == KeyCode::F12 && down && !event.repeat {
                    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
                    state.pending_shots.push(PathBuf::from(format!("screenshots/game_{stamp}.png")));
                }
                // The engine's Key_Event first ...
                if code != KeyCode::F12
                    && let Some(key) = console::key_number(code)
                {
                    state.key_input(key, down, event.repeat);
                }
                // ... then BaseUISurface::AppHandler: KEYDOWN/KEYUP with SyncModifiers.
                let vcode = gameui_host::vgui_key(&event.key_without_modifiers(), code, event.location);
                state.vgui_input.key(&mut state.gameui.ui, vcode, code, down);
                // SDL_TEXTINPUT: typed characters (no control codes) for vgui2 and the chat line.
                // With Ctrl held the character is a control code (Ctrl+V → U+0016), so it types
                // nothing; `text` would drop the Ctrl and type the plain letter after a paste.
                if down && let Some(text) = event.text_with_all_modifiers() {
                    let text: String = text.chars().filter(|c| !c.is_control()).collect();
                    if !text.is_empty() {
                        state.text_input(&text);
                    }
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        let Some(state) = self.state.as_mut().filter(|s| s.grabbed) else { return };
        // IN_Accumulate with m_rawinput: SDL relative (raw) motion.
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event
            && state.raw_input()
        {
            let visible = state.vgui_cursor_visible();
            state.input.accumulate_mouse(dx as i32, dy as i32, visible);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = self.state.as_mut() else { return };
        state.poll_page_focus();
        // CEngine::Frame's first statement, every loop iteration: CCDAudio::Frame.
        if let Some(audio) = &mut state.audio {
            audio.music_frame(&mut state.console.cvars, state.start.elapsed().as_secs_f64());
        }
        // CEngine::Frame → Host_Frame: every iteration advances realtime; the limiter decides
        // whether this one is a host frame. Frames run whether or not the window is visible.
        let started = Instant::now();
        if state.filter_time() {
            state.host_frame(event_loop);
            if state.engine.restart {
                self.restart(event_loop);
                return;
            }
        }
        state.host_profile(started.elapsed());
        // The original spins while active; a timer wake can overshoot, so the last 2 ms before
        // the next frame is due are spun as well. Inactive, it waits the 20 ms (or for input).
        let next = state.next_loop();
        let flow = match next.checked_sub(Duration::from_millis(2)) {
            _ if !state.active_app => ControlFlow::WaitUntil(next),
            Some(early) if early > Instant::now() => ControlFlow::WaitUntil(early),
            _ => ControlFlow::Poll,
        };
        event_loop.set_control_flow(flow);
    }
}

/// A `vid`-sized frame texture in the surface's format.
fn frame_texture(device: &wgpu::Device, vid: (u32, u32), format: wgpu::TextureFormat, label: &'static str, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: vid.0, height: vid.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// The fullscreen FBO (mode-sized, sampled by the presenter) and its presenter.
fn new_fbo(device: &wgpu::Device, vid: (u32, u32), format: wgpu::TextureFormat) -> (wgpu::Texture, gs_render::present::Present) {
    (frame_texture(device, vid, format, "fbo", wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING), gs_render::present::Present::new(device, format))
}

/// `SDL_GL_SetSwapInterval(interval)` as a present mode: 1 vsync, -1 adaptive (relaxed FIFO
/// where the surface has it), 0 none.
fn present_mode_for(surface: &wgpu::Surface, adapter: &wgpu::Adapter, interval: i32) -> wgpu::PresentMode {
    match interval {
        0 => wgpu::PresentMode::AutoNoVsync,
        i if i < 0 && surface.get_capabilities(adapter).present_modes.contains(&wgpu::PresentMode::FifoRelaxed) => wgpu::PresentMode::FifoRelaxed,
        _ => wgpu::PresentMode::AutoVsync,
    }
}

/// `Decal_Init`: the default game's `decals.wad` (required) with the mod's merged on top.
fn load_decal_wad(fs: &FileSystem) -> Result<gs_render::decal::DecalWad> {
    let mut wad = gs_render::decal::DecalWad::default();
    let read = |id| -> Result<gs_formats::wad::Wad> {
        let path = fs.resolve_in("decals.wad", Some(id))?;
        let data = std::fs::read(&path).with_context(|| path.display().to_string())?;
        Ok(gs_formats::wad::Wad::parse(data)?)
    };
    wad.merge(&read(PathId::DefaultGame).context("decals.wad")?);
    match read(PathId::Game) {
        Ok(game) => wad.merge(&game),
        Err(e) => log::debug!("mod decals.wad: {e:#}"),
    }
    Ok(wad)
}

/// `cle_Key_Event`'s console keys (`0x112475E0`): a playback toggles the console with them.
fn is_console_key(key: u8) -> bool {
    matches!(key, 9 | 13 | 32..=127 | 128..=134 | 149..=152 | 160..=169 | 172..=174 | 176) && key != 96 && key != 126
}
