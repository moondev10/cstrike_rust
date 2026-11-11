//! The engine client: connection handshake, sign-on, resource downloads, snapshot parsing and
//! command sending (docs/re/net-transport.md §8, docs/re/net-messages.md §6.2/§8).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs};
use std::rc::Rc;
use std::sync::Arc;

use gs_common::bytestr::{truncate_encoded, truncate_encoded_in_place};
use gs_common::info::InfoString;
use gs_common::rng::EngineRng;
use gs_fs::FileSystem;
use gs_net::{ClientSocket, Netchan};
use gs_pmove::{Physent, PmSound, PmTrace, TextureTypes};
use gs_protocol::clc;
use gs_protocol::messages::{Resource, ResourceList, ServerInfo};
use gs_protocol::state::UPDATE_MASK;
use gs_protocol::structs::{EntityState, UserCmd};
use gs_protocol::{MsgWriter, ProtocolState, ServerMessage};
use gs_world::World;

use crate::auth::Authenticator;
use crate::demo::{Demo, DemoItem};
use crate::entities::{Entities, LinkContext, LinkKind};
use crate::events::{EventQueue, FiredEvent};
use crate::level::{Clock, Level};
use crate::postrun::{PostRunCmd, PredRecord};
use crate::predict::{LocalState, PredCmd, PredictInput, Prediction, SimState};
use crate::voice::VoiceSend;
use crate::viewfx::ScreenShake;

mod connection;
mod download;
mod playback;
mod receive;
mod send;
pub use download::PlayerDecal;

const DEFAULT_PORT: u16 = 27015;
const MAX_CONNECT_RETRIES: u32 = 4;
const CMD_SLOTS: usize = 64;
/// Commands per `clc_move` before / after the server build that raised the limit.
const LEGACY_MAX_NEW_COMMANDS: usize = 16;
const BUILD_WITH_MORE_COMMANDS: i32 = 5971;
/// `cls.datagram` size (`CL_Init`).
const DATAGRAM_SIZE: usize = 6000;
/// The command packet buffer of `CL_Move`.
const MOVE_MESSAGE_SIZE: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Disconnected = 1,
    Connecting = 2,
    Connected = 3,
    /// Downloading / verifying resources (`ca_uninitialized`).
    Uninitialized = 4,
    Active = 5,
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub userinfo: InfoString,
    /// `cl_cmdrate`.
    pub cmdrate: f32,
    /// `cl_updaterate` (the cvar; `setinfo` alone changes only the userinfo).
    pub updaterate: f32,
    /// `cl_cmdbackup`.
    pub cmdbackup: u8,
    /// `cl_resend`.
    pub resend: f64,
    /// `cl_timeout`.
    pub timeout: f64,
    /// `cl_dlmax`.
    pub dlmax: i32,
    /// `cl_allowdownload`.
    pub allow_download: bool,
    /// `cl_download_ingame`.
    pub download_ingame: bool,
    /// `cl_allowupload`.
    pub allow_upload: bool,
    /// `hpk_maxsize` (MB; 0 disables `HPAK_CheckSize`).
    pub hpk_maxsize: f32,
    /// Game directory the server must run (`cstrike`).
    pub game: String,
    /// Local UDP port (`clientport`); 0 = ephemeral.
    pub port: u16,
    /// `rcon_password`, read when the rcon challenge arrives.
    pub rcon_password: String,
    /// `cl_slist`: seconds `I` replies are accepted after `pingservers` (≤ 0: always).
    pub slist: f32,
    /// `multicastport`: NS_MULTICAST's bind port when `listen` opens it.
    pub multicast_port: u16,
    /// `-loopback`: `IP_MULTICAST_LOOP` on.
    pub multicast_loop: bool,
    /// `cl_clockreset` (s): `svc_time` snaps `cl.time` beyond this error.
    pub clock_reset: f64,
    /// `cl_fixtimerate` (ms per frame of drift correction; `< 0` acts as 7.5).
    pub fix_time_rate: f64,
    /// `cl_nosmooth`, `cl_smoothtime`, `cl_idealpitchscale`, `cl_showerror`.
    pub no_smooth: bool,
    pub smooth_time: f32,
    pub ideal_pitch_scale: f32,
    pub show_error: bool,
    /// `sv_cheats` (the server's, from `svc_sendextrainfo`), `fakelag` (ms) and `fakeloss`.
    pub cheats: bool,
    pub fake_lag: f32,
    pub fake_loss: f32,
    /// `cl_fixmodelinterpolationartifacts >= 1`, `r_bmodelinterp != 0`.
    pub fix_interpolation_artifacts: bool,
    pub bmodel_interp: bool,
    /// `cl_solid_players != 0`: other players are physents in prediction.
    pub solid_players: bool,
    /// `sound_started` (the device opened) and `nosound`: without either `S_PrecacheSound`
    /// returns NULL.
    pub sound_started: bool,
    pub nosound: bool,
    /// `cl_shownet`, `cl_showmessages != 0`, `cl_needinstanced != 0` (net-messages.md §2.4).
    pub show_net: f32,
    pub show_messages: bool,
    pub need_instanced: bool,
    /// `net_showpackets` and `net_showdrop != 0` (net-transport.md §9.11).
    pub show_packets: f32,
    pub show_drop: bool,
    /// `fs_startup_timings != 0` (engine-core.md §10).
    pub startup_timings: bool,
    /// `cl_predlog` (not in the original): the prediction log next to the executable.
    pub pred_log: bool,
}

impl ClientConfig {
    pub fn new(name: &str) -> Self {
        let mut userinfo = InfoString::new(gs_common::info::MAX_USERINFO);
        for (k, v) in [
            ("name", name),
            ("topcolor", "0"),
            ("bottomcolor", "0"),
            ("rate", "25000"),
            ("cl_updaterate", "60"),
            ("cl_lw", "1"),
            ("cl_lc", "1"),
            ("cl_dlmax", "512"),
            ("_cl_autowepswitch", "1"),
        ] {
            if let Err(e) = userinfo.set(k, v) {
                log::warn!("userinfo {k}: {e}");
            }
        }
        Self {
            userinfo,
            cmdrate: 60.0,
            updaterate: 60.0,
            cmdbackup: 2,
            resend: 6.0,
            timeout: 60.0,
            dlmax: 512,
            allow_download: true,
            download_ingame: true,
            allow_upload: true,
            hpk_maxsize: 4.0,
            game: "cstrike".into(),
            port: gs_net::socket::DEFAULT_CLIENT_PORT,
            rcon_password: String::new(),
            slist: 10.0,
            multicast_port: 27025,
            multicast_loop: false,
            clock_reset: 0.1,
            fix_time_rate: 7.5,
            no_smooth: false,
            smooth_time: 0.1,
            ideal_pitch_scale: 0.8,
            show_error: false,
            cheats: false,
            fake_lag: 0.0,
            fake_loss: 0.0,
            fix_interpolation_artifacts: true,
            bmodel_interp: true,
            solid_players: true,
            sound_started: true,
            nosound: false,
            show_net: 0.0,
            show_messages: false,
            need_instanced: false,
            show_packets: 0.0,
            show_drop: false,
            startup_timings: false,
            pred_log: false,
        }
    }
}

/// `NET_AdjustLag` / `NET_LagPacket` state of the client socket.
#[derive(Debug, Default)]
struct FakeLag {
    /// `gFakeLag` (ms).
    lag: f64,
    /// The `realtime` of the last adjustment.
    lasttime: f64,
    /// `losscount[NS_CLIENT]`.
    losscount: i32,
    /// Received datagrams with their arrival `realtime`, oldest first ([INFERENCE] order).
    queue: VecDeque<(f64, (SocketAddrV4, Vec<u8>))>,
}

