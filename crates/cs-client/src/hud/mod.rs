//! The CS client HUD (docs/re/client-hud.md §4–6): `CHud` with its `hud.txt` sprite list and
//! number primitives, the user messages and commands it hooks, and its elements in draw order.
//! Everything is drawn through the engine 2D API ([`gs_draw::Screen`]).

mod ammo;
mod chat;
mod cmdmenu;
mod health;
mod message;
mod money;
mod msg;
mod notices;
mod scope;
mod status;
mod statusbar;
mod observer;
mod overview;
mod spectator;
mod specgui;
mod viewport;
mod voice;

pub use observer::{Observer, ObserverRequests};
pub use overview::{OverviewBatch, OverviewPass, OverviewTexture, TriVertex, decode_map_bmp, map_tiles};
pub use spectator::Overview;

use gs_common::token::{Args, com_parse};
use gs_draw::titles::TextMessage;
use gs_draw::{Rect, Screen, SprHandle};

pub use ammo::SelectionOutcome;
pub use msg::MsgReader;

use crate::menu::TextMenu;

/// `RGB_YELLOWISH`.
pub const YELLOWISH: [i32; 3] = [255, 160, 0];
/// `RGB_REDISH`.
pub const RED: [i32; 3] = [250, 0, 0];
/// "Empty" weapon/ammo colour.
pub const EMPTY: [i32; 3] = [255, 16, 16];

/// `m_iHideHUDDisplay` bits.
pub mod hide {
    pub const WEAPONS: i32 = 0x01;
    pub const FLASHLIGHT: i32 = 0x02;
    pub const ALL: i32 = 0x04;
    pub const HEALTH: i32 = 0x08;
    pub const TIMER: i32 = 0x10;
    pub const MONEY: i32 = 0x20;
    pub const CROSSHAIR: i32 = 0x40;
}

/// `DHN_*` flags of `DrawHudNumber`.
pub mod dhn {
    pub const DRAWZERO: i32 = 1;
    pub const TWO_DIGITS: i32 = 2;
    pub const THREE_DIGITS: i32 = 4;
    pub const FILLZERO: i32 = 8;
}

/// `hud_player_info_t` (the fields the HUD reads).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfo {
    pub name: String,
    pub thisplayer: bool,
    /// Latency in ms (`svc_pings`).
    pub ping: i32,
    /// Userinfo `*bot` is set.
    pub bot: bool,
}

/// The engine functions the HUD calls (`cl_enginefunc_t` subset).
pub trait HudEngine {
    fn screen(&mut self) -> &mut Screen;
    /// `SPR_Load`.
    fn spr_load(&mut self, name: &str) -> SprHandle;
    /// A VGUI2 image (`gfx/vgui/...` without extension).
    fn load_image(&mut self, name: &str) -> Option<gs_draw::Image>;
    /// `COM_LoadFile` through the game search path.
    fn read_file(&mut self, path: &str) -> Option<Vec<u8>>;
    fn cvar(&self, name: &str) -> f32;
    fn cvar_string(&self, name: &str) -> String;
    /// `pfnClientCmd`.
    fn client_cmd(&mut self, text: &str);
    /// `pfnServerCmd`.
    fn server_cmd(&mut self, text: &str);
    /// `pfnConsolePrint`.
    fn console_print(&mut self, text: &str);
    /// `pfnGetPlayerInfo` (1-based; `None` for empty slots).
    fn player_info(&self, index: i32) -> Option<PlayerInfo>;
    /// `pfnPlaySoundByName`.
    fn play_sound(&mut self, name: &str, volume: f32);
    /// `pfnPlaySoundByNameAtPitch` (`!` names are sentences).
    fn play_sound_pitch(&mut self, name: &str, volume: f32, pitch: i32);
    /// `pfnPlaySoundVoiceByName`.
    fn play_sound_voice(&mut self, name: &str, volume: f32, pitch: i32);
    /// `pfnTextMessageGet` (titles.txt).
    fn text_message(&self, name: &str) -> Option<TextMessage>;
    /// `GetMaxClients`.
    fn max_clients(&self) -> i32;
    /// `IsDemoPlayingBack` (`pDemoAPI->IsPlayingback`).
    fn demo_playing_back(&self) -> bool {
        false
    }
    /// `pfnGetLevelName` (`maps/<name>.bsp`).
    fn level_name(&self) -> String;
    /// `LocalPlayerInfo_ValueForKey`.
    fn local_userinfo(&self, key: &str) -> Option<String>;
    /// `pfnRandomLong` (the engine generator).
    fn random_long(&mut self, lo: i32, hi: i32) -> i32;
    /// `EV_HLDM_CreateLight`'s `CL_AllocDlight(0)`: a dlight dying at `die` (`cl.time` units).
    fn create_light(&mut self, origin: [f32; 3], radius: f32, color: [u8; 3], die: f32);
    /// `GetPlayerUniqueID(i)` (1-based): the 16-byte hashed cdkey.
    fn player_unique_id(&self, index: i32) -> Option<[u8; 16]>;
    /// `pfnServerCmdUnreliable`.
    fn server_cmd_unreliable(&mut self, text: &str);
    /// `<gamedir>/<name>` as the client DLL `fopen`s it (relative to the install directory).
    fn game_dir_file(&self, name: &str) -> std::path::PathBuf;
    /// `GetClientTime` (`cl.time`).
    fn client_time(&self) -> f32;
    /// `pfnCvar_Set` / `Cvar_SetValue`.
    fn cvar_set(&mut self, name: &str, value: &str);
    /// A direct `cvar->value` write: the string keeps its old text.
    fn cvar_set_value(&mut self, _name: &str, _value: f32) {}
    /// `IsSpectateOnly` (`cls.spectator`: an HLTV connection).
    fn spectate_only(&self) -> bool {
        false
    }
    /// `GetEntityByIndex`.
    fn entity(&self, _index: i32) -> Option<crate::view::SpecEntity> {
        None
    }
    /// `PM_TraceLine(start, end, PM_TRACELINE_ANYVISIBLE, hull 2, ignore)`.
    fn trace_line(&self, _start: [f32; 3], end: [f32; 3], _ignore: i32) -> HudTrace {
        HudTrace { fraction: 1.0, endpos: end, ent: -1 }
    }
    /// The world model's entity lump (`UTIL_FindEntityInMap`).
    fn map_entities(&self) -> Option<String> {
        None
    }
    /// `pEventAPI->EV_PlaySound`.
    #[allow(clippy::too_many_arguments)]
    fn ev_play_sound(&mut self, _ent: i32, _origin: [f32; 3], _channel: i32, _sample: &str, _volume: f32, _attn: f32, _flags: i32, _pitch: i32) {}
    /// `pfnFilteredClientCmd`.
    fn filtered_client_cmd(&mut self, _text: &str) {}
    /// `GetVGUI2KeyCodeForBind` (`cle_Key_LookupBinding`): the lowest key whose binding, a
    /// leading `+` stripped, starts with `binding`.
    fn key_for_binding(&self, _binding: &str) -> Option<u8> {
        None
    }
    /// `CClientMOTD`'s HTML control: navigate to `url` (the page arrives through the viewport's
    /// `motd_page`).
    fn motd_navigate(&mut self, _url: &str) {}
    /// `motd_temp.html` in GAMECONFIG: written and loaded with `Some(html)`, deleted with `None`.
    fn motd_temp_file(&mut self, _html: Option<&str>) {}
    /// The engine's browser shows the pages itself over the `Message` control
    /// (`Viewport::motd_rect`); the panel then draws no text there.
    fn motd_shows_pages(&self) -> bool {
        false
    }
}

/// `pmtrace_t` fields the HUD reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudTrace {
    pub fraction: f32,
    pub endpos: [f32; 3],
    /// The physent hit (-1 none).
    pub ent: i32,
}

/// `client_sprite_t`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientSprite {
    pub name: String,
    pub res: i32,
    pub sprite: String,
    pub rect: Rect,
}

