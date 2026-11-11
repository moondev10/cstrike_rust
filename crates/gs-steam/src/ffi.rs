//! The Steamworks flat API of `steam_api64.dll`, loaded at run time. With
//! `gs-voicecodec/src/ffi.rs`, the only place in the workspace that calls foreign code.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::path::Path;

use libloading::Library;

type InitFlatFn = unsafe extern "C" fn(*mut [u8; 1024]) -> i32;
type InitFn = unsafe extern "C" fn() -> bool;
type VoidFn = unsafe extern "C" fn();
type UserAccessorFn = unsafe extern "C" fn() -> *mut c_void;
type InitiateFn = unsafe extern "C" fn(*mut c_void, *mut c_void, i32, u64, u32, u16, bool) -> i32;
type TerminateFn = unsafe extern "C" fn(*mut c_void, u32, u16);
type AdvertiseFn = unsafe extern "C" fn(*mut c_void, u64, u32, u16);
type SteamIdFn = unsafe extern "C" fn(*mut c_void) -> u64;
type LoggedOnFn = unsafe extern "C" fn(*mut c_void) -> bool;
type UserVoidFn = unsafe extern "C" fn(*mut c_void);
type AvailableVoiceFn = unsafe extern "C" fn(*mut c_void, *mut u32, *mut u32, u32) -> i32;
type GetVoiceFn = unsafe extern "C" fn(*mut c_void, bool, *mut c_void, u32, *mut u32, bool, *mut c_void, u32, *mut u32, u32) -> i32;
type DecompressVoiceFn = unsafe extern "C" fn(*mut c_void, *const c_void, u32, *mut c_void, u32, *mut u32, u32) -> i32;

/// `ISteamUser` accessors by SDK generation, newest first (`SteamUser023` is the engine's).
const USER_ACCESSORS: [&[u8]; 4] = [b"SteamAPI_SteamUser_v023\0", b"SteamAPI_SteamUser_v022\0", b"SteamAPI_SteamUser_v021\0", b"SteamAPI_SteamUser_v020\0"];

/// An initialized Steam API with the `ISteamUser` calls the engine makes.
pub struct SteamApi {
    user: *mut c_void,
    run_callbacks: VoidFn,
    shutdown: VoidFn,
    initiate: InitiateFn,
    terminate: TerminateFn,
    advertise: AdvertiseFn,
    steam_id: SteamIdFn,
    logged_on: LoggedOnFn,
    start_voice: UserVoidFn,
    stop_voice: UserVoidFn,
    available_voice: AvailableVoiceFn,
    get_voice: GetVoiceFn,
    decompress_voice: DecompressVoiceFn,
    /// Keeps the function pointers above valid; dropped last.
    lib: Library,
}

/// Copies a function pointer out of the library.
///
/// SAFETY: `T` must be the exact C signature of the exported symbol, and the pointer must not
/// outlive `lib` (every copy lives in the same `SteamApi` as the library).
unsafe fn symbol<T: Copy>(lib: &Library, name: &[u8]) -> Result<T, String> {
    // SAFETY: forwarded from the caller.
    unsafe { lib.get::<T>(name) }.map(|s| *s).map_err(|e| format!("{}: {e}", String::from_utf8_lossy(&name[..name.len() - 1])))
}