/// The engine's GameUI calls during connect/loading (docs/re/gameui-engine.md §8.2).
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    /// `StartProgressBar(type, steps)`.
    StartProgress { kind: String, steps: i32 },
    /// `ContinueProgressBar(point, 0.0)`.
    ContinueProgress(i32),
    /// `SetProgressBarStatusText`.
    Status(String),
    /// `SetSecondaryProgressBar`.
    SecondaryProgress(f32),
    /// `SetSecondaryProgressBarText`.
    SecondaryText(String),
    /// `LoadingStarted(type, name)`.
    LoadingStarted { kind: String, name: String },
    /// `LoadingFinished(type, name)`.
    LoadingFinished { kind: String, name: String },
    /// `COM_ExplainDisconnection` (`extended`: `COM_ExtendedExplainDisconnection`).
    Explain { text: String, extended: bool },
    /// `CL_Disconnect` finished (`VGuiWrap2_NotifyOfServerDisconnect`).
    ServerDisconnected,
    /// `cls.state` became active (`VGuiWrap2_ConnectToServer`).
    WentActive,
    /// `DemoPlayer_ShowUI`: `IBaseUI::ActivateGameUI` + `IGameUI::ActivateDemoUI`.
    ShowDemoUi,
}

/// The engine's answer for a cvar a server queries (`svc_sendcvarvalue(2)`): the string or
/// the error text (`Bad CVAR request`, `CVAR is privileged`, …).
pub type CvarReply<'a> = &'a dyn Fn(&str) -> String;

#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    /// Console text from the engine or server.
    Print(String),
    /// Text the server wants executed (already split from `svc_stufftext`; not yet filtered).
    StuffText(String),
    Connected(ServerInfo),
    /// `CL_RegisterResources` → `R_NewMap`: the world is ready for the renderer.
    NewMap(String),
    /// The level is loaded and entity updates flow.
    EnteredGame,
    Disconnected(String),
    /// A GameUI progress/loading call.
    Ui(UiEvent),
    /// Everything not consumed by the engine layer (HUD, sounds, effects, ...).
    Message(ServerMessage),
    /// `Con_DPrintf` (shown with `developer` on).
    DevPrint(String),
    /// A demo frame or demo-segment action for the host, in file order.
    Demo(DemoItem),
    /// `Cbuf_AddText` followed by `Cbuf_Execute` (`Host_EndGame`'s `cd stop`).
    Execute(String),
    /// `Cbuf_InsertText` (the demo loop's next `playdemo`).
    InsertText(String),
    /// `Cvar_Set` by the engine (`cl_gg`).
    SetCvar { name: String, value: String },
    /// `Cvar_Set` inside `CL_Move`: the client's copy, the userinfo and `setinfo` are already
    /// done; the console's cvar takes the value without further side effects.
    CvarMirror { name: String, value: String },
    /// The game gauge finished: `quit`.
    Quit,
    /// `Cbuf_AddText` from the engine (an `L` redirect's `connect`).
    AddText(String),
    /// A player's spray customization was replaced: its decals go (`R_DecalRemoveAll(~player)`).
    CustomizationRemoved(usize),
    /// `Con_NPrintf(slot, text)`: a notify line (`cl_showerror`).
    NPrintf { slot: i32, text: String },
}

/// `cl.commands[]` entry.
type CmdSlot = PredCmd;

/// net_graph's view of the client (`cl.frames`, `cl.commands`, `cls.netchan`).
pub struct NetStats<'a> {
    pub incoming_sequence: u32,
    pub outgoing_sequence: u32,
    pub frames: &'a [gs_protocol::state::Frame],
    /// `receivedtime` (-1 dropped) and `latency` per frame slot.
    pub received: &'a [f64; CMD_SLOTS],
    pub latency: &'a [f64; CMD_SLOTS],
    pub commands: &'a [PredCmd; CMD_SLOTS],
    /// `avgkbytespersec` out, in.
    pub flow_kbytes: [f32; 2],
}