/// `SPR_GetList`: a count, then `name res sprite x y w h` per entry (missing tokens read as 0
/// or empty).
pub fn spr_get_list(data: &[u8]) -> Vec<ClientSprite> {
    let mut rest = data;
    let mut next = || -> String {
        match com_parse(rest, false) {
            Some((tok, r)) => {
                rest = r;
                String::from_utf8_lossy(&tok).into_owned()
            }
            None => String::new(),
        }
    };
    let atoi = |s: String| s.trim().parse::<i32>().unwrap_or(0);
    let count = atoi(next());
    (0..count.max(0))
        .map(|_| {
            let name: String = next().chars().take(63).collect();
            let res = atoi(next());
            let sprite: String = next().chars().take(63).collect();
            let (x, y, w, h) = (atoi(next()), atoi(next()), atoi(next()), atoi(next()));
            ClientSprite { name, res, sprite, rect: Rect { left: x, right: x + w, top: y, bottom: y + h } }
        })
        .collect()
}

/// `g_PlayerExtraInfo` entry (scoreboard and radar data).
#[derive(Debug, Clone, Default)]
pub struct PlayerExtra {
    pub frags: i32,
    pub deaths: i32,
    pub teamname: String,
    /// 1 T, 2 CT, 0 otherwise.
    pub teamnumber: i32,
    pub dead: bool,
    pub has_c4: bool,
    pub vip: bool,
    pub has_defuse_kit: bool,
    /// Radar position.
    pub origin: [f32; 3],
    pub location: String,
    /// `sb_account` / `sb_health`: -1 until the server sends them.
    pub account: i32,
    pub health: i32,
    /// `g_PlayerExtraInfo.health` (`SpecHealth` / `SpecHealth2`, the spectator label).
    pub spec_health: i32,
    /// `showhealth`: until when the status bar's `%i` health is shown.
    pub showhealth: f32,
    /// Radar flashing: next toggle time (-1 = off), on/off, toggles left.
    pub radarflash: f32,
    pub radarflashon: bool,
    pub radarflashes: i32,
}

/// What `HUD_UpdateClientData` hands the HUD every frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClientState {
    /// `m_iWeaponBits` (bit 31 = suit).
    pub weapon_bits: i32,
    /// `g_iPlayerFlags`.
    pub flags: i32,
    /// `g_flPlayerSpeed`.
    pub speed: f32,
    /// `g_iUser1` (observer mode) and `g_iUser2` (observed player).
    pub user1: i32,
    pub user2: i32,
    /// `iuser3` (bit 1 set during freeze time).
    pub user3: i32,
    /// The local player's origin and view angles.
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    /// The predicted FOV (`HUD_GetFOV`; 0 = default).
    pub fov: f32,
    /// `g_iWeaponFlags` (the active weapon's `m_iWeaponState`, client weapon prediction).
    pub weapon_flags: i32,
    /// `g_iShotsFired` (incremented by the local fire events).
    pub shots_fired: i32,
}

/// One kept `hud.txt` entry.
#[derive(Debug, Clone)]
struct HudSprite {
    name: String,
    file: String,
    handle: SprHandle,
    rect: Rect,
}

/// `CHud`.
pub struct Hud {
    /// `m_flTime`, `m_fOldTime`, `m_flTimeDelta`.
    pub time: f32,
    old_time: f32,
    pub delta: f32,
    /// `m_flCheatCheckTime` (`gHUD` is BSS: 0 until the first check).
    cheat_check_time: f32,
    /// `g_SpecScoreboardActive`: the last `(int)spec_scoreboard` acted on.
    spec_scoreboard_active: i32,
    /// `CHud::Redraw`'s statics: `flLastSensivity` (−1 in `.data`), `flSensitivityUpdateTime`
    /// and `m_flShotTime` (BSS).
    last_sensitivity: f32,
    sensitivity_update_time: f32,
    shot_time: f32,
    /// 640 or 320.
    res: i32,
    sprites: Vec<HudSprite>,
    number_0: Option<usize>,
    /// `m_iFontHeight` (digit height).
    pub font_height: i32,
    /// `m_iHideHUDDisplay`.
    pub hide: i32,
    pub state: ClientState,
    /// `m_iFOV`.
    pub fov: i32,
    pub intermission: bool,
    /// `m_bShowTimer` (cleared while the bomb is planted).
    pub show_timer: bool,
    pub teamplay: bool,
    /// `drawradar` / `hideradar`.
    pub draw_radar: bool,
    /// `g_PlayerExtraInfo[0..=33]` (33 = the bomb on the radar).
    pub players: Vec<PlayerExtra>,
    pub menu: TextMenu,
    ammo: ammo::Ammo,
    health: health::Health,
    battery: health::Battery,
    flashlight: health::Flashlight,
    chat: chat::SayText,
    centre: chat::CentrePrint,
    notices: notices::DeathNotice,
    icons: status::StatusIcons,
    money: money::Money,
    timer: money::RoundTimer,
    progress: status::ProgressBar,
    scenario: status::Scenario,
    nightvision: status::NightVision,
    messages: message::Messages,
    status_bar: statusbar::StatusBar,
    pub viewport: viewport::Viewport,
    /// `m_autoBuyStringSent`, `m_rebuyStringSent` (reset by `InitHUD`).
    autobuy_sent: bool,
    rebuy_sent: bool,
    /// The client DLL's CRT `rand()` state (CRT default seed 1).
    crt_rand: std::cell::Cell<u32>,
    /// `g_HostageInfo`: radar positions from `HostagePos` / `HostageK` (indices 0..=24).
    pub hostages: Vec<PlayerExtra>,
    /// `CHudTrain`: `Train` position (0 hides) and `sprites/<res>_train.spr`, loaded on first
    /// draw.
    train: i32,
    train_sprite: Option<SprHandle>,
    /// `CHudGeiger`: `4 × Geiger` byte.
    geiger_range: i32,
    /// `m_iLogo` and `sprites/<res>_logo.spr`, loaded on first draw.
    logo: bool,
    logo_sprite: Option<SprHandle>,
    /// A `ViewMode` message arrived: `CAM_ToFirstPerson` (the host owns the camera).
    pub view_mode: bool,
    /// `(int)max_shells`, `(int)max_smokepuffs` as the last cheat check read them (60 before).
    pub effect_queue_limits: (i32, i32),
    /// The map's overview file (`CHudSpectator`).
    pub overview: Overview,
    spectator_gui: specgui::SpectatorGui,
    /// `CHudSpectator`'s overview entities, name anchors and map sprite.
    overview_map: overview::OverviewState,
    /// `CHudSpectator`'s observer state (modes, targets, director camera).
    pub observer: Observer,
    /// `v_origin` / `v_angles` of the last `V_CalcRefdef` (director SOUND, `spec_decal`).
    pub v_origin: [f32; 3],
    pub v_angles: [f32; 3],
    /// `IsSpectateOnly()` as of the last redraw.
    hltv: bool,
    /// `g_FogParameters` from the `Fog` message (cleared by `InitHUD`).
    pub fog: FogParams,
    /// `g_VoiceStatus` / `g_VoiceStatusHud`.
    voice: voice::VoiceStatus,
    /// `gHUD.m_bRenderGunSmoke` (`gunsmoke`; off at `CHud::Init`).
    pub render_gun_smoke: bool,
    /// `CHudAmmo::m_iCrosshairColor` (`adjust_crosshair`).
    crosshair_preset: i32,
    /// `TheBuyPresets->m_hasNightvision` / `m_hasDefuser` (`ItemStatus`).
    pub item_nightvision: bool,
    pub item_defuser: bool,
    /// `m_VoiceBots`: bots that started talking through `BotVoice` (duplicates kept).
    voice_bots: Vec<i32>,
    /// `CCommandMenu` and the `BotProgress` panel (`CCSProgressBar`), and the mouse position
    /// the menu's hover follows.
    command_menu: cmdmenu::CommandMenu,
    bot_progress: cmdmenu::ProgressBar,
    cursor: (i32, i32),
    /// `m_iConcussionEffect` (`Concuss`; 0 on ResetHUD): `v_idlescale`.
    concussion: i32,
}

/// `g_FogParameters`: the map fog colour and EXP2 density (`affectsSkyBox` is always 0).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FogParams {
    pub color: [u8; 3],
    pub density: f32,
}