impl SteamApi {
    /// Loads the library at `path`, sets the app id for `SteamAPI_Init` when the environment
    /// has none, initializes the API and fetches `ISteamUser`.
    pub fn load(path: &Path, app_id: u32) -> Result<Self, String> {
        if std::env::var_os("SteamAppId").is_none() {
            // SAFETY: called while the client is being created on the main thread, before the
            // audio and terminal threads start; nothing else reads the environment meanwhile.
            unsafe {
                std::env::set_var("SteamAppId", app_id.to_string());
                std::env::set_var("SteamGameId", app_id.to_string());
            }
        }
        // SAFETY: loading the Steamworks redistributable runs only its own initializers.
        let lib = unsafe { Library::new(path) }.map_err(|e| format!("{}: {e}", path.display()))?;
        // SAFETY: the signatures match steam_api_flat.h of SDK 1.5x–1.6x.
        unsafe {
            // SDK ≥ 1.58 exports SteamAPI_InitFlat (0 = k_ESteamAPIInitResult_OK); older ones
            // export SteamAPI_Init.
            if let Ok(init) = symbol::<InitFlatFn>(&lib, b"SteamAPI_InitFlat\0") {
                let mut msg = [0u8; 1024];
                let result = init(&mut msg);
                if result != 0 {
                    let end = msg.iter().position(|&b| b == 0).unwrap_or(msg.len());
                    return Err(format!("SteamAPI_InitFlat failed ({result}): {}", String::from_utf8_lossy(&msg[..end])));
                }
            } else if !symbol::<InitFn>(&lib, b"SteamAPI_Init\0")?() {
                return Err("SteamAPI_Init failed (is Steam running and logged in?)".into());
            }
            let shutdown = symbol::<VoidFn>(&lib, b"SteamAPI_Shutdown\0")?;
            let accessor = USER_ACCESSORS.iter().find_map(|name| symbol::<UserAccessorFn>(&lib, name).ok());
            let user = accessor.map_or(std::ptr::null_mut(), |f| f());
            if user.is_null() {
                shutdown();
                return Err("no ISteamUser interface in this steam_api".into());
            }
            Ok(Self {
                user,
                run_callbacks: symbol(&lib, b"SteamAPI_RunCallbacks\0")?,
                shutdown,
                initiate: symbol(&lib, b"SteamAPI_ISteamUser_InitiateGameConnection_DEPRECATED\0")?,
                terminate: symbol(&lib, b"SteamAPI_ISteamUser_TerminateGameConnection_DEPRECATED\0")?,
                advertise: symbol(&lib, b"SteamAPI_ISteamUser_AdvertiseGame\0")?,
                steam_id: symbol(&lib, b"SteamAPI_ISteamUser_GetSteamID\0")?,
                logged_on: symbol(&lib, b"SteamAPI_ISteamUser_BLoggedOn\0")?,
                start_voice: symbol(&lib, b"SteamAPI_ISteamUser_StartVoiceRecording\0")?,
                stop_voice: symbol(&lib, b"SteamAPI_ISteamUser_StopVoiceRecording\0")?,
                available_voice: symbol(&lib, b"SteamAPI_ISteamUser_GetAvailableVoice\0")?,
                get_voice: symbol(&lib, b"SteamAPI_ISteamUser_GetVoice\0")?,
                decompress_voice: symbol(&lib, b"SteamAPI_ISteamUser_DecompressVoice\0")?,
                lib,
            })
        }
    }

    /// `ISteamUser::InitiateGameConnection_DEPRECATED` into a 1024-byte blob; the ticket length
    /// (0 on failure).
    pub fn initiate(&self, server_steam_id: u64, ip: u32, port: u16, secure: bool, blob: &mut [u8; 1024]) -> i32 {
        // SAFETY: `user` came from the accessor and stays valid until shutdown; the blob size is
        // passed as the maximum.
        unsafe { (self.initiate)(self.user, blob.as_mut_ptr().cast(), blob.len() as i32, server_steam_id, ip, port, secure) }
    }

    /// `ISteamUser::TerminateGameConnection_DEPRECATED`.
    pub fn terminate(&self, ip: u32, port: u16) {
        // SAFETY: as in `initiate`.
        unsafe { (self.terminate)(self.user, ip, port) }
    }

    /// `ISteamUser::AdvertiseGame`.
    pub fn advertise(&self, server_steam_id: u64, ip: u32, port: u16) {
        // SAFETY: as in `initiate`.
        unsafe { (self.advertise)(self.user, server_steam_id, ip, port) }
    }

    pub fn steam_id(&self) -> u64 {
        // SAFETY: as in `initiate`.
        unsafe { (self.steam_id)(self.user) }
    }

    pub fn logged_on(&self) -> bool {
        // SAFETY: as in `initiate`.
        unsafe { (self.logged_on)(self.user) }
    }

    /// `ISteamUser::StartVoiceRecording`.
    pub fn start_voice_recording(&self) {
        // SAFETY: as in `initiate`.
        unsafe { (self.start_voice)(self.user) }
    }

    /// `ISteamUser::StopVoiceRecording`.
    pub fn stop_voice_recording(&self) {
        // SAFETY: as in `initiate`.
        unsafe { (self.stop_voice)(self.user) }
    }