pub struct Client {
    fs: FileSystem,
    socket: ClientSocket,
    auth: Box<dyn Authenticator>,
    pub config: ClientConfig,
    state: State,
    server_name: String,
    /// `cls.trueaddress`: the accept's address argument (≤ 31 characters).
    true_address: String,
    server: Option<SocketAddrV4>,
    /// `rcon_adr` / `rcon_command`: the last rcon target and command (kept after use).
    rcon_adr: Option<SocketAddrV4>,
    rcon_command: String,
    /// `CL_Move`'s "WARNING:  Connection Problem" this frame.
    connection_problem: bool,
    connect_time: f64,
    connect_retry: u32,
    build_num: Option<i32>,
    netchan: Option<Netchan>,
    proto: ProtocolState,
    signon: u8,
    /// `cls.secure`: the server's challenge reply said VAC-secure.
    secure: bool,
    server_info: Option<ServerInfo>,
    resources: Option<ResourceList>,
    /// `cl.resourcesneeded` / `cl.resourcesonhand`.
    needed: Vec<Resource>,
    on_hand: Vec<Resource>,
    /// `cls.downloadurl` (`svc_resourcelocation`).
    download_url: String,
    /// `cls.dl.custom`: a customization download (no registration afterwards).
    dl_custom: bool,
    /// `TheDownloadManager` (survives disconnects).
    http: crate::http::HttpDownloads,
    /// `custom.hpk` and its queue.
    hpak: crate::hpak::Hpak,
    /// `cl.resourcelist`: the local logo (`tempdecal.wad`) and its data.
    local_resources: Vec<(Resource, Vec<u8>)>,
    /// `cl.players[i].customdata` per player slot.
    customizations: Vec<Vec<crate::custom::Customization>>,
    /// Requested logos by MD5 and the player they are for (`resource_t.playernum`).
    custom_players: Vec<([u8; 16], usize)>,
    commands: [CmdSlot; CMD_SLOTS],
    last_sent_sequence: Option<u32>,
    /// `cls.nextcmdtime` (a float in the original).
    next_cmd_time: f32,
    msec_carry: f64,
    /// `cl_lerp_smoothed`: the interpolation amount sent as `lerp_msec`, slewed toward
    /// `ex_interp` (a float in the original).
    lerp: f32,
    /// `ex_interp` in seconds.
    pub ex_interp: f64,
    /// Movement/buttons for the next command; `msec`/`lerp_msec`/`lightlevel` are filled in.
    pub next_cmd: UserCmd,
    /// `cl.viewangles` as the host left it this frame (prediction's `cl.simangles`).
    pub viewangles: [f32; 3],
    /// `gFakeLag` and the lagged packets (`fakelag`, `fakeloss`).
    fake_lag: FakeLag,
    /// `cl.light_level`: `R_DrawViewModel`'s `(r+g+b)/3` at the view entity, set by the host
    /// each rendered frame and sent in the next command.
    pub light_level: i32,
    level: Level,
    clock: Clock,
    entities: Entities,
    prediction: Prediction,
    /// `host_frametime` of the current frame.
    frametime: f64,
    events: VecDeque<ClientEvent>,
    /// `cl_events`.
    event_queue: EventQueue,
    now: f64,
    /// `SeedRandomNumberGenerator`'s generator, shared with everything that calls `RandomLong`.
    rng: EngineRng,
    /// `cls.datagram`: unreliable string commands and voice, sent with the next command packet.
    datagram: Vec<u8>,
    /// `cl.frames[i].receivedtime` per frame slot (-1 = dropped) and `cl.parsecount`.
    frame_received: [f64; CMD_SLOTS],
    /// `cl.frames[i].latency`.
    frame_latency: [f64; CMD_SLOTS],
    parsecount: Option<u32>,
    /// `cls.packet_loss` and its once-a-second recalculation time.
    packet_loss: f32,
    packet_loss_recalc: f64,
    /// `gVShake` (outside `cl`: survives `CL_ClearState`, cleared by `R_NewMap`).
    pub shake: ScreenShake,
    /// `cls.demo*`.
    pub demo: Demo,
    /// `host_framecount`.
    host_framecount: i32,
    /// `cl_gg`.
    pub cl_gg: bool,
    /// `Host_Version`'s string (`gg` results).
    version: String,
    /// The HLTV spectator's own `local_state_t` (`CL_InitSpectator`, `CL_MoveSpectatorCamera`).
    spec_state: LocalState,
    /// `g_iUser1..3` as the client DLL holds them, for `HUD_TxferPredictionData`.
    pub hud_users: [i32; 3],
    /// `cl_redirected`: an `L` redirect awaits an HLTV sign-on.
    redirected: bool,
    /// `sys_timescale.value`: an engine `cvar_t` that is never registered (no console access).
    /// `Host_Init`, `CL_Disconnect`, serverinfo and `reconnect` store 1; `svc_timescale` the
    /// server's value in HLTV and demos.
    sys_timescale: f32,
    /// `cl.delta_sequence`: what the last `clc_delta` asked for.
    delta_sequence: Option<u32>,
    /// `cl.cmd`: this frame's command as built (the HLTV camera moves with it).
    cl_cmd: UserCmd,
    /// The local player's state arrived since the last [`Client::take_local_player_update`].
    local_player_update: bool,
    /// `CL_CheckPredictionError`'s static notify-line counter.
    pred_error_counter: i32,
    /// `config.pred_log` as last applied (the log file opens and closes on changes).
    pred_log_on: bool,
    /// `g_pDemoPlayer` (shared with GameUI's demo dialogs; set by the host at `Host_Init`).
    pub demo_player: Option<Rc<RefCell<gs_hltv::player::DemoPlayer>>>,
    /// `g_bDemoPlayerActive`: set by `viewdemo`, cleared only by `playdemo`.
    demo_player_active: bool,
    /// `cl_filterstuffcmd > 0`, for the module's `ValidStuffText` (set by the host).
    pub filter_stuff_cmd: bool,
    /// `cl.serverinfo`: what `fullserverinfo` stored (≤ 511 bytes; cleared by `CL_ClearState`).
    pub serverinfo: String,
    /// The LAN list of `pingservers` / `slist` / `list`.
    pub slist: crate::slist::ServerList,
    /// `cls.challenge` (kept for the multicast unicast fallback).
    challenge: i32,
    /// `cls.passive`: the netchan reads a multicast group (`listen`); survives disconnects.
    passive: bool,
    /// `mcast_listen_adr` / `mcast_hltv_adr`: static, never cleared.
    mcast_listen_adr: SocketAddrV4,
    mcast_hltv_adr: SocketAddrV4,
    /// NS_MULTICAST, opened by the first `listen`.
    mcast_socket: Option<gs_net::socket::ClientSocket>,
    /// `fs_startup_timings`' marks: label and `(float)Sys_FloatTime()`.
    startup_timings: Vec<(&'static str, f32)>,
}

impl Client {
    pub fn new(fs: FileSystem, config: ClientConfig, auth: Box<dyn Authenticator>) -> std::io::Result<Self> {
        let socket = ClientSocket::bind(config.port)?;
        // PM_InitTextureTypes: a missing file leaves every texture type 'C'.
        let rng = EngineRng::from_time();
        let mut prediction = Prediction::new(rng.clone());
        match fs.read("sound/materials.txt") {
            Ok(data) => prediction.textures = TextureTypes::parse(&data),
            Err(e) => log::warn!("sound/materials.txt: {e}"),
        }
        let version = host_version(&fs);
        Ok(Self {
            fs,
            socket,
            auth,
            config,
            state: State::Disconnected,
            server_name: String::new(),
            true_address: String::new(),
            server: None,
            rcon_adr: None,
            rcon_command: String::new(),
            connection_problem: false,
            connect_time: 0.0,
            connect_retry: 0,
            build_num: None,
            netchan: None,
            proto: ProtocolState::default(),
            signon: 0,
            secure: false,
            server_info: None,
            resources: None,
            needed: Vec::new(),
            on_hand: Vec::new(),
            download_url: String::new(),
            dl_custom: false,
            http: crate::http::HttpDownloads::default(),
            hpak: crate::hpak::Hpak::default(),
            local_resources: Vec::new(),
            customizations: vec![Vec::new(); 32],
            custom_players: Vec::new(),
            commands: [CmdSlot::default(); CMD_SLOTS],
            last_sent_sequence: None,
            next_cmd_time: 0.0,
            msec_carry: 0.0,
            lerp: 0.1,
            ex_interp: 0.1,
            next_cmd: UserCmd::default(),
            viewangles: [0.0; 3],
            fake_lag: FakeLag::default(),
            level: Level::default(),
            clock: Clock::default(),
            entities: Entities::default(),
            prediction,
            frametime: 0.0,
            events: VecDeque::new(),
            event_queue: EventQueue::default(),
            now: 0.0,
            rng,
            datagram: Vec::new(),
            frame_received: [0.0; CMD_SLOTS],
            frame_latency: [0.0; CMD_SLOTS],
            parsecount: None,
            packet_loss: 0.0,
            packet_loss_recalc: 0.0,
            shake: ScreenShake::default(),
            demo: Demo::default(),
            host_framecount: 0,
            cl_gg: false,
            version,
            spec_state: LocalState::default(),
            hud_users: [0; 3],
            redirected: false,
            sys_timescale: 1.0,
            delta_sequence: None,
            cl_cmd: UserCmd::default(),
            local_player_update: false,
            pred_error_counter: 0,
            pred_log_on: false,
            demo_player: None,
            demo_player_active: false,
            filter_stuff_cmd: false,
            serverinfo: String::new(),
            slist: crate::slist::ServerList::default(),
            challenge: 0,
            passive: false,
            mcast_listen_adr: SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0),
            mcast_hltv_adr: SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0),
            mcast_socket: None,
            startup_timings: Vec::new(),
            light_level: 0,
        })
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// `IsConnectedToVACSecureServer`: connected or active, and the server is VAC-secure.
    pub fn vac_secure(&self) -> bool {
        matches!(self.state, State::Connected | State::Active) && self.secure
    }

    /// The engine RNG (`RandomLong` / `RandomFloat`).
    pub fn rng(&self) -> &EngineRng {
        &self.rng
    }

    pub fn level(&self) -> &Level {
        &self.level
    }

    /// `cl.time`.
    pub fn time(&self) -> f64 {
        self.clock.time
    }

    /// `cl.oldtime`.
    pub fn old_time(&self) -> f64 {
        self.clock.old_time
    }

    /// `cl_entities` and the level together (the studio renderer mutates entities while it
    /// reads models and player infos).
    pub fn entities_and_level(&mut self) -> (&mut Entities, &mut Level) {
        (&mut self.entities, &mut self.level)
    }

    /// Predicted local player state (`cl.simorg`, `cl.simvel`, ...).
    pub fn sim(&self) -> &SimState {
        &self.prediction.sim
    }

    /// Footstep/landing sounds from first-time predicted movement.
    pub fn take_pmove_sounds(&mut self) -> Vec<PmSound> {
        std::mem::take(&mut self.prediction.sounds)
    }

    /// The pmove physents render-time queries use (world + solid entities), with the world
    /// alone before the first prediction.
    fn with_physents<R>(&self, f: impl FnOnce(&gs_world::World, &[Physent]) -> R) -> Option<R> {
        let world = self.prediction.world.as_ref()?;
        if self.prediction.physents.is_empty() {
            Some(f(world, &[Physent { model: Some(0), solid: 4, ..Physent::default() }]))
        } else {
            Some(f(world, &self.prediction.physents))
        }
    }

    /// `PM_PlayerTrace` with hull `usehull` against the current pmove physents (world + solid
    /// entities); `None` before a map is loaded.
    pub fn trace(&self, start: [f32; 3], end: [f32; 3], usehull: i32, flags: i32) -> Option<PmTrace> {
        self.with_physents(|world, physents| gs_pmove::trace::player_trace(world, physents, usehull, start, end, flags, -1))
    }

    /// `PM_PointContents` against the current pmove physents (empty before a map is loaded).
    pub fn point_contents(&self, p: [f32; 3]) -> i32 {
        self.with_physents(|world, physents| gs_pmove::trace::point_contents(world, physents, p).0).unwrap_or(gs_world::contents::EMPTY)
    }

    /// `PM_WaterEntity`: -1 solid or none, 0 world liquid, else the liquid brush's entity number.
    pub fn water_entity(&self, p: [f32; 3]) -> i32 {
        self.with_physents(|world, physents| gs_pmove::trace::water_entity(world, physents, p)).unwrap_or(-1)
    }

