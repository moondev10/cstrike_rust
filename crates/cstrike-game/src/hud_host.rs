//! The engine side of the HUD (docs/re/client-hud.md §2.3): [`HudEngine`] over the console,
//! the client, the renderer's sprite uploads and the 2D screen, plus the screen's VGUI2 scheme
//! and localization files.

use std::sync::Arc;

use cs_client::hud::{HudEngine, HudTrace, PlayerInfo};
use gs_assets::RgbaImage;
use gs_client::Client;
use gs_common::info::InfoString;
use gs_console::Console;
use gs_draw::localize::Localize;
use gs_draw::scheme::Scheme;
use gs_draw::titles::{TextMessage, Titles};
use gs_draw::{Image, Screen, SprHandle, SpriteLoader};
use gs_formats::sprite::Sprite;
use gs_fs::FileSystem;
use gs_render::Renderer;
use gs_sound::chan;

use crate::audio::Audio;

/// Uploads HUD sprites and VGUI images through the renderer.
pub(crate) struct Loader<'a> {
    pub(crate) renderer: &'a mut Renderer,
    pub(crate) fs: &'a FileSystem,
}

impl SpriteLoader for Loader<'_> {
    fn load_sprite(&mut self, name: &str) -> Option<(Arc<Sprite>, u32)> {
        let data = self.fs.read(name).map_err(|e| log::debug!("HUD sprite {name}: {e}")).ok()?;
        let sprite = Arc::new(Sprite::parse(&data).map_err(|e| log::warn!("HUD sprite {name}: {e}")).ok()?);
        let texture = self.renderer.upload_overlay_sprite(name, &sprite);
        Some((sprite, texture))
    }

    fn load_image(&mut self, name: &str) -> Option<Image> {
        self.reload_image(name, None)
    }

    /// `DrawSetTextureFile`: `<name>.tga`, else `<name>.bmp` (`LoadBMP8`).
    fn reload_image(&mut self, name: &str, prev: Option<Image>) -> Option<Image> {
        let tga = format!("{name}.tga");
        let (path, w, h, pixels) = match RgbaImage::load(self.fs, &tga) {
            Ok(img) => (tga, img.width, img.height, img.pixels),
            Err(e) => {
                let bmp = format!("{name}.bmp");
                let data = self.fs.read(&bmp).map_err(|_| log::warn!("VGUI image {tga}: {e}")).ok()?;
                let (w, h, px) = load_bmp8(&data)?;
                (bmp, w, h, px)
            }
        };
        let texture = match prev {
            Some(p) => self.renderer.replace_overlay_image(p.texture, &path, w, h, &pixels),
            None => self.renderer.upload_overlay_image(&path, w, h, &pixels),
        };
        Some(Image { texture, width: w, height: h })
    }
}

/// `LoadBMP8`: an 8-bit palettised BMP (palette right after a 40-byte info header, 256
/// entries), rows with Valve's stride arithmetic, bottom-up, alpha 255.
fn load_bmp8(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if data.len() < 14 || &data[..2] != b"BM" {
        return None;
    }
    let info = &data[14..];
    let rd = |o: usize| info.get(o..o + 4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (w, h) = (rd(4)?, rd(8)?);
    let (wu, hu) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
    let pal = info.get(40..40 + 1024)?;
    let pixels = info.get(40 + 1024..)?;
    let stride = if w % 4 == 0 { w } else { w + (w + 4) % 4 } as usize;
    let mut out = vec![0u8; wu as usize * hu as usize * 4];
    for y in 0..hu as usize {
        let src = pixels.get((hu as usize - 1 - y) * stride..)?;
        for x in 0..wu as usize {
            let i = usize::from(*src.get(x)?) * 4;
            let o = (y * wu as usize + x) * 4;
            out[o..o + 4].copy_from_slice(&[pal[i + 2], pal[i + 1], pal[i], 255]);
        }
    }
    Some((wu, hu, out))
}

/// The 2D screen with the engine scheme (`TrackerScheme.res`), the client scheme
/// (`ClientScheme.res`) and the VGUI2 localization files (base game, GameUI, then the mod's),
/// at the game language.
pub fn new_screen(fs: &FileSystem) -> Screen {
    let scheme = |file: &str| {
        fs.read(file).map(|d| Scheme::parse(&String::from_utf8_lossy(&d))).unwrap_or_else(|e| {
            log::warn!("{file}: {e}");
            Scheme::default()
        })
    };
    let mut localize = Localize::default();
    for pattern in ["resource/valve_%language%.txt", "resource/gameui_%language%.txt", "resource/cstrike_%language%.txt"] {
        for file in fs.language_files(pattern) {
            match fs.read(&file) {
                Ok(data) => localize.add_file(&data),
                Err(e) => log::debug!("{file}: {e}"),
            }
        }
    }
    let mut screen = Screen::new(scheme("resource/TrackerScheme.res"), localize);
    screen.set_language(fs.language().unwrap_or("english"));
    screen.set_client_scheme(scheme("resource/ClientScheme.res"));
    screen
}

/// `TextMessageInit`: `titles.txt` through the game search path.
pub fn load_titles(fs: &FileSystem) -> Titles {
    fs.read("titles.txt").map(|d| Titles::parse(&gs_common::bytestr::decode(&d))).unwrap_or_else(|e| {
        log::warn!("titles.txt: {e}");
        Titles::default()
    })
}

pub struct HudHost<'a> {
    pub screen: &'a mut Screen,
    pub renderer: &'a mut Renderer,
    pub fs: &'a FileSystem,
    pub console: &'a mut Console,
    pub client: &'a mut Client,
    pub titles: &'a Titles,
    pub audio: Option<&'a mut Audio>,
    /// The engine's effects (`None` from console commands, which create no lights).
    pub effects: Option<&'a mut gs_effects::Effects>,
    /// The MOTD panel's browser.
    pub motd: Option<&'a mut crate::motd::MotdBrowser>,
}