    /// `ISteamUser::GetAvailableVoice(…, desired_rate)`: the `EVoiceResult`.
    pub fn available_voice(&self, desired_rate: u32) -> i32 {
        let (mut compressed, mut uncompressed) = (0u32, 0u32);
        // SAFETY: as in `initiate`; both out-pointers are valid locals.
        unsafe { (self.available_voice)(self.user, &mut compressed, &mut uncompressed, desired_rate) }
    }

    /// `ISteamUser::GetVoice(true, out, …, uncompressed.is_some(), uncompressed, …, rate)`:
    /// `(EVoiceResult, compressed bytes, uncompressed bytes)`.
    pub fn get_voice(&self, out: &mut [u8], uncompressed: Option<&mut [u8]>, rate: u32) -> (i32, u32, u32) {
        let (mut written, mut uncomp_written) = (0u32, 0u32);
        let size = u32::try_from(out.len()).unwrap_or(u32::MAX);
        let (want, udst, usize_) = match uncompressed {
            Some(u) => (true, u.as_mut_ptr().cast(), u32::try_from(u.len()).unwrap_or(u32::MAX)),
            None => (false, std::ptr::null_mut(), 0),
        };
        // SAFETY: as in `initiate`; `out` is valid for `size` bytes and `udst` for `usize_`
        // (null with 0 when no uncompressed copy is requested).
        let r = unsafe { (self.get_voice)(self.user, true, out.as_mut_ptr().cast(), size, &mut written, want, udst, usize_, &mut uncomp_written, rate) };
        (r, written, uncomp_written)
    }

    /// `ISteamUser::DecompressVoice` into `out` (16-bit mono at `rate`): `(EVoiceResult, bytes)`.
    pub fn decompress_voice(&self, data: &[u8], out: &mut [u8], rate: u32) -> (i32, u32) {
        let mut written = 0u32;
        let (len, size) = (u32::try_from(data.len()).unwrap_or(u32::MAX), u32::try_from(out.len()).unwrap_or(u32::MAX));
        // SAFETY: as in `initiate`; both buffers are valid for the sizes passed.
        let r = unsafe { (self.decompress_voice)(self.user, data.as_ptr().cast(), len, out.as_mut_ptr().cast(), size, &mut written, rate) };
        (r, written)
    }

    /// `SteamAPI_RunCallbacks`.
    pub fn run_callbacks(&self) {
        // SAFETY: the API is initialized for the lifetime of `self`.
        unsafe { (self.run_callbacks)() }
    }
}

impl Drop for SteamApi {
    fn drop(&mut self) {
        // SAFETY: last call into the API; the library is unloaded after this.
        unsafe { (self.shutdown)() }
    }
}

// ---------------------------------------------------------------------------------------------
// ISteamMatchmakingServers / ISteamMatchmaking / ISteamApps (ServerBrowser.dll's calls,
// docs/re/serverbrowser.md §8) with the C++ response interfaces implemented here.

type IfaceFn = unsafe extern "C" fn() -> *mut c_void;
type RequestListFn = unsafe extern "C" fn(*mut c_void, u32, *mut *mut KvPair, u32, *mut c_void) -> *mut c_void;
type RequestLanFn = unsafe extern "C" fn(*mut c_void, u32, *mut c_void) -> *mut c_void;
type RequestOpFn = unsafe extern "C" fn(*mut c_void, *mut c_void);
type DetailsFn = unsafe extern "C" fn(*mut c_void, *mut c_void, i32) -> *const u8;
type IsRefreshingFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> bool;
type RefreshServerFn = unsafe extern "C" fn(*mut c_void, *mut c_void, i32);
type QueryFn = unsafe extern "C" fn(*mut c_void, u32, u16, *mut c_void) -> i32;
type CancelServerQueryFn = unsafe extern "C" fn(*mut c_void, i32);
type IfaceIntFn = unsafe extern "C" fn(*mut c_void) -> i32;
type IfaceBoolFn = unsafe extern "C" fn(*mut c_void) -> bool;
type AddFavoriteFn = unsafe extern "C" fn(*mut c_void, u32, u32, u16, u16, u32, u32) -> i32;
type RemoveFavoriteFn = unsafe extern "C" fn(*mut c_void, u32, u32, u16, u16, u32) -> bool;
type RegisterCallbackFn = unsafe extern "C" fn(*mut c_void, i32);
type UnregisterCallbackFn = unsafe extern "C" fn(*mut c_void);