    /// `R_DrawScreenFade`'s colour for this draw (`V_FadeAlpha` runs once per drawn frame):
    /// `(rgba, modulate)`, or `None` when nothing is drawn.
    pub fn screen_fade(&mut self) -> Option<([u8; 4], bool)> {
        let a = self.level.fade.alpha(self.clock.time);
        (a != 0).then(|| self.level.fade.draw_color(a))
    }

    /// `V_ResetShakeFade` from `R_NewMap`: the shake cleared, the fade cleared or, with
    /// `v_dark`, a 5 s black fade-in.
    pub fn new_map_view_reset(&mut self, v_dark: bool) {
        self.shake = ScreenShake::default();
        self.level.fade.new_map(v_dark, self.clock.time);
    }

    /// `EV_TraceTexture(ent, start, end)` for a hit of [`Self::trace`] (`ent` is its physent).
    pub fn trace_hit_texture(&self, ent: i32, start: [f32; 3], end: [f32; 3]) -> Option<String> {
        self.with_physents(|world, physents| gs_pmove::trace::trace_texture(world, physents, ent, start, end).map(str::to_owned)).flatten()
    }

    /// Whether `sound/materials.txt` lists any texture of material `kind` (`PM_IsThereGrassTexture`
    /// asks for `'X'`).
    pub fn has_texture_type(&self, kind: u8) -> bool {
        self.prediction.textures.contains_type(kind)
    }

    /// `physents[ent].info` (the entity number) of a trace hit; 0 for the world or no hit.
    pub fn physent_info(&self, ent: i32) -> i32 {
        usize::try_from(ent).ok().and_then(|i| self.prediction.physents.get(i)).map_or(0, |pe| pe.info)
    }

    /// `physents[ent].origin` when that physent has a `studiomodel` (a trace hit on a non-BBOX
    /// studio entity); `None` otherwise.
    pub fn physent_studio_origin(&self, ent: i32) -> Option<[f32; 3]> {
        let pe = usize::try_from(ent).ok().and_then(|i| self.prediction.physents.get(i))?;
        pe.studiomodel.as_ref().map(|_| pe.origin)
    }

    /// The render-time physents without players (world + solid entities; the world alone before
    /// the first prediction): `EV_SetSolidPlayers` adds nothing with `cl_solid_players 0`.
    pub fn base_physents(&self) -> Vec<Physent> {
        let mut physents = self.prediction.physents.clone();
        if physents.is_empty() {
            physents.push(Physent { model: Some(0), solid: 4, ..Physent::default() });
        }
        physents
    }

    /// `EV_SetUpPlayerPrediction(0, 1)` + `EV_SetSolidPlayers(playernum)` on top of the
    /// render-time physents: player boxes at their rendered positions (the local player at its
    /// packet state). `playernum == -1` keeps every player; otherwise that player (0-based) is
    /// left out (movement.md §B.12).
    pub fn event_physents(&self, playernum: i32) -> Vec<Physent> {
        let mut physents = self.base_physents();
        let local = i32::from(self.proto.playernum);
        for state in &self.proto.current_frame().entities {
            let number = state.number();
            let j = number - 1;
            if !(0..i32::from(self.proto.maxclients)).contains(&j) || state.modelindex() == 0 || j == playernum {
                continue;
            }
            let solid = i32::from(state.solid());
            if solid == 0 {
                continue;
            }
            let Some(ent) = self.entities.get(number as usize) else { continue };
            let (origin, angles) = if j == local { (ent.curstate.origin(), state.angles()) } else { (ent.origin, ent.angles) };
            let hull = usize::try_from(state.usehull()).ok().filter(|&h| h < 4).unwrap_or(0);
            physents.push(Physent {
                player: 1,
                origin,
                model: None,
                studiomodel: None,
                mins: gs_pmove::PLAYER_MINS[hull],
                maxs: gs_pmove::PLAYER_MAXS[hull],
                info: number,
                angles,
                solid,
                skin: 0,
                rendermode: state.rendermode(),
                movetype: state.movetype(),
                team: state.team(),
                classnumber: state.playerclass(),
            });
        }
        physents
    }

    /// `PM_PlayerTrace` with hull `usehull` against explicit physents (see
    /// [`Self::event_physents`]); `None` before a map is loaded.
    pub fn trace_physents(&self, physents: &[Physent], start: [f32; 3], end: [f32; 3], usehull: i32, flags: i32) -> Option<PmTrace> {
        let world = self.prediction.world.as_ref()?;
        Some(gs_pmove::trace::player_trace(world, physents, usehull, start, end, flags, -1))
    }

    /// `PM_TraceTexture(physent, start, end)`: the texture name hit on that physent's model.
    pub fn trace_texture(&self, physents: &[Physent], ent: i32, start: [f32; 3], end: [f32; 3]) -> Option<String> {
        let world = self.prediction.world.as_ref()?;
        gs_pmove::trace::trace_texture(world, physents, ent, start, end).map(str::to_owned)
    }

    /// `PM_FindTextureType` (`sound/materials.txt`): the material char, `'C'` when unlisted.
    pub fn texture_type(&self, name: &str) -> u8 {
        self.prediction.textures.find(name)
    }

    /// The level's BSP spatial data (`cl.worldmodel`).
    pub fn world(&self) -> Option<&Arc<World>> {
        self.prediction.world.as_ref()
    }

    /// `CL_EmitEntities`' gate: active, a valid delta sequence and a valid current frame;
    /// otherwise nothing is linked, no temporary entity updates and no event fires.
    pub fn emit_frame_valid(&self) -> bool {
        self.state == State::Active && self.proto.valid_sequence != 0 && self.proto.current_frame().valid
    }

    /// `CL_FireEvents`: queued events whose time has come, for the client DLL's hooks. Runs
    /// last in `CL_EmitEntities`.
    pub fn fire_events(&mut self) -> Vec<FiredEvent> {
        if !self.emit_frame_valid() {
            return Vec::new();
        }
        self.event_queue.fire(self.clock.time, &self.level.events)
    }

    /// View angles the server forced since the last call (`svc_setangle`, `svc_addangle` yaw).
    pub fn take_view_angles(&mut self) -> (Option<[f32; 3]>, f32) {
        (self.level.set_angles.take(), std::mem::take(&mut self.level.add_yaw))
    }

    pub fn protocol(&self) -> &ProtocolState {
        &self.proto
    }