impl FogParams {
    /// `RenderFog` (`HUD_DrawTransparentTriangles`): `Fog(color, 100, 2000, on)` is on while
    /// the density is positive and the view is at most waist-deep (`waterlevel <= 1`).
    pub fn active(&self, waterlevel: i32) -> bool {
        waterlevel <= 1 && self.density > 0.0
    }
}

impl Default for Hud {
    fn default() -> Self {
        Self {
            time: 0.0,
            old_time: 0.0,
            delta: 0.0,
            cheat_check_time: 0.0,
            spec_scoreboard_active: 0,
            last_sensitivity: -1.0,
            sensitivity_update_time: 0.0,
            shot_time: 0.0,
            res: 640,
            sprites: Vec::new(),
            number_0: None,
            font_height: 0,
            hide: 0,
            state: ClientState::default(),
            fov: 90,
            intermission: false,
            show_timer: true,
            teamplay: true,
            draw_radar: true,
            players: vec![PlayerExtra { account: -1, health: -1, ..PlayerExtra::default() }; 34],
            menu: TextMenu::default(),
            ammo: ammo::Ammo::default(),
            health: health::Health::new(),
            battery: health::Battery::default(),
            flashlight: health::Flashlight::default(),
            chat: chat::SayText::default(),
            centre: chat::CentrePrint::default(),
            notices: notices::DeathNotice::default(),
            icons: status::StatusIcons::default(),
            money: money::Money::default(),
            timer: money::RoundTimer::default(),
            progress: status::ProgressBar::default(),
            scenario: status::Scenario::default(),
            nightvision: status::NightVision::default(),
            messages: message::Messages::default(),
            status_bar: statusbar::StatusBar::default(),
            viewport: viewport::Viewport::default(),
            autobuy_sent: false,
            rebuy_sent: false,
            crt_rand: std::cell::Cell::new(1),
            hostages: vec![PlayerExtra::default(); 25],
            train: 0,
            train_sprite: None,
            geiger_range: 0,
            logo: false,
            logo_sprite: None,
            view_mode: false,
            effect_queue_limits: (60, 60),
            overview: Overview::default(),
            spectator_gui: specgui::SpectatorGui::default(),
            overview_map: overview::OverviewState::default(),
            observer: Observer::default(),
            v_origin: [0.0; 3],
            v_angles: [0.0; 3],
            hltv: false,
            fog: FogParams::default(),
            voice: voice::VoiceStatus::default(),
            render_gun_smoke: false,
            crosshair_preset: 0,
            item_nightvision: false,
            item_defuser: false,
            voice_bots: Vec::new(),
            command_menu: cmdmenu::CommandMenu::default(),
            bot_progress: cmdmenu::ProgressBar::default(),
            cursor: (0, 0),
            concussion: 0,
        }
    }
}

/// `ScaleColors`: alpha baked into the colour (HUD sprites are additive).
pub fn scale(c: [i32; 3], a: i32) -> [i32; 3] {
    c.map(|v| (v as f32 * a as f32 / 255.0) as i32)
}

/// Runs `$element.method(self, ...)` with the element taken out of the HUD, so it can read
/// the rest of the HUD while it changes itself.
macro_rules! with {
    ($hud:ident.$field:ident, $method:ident($($arg:expr),*)) => {{
        #[allow(unused_mut)]
        let mut element = std::mem::take(&mut $hud.$field);
        let out = element.$method($hud, $($arg),*);
        $hud.$field = element;
        out
    }};
}

impl Hud {
    /// `CHud::VidInit`: the `hud.txt` sprites for the screen width (640 when ≥ 640 wide).
    pub fn vid_init(&mut self, eng: &mut dyn HudEngine) {
        // CounterStrikeViewport::OnLevelChange → UpdateScheme hides the progress panel.
        self.bot_progress.level_change();
        if self.sprites.is_empty() {
            self.res = if eng.screen().width > 639 { 640 } else { 320 };
            let list = eng.read_file("sprites/hud.txt").map(|d| spr_get_list(&d)).unwrap_or_default();
            self.sprites = list
                .into_iter()
                .filter(|s| s.res == self.res)
                .map(|s| HudSprite { name: s.name.chars().take(24).collect(), file: s.sprite, handle: SprHandle(0), rect: s.rect })
                .collect();
        }
        for s in &mut self.sprites {
            s.handle = eng.spr_load(&format!("sprites/{}.spr", s.file));
        }
        self.number_0 = self.sprite_index("number_0");
        self.font_height = self.number_0.map_or(0, |i| self.sprite_size(i).1);
        with!(self.ammo, vid_init(eng));
        with!(self.health, vid_init(eng));
        with!(self.messages, vid_init());
        self.progress.reset(eng.screen());
    }

    /// `GetSpriteIndex`: first entry whose name matches in its first 24 characters.
    pub fn sprite_index(&self, name: &str) -> Option<usize> {
        let name: String = name.chars().take(24).collect();
        self.sprites.iter().position(|s| s.name == name)
    }

    pub fn sprite(&self, index: usize) -> (SprHandle, Rect) {
        self.sprites.get(index).map_or((SprHandle(0), Rect::default()), |s| (s.handle, s.rect))
    }

    /// Size of a `hud.txt` entry's rect.
    pub fn sprite_size(&self, index: usize) -> (i32, i32) {
        let (_, r) = self.sprite(index);
        (r.right - r.left, r.bottom - r.top)
    }

    /// Draws `hud.txt` entry `index` additively at `(x, y)` in `color`.
    pub fn draw_sprite(&self, screen: &mut Screen, index: usize, x: i32, y: i32, color: [i32; 3]) {
        let (h, rect) = self.sprite(index);
        screen.spr_set(h, color[0], color[1], color[2]);
        screen.spr_draw_additive(0, x, y, Some(rect));
    }

    /// Digit width (`number_0` width).
    pub fn digit_width(&self) -> i32 {
        self.number_0.map_or(0, |i| self.sprite_size(i).0)
    }

    fn digit(&self, screen: &mut Screen, d: i32, x: i32, y: i32, c: [i32; 3]) {
        if let Some(n0) = self.number_0 {
            self.draw_sprite(screen, n0 + d as usize, x, y, c);
        }
    }

    /// `DrawHudNumber(x, y, flags, n, r, g, b)`: returns x after the last digit.
    pub fn draw_number(&self, screen: &mut Screen, mut x: i32, y: i32, flags: i32, n: i32, c: [i32; 3]) -> i32 {
        let w = self.digit_width();
        let fill = flags & dhn::FILLZERO != 0;
        if n > 0 {
            if n >= 100 {
                self.digit(screen, n / 100, x, y, c);
                x += w;
            } else if flags & dhn::THREE_DIGITS != 0 {
                if fill {
                    self.digit(screen, 0, x, y, c);
                }
                x += w;
            }
            if n >= 10 {
                self.digit(screen, (n % 100) / 10, x, y, c);
                x += w;
            } else if flags & (dhn::THREE_DIGITS | dhn::TWO_DIGITS) != 0 {
                if fill {
                    self.digit(screen, 0, x, y, c);
                }
                x += w;
            }
            self.digit(screen, n % 10, x, y, c);
            x += w;
        } else if flags & dhn::DRAWZERO != 0 {
            if flags & dhn::THREE_DIGITS != 0 {
                if fill {
                    self.digit(screen, 0, x, y, c);
                }
                x += w;
            }
            if flags & (dhn::THREE_DIGITS | dhn::TWO_DIGITS) != 0 {
                if fill {
                    self.digit(screen, 0, x, y, c);
                }
                x += w;
            }
            self.digit(screen, 0, x, y, c);
            x += w;
        }
        x
    }

    /// `DrawHudNumber(x, y, n, r, g, b)`: `"%5i"`, one digit width per character, digits only.
    pub fn draw_number5(&self, screen: &mut Screen, mut x: i32, y: i32, n: i32, c: [i32; 3]) -> i32 {
        let w = self.digit_width();
        for ch in format!("{n:5}").chars() {
            if let Some(d) = ch.to_digit(10) {
                self.digit(screen, d as i32, x, y, c);
            }
            x += w;
        }
        x
    }