/// `MatchMakingKeyValuePair_t`.
#[repr(C)]
pub struct KvPair {
    key: [u8; 256],
    value: [u8; 256],
}

impl KvPair {
    fn new(key: &str, value: &str) -> Self {
        let mut p = Self { key: [0; 256], value: [0; 256] };
        let (k, v) = (key.as_bytes(), value.as_bytes());
        p.key[..k.len().min(255)].copy_from_slice(&k[..k.len().min(255)]);
        p.value[..v.len().min(255)].copy_from_slice(&v[..v.len().min(255)]);
        p
    }
}

/// Where the response objects put what Steam reports.
pub type Sink = std::cell::RefCell<Vec<gs_gameui::serverbrowser::MmEvent>>;

/// `sizeof(gameserveritem_t)`.
const SERVER_ITEM_SIZE: usize = 0x174;

/// `gameserveritem_t` (§3.1) from its bytes.
fn decode_server(b: &[u8]) -> gs_gameui::serverbrowser::GameServerItem {
    let u16_at = |o: usize| b.get(o..o + 2).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]));
    let u32_at = |o: usize| b.get(o..o + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let u64_at = |o: usize| u64::from(u32_at(o)) | (u64::from(u32_at(o + 4)) << 32);
    let flag = |o: usize| b.get(o).is_some_and(|&v| v != 0);
    let text = |o: usize, n: usize| {
        let s = b.get(o..o + n).unwrap_or(&[]);
        let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
        String::from_utf8_lossy(&s[..end]).into_owned()
    };
    gs_gameui::serverbrowser::GameServerItem {
        conn_port: u16_at(0),
        query_port: u16_at(2),
        ip: u32_at(4),
        ping: u32_at(8) as i32,
        had_successful_response: flag(0xc),
        do_not_refresh: flag(0xd),
        game_dir: text(0xe, 32),
        map: text(0x2e, 32),
        game_description: text(0x4e, 64),
        app_id: u32_at(0x90),
        players: u32_at(0x94) as i32,
        max_players: u32_at(0x98) as i32,
        bot_players: u32_at(0x9c) as i32,
        password: flag(0xa0),
        secure: flag(0xa1),
        time_last_played: u32_at(0xa4),
        server_version: u32_at(0xa8) as i32,
        server_name: text(0xac, 64),
        game_tags: text(0xec, 128),
        steam_id: u64_at(0x16c),
    }
}

/// `gameserveritem_t*` → its bytes, copied.
///
/// SAFETY: `p` is null or points at a live `gameserveritem_t`.
unsafe fn server_at(p: *const u8) -> Option<gs_gameui::serverbrowser::GameServerItem> {
    if p.is_null() {
        return None;
    }
    // SAFETY: forwarded from the caller; the struct is SERVER_ITEM_SIZE bytes.
    let bytes = unsafe { std::slice::from_raw_parts(p, SERVER_ITEM_SIZE) };
    Some(decode_server(bytes))
}

/// The three response interfaces of one ServerBrowser object (a page or a dialog), each a C++
/// object: a vtable pointer, then our token and the event sink.
#[repr(C)]
struct ListResponse {
    vtable: *const ListVtable,
    token: u32,
    sink: *const Sink,
}

/// `ISteamMatchmakingServerListResponse` (MSVC x64: `this` first, no destructor slot).
#[repr(C)]
struct ListVtable {
    responded: extern "C" fn(*mut ListResponse, *mut c_void, i32),
    failed: extern "C" fn(*mut ListResponse, *mut c_void, i32),
    complete: extern "C" fn(*mut ListResponse, *mut c_void, i32),
}

#[repr(C)]
struct PingResponse {
    vtable: *const PingVtable,
    token: u32,
    sink: *const Sink,
}

/// `ISteamMatchmakingPingResponse`.
#[repr(C)]
struct PingVtable {
    responded: extern "C" fn(*mut PingResponse, *const u8),
    failed: extern "C" fn(*mut PingResponse),
}

#[repr(C)]
struct PlayersResponse {
    vtable: *const PlayersVtable,
    token: u32,
    sink: *const Sink,
}