impl HudHost<'_> {
    /// The connected game server is on this machine or its local network (a LAN game).
    fn server_is_local(&self) -> bool {
        self.client.server_addr().is_some_and(|a| gs_common::net::is_local(std::net::IpAddr::V4(*a.ip())))
    }
}

impl HudEngine for HudHost<'_> {
    fn screen(&mut self) -> &mut Screen {
        self.screen
    }

    fn spr_load(&mut self, name: &str) -> SprHandle {
        self.screen.spr_load(name, &mut Loader { renderer: self.renderer, fs: self.fs })
    }

    fn load_image(&mut self, name: &str) -> Option<Image> {
        self.screen.load_image(name, &mut Loader { renderer: self.renderer, fs: self.fs })
    }

    fn read_file(&mut self, path: &str) -> Option<Vec<u8>> {
        self.fs.read(path).ok()
    }

    fn cvar(&self, name: &str) -> f32 {
        self.console.cvars.value(name)
    }

    fn cvar_string(&self, name: &str) -> String {
        self.console.cvars.string(name).to_owned()
    }

    /// `pfnClientCmd`: `Cbuf_AddText(text)`, then `Cbuf_AddText("\n")`.
    fn client_cmd(&mut self, text: &str) {
        self.console.add_text(text);
        self.console.add_text("\n");
    }

    fn server_cmd(&mut self, text: &str) {
        self.client.server_cmd(text, false);
    }

    fn console_print(&mut self, text: &str) {
        crate::notify::con_printf(text.trim_start_matches('\u{1}'));
    }

    fn player_info(&self, index: i32) -> Option<PlayerInfo> {
        let slot = usize::try_from(index).ok()?.checked_sub(1)?;
        if index > i32::from(self.client.protocol().maxclients) {
            return None;
        }
        let p = self.client.level().players.get(slot)?;
        if p.name.is_empty() {
            return None;
        }
        let bot = InfoString::from_raw(&p.info, 512).get("*bot").is_some_and(|v| v.trim().parse::<i32>().unwrap_or(0) > 0);
        Some(PlayerInfo { name: p.name.clone(), thisplayer: slot == usize::from(self.client.protocol().playernum), ping: p.ping, bot })
    }

    fn play_sound(&mut self, name: &str, volume: f32) {
        if let Some(audio) = self.audio.as_deref_mut() {
            audio.hud_sound(chan::ITEM, name, volume, 100);
        }
    }

    fn play_sound_pitch(&mut self, name: &str, volume: f32, pitch: i32) {
        if let Some(audio) = self.audio.as_deref_mut() {
            audio.hud_sound(chan::ITEM, name, volume, pitch);
        }
    }

    fn play_sound_voice(&mut self, name: &str, volume: f32, pitch: i32) {
        if let Some(audio) = self.audio.as_deref_mut() {
            audio.hud_sound(chan::VOICE_BY_NAME, name, volume, pitch);
        }
    }

    fn text_message(&self, name: &str) -> Option<TextMessage> {
        self.titles.get(name).cloned()
    }

    fn max_clients(&self) -> i32 {
        i32::from(self.client.protocol().maxclients)
    }

    fn demo_playing_back(&self) -> bool {
        self.client.demo_playing()
    }

    fn level_name(&self) -> String {
        self.client.server_info().map(|i| i.map_model.clone()).unwrap_or_default()
    }

    fn local_userinfo(&self, key: &str) -> Option<String> {
        self.client.config.userinfo.get(key).map(str::to_owned)
    }

    fn random_long(&mut self, lo: i32, hi: i32) -> i32 {
        self.client.rng().long(lo, hi)
    }

    fn create_light(&mut self, origin: [f32; 3], radius: f32, color: [u8; 3], die: f32) {
        if let Some(effects) = self.effects.as_deref_mut() {
            let dl = effects.alloc_dlight(0);
            dl.origin = origin;
            dl.radius = radius;
            dl.color = color;
            dl.die = die;
        }
    }

    fn player_unique_id(&self, index: i32) -> Option<[u8; 16]> {
        if index > i32::from(self.client.protocol().maxclients) {
            return None;
        }
        self.client.level().player_unique_id(index)
    }

    fn server_cmd_unreliable(&mut self, text: &str) {
        self.client.server_cmd(text, true);
    }

    fn game_dir_file(&self, name: &str) -> std::path::PathBuf {
        self.fs.game_dir().map_or_else(|| std::path::PathBuf::from(name), |d| d.join(name))
    }

    fn client_time(&self) -> f32 {
        self.client.time() as f32
    }

    fn motd_navigate(&mut self, url: &str) {
        let local = self.server_is_local();
        if let Some(browser) = self.motd.as_deref_mut() {
            browser.navigate(url, local);
        }
    }

    /// `motd_temp.html` in GAMECONFIG, written verbatim and loaded (`file:///…`), or deleted.
    fn motd_temp_file(&mut self, html: Option<&str>) {
        match html {
            Some(page) => {
                if let Err(e) = self.fs.write("motd_temp.html", gs_fs::PathId::GameConfig, &gs_common::bytestr::encode(page)) {
                    log::debug!("motd_temp.html: {e}");
                }
                let local = self.server_is_local();
                if let Some(browser) = self.motd.as_deref_mut() {
                    browser.load_html(page, local);
                }
            }
            None => {
                self.fs.remove("motd_temp.html", Some(gs_fs::PathId::GameConfig));
            }
        }
    }

    fn motd_shows_pages(&self) -> bool {
        self.motd.as_deref().is_some_and(crate::motd::MotdBrowser::shows_pages)
    }

    fn cvar_set(&mut self, name: &str, value: &str) {
        if self.console.cvars.direct_set(name, value) == gs_console::cvar::SetResult::Missing {
            log::debug!("Cvar_Set: variable {name} not found");
        }
    }

    fn cvar_set_value(&mut self, name: &str, value: f32) {
        self.console.cvars.set_value_only(name, value);
    }

    fn spectate_only(&self) -> bool {
        self.client.spectator()
    }

    fn key_for_binding(&self, binding: &str) -> Option<u8> {
        self.console.bindings.lookup(binding)
    }

    fn entity(&self, index: i32) -> Option<cs_client::view::SpecEntity> {
        crate::spectate::spec_entity(self.client, index)
    }

    fn trace_line(&self, start: [f32; 3], end: [f32; 3], ignore: i32) -> HudTrace {
        let Some(world) = self.client.world() else { return HudTrace { fraction: 1.0, endpos: end, ent: -1 } };
        let physents = self.client.event_physents(-1);
        let tr = gs_pmove::trace::player_trace(world, &physents, 2, start, end, 0, ignore);
        HudTrace { fraction: tr.fraction, endpos: tr.endpos, ent: tr.ent }
    }

    fn map_entities(&self) -> Option<String> {
        let lump = &self.client.level().entity_lump;
        (!lump.is_empty()).then(|| lump.clone())
    }

    fn ev_play_sound(&mut self, ent: i32, origin: [f32; 3], channel: i32, sample: &str, volume: f32, attn: f32, flags: i32, pitch: i32) {
        if let Some(audio) = self.audio.as_deref_mut() {
            audio.request(&crate::audio::SoundRequest::Play {
                ent,
                origin,
                channel,
                sample: sample.to_owned(),
                volume,
                attenuation: attn,
                flags,
                pitch,
            });
        }
    }

    /// `pfnFilteredClientCmd`: `CL_FilteredCmdAllowed`, then the filtered buffer.
    fn filtered_client_cmd(&mut self, text: &str) {
        let policy = gs_console::stufftext::StuffPolicy {
            single_player: self.client.protocol().maxclients == 1 && !self.client.demo_playing(),
            filter: self.console.cvars.value("cl_filterstuffcmd") > 0.0,
            ..Default::default()
        };
        if gs_console::stufftext::valid_stuff_text(text, policy) {
            self.console.add_filtered_text(text);
            self.console.add_filtered_text("\n");
        }
    }
}