    /// `HUD_UpdateClientData` → `CHud::Think`: FOV (the HLTV observer's own on a proxy) and
    /// owned weapons. `g_iUser1..3` are not part of it (`HUD_ProcessPlayerState` and the
    /// observer logic own them).
    pub fn update_client_data(&mut self, state: ClientState, default_fov: f32, eng: &dyn HudEngine) {
        self.state = ClientState { user1: self.state.user1, user2: self.state.user2, user3: self.state.user3, ..state };
        self.fov = if state.fov != 0.0 { state.fov as i32 } else { default_fov as i32 };
        if self.fov == 0 {
            self.fov = default_fov.max(90.0) as i32;
        }
        if let Some(fov) = self.hltv_fov(eng) {
            self.fov = fov;
        }
        self.ammo.think(state.weapon_bits);
    }

    /// `CHudAmmo::Think`'s selection confirmation with this frame's `+attack` state.
    pub fn confirm_selection(&mut self, attack: bool, eng: &mut dyn HudEngine) -> SelectionOutcome {
        self.ammo.confirm(attack, eng)
    }

    /// `gHUD.m_Health.m_iHealth`.
    pub fn health(&self) -> i32 {
        self.health.value()
    }

    /// Whether the local player is alive with a suit (`m_iWeaponBits < 0`).
    pub fn has_suit(&self) -> bool {
        self.state.weapon_bits < 0
    }

    /// `CHudStatusIcons::InBuyZone`.
    pub fn in_buy_zone(&self) -> bool {
        self.icons.has("buyzone")
    }

    /// The local player's `g_PlayerExtraInfo` index (`GetPlayerInfo(i).thisplayer`).
    pub fn local_index(&self, eng: &dyn HudEngine) -> Option<usize> {
        (1..=32).find(|&i| eng.player_info(i).is_some_and(|p| p.thisplayer)).map(|i| i as usize)
    }

    /// What the viewport's menus gate on.
    pub fn view_state(&self, eng: &dyn HudEngine) -> viewport::ViewState {
        let local = self.local_index(eng).map(|i| &self.players[i]);
        viewport::ViewState {
            local_team: local.map_or(0, |p| p.teamnumber),
            local_dead: local.is_none_or(|p| p.dead),
            freeze_over: self.state.user3 & 2 == 0,
            money: self.money.balance(),
            level_name: eng.level_name(),
        }
    }

    /// `IN_ScoreDown` / `IN_ScoreUp`: the scoreboard (multiplayer only); hiding it brings back
    /// a queued menu (`HideScoreBoard`'s `DisplayVGUIMenu`, which hides the spectator GUI).
    pub fn score(&mut self, down: bool, eng: &mut dyn HudEngine) {
        if down {
            if eng.max_clients() > 1 {
                self.viewport.scoreboard = true;
            }
        } else if self.viewport.scoreboard && !self.intermission {
            self.viewport.scoreboard = false;
            if self.viewport.menu_active() {
                self.viewport.spec_events.push(viewport::SpecEvent::Hide);
            }
        }
    }

    /// A VGUI menu, the command menu, the progress panel or the spectator bottom bar has the
    /// mouse: the cursor shows, mouse look and mouse binds stop.
    pub fn cursor_visible(&self) -> bool {
        self.viewport.cursor_visible() || self.command_menu.visible() || self.bot_progress.visible() || self.spectator_cursor()
    }

    /// The mouse position for menu highlighting.
    pub fn set_cursor(&mut self, x: i32, y: i32) {
        self.cursor = (x, y);
        self.viewport.set_cursor(x, y);
    }

    /// A left-button release at `(x, y)`: the command menu fires the item under it
    /// (`CCommandMenu::OnCommand`), else the spectator bar's button or drop-down item.
    pub fn release(&mut self, x: i32, y: i32, eng: &mut dyn HudEngine) {
        if !self.command_menu.visible() && self.spectator_release(x, y, eng) {
            return;
        }
        match self.command_menu.release(x, y, eng.screen()) {
            Some(cmdmenu::MenuCommand::ShowVgui(id)) => {
                let mut vp = std::mem::take(&mut self.viewport);
                vp.show(id, self, eng);
                self.viewport = vp;
            }
            Some(cmdmenu::MenuCommand::ClientCmd(command)) => eng.client_cmd(&command),
            None => {}
        }
    }

    /// A mouse press at screen `(x, y)` (`left`: the left button) on the open menu or the
    /// spectator bar; `true` when one took it.
    pub fn click(&mut self, x: i32, y: i32, left: bool, eng: &mut dyn HudEngine) -> bool {
        if !self.viewport.cursor_visible() && !self.command_menu.visible() {
            return self.spectator_press(x, y, left, eng);
        }
        if !left {
            return false;
        }
        let mut vp = std::mem::take(&mut self.viewport);
        let taken = vp.click(x, y, self, eng);
        self.viewport = vp;
        taken
    }

    /// Mouse-wheel notches (up positive) while the cursor shows; `true` when taken.
    pub fn wheel(&mut self, notches: i32, eng: &mut dyn HudEngine) -> bool {
        self.spectator_wheel(notches, eng)
    }

    /// A key press offered to the open menu or the spectator bar before its binding runs
    /// (`binding` is the key's bind); `true` when consumed.
    pub fn menu_key(&mut self, key: u8, binding: &str, eng: &mut dyn HudEngine) -> bool {
        if !self.viewport.cursor_visible() && self.spectator_cursor() {
            return self.spectator_key(key, eng);
        }
        let mut vp = std::mem::take(&mut self.viewport);
        let taken = vp.key(key, binding, self, eng);
        self.viewport = vp;
        taken
    }

    /// `m_iAccount` from `Money`.
    pub fn money(&self) -> i32 {
        self.money.balance()
    }

    /// The round timer's `m_flTimeEnd`.
    pub(super) fn round_time_end(&self) -> f32 {
        self.timer.time_end()
    }

    /// `IsSpectatorGUIVisible`.
    pub fn spectator_gui_visible(&self) -> bool {
        self.spectator_gui.visible
    }

    /// `HUD_ChatInputPosition` (`TeamFortressViewport::ChatInputPosition`): while observing
    /// (or on HLTV) the engine chat line goes below the PIP inset (`pip`: `spec_pip` is on) or
    /// the spectator top bar; `None` leaves it in place.
    pub fn chat_input_y(&self, screen_h: i32, pip: bool) -> Option<i32> {
        (self.state.user1 != 0 || self.hltv).then(|| if pip { self.overview.window[1] + 5 } else { specgui::psv(64, screen_h) + 5 })
    }

    /// `CHud::AutoBuy` / `CHud::Rebuy`: the first use per map sends `cl_setautobuy "<tokens>"`
    /// (`Send*String`: nothing when the file is missing, the flag is set anyway), later only
    /// the short command.
    fn auto_buy(&mut self, rebuy: bool, eng: &mut dyn HudEngine) {
        let (sent, file, set, run) = if rebuy {
            (&mut self.rebuy_sent, "rebuy.txt", "cl_setrebuy", "cl_rebuy")
        } else {
            (&mut self.autobuy_sent, "autobuy.txt", "cl_setautobuy", "cl_autobuy")
        };
        if *sent {
            eng.client_cmd(run);
            return;
        }
        *sent = true;
        let Some(data) = eng.read_file(file) else { return };
        // char buf[256] filled with strncat(buf, s, 255 - strlen(buf)).
        let mut buf: Vec<u8> = format!("{set} \"").into_bytes();
        let cat = |buf: &mut Vec<u8>, s: &[u8]| {
            let room = 255usize.saturating_sub(buf.len());
            buf.extend_from_slice(&s[..s.len().min(room)]);
        };
        let mut rest = data.as_slice();
        let mut first = true;
        while let Some((tok, r)) = com_parse(rest, false) {
            rest = r;
            // SendRebuyString skips ExtraCash (strcasecmp).
            if rebuy && tok.eq_ignore_ascii_case(b"ExtraCash") {
                continue;
            }
            if !first {
                cat(&mut buf, b" ");
            }
            first = false;
            cat(&mut buf, &tok);
        }
        cat(&mut buf, b"\"");
        eng.client_cmd(&gs_common::bytestr::decode(&buf));
    }

