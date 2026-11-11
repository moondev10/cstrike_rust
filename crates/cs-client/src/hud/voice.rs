//! `CVoiceStatus`, `CVoiceStatusHud` and `CVoiceBanMgr` (docs/re/voice-ui.md): who is talking,
//! the talker labels and the local transmit icon, the `VModEnable` / `vban` server sync and
//! the voice bans kept in `<gamedir>/voice_ban.dt`.

use super::msg::MsgReader;
use super::{Hud, HudEngine};

/// `CVoiceBanMgr`: 256 buckets keyed by the byte sum of the 16-byte player id.
#[derive(Clone)]
struct BanMgr {
    buckets: Vec<Vec<[u8; 16]>>,
}

impl Default for BanMgr {
    fn default() -> Self {
        Self { buckets: vec![Vec::new(); 256] }
    }
}

impl BanMgr {
    fn bucket(id: &[u8; 16]) -> usize {
        usize::from(id.iter().fold(0u8, |a, &b| a.wrapping_add(b)))
    }

    fn banned(&self, id: &[u8; 16]) -> bool {
        self.buckets[Self::bucket(id)].contains(id)
    }

    /// `SetPlayerBan`.
    fn set(&mut self, id: [u8; 16], ban: bool) {
        let bucket = &mut self.buckets[Self::bucket(&id)];
        match bucket.iter().position(|b| *b == id) {
            None if ban => bucket.push(id),
            Some(i) if !ban => {
                bucket.remove(i);
            }
            _ => {}
        }
    }

    /// `ForEachBannedPlayer`: buckets 0..255, list order.
    fn all(&self) -> impl Iterator<Item = &[u8; 16]> {
        self.buckets.iter().flatten()
    }

    /// `CVoiceBanMgr::Init`: version 1, then 16-byte ids (no duplicate check).
    fn load(data: &[u8]) -> Self {
        let mut mgr = Self::default();
        if data.get(..4).is_some_and(|v| i32::from_le_bytes([v[0], v[1], v[2], v[3]]) == 1) {
            for chunk in data[4..].chunks_exact(16) {
                let mut id = [0u8; 16];
                id.copy_from_slice(chunk);
                mgr.buckets[Self::bucket(&id)].push(id);
            }
        }
        mgr
    }

    /// `SaveState`.
    fn save(&self) -> Vec<u8> {
        let mut out = 1i32.to_le_bytes().to_vec();
        for id in self.all() {
            out.extend_from_slice(id);
        }
        out
    }
}

/// `CVoiceLabel` + its `VoiceVGUILabel`.
#[derive(Clone, Default)]
struct Label {
    /// 0-based player, -1 free.
    client: i32,
    text: String,
    /// Team colour background (alpha 180).
    color: [u8; 3],
    visible: bool,
    pos: (i32, i32),
    size: (i32, i32),
}

#[derive(Clone)]
pub(super) struct VoiceStatus {
    /// `m_LastUpdateServerState`.
    last_update: f32,
    /// `m_bServerModEnable` (-1 until the first update).
    server_mod_enable: i32,
    /// `m_VoicePlayers`, `m_AudiblePlayers`, `m_VoiceEnabledPlayers`, `m_ServerBannedPlayers`.
    voice_players: u32,
    audible_players: u32,
    enabled_players: u32,
    server_banned: u32,
    /// `m_bTalking` (local mic), `m_bServerAcked`.
    talking: bool,
    server_acked: bool,
    bans: BanMgr,
    bans_loaded: bool,
    labels: Vec<Label>,
    /// `LocalPlayerIcon`: shown at this position.
    local_icon: Option<(i32, i32)>,
}

impl Default for VoiceStatus {
    fn default() -> Self {
        Self {
            last_update: 0.0,
            server_mod_enable: -1,
            voice_players: 0,
            audible_players: 0,
            enabled_players: 0,
            server_banned: 0,
            talking: false,
            server_acked: false,
            bans: BanMgr::default(),
            bans_loaded: false,
            labels: Vec::new(),
            local_icon: None,
        }
    }
}

const BAN_FILE: &str = "voice_ban.dt";

fn bit(i: i32) -> u32 {
    u32::try_from(i).ok().filter(|&i| i < 32).map_or(0, |i| 1 << i)
}

impl VoiceStatus {
    /// `CVoiceStatus::Init`'s `CVoiceBanMgr::Init`.
    pub(super) fn init(&mut self, eng: &mut dyn HudEngine) {
        let path = eng.game_dir_file(BAN_FILE);
        self.bans = std::fs::read(&path).map(|d| BanMgr::load(&d)).unwrap_or_default();
        self.bans_loaded = true;
    }