    /// What net_graph reads: the frame and command rings, receive times and latencies, the
    /// sequences and the flow rates (`None` without a channel).
    pub fn net_stats(&self) -> Option<NetStats<'_>> {
        let chan = self.netchan.as_ref()?;
        Some(NetStats {
            incoming_sequence: chan.incoming_sequence,
            outgoing_sequence: chan.outgoing_sequence,
            frames: &self.proto.frames,
            received: &self.frame_received,
            latency: &self.frame_latency,
            commands: &self.commands,
            flow_kbytes: chan.flow_kbytes(),
        })
    }

    /// `cl_entities`.
    pub fn entities(&self) -> &Entities {
        &self.entities
    }

    pub fn entities_mut(&mut self) -> &mut Entities {
        &mut self.entities
    }

    /// `CL_EmitEntities` linking for this render frame: visible entities in engine order.
    pub fn link_entities(&mut self, simorg: [f32; 3], third_person: bool) -> Vec<(usize, LinkKind)> {
        if !self.emit_frame_valid() {
            return Vec::new();
        }
        let level = &self.level;
        let time = self.clock.time;
        let estimate = |s: &EntityState| level.estimate_frame(s, time);
        let ctx = LinkContext {
            time,
            ex_interp: self.ex_interp,
            maxclients: usize::from(self.proto.maxclients),
            playernum: usize::from(self.proto.playernum),
            view_entity: if level.view_entity > 0 { level.view_entity as usize } else { usize::from(self.proto.playernum) + 1 },
            simorg,
            third_person,
            model_names: &level.models,
            loopback: false,
            estimate_frame: &estimate,
            fix_interpolation_artifacts: self.config.fix_interpolation_artifacts,
            bmodel_interp: self.config.bmodel_interp,
            timedemo: self.demo.timedemo(),
            onground: self.prediction.sim.onground,
            onground_moving: self.prediction.sim.moving,
        };
        self.entities.link(self.proto.current_frame(), &ctx)
    }

    pub fn server_info(&self) -> Option<&ServerInfo> {
        self.server_info.as_ref()
    }

    pub fn netchan(&self) -> Option<&Netchan> {
        self.netchan.as_ref()
    }

    pub fn poll_event(&mut self) -> Option<ClientEvent> {
        self.events.pop_front()
    }

    fn print(&mut self, text: String) {
        log::info!("{}", text.trim_end());
        self.events.push_back(ClientEvent::Print(text));
    }

    fn ui(&mut self, e: UiEvent) {
        self.events.push_back(ClientEvent::Ui(e));
    }

    /// The map's base name for `LoadingStarted`/`LoadingFinished` (`maps/de_dust2.bsp` →
    /// `de_dust2`).
    fn map_base_name(&self) -> String {
        let model = self.server_info.as_ref().map_or("", |i| i.map_model.as_str());
        let file = model.rsplit(['/', '\\']).next().unwrap_or(model);
        file.rsplit_once('.').map_or(file, |(b, _)| b).to_owned()
    }

    /// `connect <address>`: starts the challenge handshake on the next frame.
    pub fn connect(&mut self, address: &str) {
        self.disconnect("connecting elsewhere");
        // cmd_connect: an HLTV connection ends here (it survives disconnects).
        self.proto.spectator = false;
        self.passive = false;
        // cmd_connect zeroes the cl_messages counters (nothing else does but `listen`).
        self.proto.msg_stats = gs_protocol::state::MsgStats::default();
        self.server_name = address.trim().to_owned();
        self.state = State::Connecting;
        // cmd_connect: the 12-step "Connecting" bar.
        self.ui(UiEvent::StartProgress { kind: "Connecting".to_owned(), steps: 12 });
        self.ui(UiEvent::Status("#GameUI_EstablishingConnection".to_owned()));
        self.connect_time = -99999.0;
        self.connect_retry = 0;
        self.build_num = None;
        self.server = None;
    }

    /// `reconnect` (servers send it on map change): restart sign-on on the same netchan.
    pub fn reconnect(&mut self) {
        if self.state <= State::Connecting {
            return;
        }
        // cmd_reconnect while passive: re-listen to the group and nothing else.
        if self.passive {
            let adr: String = self.mcast_listen_adr.to_string().chars().take(127).collect();
            let adr = adr.split([';', '\n']).next().unwrap_or("");
            self.events.push_back(ClientEvent::AddText(format!("listen {adr}\n")));
            return;
        }
        self.signon = 0;
        self.sys_timescale = 1.0;
        self.state = State::Connected;
        self.resources = None;
        if let Some(chan) = self.netchan.as_mut() {
            chan.clear();
            chan.clear_reliable();
        }
        self.send_string_cmd("new");
    }

    /// `cls.servername`: the address string given to `connect`.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// `cls.trueaddress`.
    pub fn true_address(&self) -> &str {
        &self.true_address
    }

    /// `cmd_retry`: `connect` (or `listen` while passive) to `cls.servername` through the
    /// command buffer.
    pub fn retry(&mut self) {
        if self.server_name.is_empty() {
            self.print("Can't retry, no previous connection\n".into());
            return;
        }
        if self.server_name.contains(['\n', ';']) {
            self.print("Invalid command separator in server name, refusing retry\n".into());
            return;
        }
        let verb = if self.passive { "listen" } else { "connect" };
        let command = format!("{verb} {}\n", self.server_name);
        self.events.push_back(ClientEvent::AddText(truncate_encoded(&command, 259).into_owned()));
        self.print(format!("Commencing connection retry to {}\n", self.server_name));
    }

    /// `Host_Error` on a client: `Host_Error: <text>` printed, then `CL_Disconnect` (the
    /// caller abandons the rest of the message, as the original's longjmp does).
    fn host_error(&mut self, text: &str) {
        self.print(format!("Host_Error: {text}\n"));
        self.disconnect(text);
    }

    /// `CL_DemoShutdown` (from `Host_Shutdown`): `demoheader.dmf` closed and deleted.
    pub fn demo_shutdown(&mut self) {
        self.demo.shutdown(&self.fs);
    }

    pub fn disconnect(&mut self, reason: &str) {
        // CL_Disconnect: a recording is finished, a playback closed.
        if self.demo.recording() && self.state >= State::Connecting {
            self.with_ctx(|demo, ctx| demo.cmd_stop(ctx));
        }
        self.stop_playback();
        if self.state >= State::Connecting {
            // A passive client leaves both groups instead of sending `dropclient`;
            // `cls.passive` itself survives.
            if self.passive {
                let (listen, hltv) = (self.mcast_listen_adr, self.mcast_hltv_adr);
                self.leave_group(listen);
                self.leave_group(hltv);
            } else if let (Some(chan), Some(server)) = (self.netchan.as_mut(), self.server) {
                let mut w = MsgWriter::new();
                clc::stringcmd(&mut w, "dropclient\n");
                let now = self.now;
                let packets: Vec<Vec<u8>> = (0..3).filter_map(|_| chan.transmit(w.as_bytes(), now).ok()).collect();
                for packet in packets {
                    let _ = self.socket.send(server, &packet);
                    self.trace_outgoing(&packet);
                }
            }
            self.auth.end_session();
            self.events.push_back(ClientEvent::Disconnected(reason.to_owned()));
        }
        self.state = State::Disconnected;
        self.sys_timescale = 1.0;
        self.netchan = None;
        self.signon = 0;
        self.server_info = None;
        self.resources = None;
        // CL_ClearState's resource part and the players' customizations.
        self.clear_resources();
        for c in &mut self.customizations {
            c.clear();
        }
        self.custom_players.clear();
        self.last_sent_sequence = None;
        self.reset_protocol();
        self.datagram.clear();
        self.parsecount = None;
        self.level = Level::default();
        self.clear_cl_commands();
        self.clock = Clock::default();
        self.entities.clear();
        self.prediction.reset();
        self.event_queue.clear();
        // CL_Disconnect's last statements: VGuiWrap2_DisconnectFromServer and
        // NotifyOfServerDisconnect (the menu comes back, the progress bar stops).
        self.ui(UiEvent::ServerDisconnected);
    }

    /// `CL_ClearState`'s memset of `cl`: the command ring, `cl.cmd`, `cl.viewangles` and the
    /// frames' receive times start over (`cls.lastoutgoingcommand` lives in `cls` and stays).
    fn clear_cl_commands(&mut self) {
        self.commands = [CmdSlot::default(); CMD_SLOTS];
        self.cl_cmd = UserCmd::default();
        self.frame_received = [0.0; CMD_SLOTS];
        self.viewangles = [0.0; 3];
    }

    /// Queues a reliable `clc_stringcmd` the engine writes itself (`new`, `sendres`, …).
    pub fn send_string_cmd(&mut self, text: &str) {
        if let Some(chan) = self.netchan.as_mut() {
            let mut w = MsgWriter::new();
            clc::stringcmd(&mut w, text);
            chan.reliable().extend_from_slice(w.as_bytes());
        }
    }

    /// `Cmd_ForwardToServerInternal` into the reliable stream: nothing during a demo playback,
    /// and a command that does not fit `cls.netchan.message` is dropped.
    pub fn forward_string_cmd(&mut self, text: &str) {
        if self.demo.playing() {
            return;
        }
        let Some(chan) = self.netchan.as_mut() else { return };
        let mut w = MsgWriter::new();
        clc::stringcmd(&mut w, text);
        if chan.reliable().len() + w.len() > gs_net::netchan::MAX_MSGLEN {
            return;
        }
        chan.reliable().extend_from_slice(w.as_bytes());
    }

    /// `pfnServerCmd` / `pfnServerCmdUnreliable`: `"cmd %s"` (2047 characters) tokenized and
    /// forwarded like the console's `cmd`.
    pub fn server_cmd(&mut self, text: &str, unreliable: bool) {
        let mut line = format!("cmd {text}");
        truncate_encoded_in_place(&mut line, 2047);
        let args = gs_common::token::tokenize(&line);
        let Some(out) = gs_console::forward_text(&args) else { return };
        if self.state < State::Connected {
            self.print(format!("Can't \"{}\", not connected\n", args.arg(0)));
            return;
        }
        if self.demo.playing() {
            return;
        }
        if unreliable {
            self.send_string_cmd_unreliable(&out);
        } else {
            self.forward_string_cmd(&out);
        }
    }

    /// `Parse_svc_sendcvarvalue(2)`'s reliable write: `clc_cvarvalue(2)` with `value`.
    fn reply_cvar(&mut self, request_id: Option<i32>, name: &str, value: &str) {
        let Some(chan) = self.netchan.as_mut() else { return };
        let mut w = MsgWriter::new();
        match request_id {
            Some(id) => clc::cvar_value2(&mut w, id, name, value),
            None => clc::cvar_value(&mut w, value),
        }
        chan.reliable().extend_from_slice(w.as_bytes());
    }

    /// `pfnServerCmdUnreliable` / `Cmd_ForwardToServerUnreliable`: a `clc_stringcmd` in
    /// `cls.datagram`, sent with the next command packet; `false` when it does not fit.
    pub fn send_string_cmd_unreliable(&mut self, text: &str) -> bool {
        if self.netchan.is_none() {
            return false;
        }
        let mut w = MsgWriter::new();
        clc::stringcmd(&mut w, text);
        if self.datagram.len() + w.len() > DATAGRAM_SIZE {
            return false;
        }
        self.datagram.extend_from_slice(w.as_bytes());
        true
    }

    /// `CL_AddVoiceToDatagram`: the voice engine's compressed mic data as `clc_voicedata`
    /// (in game only; dropped when it would overflow `cls.datagram`).
    pub fn add_voice_to_datagram(&mut self, voice: &mut dyn VoiceSend) {
        voice_into_datagram(&mut self.datagram, self.state, voice);
    }

    /// `CL_CheckClientState`, at the top of the host frame (before `Cbuf_Execute`,
    /// `HUD_UpdateClientData` and `CL_Move`): signon 2 makes the client active.
    pub fn check_client_state(&mut self) {
        if matches!(self.state, State::Connected | State::Uninitialized) && self.signon == 2 {
            self.state = State::Active;
            self.print_startup_timings();
            // VGuiWrap2_ConnectToServer at the moment the client goes active.
            self.ui(UiEvent::WentActive);
            self.events.push_back(ClientEvent::EnteredGame);
        }
    }

    /// `fs_startup_timings`: a mark (`StartupTimingMark`) while the cvar is on.
    pub(crate) fn startup_mark(&mut self, label: &'static str) {
        if self.config.startup_timings {
            self.startup_timings.push((label, sys_float_time()));
        }
    }

    /// `CL_CheckClientState`'s table on going active: the total, then each mark's time since
    /// the one before, the last mark left out like the original's loop does.
    fn print_startup_timings(&mut self) {
        let (Some(&(_, first)), Some(&(_, last))) = (self.startup_timings.first(), self.startup_timings.last()) else { return };
        if !self.config.startup_timings {
            return;
        }
        let mut text = format!("Startup timings ({:.2} total)\n    0.00    Startup\n", last - first);
        let marks = &self.startup_timings;
        for i in 1..marks.len() - 1 {
            text.push_str(&format!("    {:.2}    {}\n", marks[i].1 - marks[i - 1].1, marks[i].0));
        }
        self.print(text);
    }

    /// A host frame's `CL_Move`: the command for this frame (sent unless held back). `now` is
    /// the engine clock in seconds; `weapons` is the client DLL's `HUD_PostRunCmd`, `voice` the
    /// voice engine's send side. The client DLL's `HUD_Frame` runs next, then
    /// [`Client::receive`].
    pub fn send(&mut self, now: f64, frametime: f64, weapons: &mut dyn PostRunCmd, voice: &mut dyn VoiceSend) {
        self.now = now;
        self.frametime = frametime;
        self.auth.run_frame();
        self.send_commands(frametime, weapons, voice);
        self.drain_netchan_prints();
    }

    /// The netchan's `Con_DPrintf` lines as developer prints.
    fn drain_netchan_prints(&mut self) {
        let Some(chan) = self.netchan.as_mut() else { return };
        for text in std::mem::take(&mut chan.dprints) {
            self.events.push_back(ClientEvent::DevPrint(text));
        }
    }

    /// The rest of the client frame: the clock, `CL_ReadPackets` (with the re-prediction a new
    /// packet brings), connection retries, HTTP downloads, the timeout and `CL_EmitEntities`'
    /// clock rules.
    pub fn receive(&mut self, weapons: &mut dyn PostRunCmd, cvars: CvarReply) {
        if self.config.pred_log != self.pred_log_on {
            self.pred_log_on = self.config.pred_log;
            self.prediction.log = if self.pred_log_on { crate::predlog::PredLog::open() } else { None };
            if let Some(log) = self.prediction.log.as_mut() {
                // The state the log starts from: the server and the movevars already in force.
                log.note(self.now, &format!("SERVER {} ({:?})", self.server_name, self.server));
                log.movevars(self.now, &self.level.movevars);
                let line = format!("cl_predlog: writing {}\n", log.path.display());
                self.print(line);
            }
        }
        self.clock.reset = self.config.clock_reset;
        self.clock.fix_rate = self.config.fix_time_rate;
        self.clock.advance(self.frametime);
        let incoming = self.netchan.as_ref().map(|c| c.incoming_sequence);
        self.read_packets(cvars);
        // CL_ReadPackets' tail: the cl_timeout check (not while a demo plays), then cl_shownet's
        // newline once per frame unless the connection just timed out.
        let timed_out = !self.demo.playing() && self.check_timeout();
        if self.config.show_net != 0.0 && !timed_out {
            self.print("\n".into());
        }
        // CL_RedoPrediction: a new packet re-runs every unacknowledged command, then repeats
        // CL_CheckPredictionError's body.
        if self.netchan.as_ref().map(|c| c.incoming_sequence) != incoming {
            self.run_prediction(true, weapons);
            if let Some(chan) = self.netchan.as_ref() {
                let (maxclients, playernum) = (usize::from(self.proto.maxclients), usize::from(self.proto.playernum));
                let at = (self.now, playernum, "redo");
                check_prediction_error(&mut self.prediction, self.proto.current_frame(), chan, maxclients, &self.config, &mut self.pred_error_counter, &mut self.events, at);
            }
        }
        self.check_resend();
        // CL_HTTPUpdate.
        self.http_update();
        self.drain_netchan_prints();
        // CL_EmitEntities: two svc_time with the same server time (a paused server) pin
        // cl.time to it, so the client clock and everything timed by it stand still; the
        // interpolation fraction goes to the last command for net_graph.
        if self.state == State::Active && self.proto.valid_sequence != 0 && self.proto.current_frame().valid {
            // Timedemo always snaps; a playback keeps the frame delta below the snapped time.
            let frame_lerp = if self.proto.mtime[0] - self.proto.mtime[1] == 0.0 || self.demo.timedemo {
                let dt = self.clock.time - self.clock.old_time;
                self.clock.time = self.proto.mtime[0];
                if self.demo.playing() {
                    self.clock.old_time = self.proto.mtime[0] - f64::from(dt as f32);
                }
                1.0
            } else if self.ex_interp <= 0.001 {
                1.0
            } else {
                (self.clock.time - self.proto.mtime[0]) / self.ex_interp
            };
            if let Some(chan) = &self.netchan {
                self.commands[(chan.outgoing_sequence.wrapping_sub(1) & UPDATE_MASK) as usize].frame_lerp = frame_lerp as f32;
            }
        }
    }

    /// `CL_PredictMove`, then the view-model animations the weapon code started.
    fn run_prediction(&mut self, repredicting: bool, weapons: &mut dyn PostRunCmd) {
        // CL_PredictMove does nothing while cl.intermission is set (simorg & co. freeze).
        if self.state != State::Active || self.proto.valid_sequence == 0 || self.demo.playing() || self.spectator() || self.level.intermission != 0 {
            return;
        }
        let Some(chan) = self.netchan.as_ref() else { return };
        let maxclients = usize::from(self.proto.maxclients);
        let input = PredictInput {
            frame: self.proto.current_frame(),
            entities: &self.entities,
            model_names: &self.level.models,
            studio_models: &self.level.studio,
            movevars: &self.level.movevars,
            maxclients,
            playernum: usize::from(self.proto.playernum),
            incoming_acknowledged: chan.incoming_acknowledged,
            outgoing_sequence: chan.outgoing_sequence,
            realtime: self.now,
            frametime: self.frametime,
            interp_time: (self.clock.time - self.ex_interp) as f32,
            cl_time: self.clock.time,
            event_names: &self.level.events,
            viewangles: self.viewangles,
            no_smooth: self.config.no_smooth,
            smooth_time: self.config.smooth_time,
            ideal_pitch_scale: self.config.ideal_pitch_scale,
            solid_players: self.config.solid_players,
        };
        self.prediction.predict(&input, &mut self.commands, repredicting, &mut self.event_queue, weapons);
        if std::mem::take(&mut self.prediction.smooth_time_reset) {
            self.events.push_back(ClientEvent::SetCvar { name: "cl_smoothtime".into(), value: "0.1".into() });
        }
        // CL_DemoEvent / CL_DemoAnim / CL_DemoWriteSound of the first-time runs.
        for r in std::mem::take(&mut self.prediction.demo_log) {
            let (now, frame) = (self.now, self.host_framecount);
            match r {
                PredRecord::Event { flags, index, delay, args } => self.demo.record_event(flags, index, delay, &args, now, frame),
                PredRecord::Anim { sequence, body } => self.demo.record_anim(sequence, body, now, frame),
                PredRecord::Sound(s) => self.demo.record_sound(s.channel, &s.sample, s.volume, s.attenuation, 0, 100, now, frame),
            }
        }
        for (sequence, body) in std::mem::take(&mut self.prediction.weapon_anims) {
            self.level.set_weapon_anim(sequence, body);
        }
        // ItemPostFrame's resume zoom: EF_NODRAW on GetViewEntity()'s baseline and curstate.
        if std::mem::take(&mut self.prediction.hide_view_entity) {
            const EF_NODRAW: i32 = 0x80;
            let view = if self.level.view_entity > 0 { self.level.view_entity as usize } else { usize::from(self.proto.playernum) + 1 };
            if let Some(b) = self.proto.baselines.get_mut(view) {
                let e = b.effects() | EF_NODRAW;
                b.set_effects(e);
            }
            if let Some(ent) = self.entities.get_mut(view) {
                let e = ent.curstate.effects() | EF_NODRAW;
                ent.curstate.set_effects(e);
            }
        }
    }

    /// The `cl_timeout` check; true when it disconnected.
    fn check_timeout(&mut self) -> bool {
        let timed_out = self
            .netchan
            .as_ref()
            .is_some_and(|c| self.state >= State::Connected && self.now - c.last_received > self.config.timeout);
        if timed_out {
            self.disconnect("Server connection timed out");
        }
        timed_out
    }

    // ------------------------------------------------------------------ handshake

    fn resolve(&self) -> Option<SocketAddrV4> {
        string_to_adr(&self.server_name)
    }

    /// The address of the server being connected or connected to.
    pub fn server_addr(&self) -> Option<SocketAddrV4> {
        self.server
    }

    /// Whether `CL_Move` raised "WARNING:  Connection Problem" since the last call.
    pub fn take_connection_problem(&mut self) -> bool {
        std::mem::take(&mut self.connection_problem)
    }

    /// `cl_predlog`'s frame line, with the view origin the frame renders from.
    pub fn pred_log_view(&mut self, vieworg: [f32; 3]) {
        let (now, frametime) = (self.now, self.frametime);
        self.prediction.log_view(now, frametime, vieworg);
    }

    /// `host_version()` (`Exe version` text).
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The `cl_messages` counters.
    pub fn msg_stats(&self) -> gs_protocol::state::MsgStats {
        self.proto.msg_stats
    }

    /// `cmd_delta_stats`'s text.
    pub fn delta_stats_text(&self) -> String {
        self.proto.delta_stats_text()
    }

    /// `cmd_delta_clear`'s reset.
    pub fn clear_delta_stats(&self) {
        self.proto.clear_delta_stats();
    }

    /// `hpklist` / `hpkremove` / `hpkval` / `hpkextract`: the queued logos are flushed into
    /// their pack first (`HPAK_FlushHostQueue`).
    pub fn hpak_command(&mut self, name: &str, argv: &[&str]) -> String {
        self.hpak.flush(&self.fs);
        crate::hpak::Hpak::command(&self.fs, name, argv)
    }
}