/// `ISteamMatchmakingPlayersResponse`.
#[repr(C)]
struct PlayersVtable {
    add: extern "C" fn(*mut PlayersResponse, *const std::ffi::c_char, i32, f32),
    failed: extern "C" fn(*mut PlayersResponse),
    complete: extern "C" fn(*mut PlayersResponse),
}

/// Pushes `ev` into the sink of a response object.
///
/// SAFETY: `sink` points at the sink owned by the `Matchmaking` that owns the object.
unsafe fn push(sink: *const Sink, ev: gs_gameui::serverbrowser::MmEvent) {
    // SAFETY: forwarded; callbacks run on the main thread inside SteamAPI_RunCallbacks, while
    // nothing else borrows the sink.
    if let Ok(mut v) = unsafe { &*sink }.try_borrow_mut() {
        v.push(ev);
    }
}

extern "C" fn list_responded(this: *mut ListResponse, h: *mut c_void, i: i32) {
    // SAFETY: Steam calls back with the object we passed; it lives until released.
    let r = unsafe { &*this };
    // SAFETY: as above.
    unsafe { push(r.sink, gs_gameui::serverbrowser::MmEvent::ServerResponded { responder: r.token, request: h as u64, server: i }) }
}

extern "C" fn list_failed(this: *mut ListResponse, h: *mut c_void, i: i32) {
    // SAFETY: as in `list_responded`.
    let r = unsafe { &*this };
    // SAFETY: as above.
    unsafe { push(r.sink, gs_gameui::serverbrowser::MmEvent::ServerFailedToRespond { responder: r.token, request: h as u64, server: i }) }
}

extern "C" fn list_complete(this: *mut ListResponse, h: *mut c_void, response: i32) {
    // SAFETY: as in `list_responded`.
    let r = unsafe { &*this };
    // SAFETY: as above.
    unsafe { push(r.sink, gs_gameui::serverbrowser::MmEvent::RefreshComplete { responder: r.token, request: h as u64, response }) }
}

extern "C" fn ping_responded(this: *mut PingResponse, server: *const u8) {
    // SAFETY: as in `list_responded`; `server` is the reference Steam passes for this call.
    let r = unsafe { &*this };
    // SAFETY: as above.
    let Some(server) = (unsafe { server_at(server) }) else { return };
    // SAFETY: as above.
    unsafe { push(r.sink, gs_gameui::serverbrowser::MmEvent::PingResponded { responder: r.token, server }) }
}

extern "C" fn ping_failed(this: *mut PingResponse) {
    // SAFETY: as in `list_responded`.
    let r = unsafe { &*this };
    // SAFETY: as above.
    unsafe { push(r.sink, gs_gameui::serverbrowser::MmEvent::PingFailed { responder: r.token }) }
}

extern "C" fn players_add(this: *mut PlayersResponse, name: *const std::ffi::c_char, score: i32, time: f32) {
    // SAFETY: as in `list_responded`; `name` is a NUL-terminated string valid for this call.
    let r = unsafe { &*this };
    let name = if name.is_null() {
        String::new()
    } else {
        // SAFETY: as above.
        unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned()
    };
    // SAFETY: as above.
    unsafe { push(r.sink, gs_gameui::serverbrowser::MmEvent::AddPlayer { responder: r.token, name, score, time }) }
}

extern "C" fn players_failed(this: *mut PlayersResponse) {
    // SAFETY: as in `list_responded`.
    let r = unsafe { &*this };
    // SAFETY: as above.
    unsafe { push(r.sink, gs_gameui::serverbrowser::MmEvent::PlayersFailed { responder: r.token }) }
}

extern "C" fn players_complete(this: *mut PlayersResponse) {
    // SAFETY: as in `list_responded`.
    let r = unsafe { &*this };
    // SAFETY: as above.
    unsafe { push(r.sink, gs_gameui::serverbrowser::MmEvent::PlayersComplete { responder: r.token }) }
}

static LIST_VTABLE: ListVtable = ListVtable { responded: list_responded, failed: list_failed, complete: list_complete };
static PING_VTABLE: PingVtable = PingVtable { responded: ping_responded, failed: ping_failed };
static PLAYERS_VTABLE: PlayersVtable = PlayersVtable { add: players_add, failed: players_failed, complete: players_complete };

/// One token's response objects (boxed: Steam keeps their addresses).
struct Responders {
    list: Box<ListResponse>,
    ping: Box<PingResponse>,
    players: Box<PlayersResponse>,
}