    /// The destructor's `SaveState` (client shutdown).
    pub(super) fn shutdown(&self, eng: &mut dyn HudEngine) {
        if !self.bans_loaded {
            return;
        }
        let path = eng.game_dir_file(BAN_FILE);
        if let Err(e) = std::fs::write(&path, self.bans.save()) {
            log::warn!("{}: {e}", path.display());
        }
    }

    fn debug(eng: &mut dyn HudEngine, text: &str) {
        if eng.cvar("voice_clientdebug") != 0.0 {
            eng.console_print(text);
        }
    }

    /// `GetSpeakerStatus(i)` (0-based): 1 talking, 2 banned, 3 unknown/never talked, 4 silent.
    fn speaker_status(&self, i: i32, eng: &dyn HudEngine) -> i32 {
        let Some(id) = eng.player_unique_id(i + 1) else { return 3 };
        if self.bans.banned(&id) {
            2
        } else if self.enabled_players & bit(i) != 0 {
            if self.voice_players & bit(i) != 0 { 1 } else { 4 }
        } else {
            3
        }
    }

    /// Players (1-based bits) whose head icon `CreateEntities` may draw: talking, not banned,
    /// with an id. The caller applies the entity checks.
    pub(super) fn speaking_mask(&self, eng: &dyn HudEngine) -> u32 {
        (0..32).filter(|&i| self.speaker_status(i, eng) == 1).fold(0, |m, i| m | 1 << i)
    }

    /// `CVoiceStatus::Frame`.
    pub(super) fn frame(&mut self, eng: &mut dyn HudEngine) {
        if eng.client_time() - self.last_update > 1.0 {
            self.update_server_state(false, eng);
        }
    }

    /// `UpdateServerState`: `VModEnable` (reliable) on change, `vban` (unreliable) when the
    /// server's echo differs.
    fn update_server_state(&mut self, force: bool, eng: &mut dyn HudEngine) {
        if eng.level_name().is_empty() {
            Self::debug(eng, "CVoiceStatus::UpdateServerState: pLevelName[0]==0\n");
            return;
        }
        let enable = i32::from(eng.cvar("voice_modenable") != 0.0);
        if force || enable != self.server_mod_enable {
            self.server_mod_enable = enable;
            let cmd = format!("VModEnable {enable}");
            eng.server_cmd(&cmd);
            Self::debug(eng, &format!("CVoiceStatus::UpdateServerState: Sending '{cmd}'\n"));
        }
        let (mut ban_mask, mut server_mask) = (0u32, 0u32);
        for i in 0..32 {
            if let Some(id) = eng.player_unique_id(i + 1) {
                if self.bans.banned(&id) {
                    ban_mask |= 1 << i;
                }
                server_mask |= self.server_banned & (1 << i);
            }
        }
        let msg = format!("vban {ban_mask:x}");
        if ban_mask != server_mask || force {
            Self::debug(eng, &format!("CVoiceStatus::UpdateServerState: Sending '{msg}'\n"));
            eng.server_cmd_unreliable(&msg);
        } else {
            Self::debug(eng, "CVoiceStatus::UpdateServerState: no change\n");
        }
        self.last_update = eng.client_time();
    }

    /// `__MsgFunc_VoiceMask`: the third byte reads past the 8-byte message and yields -1.
    pub(super) fn msg_voice_mask(&mut self, r: &mut MsgReader, eng: &mut dyn HudEngine) {
        self.audible_players = r.long() as u32;
        self.server_banned = r.long() as u32;
        Self::debug(eng, &format!("CVoiceStatus::HandleVoiceMaskMsg\n    - m_AudiblePlayers[0] = {}\n    - m_ServerBannedPlayers[0] = {}\n", self.audible_players, self.server_banned));
        self.server_mod_enable = r.byte();
    }

    /// `__MsgFunc_ReqState`.
    pub(super) fn msg_req_state(&mut self, eng: &mut dyn HudEngine) {
        Self::debug(eng, "CVoiceStatus::HandleReqStateMsg\n");
        self.update_server_state(true, eng);
    }

    /// `voice_showbanned`: bytes printed sign-extended, the line upper-cased (`_strupr`).
    pub(super) fn show_banned(&self, eng: &mut dyn HudEngine) {
        eng.console_print("------- BANNED PLAYERS -------\n");
        for (n, id) in self.bans.all().enumerate() {
            let hex: String = id.iter().map(|&b| format!("{:2x}", i32::from(b as i8))).collect();
            eng.console_print(&format!("Ban {n}: {hex}\n").to_ascii_uppercase());
        }
        eng.console_print("------------------------------\n");
    }