/// The host frame's end, the HLTV state and the spectator camera.
impl Client {
    /// `host_framecount`.
    pub fn host_framecount(&self) -> i32 {
        self.host_framecount
    }

    /// The end of `_Host_Frame`: `CL_GGSpeeds(t)` while `cl_gg` is on, `host_framecount++`, then
    /// `CL_AdjustClock` (a negative `cl_fixtimerate` is written back as 7.5).
    pub fn end_frame(&mut self, t: f32, console_up: bool) {
        if self.cl_gg {
            self.demo.gg_speeds(t, self.host_framecount, console_up, self.now);
        }
        self.host_framecount = self.host_framecount.wrapping_add(1);
        if self.clock.fix_rate < 0.0 {
            self.clock.fix_rate = 7.5;
            self.engine_cvar_set("cl_fixtimerate", "7.500000");
        }
        self.clock.adjust();
    }

    /// `cls.spectator`: an HLTV connection (or HLTV demo).
    pub fn spectator(&self) -> bool {
        self.proto.spectator
    }

    /// `sys_timescale.value` (`_Host_Frame`'s clock and the sound start functions).
    pub fn sys_timescale(&self) -> f32 {
        self.sys_timescale
    }

    /// Whether `HUD_ProcessPlayerState` ran for the local player since the last call.
    pub fn take_local_player_update(&mut self) -> bool {
        std::mem::take(&mut self.local_player_update)
    }