/// `CCallback<…, FavoritesListChanged_t>`: `CCallbackBase` (vtable, `m_nCallbackFlags`,
/// `m_iCallback`) plus the sink.
#[repr(C)]
struct FavoritesCallback {
    vtable: *const CallbackVtable,
    flags: u8,
    callback: i32,
    sink: *const Sink,
}

/// `CCallbackBase` as MSVC lays it out: the two `Run` overloads in reverse declaration order,
/// then `GetCallbackSizeBytes`.
#[repr(C)]
struct CallbackVtable {
    run_result: extern "C" fn(*mut FavoritesCallback, *mut c_void, bool, u64),
    run: extern "C" fn(*mut FavoritesCallback, *mut c_void),
    size: extern "C" fn(*mut FavoritesCallback) -> i32,
}

/// `FavoritesListChanged_t` (callback 502): ip, query port, connection port, app id, flags,
/// added, account id.
const FAVORITES_CHANGED: i32 = 502;
const FAVORITES_CHANGED_SIZE: i32 = 0x1c;

extern "C" fn favorites_run(this: *mut FavoritesCallback, param: *mut c_void) {
    if param.is_null() {
        return;
    }
    // SAFETY: Steam calls the registered object with a FavoritesListChanged_t of 0x1c bytes.
    let (cb, b) = unsafe { (&*this, std::slice::from_raw_parts(param as *const u8, FAVORITES_CHANGED_SIZE as usize)) };
    let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let ev = gs_gameui::serverbrowser::MmEvent::FavoritesChanged { ip: u32_at(0), query_port: u32_at(4), conn_port: u32_at(8), app_id: u32_at(0xc), flags: u32_at(0x10), add: b[0x14] != 0 };
    // SAFETY: the sink outlives the registration.
    unsafe { push(cb.sink, ev) }
}

extern "C" fn favorites_run_result(this: *mut FavoritesCallback, param: *mut c_void, _io_failure: bool, _call: u64) {
    favorites_run(this, param);
}

extern "C" fn favorites_size(_this: *mut FavoritesCallback) -> i32 {
    FAVORITES_CHANGED_SIZE
}

static CALLBACK_VTABLE: CallbackVtable = CallbackVtable { run_result: favorites_run_result, run: favorites_run, size: favorites_size };

/// ServerBrowser.dll's Steam interfaces on an initialized API.
pub struct Matchmaking {
    api: std::rc::Rc<SteamApi>,
    servers: *mut c_void,
    matchmaking: *mut c_void,
    apps: *mut c_void,
    request_internet: RequestListFn,
    request_lan: RequestLanFn,
    request_friends: RequestListFn,
    request_favorites: RequestListFn,
    request_history: RequestListFn,
    request_spectator: RequestListFn,
    release_request: RequestOpFn,
    server_details: DetailsFn,
    cancel_query: RequestOpFn,
    refresh_query: RequestOpFn,
    is_refreshing: IsRefreshingFn,
    refresh_server: RefreshServerFn,
    ping_server: QueryFn,
    player_details: QueryFn,
    cancel_server_query: CancelServerQueryFn,
    favorite_count: IfaceIntFn,
    add_favorite: AddFavoriteFn,
    remove_favorite: RemoveFavoriteFn,
    vac_banned: Option<IfaceBoolFn>,
    unregister: UnregisterCallbackFn,
    sink: std::rc::Rc<Sink>,
    responders: std::collections::HashMap<u32, Responders>,
    favorites_cb: Box<FavoritesCallback>,
}