    /// `IsPlayerBlocked` / `SetPlayerBlockedState` (the mute UI).
    pub fn is_blocked(&self, player: i32, eng: &dyn HudEngine) -> bool {
        eng.player_unique_id(player).is_some_and(|id| self.bans.banned(&id))
    }

    pub fn set_blocked(&mut self, player: i32, block: bool, eng: &mut dyn HudEngine) {
        let Some(id) = eng.player_unique_id(player) else { return };
        Self::debug(eng, &format!("CVoiceStatus::SetPlayerBlockedState: setting player {player} ban to {}\n", i32::from(!self.bans.banned(&id))));
        self.bans.set(id, block);
        self.update_server_state(false, eng);
    }
}

impl Hud {
    /// `HUD_VoiceStatus(entindex, bTalking)`: the radar flash, then
    /// `CVoiceStatus::UpdateSpeakerStatus`.
    pub fn voice_status(&mut self, entindex: i32, talking: bool, eng: &mut dyn HudEngine) {
        if (0..eng.max_clients()).contains(&entindex)
            && let Some(p) = self.players.get_mut(entindex as usize)
        {
            if talking {
                p.radarflash = self.time;
                p.radarflashon = true;
                p.radarflashes = 99999;
            } else {
                p.radarflash = -1.0;
                p.radarflashon = false;
                p.radarflashes = 0;
            }
        }
        self.update_speaker_status(entindex, talking, eng);
    }

    /// `CVoiceStatus::UpdateSpeakerStatus`.
    fn update_speaker_status(&mut self, entindex: i32, talking: bool, eng: &mut dyn HudEngine) {
        if eng.level_name().is_empty() {
            return;
        }
        if eng.cvar("voice_clientdebug") != 0.0 {
            eng.console_print(&format!("CVoiceStatus::UpdateSpeakerStatus: ent {entindex} talking = {}\n", i32::from(talking)));
        }
        match entindex {
            -1 => {
                self.voice.talking = talking;
                if talking {
                    eng.client_cmd("voice_modenable 1");
                }
                if let Some(local) = self.local_index(eng) {
                    self.hud_speaker_status(-2, talking, eng);
                    let b = bit(local as i32 - 1);
                    if talking {
                        self.voice.voice_players |= b;
                    } else {
                        self.voice.voice_players &= !b;
                    }
                    self.voice.enabled_players |= b;
                }
            }
            -2 => self.voice.server_acked = talking,
            1..=32 => {
                self.hud_speaker_status(entindex, talking, eng);
                let b = bit(entindex - 1);
                if talking {
                    self.voice.voice_players |= b;
                    self.voice.enabled_players |= b;
                } else {
                    self.voice.voice_players &= !b;
                }
            }
            0 => return,
            _ => {}
        }
        self.reposition_voice_labels(eng);
    }

    /// `CVoiceStatusHud::UpdateSpeakerStatus`: the local icon (-2) or a talker label (1..64).
    fn hud_speaker_status(&mut self, entindex: i32, talking: bool, eng: &mut dyn HudEngine) {
        if entindex == -2 {
            self.voice.local_icon = None;
            if talking && let Some(img) = eng.load_image("gfx/vgui/icntlk_pl") {
                let screen = eng.screen();
                let (w, h) = (screen.width as i32, screen.height as i32);
                let ack_height = 3 * self.font_height + 6;
                self.voice.local_icon = Some((w - img.width as i32 - 10, h - ack_height - img.height as i32));
            }
            return;
        }
        if !(1..=64).contains(&entindex) {
            return;
        }
        let idx = entindex - 1;
        let found = self.voice.labels.iter().position(|l| l.client == idx);
        match (talking, found) {
            (true, Some(_)) => {}
            (true, None) => {
                let slot = match self.voice.labels.iter().position(|l| l.client == -1) {
                    Some(i) => i,
                    None => {
                        self.voice.labels.push(Label { client: -1, ..Label::default() });
                        self.voice.labels.len() - 1
                    }
                };
                let name = eng.player_info(entindex).map(|p| p.name).filter(|n| !n.is_empty());
                // CCStrikeVoiceStatusHelper::GetPlayerTextColor: "team<n>" of the client scheme.
                let team = self.players.get(entindex as usize).map_or(0, |p| p.teamnumber);
                let scheme = if (0..=3).contains(&team) { format!("team{team}") } else { "team4".to_owned() };
                let c = eng.screen().client_color(&scheme).unwrap_or([255, 255, 255, 255]);
                // CanShowSpeakerLabels: hidden while the scoreboard is up, checked only here.
                let visible = !self.viewport.scoreboard;
                let label = &mut self.voice.labels[slot];
                label.text = name.map(|n| format!("{n}   ")).unwrap_or_default();
                label.color = [c[0], c[1], c[2]];
                label.client = idx;
                label.visible = visible;
            }
            (false, Some(i)) => {
                self.voice.labels[i].visible = false;
                self.voice.labels[i].client = -1;
            }
            (false, None) => {}
        }
        self.reposition_voice_labels(eng);
    }