    /// Runs a HUD command; `false` when `args` is not one.
    pub fn command(&mut self, args: &Args, eng: &mut dyn HudEngine) -> bool {
        let name = args.arg(0).to_ascii_lowercase();
        if let Some(n) = name.strip_prefix("slot").and_then(|n| n.parse::<usize>().ok()) {
            // SlotInput: the viewport first, then a text menu, then weapon slots 1..6.
            let mut vp = std::mem::take(&mut self.viewport);
            let taken = n >= 1 && vp.slot_input(n - 1, self, eng);
            self.viewport = vp;
            if taken {
                return true;
            }
            if self.menu.is_displayed() {
                if let Some(select) = self.menu.select(n as i32) {
                    eng.client_cmd(&select);
                }
            } else if (1..=6).contains(&n) {
                self.ammo.select_slot(n - 1, self.hide, eng);
            }
            return true;
        }
        match name.as_str() {
            "invnext" => self.ammo.cycle(true, self.hide, eng),
            "invprev" => self.ammo.cycle(false, self.hide, eng),
            "cancelselect" => self.ammo.cancel(eng),
            "+nvgadjust" => self.nightvision.alpha = (self.nightvision.alpha + 10).min(220),
            "-nvgadjust" => self.nightvision.alpha = (self.nightvision.alpha - 10).max(30),
            "drawradar" => self.draw_radar = true,
            "hideradar" => self.draw_radar = false,
            "hud_saytext" => {
                let on = eng.cvar("hud_saytext_internal") != 0.0;
                eng.client_cmd(&format!("hud_saytext_internal {}\n", u8::from(!on)));
            }
            "buy" => {
                let old_style = eng.local_userinfo("_vgui_menus").is_some_and(|v| v.trim().parse::<i32>().unwrap_or(0) == 0);
                let mut vp = std::mem::take(&mut self.viewport);
                vp.buy(self, old_style, eng);
                self.viewport = vp;
            }
            "autobuy" => self.auto_buy(false, eng),
            "rebuy" => self.auto_buy(true, eng),
            "togglescores" => {
                let down = !self.viewport.scoreboard;
                self.score(down, eng);
            }
            "spec_mode" => self.cmd_spec_mode(args, eng),
            "spec_toggleinset" => self.cmd_spec_toggleinset(eng),
            "spec_decal" => {
                let (o, a) = (self.v_origin, self.v_angles);
                self.cmd_spec_decal(eng, o, a);
            }
            "spec_help" => {
                let mut vp = std::mem::take(&mut self.viewport);
                vp.show(9, self, eng);
                self.viewport = vp;
            }
            // SpectatorMenu: 0 → DeactivateSpectatorGUI, else ShowSpectatorGUI (which closes
            // an open bottom bar).
            "spec_menu" => {
                if args.argc() <= 1 {
                    eng.console_print("usage:  spec_menu <0|1>\n");
                } else if specgui::atoi(args.arg(1)) == 0 {
                    self.deactivate_spectator_gui(eng);
                } else {
                    self.show_spectator_gui(eng);
                }
            }
            "spec_drawnames" | "spec_drawcone" | "spec_drawstatus" | "spec_autodirector" => {
                let var = format!("{name}_internal");
                let off = eng.cvar(&var) == 0.0;
                eng.cvar_set(&var, if off { "1" } else { "0" });
            }
            "voice_showbanned" => self.voice.show_banned(eng),
            "adjust_crosshair" => self.adjust_crosshair(args, eng),
            // ShowCommandMenu / HideCommandMenu.
            "+commandmenu" => {
                let (team, names) = (self.view_state(eng).local_team, self.viewport.team_names());
                self.command_menu.show(team, &names, self.time, eng);
            }
            "-commandmenu" => self.command_menu.hide(self.time),
            "trackplayer" => self.health.track_player(),
            "clearplayers" => self.health.clear_players(),
            // __CmdFunc_ToggleGunSmoke.
            "gunsmoke" => self.render_gun_smoke = !self.render_gun_smoke,
            _ => return false,
        }
        true
    }

