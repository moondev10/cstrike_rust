//! Steam through the local Steam client: authentication for the connect packet
//! (docs/re/net-transport.md §7) — `ISteamUser::InitiateGameConnection_DEPRECATED` makes the
//! ticket the server validates, `TerminateGameConnection_DEPRECATED` and `AdvertiseGame` end the
//! session — the voice codec (docs/re/audio.md §11): `ISteamUser` recording, `GetVoice` and
//! `DecompressVoice` — and the server browser's matchmaking (docs/re/serverbrowser.md §8).
//!
//! The Steamworks redistributable (`steam_api64.dll` from the SDK's
//! `redistributable_bin/win64`, `libsteam_api.so` elsewhere) is loaded at run time from
//! `CSTRIKE_GAME_STEAM_API` (a file path) or next to the executable. Never from the current
//! directory: a library planted there would run at every start.
//! Steam must be running and logged in with an account that owns the game.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::rc::Rc;

use gs_client::auth::Authenticator;
use gs_sound::voice::VoiceCodec;

mod ffi;

/// The app id `Steam_InitClient` puts in `SteamAppId` (Counter-Strike) [UNVERIFIED in the
/// binary, which writes `GetAppID()`].
pub const CSTRIKE_APP_ID: u32 = 10;

#[cfg(windows)]
const LIBRARY: &str = "steam_api64.dll";
#[cfg(target_os = "macos")]
const LIBRARY: &str = "libsteam_api.dylib";
#[cfg(not(any(windows, target_os = "macos")))]
const LIBRARY: &str = "libsteam_api.so";

/// The ticket of the last connection tuple (`Steam_GSInitiateGameConnection`'s cache): retries
/// to the same server reuse it.
struct Session {
    ip: u32,
    port: u16,
    server_steam_id: u64,
    secure: bool,
    ticket: Vec<u8>,
}

/// [`Authenticator`] over the Steam client.
pub struct SteamAuthenticator {
    api: Rc<ffi::SteamApi>,
    session: Option<Session>,
}

/// [`VoiceCodec`] over the same Steam client.
pub struct SteamVoice {
    api: Rc<ffi::SteamApi>,
}

/// `EVoiceResult::k_EVoiceResultOK`.
const VOICE_OK: i32 = 0;
/// The rate the engine asks Steam for (`g_VoiceSampleFormat`).
const VOICE_RATE: u32 = 11025;

/// The Steam client context (`Steam_InitClient`: the app id, `SteamAPI_Init`, `ISteamUser`),
/// shared by the authenticator, the voice codec and the server browser.
#[derive(Clone)]
pub struct SteamClient {
    api: Rc<ffi::SteamApi>,
}

impl SteamClient {
    pub fn init() -> Result<Self, String> {
        let path = library_path().ok_or_else(|| format!("{LIBRARY} not found (set CSTRIKE_GAME_STEAM_API or put it next to the executable)"))?;
        let api = Rc::new(ffi::SteamApi::load(&path, CSTRIKE_APP_ID)?);
        log::info!("Steam: {} loaded, user {} ({})", path.display(), api.steam_id(), if api.logged_on() { "logged on" } else { "offline" });
        Ok(Self { api })
    }

    pub fn authenticator(&self) -> SteamAuthenticator {
        SteamAuthenticator { api: self.api.clone(), session: None }
    }

    pub fn voice(&self) -> SteamVoice {
        SteamVoice { api: self.api.clone() }
    }

    /// ServerBrowser.dll's `SteamMatchMakingServers002` / `SteamMatchMaking009` / `SteamApps`.
    pub fn matchmaking(&self) -> Result<SteamMatchmaking, String> {
        ffi::Matchmaking::new(self.api.clone()).map(SteamMatchmaking)
    }
}

/// [`gs_gameui::serverbrowser::Matchmaking`] over the Steam client.
pub struct SteamMatchmaking(ffi::Matchmaking);