    /// A fresh `cl` protocol state; `cls.spectator` and the `cl_messages` counters live in
    /// `cls` and survive it.
    fn reset_protocol(&mut self) {
        let (spectator, msg_stats) = (self.proto.spectator, self.proto.msg_stats);
        self.proto = ProtocolState::default();
        self.proto.spectator = spectator;
        self.proto.msg_stats = msg_stats;
    }

    /// `CL_MoveSpectatorCamera` (every frame after `CAM_Think`): the HLTV camera pmove with
    /// this frame's command; `jump` is the HUD's `iJumpSpectator`.
    pub fn move_spectator_camera(&mut self, jump: &mut Option<([f32; 3], [f32; 3])>, weapons: &mut dyn PostRunCmd) {
        if self.state != State::Active || !self.spectator() {
            return;
        }
        let Some(chan) = self.netchan.as_ref() else { return };
        let input = PredictInput {
            frame: self.proto.current_frame(),
            entities: &self.entities,
            model_names: &self.level.models,
            studio_models: &self.level.studio,
            movevars: &self.level.movevars,
            maxclients: usize::from(self.proto.maxclients),
            playernum: usize::from(self.proto.playernum),
            incoming_acknowledged: chan.incoming_acknowledged,
            outgoing_sequence: chan.outgoing_sequence,
            realtime: self.now,
            frametime: self.frametime,
            interp_time: (self.clock.time - self.ex_interp) as f32,
            cl_time: self.clock.time,
            event_names: &self.level.events,
            viewangles: self.viewangles,
            no_smooth: self.config.no_smooth,
            smooth_time: self.config.smooth_time,
            ideal_pitch_scale: self.config.ideal_pitch_scale,
            solid_players: self.config.solid_players,
        };
        let mut state = self.spec_state;
        self.prediction.move_spectator(&input, &mut state, &self.cl_cmd, jump, &mut self.event_queue, weapons);
        self.spec_state = state;
        self.prediction.demo_log.clear();
    }
}