    /// Dispatches a user message; `false` when the HUD does not hook it.
    pub fn msg(&mut self, name: &str, data: &[u8], eng: &mut dyn HudEngine) -> bool {
        let mut r = MsgReader::new(data);
        {
            let mut vp = std::mem::take(&mut self.viewport);
            let taken = vp.msg(name, &mut r, self, eng);
            self.viewport = vp;
            if taken {
                return true;
            }
        }
        if matches!(name, "SayText" | "TextMsg") && !self.viewport.allowed_to_print() {
            return true;
        }
        let player = |r: &mut MsgReader| usize::try_from(r.byte()).ok().filter(|p| (1..=32).contains(p));
        match name {
            "ResetHUD" => self.reset_hud(eng),
            "InitHUD" => self.init_hud(eng),
            "GameMode" => self.teamplay = r.byte() != 0,
            "HideWeapon" => self.hide = r.byte(),
            "VoiceMask" => {
                let mut voice = std::mem::take(&mut self.voice);
                voice.msg_voice_mask(&mut r, eng);
                self.voice = voice;
            }
            "ReqState" => {
                let mut voice = std::mem::take(&mut self.voice);
                voice.msg_req_state(eng);
                self.voice = voice;
            }
            "ShowTimer" => self.show_timer = true,
            "BotProgress" => self.bot_progress.msg(&mut r),
            // MsgFunc_Fog: RGB, then the density float; an overrun read leaves fog off.
            "Fog" => {
                let color = [r.byte(), r.byte(), r.byte()].map(|c| c as u8);
                let density = f32::from_le_bytes([r.byte(), r.byte(), r.byte(), r.byte()].map(|b| b as u8));
                self.fog = if r.ok() { FogParams { color, density } } else { FogParams::default() };
            }
            "Health" => self.health.msg_health(&mut r),
            "Damage" => self.health.msg_damage(&mut r, self.state.origin, self.state.angles, self.time),
            "Battery" => self.battery.msg_battery(&mut r),
            "ArmorType" => self.battery.armor_type = r.byte(),
            "Flashlight" => self.flashlight.msg_flashlight(&mut r),
            "FlashBat" => self.flashlight.msg_flashbat(&mut r),
            "Money" => self.money.msg_money(&mut r),
            "BlinkAcct" => self.money.blink(r.byte()),
            "RoundTime" => self.timer.msg_round_time(&mut r),
            "CurWeapon" => self.ammo.msg_cur_weapon(&mut r, self.state.user1),
            "Crosshair" => {
                let _on = r.byte();
                if !eng.spectate_only() && eng.cvar("cl_observercrosshair") != 0.0 {
                    self.ammo.observer_crosshair = true;
                }
            }
            "WeaponList" => self.ammo.msg_weapon_list(&mut r, self.res, eng),
            "AmmoX" => self.ammo.msg_ammo_x(&mut r),
            "DeathMsg" => {
                let mut notices = std::mem::take(&mut self.notices);
                notices.msg_death(&mut r, &self.players, self, eng);
                self.notices = notices;
                // The scoreboard's DeathMsg: the victim is dead and drops C4 and kit.
                if let Some(&victim) = data.get(1)
                    && (1..=32).contains(&victim)
                {
                    let e = &mut self.players[usize::from(victim)];
                    e.dead = true;
                    e.has_c4 = false;
                    e.has_defuse_kit = false;
                    if self.local_index(eng) == Some(usize::from(victim)) {
                        self.update_spectator_panel(eng);
                    }
                }
                // CHudSpectator::DeathMessage, also when not spectating.
                if let Some(&victim) = data.get(1) {
                    self.overview_death(i32::from(victim), eng);
                }
            }
            "SayText" => self.chat.msg_say_text(&mut r, self.time, &self.players, eng),
            "TextMsg" => chat::msg_text_msg(&mut r, &mut self.chat, &mut self.centre, &self.players, self.time, eng),
            "StatusIcon" => {
                let mut icons = std::mem::take(&mut self.icons);
                icons.msg_status_icon(&mut r, self, eng);
                self.icons = icons;
            }
            // MsgFunc_Concuss: the idle-sway scale (v_idlescale) and the dmg_concuss icon.
            "Concuss" => {
                self.concussion = r.byte();
                let mut icons = std::mem::take(&mut self.icons);
                if self.concussion != 0 {
                    icons.enable("dmg_concuss".to_owned(), [255, 160, 0], false, self, eng);
                } else {
                    icons.disable("dmg_concuss");
                }
                self.icons = icons;
            }
            "BarTime" => self.progress.msg_bar_time(&mut r, self.time),
            "BarTime2" => self.progress.msg_bar_time2(&mut r, self.time),
            "Scenario" => {
                let mut scenario = std::mem::take(&mut self.scenario);
                scenario.msg_scenario(&mut r, self, self.time);
                self.scenario = scenario;
            }
            "NVGToggle" => self.nightvision.on = r.byte() != 0,
            "ShowMenu" => {
                // BufferedLocaliseTextString, then KB_ConvertString with Key_LookupBinding's names.
                self.menu.show(data, self.time, |text| {
                    let text = chat::localise_text_string(&text, eng);
                    crate::menu::kb_convert_string(&text, |b| eng.key_for_binding(b).map(gs_console::keys::key_name))
                });
                if self.menu.is_displayed() {
                    eng.console_print(&format!("{}\n", self.menu.text));
                }
            }
            "TeamInfo" => {
                if let Some(p) = player(&mut r) {
                    let team = r.string();
                    let e = &mut self.players[p];
                    e.teamnumber = if team.eq_ignore_ascii_case("CT") {
                        2
                    } else if team.eq_ignore_ascii_case("TERRORIST") {
                        1
                    } else {
                        0
                    };
                    e.teamname = gs_common::bytestr::truncate_encoded(&team, 15).into_owned();
                }
            }
            // CounterStrikeViewport::MsgFunc_ScoreInfo also rebuilds the spectator lists.
            "ScoreInfo" => {
                if let Some(p) = player(&mut r) {
                    let (frags, deaths) = (r.short(), r.short());
                    (self.players[p].frags, self.players[p].deaths) = (frags, deaths);
                }
                self.spectator_player_list(eng);
            }
            "ScoreAttrib" => {
                if let Some(p) = player(&mut r) {
                    let f = r.byte();
                    let e = &mut self.players[p];
                    e.dead = f & 1 != 0;
                    e.has_c4 = f & 2 != 0;
                    e.vip = f & 4 != 0;
                    e.has_defuse_kit = f & 8 != 0;
                }
            }
            "Radar" => {
                let p = r.byte();
                let origin = [r.coord(), r.coord(), r.coord()];
                if let Some(e) = usize::try_from(p).ok().and_then(|p| self.players.get_mut(p)) {
                    e.origin = origin;
                }
            }
            "Account" => {
                if let Some(p) = player(&mut r) {
                    self.players[p].account = r.long();
                }
            }
            "HealthInfo" => {
                if let Some(p) = player(&mut r) {
                    self.players[p].health = r.long();
                }
            }
            "BombDrop" => {
                let origin = [r.coord(), r.coord(), r.coord()];
                let planted = r.byte();
                let bomb = &mut self.players[33];
                bomb.origin = origin;
                bomb.radarflash = self.time;
                bomb.radarflashon = true;
                bomb.radarflashes = 99999;
                bomb.teamname = "TERRORIST".into();
                bomb.dead = false;
                if planted == 1 {
                    self.show_timer = false;
                }
            }
            "BombPickup" => {
                let bomb = &mut self.players[33];
                bomb.radarflash = -1.0;
                bomb.radarflashon = false;
                bomb.radarflashes = 0;
            }
            "Location" => {
                if let Some(p) = player(&mut r) {
                    self.players[p].location = gs_common::bytestr::truncate_encoded(&r.string(), 31).into_owned();
                }
            }
            // BuyPresetManager: nightvision / defuser ownership (read by czero's buy presets).
            "ItemStatus" => {
                let bits = r.byte();
                self.item_nightvision = bits & 1 != 0;
                self.item_defuser = bits & 2 != 0;
            }
            // MsgFunc_ADStop: the auto-director turns off.
            "ADStop" => eng.cvar_set("spec_autodirector_internal", "0"),
            "HLTV" => self.msg_hltv(&mut r, eng),
            "HudText" | "HudTextPro" | "HudTextArgs" | "GameTitle" => {
                let now = self.time;
                self.messages.msg(name, &mut r, now, eng);
            }
            "StatusText" | "StatusValue" => {
                self.status_bar.msg(name, &mut r);
            }
            "AmmoPickup" | "WeapPickup" | "ItemPickup" => with!(self.ammo, msg_pickup(name, &mut r, eng)),
            "SendAudio" => {
                let sender = r.byte();
                let sound = r.string();
                let pitch = r.short();
                self.send_audio(sender, &sound, pitch, eng);
            }
            "ReloadSound" => {
                let volume = r.byte() as f32 / 255.0;
                let sample = if r.byte() != 0 { "weapons/generic_reload.wav" } else { "weapons/generic_shot_reload.wav" };
                eng.play_sound(sample, volume);
            }
            "BotVoice" => {
                let talking = r.byte() != 0;
                let ent = r.byte();
                self.voice_status(ent, talking, eng);
                if talking {
                    self.voice_bots.push(ent);
                } else if let Some(i) = self.voice_bots.iter().position(|&b| b == ent) {
                    self.voice_bots.remove(i);
                }
            }
            "HostagePos" => {
                let flag = r.byte();
                let index = r.byte();
                let origin = [r.coord(), r.coord(), r.coord()];
                let now = self.time;
                if let Some(h) = usize::try_from(index).ok().and_then(|i| self.hostages.get_mut(i)) {
                    h.origin = origin;
                    h.teamname = "CT".into();
                    h.dead = false;
                    if flag == 1 {
                        h.radarflash = now;
                        h.radarflashon = true;
                        h.radarflashes = 99999;
                    }
                }
            }
            "HostageK" => {
                let index = r.byte();
                let now = self.time;
                if let Some(h) = usize::try_from(index).ok().and_then(|i| self.hostages.get_mut(i)) {
                    h.radarflash = now;
                    h.radarflashes = 15;
                    h.dead = true;
                }
            }
            "Geiger" => self.geiger_range = r.byte() * 4,
            "Train" => self.train = r.byte(),
            "Logo" => self.logo = r.byte() != 0,
            // MsgFunc_ViewMode: a tail jump to CAM_ToFirstPerson.
            "ViewMode" => self.view_mode = true,
            // SpecHealth / SpecHealth2 also make the observed player m_iPlayerLastPointedAt,
            // then refresh the spectator panel.
            "SpecHealth" => {
                let health = r.byte();
                if let Some(p) = usize::try_from(self.state.user2).ok().filter(|p| (1..=32).contains(p)) {
                    self.players[p].spec_health = health;
                }
                self.health.last_pointed_at = self.state.user2;
                self.update_spectator_panel(eng);
            }
            "SpecHealth2" => {
                let health = r.byte();
                if let Some(p) = player(&mut r) {
                    self.players[p].spec_health = health;
                }
                self.health.last_pointed_at = self.state.user2;
                self.update_spectator_panel(eng);
            }
            _ => return false,
        }
        true
    }

    /// `CHudAmmo::UserCmd_Adjust_Crosshair`: presets 0..4 green, red, blue, yellow, cyan
    /// (additive), 5..9 the same opaque; `n % 10` from the argument (a negative remainder falls
    /// to green), else the next of 0..4.
    fn adjust_crosshair(&mut self, args: &Args, eng: &mut dyn HudEngine) {
        let n = if args.argc() > 1 { crate::input::strtol(args.arg(1)) % 10 } else { (self.crosshair_preset + 1) % 5 };
        self.crosshair_preset = n;
        let (rgb, translucent) = match n {
            1 => ([250, 50, 50], true),
            2 => ([50, 50, 250], true),
            3 => ([250, 250, 50], true),
            4 => ([50, 250, 250], true),
            5 => ([50, 250, 50], false),
            6 => ([250, 50, 50], false),
            7 => ([50, 50, 250], false),
            8 => ([250, 250, 50], false),
            9 => ([50, 250, 250], false),
            _ => ([50, 250, 50], true),
        };
        eng.cvar_set("cl_crosshair_color", &format!("{} {} {}", rgb[0], rgb[1], rgb[2]));
        eng.cvar_set("cl_crosshair_translucent", if translucent { "1" } else { "0" });
    }