impl Matchmaking {
    /// The interfaces (`SteamMatchMakingServers002`, `SteamMatchMaking009`, `SteamApps`) and the
    /// `FavoritesListChanged_t` registration.
    pub fn new(api: std::rc::Rc<SteamApi>) -> Result<Self, String> {
        let lib = &api.lib;
        // SAFETY: the signatures match steam_api_flat.h of SDK 1.5x–1.6x; every pointer copied
        // here lives in `self` next to the `Rc` that keeps the library loaded.
        unsafe {
            let servers = symbol::<IfaceFn>(lib, b"SteamAPI_SteamMatchmakingServers_v002\0")?();
            let matchmaking = symbol::<IfaceFn>(lib, b"SteamAPI_SteamMatchmaking_v009\0")?();
            let apps = [b"SteamAPI_SteamApps_v009\0".as_slice(), b"SteamAPI_SteamApps_v008\0"].iter().find_map(|n| symbol::<IfaceFn>(lib, n).ok()).map_or(std::ptr::null_mut(), |f| f());
            if servers.is_null() || matchmaking.is_null() {
                return Err("no ISteamMatchmakingServers/ISteamMatchmaking in this steam_api".into());
            }
            let sink = std::rc::Rc::new(Sink::default());
            let mut favorites_cb = Box::new(FavoritesCallback { vtable: &CALLBACK_VTABLE, flags: 0, callback: 0, sink: std::rc::Rc::as_ptr(&sink) });
            let register = symbol::<RegisterCallbackFn>(lib, b"SteamAPI_RegisterCallback\0")?;
            let unregister = symbol::<UnregisterCallbackFn>(lib, b"SteamAPI_UnregisterCallback\0")?;
            register(std::ptr::from_mut::<FavoritesCallback>(&mut favorites_cb).cast(), FAVORITES_CHANGED);
            Ok(Self {
                request_internet: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_RequestInternetServerList\0")?,
                request_lan: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_RequestLANServerList\0")?,
                request_friends: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_RequestFriendsServerList\0")?,
                request_favorites: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_RequestFavoritesServerList\0")?,
                request_history: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_RequestHistoryServerList\0")?,
                request_spectator: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_RequestSpectatorServerList\0")?,
                release_request: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_ReleaseRequest\0")?,
                server_details: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_GetServerDetails\0")?,
                cancel_query: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_CancelQuery\0")?,
                refresh_query: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_RefreshQuery\0")?,
                is_refreshing: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_IsRefreshing\0")?,
                refresh_server: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_RefreshServer\0")?,
                ping_server: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_PingServer\0")?,
                player_details: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_PlayerDetails\0")?,
                cancel_server_query: symbol(lib, b"SteamAPI_ISteamMatchmakingServers_CancelServerQuery\0")?,
                favorite_count: symbol(lib, b"SteamAPI_ISteamMatchmaking_GetFavoriteGameCount\0")?,
                add_favorite: symbol(lib, b"SteamAPI_ISteamMatchmaking_AddFavoriteGame\0")?,
                remove_favorite: symbol(lib, b"SteamAPI_ISteamMatchmaking_RemoveFavoriteGame\0")?,
                vac_banned: if apps.is_null() { None } else { symbol(lib, b"SteamAPI_ISteamApps_BIsVACBanned\0").ok() },
                unregister,
                servers,
                matchmaking,
                apps,
                sink,
                responders: std::collections::HashMap::new(),
                favorites_cb,
                api: api.clone(),
            })
        }
    }

    /// The response objects of `token`, created on first use.
    fn responders(&mut self, token: u32) -> &mut Responders {
        let sink = std::rc::Rc::as_ptr(&self.sink);
        self.responders.entry(token).or_insert_with(|| Responders {
            list: Box::new(ListResponse { vtable: &LIST_VTABLE, token, sink }),
            ping: Box::new(PingResponse { vtable: &PING_VTABLE, token, sink }),
            players: Box::new(PlayersResponse { vtable: &PLAYERS_VTABLE, token, sink }),
        })
    }

    /// `Request*ServerList`: `kind` 0 Internet, 1 LAN, 2 Friends, 3 Favorites, 4 History,
    /// 5 Spectator.
    pub fn request_list(&mut self, kind: u32, app: u32, filters: &[(String, String)], token: u32) -> u64 {
        let servers = self.servers;
        let resp: *mut c_void = std::ptr::from_mut::<ListResponse>(&mut self.responders(token).list).cast();
        let mut pairs: Vec<KvPair> = filters.iter().map(|(k, v)| KvPair::new(k, v)).collect();
        let mut base = pairs.as_mut_ptr();
        let n = u32::try_from(pairs.len()).unwrap_or(0);
        let f = match kind {
            1 => {
                // SAFETY: `servers` is the live interface; `resp` stays valid until released.
                return unsafe { (self.request_lan)(servers, app, resp) } as u64;
            }
            2 => self.request_friends,
            3 => self.request_favorites,
            4 => self.request_history,
            5 => self.request_spectator,
            _ => self.request_internet,
        };
        // SAFETY: as above; the filter array is valid for the call (Steam copies it).
        unsafe { f(servers, app, &mut base, n, resp) as u64 }
    }