/// `Host_Version`: `PatchVersion` of `steam.inf` (default `1.0.1.4`) with the Steam
/// filesystem tag.
fn host_version(fs: &FileSystem) -> String {
    let patch = fs
        .read("steam.inf")
        .ok()
        .and_then(|d| {
            String::from_utf8_lossy(&d).lines().find_map(|l| l.trim().strip_prefix("PatchVersion=").map(|v| v.trim().to_owned()))
        })
        .unwrap_or_else(|| "1.0.1.4".to_owned());
    format!("{patch}/Stdio")
}

/// `Sys_FloatTime`: seconds on a monotonic clock started by the first call (as float).
fn sys_float_time() -> f32 {
    static START: std::sync::LazyLock<std::time::Instant> = std::sync::LazyLock::new(std::time::Instant::now);
    START.elapsed().as_secs_f64() as f32
}

/// `NET_StringToAdr`: `host[:port]` (default port 27015, `local` = localhost), IPv4 only.
fn string_to_adr(name: &str) -> Option<SocketAddrV4> {
    let name = if name.eq_ignore_ascii_case("local") { "localhost" } else { name };
    let with_port = if name.contains(':') { name.to_owned() } else { format!("{name}:{DEFAULT_PORT}") };
    with_port.to_socket_addrs().ok()?.find_map(|a| match a {
        SocketAddr::V4(v4) => Some(v4),
        SocketAddr::V6(_) => None,
    })
}

/// `NET_StringToAdr` as `listen` uses it: no `:port` leaves the port 0.
fn string_to_adr_no_port(name: &str) -> Option<SocketAddrV4> {
    let with_port = if name.contains(':') { name.to_owned() } else { format!("{name}:0") };
    with_port.to_socket_addrs().ok()?.find_map(|a| match a {
        SocketAddr::V4(v4) => Some(v4),
        SocketAddr::V6(_) => None,
    })
}

/// `CL_CheckPredictionError` (its body is repeated at the end of `CL_RedoPrediction`); with
/// `cl_showerror` the error goes to notify line `(++counter & 3) + 10` (`Con_NPrintf`, no
/// console line). `(now, playernum, phase)` label the `cl_predlog` line.
#[allow(clippy::too_many_arguments)]
fn check_prediction_error(
    prediction: &mut Prediction,
    frame: &gs_protocol::state::Frame,
    chan: &Netchan,
    maxclients: usize,
    config: &ClientConfig,
    counter: &mut i32,
    events: &mut VecDeque<ClientEvent>,
    (now, playernum, phase): (f64, usize, &str),
) {
    prediction.log_check(now, phase, frame, chan.incoming_sequence, chan.incoming_acknowledged, chan.outgoing_sequence, playernum);
    let error = prediction.check_error(frame, chan.incoming_acknowledged, chan.incoming_sequence, maxclients, config.smooth_time);
    if let Some(len) = error.filter(|_| config.show_error) {
        *counter = counter.wrapping_add(1);
        events.push_back(ClientEvent::NPrintf { slot: (*counter & 3) + 10, text: format!("pred. error {len:.3} units") });
    }
}

/// `CL_AddVoiceToDatagram`: in game, the compressed mic data as `clc_voicedata` when
/// `len + cursize + 3 < maxsize`.
fn voice_into_datagram(datagram: &mut Vec<u8>, state: State, voice: &mut dyn VoiceSend) {
    if state != State::Active {
        return;
    }
    let mut buf = [0u8; 2048];
    let len = voice.compressed(&mut buf).min(buf.len());
    if len > 0 && len + datagram.len() + 3 < DATAGRAM_SIZE {
        let mut w = MsgWriter::new();
        clc::voicedata(&mut w, &buf[..len]);
        datagram.extend_from_slice(w.as_bytes());
    }
}

/// `IsSafeFileToDownload` (docs/re/engine-core.md §1.8) with an allowlist instead of its
/// blocklist, plus Windows device names. Only the formats the game reads are written: a
/// blocklist misses types Windows acts on by itself (`.url`/`.scf`/`.library-ms` make Explorer
/// authenticate to the server's SMB share just by showing the folder) or on a click (`.lnk`,
/// `.hta`, `.ps1`, `.cmd`, `.scr`, `.msi`, `.reg`, …). Nothing goes into `resource/` either:
/// the download directory outranks `valve`, and a GameUI layout from there could put an
/// `engine <command>` button, run with local privileges, into a dialog. docs/deviations.md
/// "Security".
fn is_safe_download(name: &str) -> bool {
    const ALLOWED: [&str; 11] = ["bsp", "mdl", "spr", "wav", "mp3", "wad", "tga", "bmp", "txt", "res", "sc"];
    // The engine's own files, and the menus whose entries run as local commands.
    const PROTECTED: [&str; 6] = ["halflife.wad", "pak0.pak", "xeno.wad", "commandmenu.txt", "spectatormenu.txt", "spectcammenu.txt"];
    let lower = truncate_encoded(name, 259).to_ascii_lowercase();
    let single_dot = lower.find('.') == lower.rfind('.');
    let extension = lower.rsplit_once('.').map(|(_, ext)| ext);
    // Windows drops a component's trailing spaces and dots: `resource /` is `resource/`.
    let in_resource = lower.split(['/', '\\']).find(|p| !p.is_empty()).is_some_and(|p| p.trim_end_matches([' ', '.']) == "resource");
    !lower.is_empty()
        && !lower.contains(':')
        && !lower.contains("..")
        && !lower.contains('~')
        && !lower.starts_with(['/', '\\'])
        && single_dot
        && extension.is_some_and(|ext| ALLOWED.contains(&ext))
        && !PROTECTED.iter().any(|p| lower.contains(p))
        && !in_resource
        && !has_device_name(&lower)
}

/// A path component naming a Windows device (`con`, `nul`, `com1`, …, with or without an
/// extension): opening it for writing opens the device (a serial port can block the game).
fn has_device_name(lower: &str) -> bool {
    lower.split(['/', '\\']).any(|part| {
        let stem = part.split('.').next().unwrap_or("").trim_end_matches(' ');
        let numbered = stem.len() == 4 && (stem.starts_with("com") || stem.starts_with("lpt")) && stem.as_bytes()[3].is_ascii_digit();
        numbered || matches!(stem, "con" | "prn" | "aux" | "nul" | "conin$" | "conout$" | "clock$")
    })
}

#[cfg(test)]
mod tests {
    use super::is_safe_download;

    #[test]
    fn download_name_filter() {
        assert!(is_safe_download("maps/de_custom.bsp"));
        assert!(is_safe_download("sound\\misc\\x.wav"));
        assert!(is_safe_download("events/ak47.sc"));
        assert!(is_safe_download("sound/console.wav"), "a device name only as the whole stem");
        assert!(is_safe_download("models/comx.mdl"));
        for bad in ["../x.wav", "c:/x.wav", "/x.wav", "a.b.wav", "maps/x.cfg", "x.dll.bsp", "models/a~b.mdl", "halflife.wad", "README"] {
            assert!(!is_safe_download(bad), "{bad}");
        }
        // Types Windows runs or Explorer resolves (an SMB icon fetch leaks the NTLM hash).
        for bad in ["sound/x.url", "maps/x.scf", "x.library-ms", "x.lnk", "x.hta", "x.ps1", "x.cmd", "x.scr", "x.msi", "x.reg", "x.js", "x.wav.", "sound/x.wav/"] {
            assert!(!is_safe_download(bad), "{bad}");
        }
        for device in ["sound/con.wav", "NUL", "maps/com1.bsp", "aux/x.wav", "sound\\LPT9.wav", "gfx/conout$.tga"] {
            assert!(!is_safe_download(device), "{device}");
        }
        assert!(is_safe_download("maps/de_custom.res") && is_safe_download("sprites/hud.txt"));
        assert!(!is_safe_download("commandmenu.txt") && !is_safe_download("SpectatorMenu.txt"));
        for ui in ["resource/LoadingDialog.res", "RESOURCE\\ui\\x.res", "resource /x.res", "resource/x.tga"] {
            assert!(!is_safe_download(ui), "{ui}");
        }
    }
}