    /// MSVC `rand()`.
    pub(super) fn rand(&self) -> i32 {
        let s = self.crt_rand.get().wrapping_mul(214_013).wrapping_add(2_531_011);
        self.crt_rand.set(s);
        ((s >> 16) & 0x7FFF) as i32
    }

    /// `MsgFunc_SendAudio`: a `%!NAME` radio sentence (`!NAME`, ≤ 31 chars of 0x20..0x7A) or a
    /// sound, then the sender's radar dot flashes 22 times.
    fn send_audio(&mut self, sender: i32, sound: &str, pitch: i32, eng: &mut dyn HudEngine) {
        match sound.find("%!") {
            Some(i) => {
                let name: String = sound[i + 2..].chars().take_while(|c| (' '..='z').contains(c)).take(31).collect();
                eng.play_sound_pitch(&format!("!{name}"), 1.0, pitch);
            }
            None => eng.play_sound_voice(sound, 1.0, pitch),
        }
        let now = self.time;
        if let Some(p) = usize::try_from(sender).ok().filter(|p| (1..=32).contains(p)).map(|p| &mut self.players[p]) {
            p.radarflash = now;
            p.radarflashon = true;
            p.radarflashes = 22;
        }
    }

    /// `DrawRadar`'s flash toggles: players every 0.15 s, the bomb every 0.35 s (0.15 once
    /// planted), hostages 0.35 s alive / 0.15 s dead.
    fn tick_radar_flash(&mut self) {
        let now = self.time;
        let planted = !self.show_timer;
        let flash = |p: &mut PlayerExtra, interval: f32| {
            if p.radarflash != -1.0 && now > p.radarflash && p.radarflashes > 0 {
                p.radarflash = now + interval;
                p.radarflashon = !p.radarflashon;
                p.radarflashes -= 1;
            }
        };
        for (i, p) in self.players.iter_mut().enumerate() {
            flash(p, if i == 33 && !planted { 0.35 } else { 0.15 });
        }
        for h in &mut self.hostages {
            let interval = if h.dead { 0.15 } else { 0.35 };
            flash(h, interval);
        }
    }

    /// `CHudGeiger::Draw`: random clicks, more often and louder the closer the source.
    fn geiger(&self, eng: &mut dyn HudEngine) {
        let range = self.geiger_range;
        if !(1..1000).contains(&range) || range > 800 {
            return;
        }
        let (pct, volume, variants) = match range {
            601.. => (2, 0.4, 2),
            501.. => (4, 0.5, 2),
            401.. => (8, 0.6, 3),
            301.. => (8, 0.7, 3),
            201.. => (28, 0.78, 3),
            151.. => (40, 0.8, 3),
            101.. => (60, 0.85, 3),
            76.. => (80, 0.9, 3),
            51.. => (90, 0.95, 2),
            _ => (95, 1.0, 2),
        };
        let volume = volume * (self.rand() & 127) as f32 / 255.0 + 0.25;
        if (self.rand() & 127) < pct || (self.rand() & 127) < pct {
            let mut j = self.rand() & 1;
            if variants > 2 {
                j += self.rand() & 1;
            }
            eng.play_sound(&format!("player/geiger{}.wav", j + 1), volume);
        }
    }

    /// `CHudTrain::Draw`: frame `pos - 1` right of the armor number.
    fn draw_train(&mut self, eng: &mut dyn HudEngine) {
        if self.train == 0 {
            return;
        }
        let res = self.res;
        let sprite = *self.train_sprite.get_or_insert_with(|| eng.spr_load(&format!("sprites/{res}_train.spr")));
        let screen = eng.screen();
        let (w, h) = screen.spr_size(sprite, 0);
        screen.spr_set(sprite, YELLOWISH[0], YELLOWISH[1], YELLOWISH[2]);
        let (x, y) = (screen.width as i32 / 3 + w / 4, screen.height as i32 - h - self.font_height);
        screen.spr_draw_additive(self.train - 1, x, y, None);
    }

    /// The spray-logo animation (`m_iLogo`) at 20 frames per second.
    fn draw_logo(&mut self, eng: &mut dyn HudEngine) {
        const FRAMES: [i32; 56] = [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 13, 13, 13, 13, 13, 12, 11, 10, 9, 8, 14, 15, 16, 17, 18, 19, 20, 20, 20, 20, 20, 20, 21, 22, 23, 24, 25, 26,
            27, 28, 29, 29, 29, 29, 29, 29, 28, 27, 26, 25, 24, 30, 31,
        ];
        if !self.logo {
            return;
        }
        let res = self.res;
        let sprite = *self.logo_sprite.get_or_insert_with(|| eng.spr_load(&format!("sprites/{res}_logo.spr")));
        let screen = eng.screen();
        let (w, h) = screen.spr_size(sprite, 0);
        screen.spr_set(sprite, 250, 250, 250);
        let frame = FRAMES[(self.time * 20.0) as usize % FRAMES.len()] - 1;
        screen.spr_draw_additive(frame, screen.width as i32 - w, h / 2, None);
    }

    /// `MsgFunc_ResetHUD` (every spawn).
    fn reset_hud(&mut self, eng: &mut dyn HudEngine) {
        self.ammo.reset(&mut self.hide);
        self.health.reset();
        self.battery.reset();
        self.flashlight.reset();
        self.icons.reset();
        self.money.reset();
        self.progress.reset(eng.screen());
        self.nightvision.on = false;
        self.scenario.reset();
        self.status_bar.reset();
        self.concussion = 0;
        // The bots that were talking go quiet.
        for ent in std::mem::take(&mut self.voice_bots) {
            self.voice_status(ent, false, eng);
        }
        // CHudSpectator::Reset.
        self.observer_reset(eng);
        // Every player's HLTV FOV back to the default.
        let fov = eng.cvar("default_fov");
        self.observer.player_fov = [if fov >= 90.0 { fov as i32 } else { 90 }; 64];
    }

    /// `v_idlescale`, which `CHud::UpdateClientData` sets from `m_iConcussionEffect`.
    pub fn idle_scale(&self) -> f32 {
        self.concussion as f32
    }

    /// `IClientVGUI::HideAllVGUIMenu` (the engine's connect/disconnect paths): every viewport
    /// panel hidden, the queue cleared, the command menu hidden (0.3 s rule).
    pub fn hide_all_vgui_menu(&mut self) {
        self.viewport.hide_all();
        self.command_menu.hide(self.time);
    }

    /// `MsgFunc_InitHUD` (once per map): the viewport's `OnLevelChange` (`HideAllVGUIMenu`,
    /// whose command-menu hide keeps the 0.3 s rule), then every element's `InitHUDData`.
    fn init_hud(&mut self, eng: &mut dyn HudEngine) {
        self.viewport.hide_all();
        self.command_menu.hide(self.time);
        self.notices.clear();
        self.chat.clear();
        self.menu = TextMenu::default();
        for p in &mut self.players {
            p.radarflash = -1.0;
            p.radarflashon = false;
            p.radarflashes = 0;
            p.dead = true;
            p.location.clear();
            p.account = -1;
            p.health = -1;
        }
        for h in &mut self.hostages {
            h.radarflash = -1.0;
            h.radarflashon = false;
            h.radarflashes = 0;
            h.dead = true;
        }
        self.show_timer = true;
        self.autobuy_sent = false;
        self.rebuy_sent = false;
        self.fog = FogParams::default();
        // CHudSpectator::InitHUDData.
        self.observer_init_hud_data(eng);
    }