    /// `RepositionLabels`: right-aligned 8 px from the edge, from mid-screen down, every label
    /// (free ones too) taking `max(h, 32) + 2`.
    fn reposition_voice_labels(&mut self, eng: &mut dyn HudEngine) {
        let image = eng.load_image("gfx/vgui/speaker4");
        let screen = eng.screen();
        let (w, h) = (screen.width as i32, screen.height as i32);
        let font = screen.client_font("Default");
        let tall = screen.font_tall(font);
        let mut y = h / 2;
        for label in &mut self.voice.labels {
            let (iw, ih) = image.map_or((0, 0), |i| (i.width as i32, i.height as i32));
            let cw = 1 + iw + 2 + screen.text_width(font, &label.text);
            let ch = tall.max(ih);
            label.pos = (w - cw - 8, y);
            label.size = (cw, ch);
            y += ch.max(32) + 2;
        }
    }

    /// The VGUI panels of `CVoiceStatusHud`: visible talker labels and the local icon.
    pub(super) fn draw_voice(&mut self, eng: &mut dyn HudEngine) {
        if self.voice.labels.iter().any(|l| l.visible) {
            let image = eng.load_image("gfx/vgui/speaker4");
            let screen = eng.screen();
            let font = screen.client_font("Default");
            let tall = screen.font_tall(font);
            for l in self.voice.labels.iter().filter(|l| l.visible) {
                let (x, y) = l.pos;
                let (w, h) = l.size;
                screen.fill_rgba_blend(x, y, w, h, i32::from(l.color[0]), i32::from(l.color[1]), i32::from(l.color[2]), 180);
                let mut tx = x + 1;
                if let Some(img) = image {
                    let (iw, ih) = (img.width as i32, img.height as i32);
                    screen.draw_image(img, tx, y + (h - ih) / 2, iw, ih, [255; 4]);
                    tx += iw;
                }
                tx += 2;
                super::viewport::draw_text(screen, font, tx, y + (h - tall) / 2, &l.text, [255; 4]);
            }
        }
        if let Some((x, y)) = self.voice.local_icon
            && let Some(img) = eng.load_image("gfx/vgui/icntlk_pl")
        {
            eng.screen().draw_image(img, x, y, img.width as i32, img.height as i32, [255; 4]);
        }
    }

    /// `HUD_CreateEntities`' voice part: players whose speaker icon may be drawn.
    pub fn voice_speaking_mask(&self, eng: &dyn HudEngine) -> u32 {
        self.voice.speaking_mask(eng)
    }

    /// `IGameClientExports::IsPlayerGameVoiceMuted` / `Mute` / `Unmute` (1-based players).
    pub fn voice_muted(&self, player: i32, eng: &dyn HudEngine) -> bool {
        self.voice.is_blocked(player, eng)
    }

    pub fn set_voice_muted(&mut self, player: i32, muted: bool, eng: &mut dyn HudEngine) {
        let mut voice = std::mem::take(&mut self.voice);
        voice.set_blocked(player, muted, eng);
        self.voice = voice;
    }

    /// Client shutdown: `CVoiceStatus`'s destructor saves the bans.
    pub fn shutdown(&self, eng: &mut dyn HudEngine) {
        self.voice.shutdown(eng);
    }
}

#[cfg(test)]
mod tests {
    use super::BanMgr;

    #[test]
    fn ban_file_round_trips_and_ignores_other_versions() {
        let mut m = BanMgr::default();
        m.set([1; 16], true);
        m.set([200; 16], true);
        m.set([1; 16], true);
        let data = m.save();
        assert_eq!(data.len(), 4 + 32, "version + two ids, no duplicate");
        let back = BanMgr::load(&data);
        assert!(back.banned(&[1; 16]) && back.banned(&[200; 16]));
        let mut v2 = data.clone();
        v2[0] = 2;
        assert!(!BanMgr::load(&v2).banned(&[1; 16]));
        let mut partial = data;
        partial.extend_from_slice(&[7; 10]);
        assert_eq!(BanMgr::load(&partial).all().count(), 2, "a trailing partial record is ignored");
    }
}