impl gs_gameui::serverbrowser::Matchmaking for SteamMatchmaking {
    fn request_server_list(&mut self, kind: gs_gameui::serverbrowser::ListKind, app: u32, filters: &[(String, String)], responder: u32) -> u64 {
        self.0.request_list(kind as u32, app, filters, responder)
    }
    fn release_request(&mut self, request: u64) {
        self.0.release_request(request);
    }
    fn server_details(&self, request: u64, server: i32) -> Option<gs_gameui::serverbrowser::GameServerItem> {
        self.0.server_details(request, server)
    }
    fn cancel_query(&mut self, request: u64) {
        self.0.cancel_query(request);
    }
    fn refresh_query(&mut self, request: u64) {
        self.0.refresh_query(request);
    }
    fn is_refreshing(&self, request: u64) -> bool {
        self.0.is_refreshing(request)
    }
    fn refresh_server(&mut self, request: u64, server: i32) {
        self.0.refresh_server(request, server);
    }
    fn ping_server(&mut self, ip: u32, port: u16, responder: u32) -> i32 {
        self.0.ping_server(ip, port, responder)
    }
    fn player_details(&mut self, ip: u32, port: u16, responder: u32) -> i32 {
        self.0.player_details(ip, port, responder)
    }
    fn cancel_server_query(&mut self, query: i32) {
        self.0.cancel_server_query(query);
    }
    fn release_responder(&mut self, responder: u32) {
        self.0.release_responder(responder);
    }
    fn favorite_game_count(&self) -> i32 {
        self.0.favorite_count()
    }
    fn add_favorite_game(&mut self, app: u32, ip: u32, conn_port: u16, query_port: u16, flags: u32, time: u32) -> i32 {
        self.0.add_favorite(app, ip, conn_port, query_port, flags, time)
    }
    fn remove_favorite_game(&mut self, app: u32, ip: u32, conn_port: u16, query_port: u16, flags: u32) -> bool {
        self.0.remove_favorite(app, ip, conn_port, query_port, flags)
    }
    fn is_vac_banned(&self) -> bool {
        self.0.vac_banned()
    }
    fn run_callbacks(&mut self) -> Vec<gs_gameui::serverbrowser::MmEvent> {
        self.0.run_callbacks()
    }
}

impl SteamAuthenticator {
    /// `Steam_ClientDisconnect`'s terminate step.
    fn terminate(&mut self) {
        if let Some(s) = self.session.take() {
            self.api.terminate(s.ip, s.port);
        }
    }
}

fn library_path() -> Option<PathBuf> {
    let env = std::env::var_os("CSTRIKE_GAME_STEAM_API").map(PathBuf::from);
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(LIBRARY)));
    [env, exe_dir].into_iter().flatten().find(|p| p.is_file())
}

impl Authenticator for SteamAuthenticator {
    /// `Steam_GSInitiateGameConnection`: ip and port in host order; the same tuple reuses the
    /// cached ticket, a new one ends the previous game connection first.
    fn ticket(&mut self, server_steam_id: u64, ip: Ipv4Addr, port: u16, secure: bool) -> Result<Vec<u8>, String> {
        let ip = u32::from(ip);
        if let Some(s) = &self.session
            && (s.ip, s.port, s.server_steam_id, s.secure) == (ip, port, server_steam_id, secure)
        {
            return Ok(s.ticket.clone());
        }
        self.terminate();
        let mut blob = [0u8; 1024];
        let len = self.api.initiate(server_steam_id, ip, port, secure, &mut blob);
        let len = usize::try_from(len).ok().filter(|&l| l > 0 && l < blob.len()).ok_or_else(|| format!("InitiateGameConnection returned {len}"))?;
        let ticket = blob[..len].to_vec();
        self.session = Some(Session { ip, port, server_steam_id, secure, ticket: ticket.clone() });
        Ok(ticket)
    }

    /// `Steam_ClientDisconnect`: terminate the cached connection, stop advertising the game.
    fn end_session(&mut self) {
        self.terminate();
        self.api.advertise(0, 0, 0);
    }

    fn run_frame(&mut self) {
        self.api.run_callbacks();
    }
}

impl VoiceCodec for SteamVoice {
    fn start_recording(&mut self) {
        self.api.start_voice_recording();
    }

    fn stop_recording(&mut self) {
        self.api.stop_voice_recording();
    }

    fn get_voice(&mut self, out: &mut [u8], uncompressed: Option<&mut [u8]>) -> (usize, usize) {
        let rate = if uncompressed.is_some() { VOICE_RATE } else { 0 };
        if self.api.available_voice(rate) != VOICE_OK {
            return (0, 0);
        }
        let (_, written, uncomp) = self.api.get_voice(out, uncompressed, VOICE_RATE);
        ((written as usize).min(out.len()), uncomp as usize)
    }

    fn decompress(&mut self, _sender: i32, data: &[u8], out: &mut [u8]) -> Option<usize> {
        match self.api.decompress_voice(data, out, VOICE_RATE) {
            (VOICE_OK, written) => Some((written as usize).min(out.len())),
            _ => None,
        }
    }
}