    /// `CHud::Redraw(time, intermission)`, then the VGUI2 viewport on top.
    pub fn redraw(&mut self, time: f32, intermission: bool, eng: &mut dyn HudEngine) {
        self.old_time = self.time;
        self.time = time;
        self.delta = (time - self.old_time).max(0.0);
        // hud_takesshots: a `snapshot` one second into the intermission.
        if intermission && !self.intermission && eng.cvar("hud_takesshots") != 0.0 {
            self.shot_time = time + 1.0;
        }
        if self.shot_time != 0.0 && time > self.shot_time {
            eng.client_cmd("snapshot\n");
            self.shot_time = 0.0;
        }
        self.intermission = intermission;
        self.menu.think(time);
        self.draw_elements(intermission, eng);
        self.cheat_check(time, eng);
        // spec_scoreboard differing from the latch toggles the scoreboard and latches its
        // integer part (so a fractional value toggles every frame).
        let spec_scoreboard = eng.cvar("spec_scoreboard");
        if spec_scoreboard != self.spec_scoreboard_active as f32 {
            let down = !self.viewport.scoreboard;
            self.score(down, eng);
            self.spec_scoreboard_active = spec_scoreboard as i32;
        }
        self.hltv = eng.spectate_only();
        self.observer_think(eng);
        // The viewport's OnTick (spectator part), then its panels: the spectator GUI first.
        self.spectator_tick(eng);
        self.spectator_draw(eng);
        let mut vp = std::mem::take(&mut self.viewport);
        vp.tick(self, eng);
        vp.draw(self, eng);
        self.viewport = vp;
        self.draw_voice(eng);
        // The popups over the viewport: the progress panel and the command menu.
        self.bot_progress.draw(eng);
        self.command_menu.hover(self.cursor.0, self.cursor.1, eng.screen());
        self.command_menu.draw(eng);
    }

    /// `CHud::Redraw`'s check every 0.5 s (client.so 0xb61a0): the view model, pitch limits,
    /// sensitivity, lighting and `m_pitch` are put back to allowed values through the engine's
    /// `Cvar_SetValue` and console commands.
    fn cheat_check(&mut self, time: f32, eng: &mut dyn HudEngine) {
        if !(time > self.cheat_check_time || self.cheat_check_time == -1.0) {
            return;
        }
        let set = |eng: &mut dyn HudEngine, name: &str, v: f32| eng.cvar_set(name, &gs_console::cvar::format_value(v));
        if eng.cvar("r_drawviewmodel") != 1.0 {
            set(eng, "r_drawviewmodel", 1.0);
        }
        for name in ["cl_pitchup", "cl_pitchdown"] {
            if eng.cvar(name) != 89.0 {
                set(eng, name, 89.0);
                eng.client_cmd("kill\n");
                eng.client_cmd("disconnect\n");
            }
        }
        if self.last_sensitivity < 0.0 {
            self.last_sensitivity = eng.cvar("sensitivity");
        }
        let mut value = eng.cvar("sensitivity");
        if f64::from(value) < 0.2 {
            eng.client_cmd("sensitivity 1\n");
            value = eng.cvar("sensitivity");
        } else if self.sensitivity_update_time > time && (f64::from(value) - f64::from(self.last_sensitivity)).abs() > 0.2 {
            eng.client_cmd(&format!("sensitivity {:.2}\n", f64::from(self.last_sensitivity)));
            self.sensitivity_update_time = time + 2.0;
            value = eng.cvar("sensitivity");
        }
        if value != self.last_sensitivity && time > self.sensitivity_update_time {
            self.last_sensitivity = value;
            self.sensitivity_update_time = time + 2.0;
        }
        if !self.nightvision.on {
            if eng.cvar("lightgamma") != 2.5 {
                set(eng, "lightgamma", 2.5);
                eng.client_cmd("kill\n");
            }
            if f64::from(eng.cvar("direct")) < 0.8 {
                set(eng, "direct", 0.9);
                eng.client_cmd("kill\n");
            }
        }
        let pitch = f64::from(eng.cvar("m_pitch"));
        if (0.0..0.022).contains(&pitch) {
            eng.client_cmd("m_pitch 0.022\n");
        } else if pitch > -0.022 && pitch < 0.0 {
            eng.client_cmd("m_pitch -0.022\n");
        }
        // g_ShellQueue / g_SmokePuffQueue maxelements.
        self.effect_queue_limits = (eng.cvar("max_shells") as i32, eng.cvar("max_smokepuffs") as i32);
        self.cheat_check_time = time + 0.5;
    }

    /// `CHud::Init`'s once-only work: `CVoiceStatus::Init` loads the voice bans.
    pub fn init(&mut self, eng: &mut dyn HudEngine) {
        self.voice.init(eng);
    }

    /// `HUD_Frame`: `CVoiceStatus::Frame` (the periodic server sync).
    pub fn frame(&mut self, eng: &mut dyn HudEngine) {
        let mut voice = std::mem::take(&mut self.voice);
        voice.frame(eng);
        self.voice = voice;
    }

    /// The HUD elements in `CHud::Init` order; in intermission only those with
    /// `HUD_INTERMISSION`. Night vision and the scope draw even with `hud_draw 0`.
    fn draw_elements(&mut self, intermission: bool, eng: &mut dyn HudEngine) {
        if self.hide & hide::ALL != 0 {
            return;
        }
        self.nightvision.draw(eng, self.state.origin, self.time);
        if !intermission {
            scope::draw(self, eng);
        }
        if eng.cvar("hud_draw") != 0.0 {
            self.draw_hud_elements(intermission, eng);
        }
        self.draw_logo(eng);
    }

    fn draw_hud_elements(&mut self, intermission: bool, eng: &mut dyn HudEngine) {
        let printing = self.viewport.allowed_to_print();
        if !intermission {
            with!(self.ammo, draw(eng));
            let radar = self.draw_radar
                && self.has_suit()
                && self.state.user1 == 0
                && self.hide & hide::HEALTH == 0
                && self.health.active()
                && self.local_index(eng).is_some_and(|i| !self.players[i].dead);
            if radar {
                self.tick_radar_flash();
            }
            with!(self.health, draw(eng));
        }
        if printing {
            with!(self.chat, draw(eng));
        }
        if intermission {
            return;
        }
        self.geiger(eng);
        self.draw_train(eng);
        with!(self.battery, draw(eng));
        with!(self.flashlight, draw(eng));
        with!(self.messages, draw(eng));
        with!(self.status_bar, draw(eng));
        if printing {
            with!(self.notices, draw(eng));
        }
        with!(self.icons, draw(eng));
        with!(self.money, draw(eng));
        with!(self.timer, draw(eng));
        with!(self.progress, draw(eng));
        let right = self.timer.closest_right;
        with!(self.scenario, draw(eng, right));
        self.draw_menu(eng);
        with!(self.centre, draw(eng));
    }

    /// `CHudMenu::Draw` (text menus; escapes `\w \y \r \d \R`).
    fn draw_menu(&mut self, eng: &mut dyn HudEngine) {
        if !self.menu.is_displayed() {
            return;
        }
        let text = self.menu.text.clone();
        let screen = eng.screen();
        let lines = text.matches('\n').count() as i32;
        let ch = screen.screen_info().char_height.max(12);
        let (w, h) = (screen.width as i32, screen.height as i32);
        let mut y = h / 2 - ch * (lines / 2) - (3 * ch + ch / 3);
        for line in text.split('\n') {
            let mut x = 20;
            let mut color = [255, 255, 255];
            let mut right = false;
            let mut seg = String::new();
            let mut chars = line.chars();
            loop {
                let c = chars.next();
                if matches!(c, None | Some('\\')) {
                    let s = gs_common::bytestr::truncate_encoded(&seg, 79);
                    if right {
                        screen.draw_string_reverse(w / 2, y, &s, color[0], color[1], color[2]);
                    } else {
                        x += screen.draw_string(x, y, &s, color[0], color[1], color[2]);
                    }
                    seg.clear();
                }
                match c {
                    None => break,
                    Some('\\') => match chars.next() {
                        Some('w') => color = [255, 255, 255],
                        Some('y') => color = [255, 210, 64],
                        Some('r') => color = [210, 24, 0],
                        Some('d') => color = [100, 100, 100],
                        Some('R') => right = true,
                        _ => {}
                    },
                    Some(c) => seg.push(c),
                }
            }
            y += ch + 2;
        }
    }
}

#[cfg(test)]
mod tests;
