//! The resource phase of the sign-on and in-game customizations (docs/re/engine-core.md §5,
//! engine-misc.md §4–§5): the needed / on-hand resource lists, `CL_StartResourceDownloading` and
//! `CL_BatchResourceRequest` (UDP `dlfile` or `sv_downloadurl` HTTP with the disconnect and
//! `retry`), `CL_ProcessFile`, the local logo in `custom.hpk`, `clc_resourcelist`,
//! `svc_customization`, `upload` and `httpstop`.

use gs_protocol::MsgWriter;
use gs_protocol::clc::{self, op};
use gs_protocol::messages::{Customization as CustomizationMsg, Resource, ResourceList, resource_flags};

use super::{Client, ClientEvent, State, UiEvent, is_safe_download};
use crate::custom::{Customization, T_DECAL, local_logo};
use crate::hpak::{Hpak, md5_from_name, md5_hex};
use crate::resources::{self, kind};

/// `RES_REQUESTED`: `dlfile !MD5…` was sent for it.
const REQUESTED: u8 = 0x08;

/// What a player's spray draws (`TE_PLAYERDECAL`).
#[derive(Debug, Clone, Copy)]
pub enum PlayerDecal<'a> {
    /// No customization: the `decals.wad` decal of the message's index.
    None,
    /// A customization without a valid logo: nothing.
    Invalid,
    /// The logo texture and its MD5.
    Logo(&'a gs_formats::miptex::MipTex, [u8; 16]),
}

impl Client {
    /// `CL_ClearState`'s resource part: the lists and the download URL cleared, the HPAK queue
    /// flushed, and `tempdecal.wad` registered as the local custom resource (stored in
    /// `custom.hpk` right away).
    pub(super) fn clear_resources(&mut self) {
        self.needed.clear();
        self.on_hand.clear();
        self.download_url.clear();
        self.hpak.flush(&self.fs);
        self.local_resources.clear();
        if let Some((resource, data)) = local_logo(&self.fs) {
            if let Err(e) = Hpak::add(&self.fs, &resource, &data) {
                log::warn!("{e}");
            }
            self.local_resources.push((resource, data));
        }
    }

    /// `CL_ParseServerInfo`'s `HPAK_CheckSize("custom")`, then (after the level is set up) the
    /// local player's own customizations from the local list.
    pub(super) fn server_info_customizations(&mut self, playernum: usize) {
        Hpak::check_size(&self.fs, self.config.hpk_maxsize);
        for c in &mut self.customizations {
            c.clear();
        }
        let local: Vec<_> = self.local_resources.clone();
        for (resource, data) in local {
            self.add_customization(playernum, resource, data);
        }
    }

    /// `svc_resourcerequest`: `clc_resourcelist` with the local custom resources (the logo).
    pub(super) fn resource_request(&mut self, spawncount: i32, start: i32) {
        if self.state < State::Connecting {
            return;
        }
        let same_level = self.demo.playing() || self.server_info.as_ref().is_some_and(|i| i.spawncount == spawncount);
        let total = self.local_resources.len();
        if !same_level {
            log::warn!("svc_resourcerequest from a different level ({spawncount})");
            return;
        }
        let Some(start) = usize::try_from(start).ok().filter(|&s| s <= total) else { return };
        let mut w = MsgWriter::new();
        w.write_u8(op::RESOURCELIST);
        w.write_i16(total as i16);
        for (r, _) in &self.local_resources[start..] {
            w.write_string(&r.name);
            w.write_u8(r.kind);
            w.write_i16(r.index as i16);
            w.write_i32(r.download_size as i32);
            w.write_u8(r.flags);
            if r.flags & resource_flags::CUSTOM != 0 {
                w.write_bytes(&r.md5.unwrap_or_default());
            }
        }
        if let Some(chan) = self.netchan.as_mut() {
            chan.queue_fragmented(w.as_bytes());
        }
    }