    pub fn release_request(&self, h: u64) {
        if h != 0 {
            // SAFETY: `h` came from a request on this interface.
            unsafe { (self.release_request)(self.servers, h as *mut c_void) }
        }
    }

    pub fn server_details(&self, h: u64, i: i32) -> Option<gs_gameui::serverbrowser::GameServerItem> {
        if h == 0 {
            return None;
        }
        // SAFETY: Steam returns null or its own item, valid until the next call.
        unsafe { server_at((self.server_details)(self.servers, h as *mut c_void, i)) }
    }

    pub fn cancel_query(&self, h: u64) {
        if h != 0 {
            // SAFETY: as in `release_request`.
            unsafe { (self.cancel_query)(self.servers, h as *mut c_void) }
        }
    }

    pub fn refresh_query(&self, h: u64) {
        if h != 0 {
            // SAFETY: as in `release_request`.
            unsafe { (self.refresh_query)(self.servers, h as *mut c_void) }
        }
    }

    pub fn is_refreshing(&self, h: u64) -> bool {
        // SAFETY: as in `release_request`.
        h != 0 && unsafe { (self.is_refreshing)(self.servers, h as *mut c_void) }
    }

    pub fn refresh_server(&self, h: u64, i: i32) {
        if h != 0 {
            // SAFETY: as in `release_request`.
            unsafe { (self.refresh_server)(self.servers, h as *mut c_void, i) }
        }
    }

    pub fn ping_server(&mut self, ip: u32, port: u16, token: u32) -> i32 {
        let servers = self.servers;
        let resp: *mut c_void = std::ptr::from_mut::<PingResponse>(&mut self.responders(token).ping).cast();
        // SAFETY: `resp` stays valid until the token is released after its queries end.
        unsafe { (self.ping_server)(servers, ip, port, resp) }
    }

    pub fn player_details(&mut self, ip: u32, port: u16, token: u32) -> i32 {
        let servers = self.servers;
        let resp: *mut c_void = std::ptr::from_mut::<PlayersResponse>(&mut self.responders(token).players).cast();
        // SAFETY: as in `ping_server`.
        unsafe { (self.player_details)(servers, ip, port, resp) }
    }

    pub fn cancel_server_query(&self, q: i32) {
        if q != -1 {
            // SAFETY: `q` came from PingServer/PlayerDetails on this interface.
            unsafe { (self.cancel_server_query)(self.servers, q) }
        }
    }

    /// The token's objects go; its queries were cancelled by the caller.
    pub fn release_responder(&mut self, token: u32) {
        self.responders.remove(&token);
    }

    pub fn favorite_count(&self) -> i32 {
        // SAFETY: the interface is live.
        unsafe { (self.favorite_count)(self.matchmaking) }
    }

    pub fn add_favorite(&self, app: u32, ip: u32, conn: u16, query: u16, flags: u32, time: u32) -> i32 {
        // SAFETY: the interface is live.
        unsafe { (self.add_favorite)(self.matchmaking, app, ip, conn, query, flags, time) }
    }

    pub fn remove_favorite(&self, app: u32, ip: u32, conn: u16, query: u16, flags: u32) -> bool {
        // SAFETY: the interface is live.
        unsafe { (self.remove_favorite)(self.matchmaking, app, ip, conn, query, flags) }
    }

    pub fn vac_banned(&self) -> bool {
        // SAFETY: `apps` is the live interface whenever the function was found.
        self.vac_banned.is_some_and(|f| unsafe { f(self.apps) })
    }

    /// `SteamAPI_RunCallbacks`, then what the response objects collected.
    pub fn run_callbacks(&mut self) -> Vec<gs_gameui::serverbrowser::MmEvent> {
        self.api.run_callbacks();
        std::mem::take(&mut *self.sink.borrow_mut())
    }
}

impl Drop for Matchmaking {
    fn drop(&mut self) {
        // SAFETY: the callback object was registered in `new` and is still alive.
        unsafe { (self.unregister)(std::ptr::from_mut::<FavoritesCallback>(&mut self.favorites_cb).cast()) }
    }
}