    /// `CL_ParseResourceLocation`: an http(s) base URL, with a trailing `/`, up to 127 chars.
    pub(super) fn resource_location(&mut self, url: &str) {
        let lower = url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return;
        }
        let mut s = if url.ends_with('/') { url.to_owned() } else { format!("{url}/") };
        gs_common::bytestr::truncate_encoded_in_place(&mut s, 127);
        self.events.push_back(ClientEvent::DevPrint(format!("Using {s} as primary download location\n")));
        self.download_url = s;
    }

    /// `svc_resourcelist`: the list becomes the needed list (WASMISSING stripped), then
    /// `CL_StartResourceDownloading`.
    pub(super) fn resource_list_received(&mut self, list: ResourceList) {
        self.needed = list
            .resources
            .iter()
            .map(|r| Resource { flags: r.flags & !resource_flags::WAS_MISSING, ..r.clone() })
            .collect();
        self.startup_mark("end   CL_ParseResourceList()");
        self.startup_mark("begin CL_ParseConsistencyInfo()");
        if !list.consistency.is_empty() {
            self.startup_mark("end   CL_ParseConsistencyInfo()");
        }
        self.resources = Some(list);
        self.events.push_back(ClientEvent::DevPrint("Verifying and downloading resources...\n".into()));
        self.start_resource_downloading(false);
    }

    /// `CL_StartResourceDownloading`: missing files get WASMISSING (present ones go on hand),
    /// then the batch request.
    fn start_resource_downloading(&mut self, custom: bool) {
        self.startup_mark("begin CL_StartResourceDownloading()");
        let mut needed = Vec::with_capacity(self.needed.len());
        for mut r in std::mem::take(&mut self.needed) {
            let missing = if r.kind == T_DECAL { r.flags & resource_flags::CUSTOM != 0 } else { resources::is_missing(&self.fs, &r) };
            if missing {
                r.flags |= resource_flags::WAS_MISSING;
                needed.push(r);
            } else if r.flags & resource_flags::WAS_MISSING != 0 {
                needed.push(r);
            } else {
                self.on_hand.push(r);
            }
        }
        self.needed = needed;
        self.dl_custom = custom;
        if !custom {
            self.state = State::Uninitialized;
        }
        self.startup_mark("end   CL_StartResourceDownloading()");
        self.batch_resource_request();
    }

    /// `CL_BatchResourceRequest`: every missing file requested in one fragmented buffer
    /// (`dlfile`), or queued for HTTP — which disconnects for the downloads; with nothing
    /// outstanding the registration follows in the same buffer.
    fn batch_resource_request(&mut self) {
        let mut w = MsgWriter::new();
        let mut http = false;
        let mut needed = Vec::with_capacity(self.needed.len());
        for mut r in std::mem::take(&mut self.needed) {
            if r.flags & resource_flags::WAS_MISSING == 0 {
                needed.push(r);
                continue;
            }
            if self.state == State::Active && !self.config.download_ingame {
                self.print(format!("Skipping in game download of {}\n", r.name));
                self.on_hand.push(r);
                continue;
            }
            if !is_safe_download(&r.name) {
                self.print(format!("Invalid file type...skipping download of {}\n", r.name));
                continue;
            }
            let on_hand = match r.kind {
                kind::SOUND if !r.name.starts_with('*') => self.check_file(&format!("sound/{}", r.name), &mut w, &mut http),
                kind::SKIN => true,
                kind::MODEL if !r.name.starts_with('*') => self.check_file(&r.name.clone(), &mut w, &mut http),
                kind::GENERIC | kind::EVENTSCRIPT => self.check_file(&r.name.clone(), &mut w, &mut http),
                T_DECAL => {
                    let md5 = r.md5.unwrap_or_default();
                    if self.hpak.get(&self.fs, &md5).is_some() {
                        true
                    } else {
                        if r.flags & REQUESTED == 0 {
                            clc::stringcmd(&mut w, &format!("dlfile !MD5{}", md5_hex(&md5)));
                            r.flags |= REQUESTED;
                        }
                        false
                    }
                }
                _ => false,
            };
            if on_hand {
                self.on_hand.push(r);
            } else {
                needed.push(r);
            }
        }
        self.needed = needed;
        if http {
            // The HTTP downloads run disconnected; `retry` comes back afterwards. Meanwhile the
            // loading dialog shows them (VGuiWrap2_StartProgressBar with the URL as the type,
            // skipped in a single-player game, and the status text).
            let key = self.http_key();
            self.http.mark_tried(&key);
            let (url, steps) = (self.download_url.clone(), self.http.queued() as i32);
            let single = self.proto.maxclients == 1;
            self.disconnect("");
            if !single {
                self.ui(UiEvent::StartProgress { kind: url, steps });
            }
            self.ui(UiEvent::Status("#GameUI_VerifyingAndDownloading".to_owned()));
            return;
        }
        if self.needed.is_empty() && !self.dl_custom {
            if !self.precache_check() {
                return;
            }
            self.register_resources(&mut w);
        }
        // Netchan_CreateFragments even for an empty batch: pending reliable data moves into
        // fragments too.
        if !self.demo.playing()
            && !self.passive
            && let Some(chan) = self.netchan.as_mut()
        {
            chan.queue_fragmented(w.as_bytes());
        }
    }

    /// `"<cls.servername>:<cl.levelname>"`.
    fn http_key(&self) -> String {
        format!("{}:{}", self.server_name, self.server_info.as_ref().map_or("", |i| i.map_model.as_str()))
    }

    /// `CL_CheckFile`: `true` when nothing is fetched (refused, present, playback); else an HTTP
    /// download is queued or `dlfile` written.
    fn check_file(&mut self, name: &str, w: &mut MsgWriter, http: &mut bool) -> bool {
        if !self.config.allow_download {
            self.events.push_back(ClientEvent::DevPrint("Download refused, cl_allow_download is 0\n".into()));
            return true;
        }
        if self.state == State::Active && !self.config.download_ingame {
            self.events.push_back(ClientEvent::DevPrint("In-game download refused...\n".into()));
            return true;
        }
        if self.fs.exists(name) {
            return true;
        }
        if self.demo.playing() || self.passive {
            self.print(format!("Warning! File {name} missing during demo playback.\n"));
            return true;
        }
        let name = gs_common::bytestr::truncate_encoded(name, 259).into_owned();
        if self.http.can_use(&self.download_url, &self.http_key()) {
            let server = self.server.map_or_else(|| self.server_name.clone(), |a| a.to_string());
            self.http.queue(&self.download_url, &name, &server);
            *http = true;
        } else {
            clc::stringcmd(w, &format!("dlfile {name}"));
        }
        false
    }

    /// A finished (`Some(data)`) or failed file (`CL_ProcessFile`): matched by MD5 for `!MD5`
    /// names, else by name without `sound/`; a custom lump of the declared size goes into the
    /// HPAK queue and becomes its player's customization; the entry moves on hand and the
    /// registration follows once nothing is needed.
    pub(super) fn process_file(&mut self, name: &str, data: Option<&[u8]>) {
        let md5 = md5_from_name(name);
        let bare = name.get(..6).filter(|p| p.eq_ignore_ascii_case("sound/")).map_or(name, |_| &name[6..]);
        let found = self.needed.iter().position(|r| match md5 {
            Some(m) => r.md5 == Some(m),
            None => r.name.eq_ignore_ascii_case(bare) || r.name.eq_ignore_ascii_case(name),
        });
        let Some(at) = found else {
            log::debug!("CL_ProcessFile: {name} not in the needed list");
            return;
        };
        let mut r = self.needed.remove(at);
        if let Some(data) = data {
            r.flags &= !resource_flags::WAS_MISSING;
            if name.starts_with('!') {
                if data.len() != r.download_size as usize {
                    self.print(format!("Downloaded {} bytes for purported {} byte file, ignoring download\n", data.len(), r.download_size));
                } else if r.flags & resource_flags::CUSTOM != 0 && self.hpak.add_queued(&r, data) {
                    let player = self.custom_players.iter().position(|(m, _)| Some(*m) == r.md5).map(|i| self.custom_players.remove(i).1);
                    if let Some(player) = player {
                        self.add_customization(player, r.clone(), data.to_vec());
                    }
                }
            }
        }
        self.on_hand.push(r);
        if self.needed.is_empty() && !self.dl_custom {
            if !self.precache_check() {
                return;
            }
            let mut w = MsgWriter::new();
            self.register_resources(&mut w);
            if let Some(chan) = self.netchan.as_mut() {
                chan.queue_fragmented(w.as_bytes());
            }
        } else if self.needed.is_empty() {
            self.dl_custom = false;
        }
    }

    /// `CL_PrecacheResources`' outcome for files still missing, in on-hand order: a sound
    /// disconnects only when it is fatal and `S_PrecacheSound` gives NULL (no sound system,
    /// `nosound`, a name of 64+ bytes; a `WASMISSING` sound just gets no slot); a model (not
    /// `*N`) prints when it has flags and disconnects when fatal; a script prints and
    /// disconnects when fatal; skins and generic files are never checked.
    fn precache_check(&mut self) -> bool {
        self.ui(UiEvent::Status("#GameUI_PrecachingResources".to_owned()));
        self.ui(UiEvent::ContinueProgress(7));
        let mut reason = None;
        let mut lines = Vec::new();
        for r in &self.on_hand {
            let fatal = r.flags & resource_flags::FATAL_IF_MISSING != 0;
            let exists = || self.fs.exists(&resources::resource_path(r));
            match r.kind {
                kind::SOUND => {
                    let null = !self.config.sound_started || self.config.nosound || gs_common::bytestr::encoded_len(&r.name) >= 64;
                    if r.flags & resource_flags::WAS_MISSING == 0 && null && fatal {
                        reason = Some(format!("Cannot continue without sound {}, disconnecting.", r.name));
                    }
                }
                kind::MODEL if !r.name.starts_with('*') && !exists() => {
                    if r.flags != 0 {
                        lines.push((false, format!("Model {} not found and not available from server\n", r.name)));
                    }
                    if fatal {
                        reason = Some(format!("Cannot continue without model {}, disconnecting.", r.name));
                    }
                }
                kind::EVENTSCRIPT if !exists() => {
                    lines.push((false, format!("Script {} not found and not available from server\n", r.name)));
                    if fatal {
                        reason = Some(format!("Cannot continue without script {}, disconnecting.", r.name));
                    }
                }
                kind::SKIN | kind::MODEL | kind::DECAL | kind::GENERIC | kind::EVENTSCRIPT => {}
                _ => lines.push((true, "Unknown resource type\n".to_owned())),
            }
            if reason.is_some() {
                break;
            }
        }
        for (developer, line) in lines {
            if developer {
                self.events.push_back(ClientEvent::DevPrint(line));
            } else {
                self.print(line);
            }
        }
        let Some(reason) = reason else { return true };
        self.print(format!("{reason}\n"));
        self.disconnect(&reason);
        false
    }

    /// A received file stream: `!` files stay in memory; others are written to GAMEDOWNLOAD
    /// unless present; either way `CL_ProcessFile`.
    pub(super) fn file_received(&mut self, name: &str, data: &[u8]) {
        if !self.config.allow_download {
            self.print("File fragment received but cl_allow_download set to 0, ignoring\n".into());
            return;
        }
        if !name.starts_with('!') {
            if !is_safe_download(name) {
                self.print(format!("refusing unsafe download name {name}\n"));
                return;
            }
            if self.fs.exists(name) {
                self.print(format!("Can't download {name}, already exists\n"));
            } else if let Err(e) = self.fs.write(name, gs_fs::PathId::GameDownload, data) {
                self.print(format!("writing {name}: {e}\n"));
            }
        }
        self.process_file(name, Some(data));
    }

    /// `COM_CreateCustomization` for `player`, unless that MD5 is already there.
    fn add_customization(&mut self, player: usize, resource: Resource, data: Vec<u8>) {
        let Some(list) = self.customizations.get_mut(player) else { return };
        if list.iter().any(|c| c.resource.md5 == resource.md5) {
            return;
        }
        list.push(Customization::new(resource, data));
    }

    /// `CL_ParseCustomization`: another player's logo, from `custom.hpk` or requested from the
    /// server (`dlfile !MD5…`); the previous one of that type is dropped (and its decals).
    pub(super) fn customization(&mut self, msg: CustomizationMsg) {
        let player = usize::from(msg.player);
        let mut r = msg.resource;
        r.flags &= !resource_flags::WAS_MISSING;
        if !self.config.allow_download {
            self.print("Refusing new resource, cl_allow_download set to 0\n".into());
            return;
        }
        if self.state == State::Active && !self.config.download_ingame {
            self.print("Refusing new resource, cl_download_ingame set to 0\n".into());
            return;
        }
        if let Some(list) = self.customizations.get_mut(player) {
            let before = list.len();
            list.retain(|c| c.resource.kind != r.kind);
            if r.kind == T_DECAL && before != list.len() {
                self.events.push_back(ClientEvent::CustomizationRemoved(player));
            }
        }
        let md5 = r.md5.unwrap_or_default();
        match self.hpak.get(&self.fs, &md5) {
            Some(data) => self.add_customization(player, r, data),
            None => {
                self.print(format!("Requesting {} from server\n", r.name));
                r.flags |= resource_flags::WAS_MISSING;
                self.custom_players.push((md5, player));
                self.needed.push(r);
                self.start_resource_downloading(true);
            }
        }
    }

    /// `TE_PLAYERDECAL`'s customization lookup for 0-based `slot`.
    pub fn player_decal(&self, slot: usize) -> PlayerDecal<'_> {
        let Some(list) = self.customizations.get(slot).filter(|l| !l.is_empty()) else { return PlayerDecal::None };
        match list.iter().find(|c| c.resource.kind == T_DECAL && c.resource.flags & resource_flags::CUSTOM != 0) {
            Some(c) => c.texture.as_ref().map_or(PlayerDecal::Invalid, |t| PlayerDecal::Logo(t, c.resource.md5.unwrap_or_default())),
            None => PlayerDecal::Invalid,
        }
    }

    /// `CL_HTTPUpdate`, every frame (connected or not): the downloads' lines, the secondary
    /// progress, each start's "Http" progress point with its file, and at the end the dialog
    /// stopped (`NotifyOfServerDisconnect`) before `retry`.
    pub(super) fn http_update(&mut self) {
        let out = self.http.update(&self.fs);
        for (dev, line) in out.lines {
            self.events.push_back(if dev { ClientEvent::DevPrint(line) } else { ClientEvent::Print(line) });
        }
        if let Some((fraction, file)) = out.progress {
            self.ui(UiEvent::SecondaryProgress(fraction));
            self.ui(UiEvent::SecondaryText(file));
        }
        if out.retry {
            self.ui(UiEvent::ServerDisconnected);
            self.events.push_back(ClientEvent::AddText("retry\n".into()));
        }
        for (point, file) in out.started {
            self.ui(UiEvent::ContinueProgress(point));
            self.ui(UiEvent::Status("#GameUI_VerifyingAndDownloading".to_owned()));
            self.ui(UiEvent::SecondaryText(file));
            self.ui(UiEvent::SecondaryProgress(0.0));
        }
    }

    /// `upload "!MD5<hex>"` (the server asking for our logo).
    pub(super) fn cmd_upload(&mut self, argv: &[&str]) {
        if !self.config.allow_upload {
            return;
        }
        let Some(&name) = argv.get(1).filter(|n| n.len() == 36) else { return };
        let Some(md5) = md5_from_name(name) else { return };
        let Some(data) = self.hpak.upload_data(&self.fs, &md5) else {
            log::warn!("upload {name}: not in custom.hpk");
            return;
        };
        if let Some(chan) = self.netchan.as_mut() {
            chan.queue_file(name, &data);
        }
    }

    /// `httpstop`.
    pub(super) fn cmd_httpstop(&mut self) {
        for line in self.http.stop() {
            self.events.push_back(ClientEvent::DevPrint(line));
        }
    }
}
