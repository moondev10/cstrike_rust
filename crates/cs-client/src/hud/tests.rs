use std::collections::HashMap;
use std::sync::Arc;

use gs_draw::localize::Localize;
use gs_draw::scheme::Scheme;
use gs_draw::{SpriteLoader, Texture};
use gs_formats::sprite::{Sprite, SpriteFormat, SpriteFrame, SpriteFrameSlot, SpriteType};

use super::*;

struct Loader;

impl SpriteLoader for Loader {
    fn load_sprite(&mut self, name: &str) -> Option<(Arc<Sprite>, u32)> {
        let frame = SpriteFrameSlot::Single(SpriteFrame { width: 256, height: 256, up: 0.0, down: 0.0, left: 0.0, right: 0.0, pixels: vec![0; 256 * 256] });
        let sprite = Sprite {
            kind: SpriteType::ParallelUpright,
            format: SpriteFormat::Additive,
            max_width: 256,
            max_height: 256,
            beam_length: 0.0,
            sync_type: 0,
            palette: vec![[0; 3]; 256],
            frames: vec![frame],
        };
        Some((Arc::new(sprite), name.len() as u32))
    }
    fn load_image(&mut self, _name: &str) -> Option<gs_draw::Image> {
        None
    }
}

pub(super) struct Engine {
    screen: Screen,
    files: HashMap<String, String>,
    cvars: HashMap<String, String>,
    server: Vec<String>,
    client: Vec<String>,
    sounds: Vec<String>,
    /// `titles.txt` entries by name.
    pub(super) titles: HashMap<String, TextMessage>,
    /// `motd_navigate` / `motd_temp_file` calls.
    pub(super) motd: Vec<String>,
}

impl Engine {
    pub(super) fn new() -> Self {
        let mut screen = Screen::new(Scheme::default(), Localize::default());
        screen.width = 800;
        screen.height = 600;
        let mut hud_txt = String::from("17\n");
        for d in 0..10 {
            hud_txt.push_str(&format!("number_{d} 640 640hud1 {} 0 20 24\n", d * 20));
        }
        for b in 1..=5 {
            hud_txt.push_str(&format!("bucket{b} 640 640hud3 {} 0 20 20\n", b * 20));
        }
        hud_txt.push_str("selection 640 640hud3 0 180 170 45\nnumber_0 320 320hud1 0 0 12 16\n");
        let weapons = "2\nweapon 640 640hud2 0 0 170 45\nweapon_s 640 640hud5 0 0 170 45\n";
        let files = HashMap::from([("sprites/hud.txt".to_owned(), hud_txt), ("sprites/weapon_ak47.txt".to_owned(), weapons.to_owned())]);
        let cvars = HashMap::from([("hud_fastswitch".to_owned(), "0".to_owned())]);
        Self { screen, files, cvars, server: Vec::new(), client: Vec::new(), sounds: Vec::new(), titles: HashMap::new(), motd: Vec::new() }
    }
}

impl HudEngine for Engine {
    fn screen(&mut self) -> &mut Screen {
        &mut self.screen
    }
    fn spr_load(&mut self, name: &str) -> SprHandle {
        self.screen.spr_load(name, &mut Loader)
    }
    fn load_image(&mut self, name: &str) -> Option<gs_draw::Image> {
        self.screen.load_image(name, &mut Loader)
    }
    fn read_file(&mut self, path: &str) -> Option<Vec<u8>> {
        self.files.get(path).map(|s| s.clone().into_bytes())
    }
    fn cvar(&self, name: &str) -> f32 {
        self.cvars.get(name).and_then(|v| v.parse().ok()).unwrap_or(0.0)
    }
    fn cvar_string(&self, name: &str) -> String {
        self.cvars.get(name).cloned().unwrap_or_default()
    }
    fn client_cmd(&mut self, text: &str) {
        self.client.push(text.to_owned());
    }
    fn server_cmd(&mut self, text: &str) {
        self.server.push(text.to_owned());
    }
    fn console_print(&mut self, _text: &str) {}
    fn player_info(&self, index: i32) -> Option<PlayerInfo> {
        (index == 1).then(|| PlayerInfo { name: "me".into(), thisplayer: true, ping: 5, bot: false })
    }
    fn play_sound(&mut self, name: &str, _volume: f32) {
        self.sounds.push(name.to_owned());
    }
    fn play_sound_pitch(&mut self, name: &str, _volume: f32, _pitch: i32) {
        self.sounds.push(name.to_owned());
    }
    fn play_sound_voice(&mut self, name: &str, _volume: f32, _pitch: i32) {
        self.sounds.push(name.to_owned());
    }
    fn text_message(&self, name: &str) -> Option<TextMessage> {
        self.titles.get(name).cloned()
    }
    fn max_clients(&self) -> i32 {
        32
    }
    fn level_name(&self) -> String {
        "maps/de_test.bsp".into()
    }
    fn local_userinfo(&self, _key: &str) -> Option<String> {
        None
    }
    fn random_long(&mut self, lo: i32, _hi: i32) -> i32 {
        lo
    }
    fn create_light(&mut self, _origin: [f32; 3], _radius: f32, _color: [u8; 3], _die: f32) {}
    fn player_unique_id(&self, _index: i32) -> Option<[u8; 16]> {
        None
    }
    fn server_cmd_unreliable(&mut self, _text: &str) {}
    fn game_dir_file(&self, name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(name)
    }
    fn client_time(&self) -> f32 {
        0.0
    }
    fn cvar_set(&mut self, name: &str, value: &str) {
        self.cvars.insert(name.to_owned(), value.to_owned());
    }
    fn motd_navigate(&mut self, url: &str) {
        self.motd.push(format!("navigate {url}"));
    }
    fn motd_temp_file(&mut self, html: Option<&str>) {
        self.motd.push(html.map_or_else(|| "delete".to_owned(), |h| format!("temp {h}")));
    }
}

fn hud(eng: &mut Engine) -> Hud {
    let mut hud = Hud::default();
    hud.vid_init(eng);
    hud
}

/// `(x, rect.left)` of every sprite quad: where it is drawn and which hud.txt cell.
fn digits(eng: &Engine) -> Vec<(f32, f32)> {
    eng.screen.list.iter().filter(|c| matches!(c.texture, Texture::Sprite { .. })).map(|c| (c.rect[0] - 0.5, c.uv[0] * 256.0 - 0.5)).collect()
}

#[test]
fn hud_txt_keeps_only_the_current_resolution() {
    let mut eng = Engine::new();
    let hud = hud(&mut eng);
    assert_eq!(hud.res, 640);
    assert_eq!(hud.font_height, 24);
    assert_eq!(hud.digit_width(), 20);
    assert_eq!(hud.sprite_index("selection"), Some(15));
}

#[test]
fn hud_numbers_pad_and_fill_like_draw_hud_number() {
    let mut eng = Engine::new();
    let hud = hud(&mut eng);
    let end = hud.draw_number(&mut eng.screen, 100, 0, dhn::THREE_DIGITS | dhn::DRAWZERO, 7, YELLOWISH);
    assert_eq!(end, 160);
    assert_eq!(digits(&eng), [(140.0, 140.0)], "blank padding, then the 7 cell");
    eng.screen.begin_frame();
    hud.draw_number(&mut eng.screen, 0, 0, dhn::TWO_DIGITS | dhn::DRAWZERO | dhn::FILLZERO, 5, YELLOWISH);
    assert_eq!(digits(&eng), [(0.0, 0.0), (20.0, 100.0)]);
    eng.screen.begin_frame();
    let end = hud.draw_number(&mut eng.screen, 0, 0, 0, 0, YELLOWISH);
    assert_eq!((end, digits(&eng).len()), (0, 0), "zero needs DRAWZERO");
    eng.screen.begin_frame();
    hud.draw_number5(&mut eng.screen, 0, 0, 800, YELLOWISH);
    assert_eq!(digits(&eng), [(40.0, 160.0), (60.0, 0.0), (80.0, 0.0)], "\"%5i\" leaves two blanks");
}

fn weapon_list(name: &str, ammo: i8, max: u8, slot: i8, pos: i8, id: i8) -> Vec<u8> {
    let mut m = name.as_bytes().to_vec();
    m.push(0);
    m.extend_from_slice(&[ammo as u8, max, 0xFF, 0xFF, slot as u8, pos as u8, id as u8, 0]);
    m
}

#[test]
fn weapon_list_rejects_out_of_range_entries() {
    let mut eng = Engine::new();
    let mut hud = hud(&mut eng);
    hud.msg("WeaponList", &weapon_list("weapon_bad", 2, 90, 6, 0, 28), &mut eng);
    hud.msg("WeaponList", &weapon_list("weapon_bad", 2, 0, 0, 0, 28), &mut eng);
    assert!(!hud.ammo.known(28), "slot 6 and a zero max are refused");
    hud.msg("WeaponList", &weapon_list("weapon_ak47", 2, 90, 0, 1, 28), &mut eng);
    assert!(hud.ammo.known(28));
}

#[test]
fn slot_key_opens_the_list_and_attack_confirms_it() {
    let mut eng = Engine::new();
    let mut hud = hud(&mut eng);
    hud.msg("WeaponList", &weapon_list("weapon_ak47", 2, 90, 0, 1, 28), &mut eng);
    hud.update_client_data(ClientState { weapon_bits: i32::MIN | (1 << 28), ..ClientState::default() }, 90.0, &eng);
    let slot1 = gs_common::token::tokenize("slot1");
    assert!(hud.command(&slot1, &mut eng));
    assert_eq!(eng.sounds, ["common/wpn_hudon.wav"]);
    assert!(eng.server.is_empty());
    let out = hud.confirm_selection(true, &mut eng);
    assert_eq!(out, SelectionOutcome { weaponselect: Some(28), release_attack: true });
    assert_eq!(eng.server, ["weapon_ak47"]);
    assert_eq!(hud.confirm_selection(true, &mut eng), SelectionOutcome::default(), "the list closed");
}

#[test]
fn fast_switch_selects_a_lone_weapon_at_once() {
    let mut eng = Engine::new();
    eng.cvars.insert("hud_fastswitch".into(), "1".into());
    let mut hud = hud(&mut eng);
    hud.msg("WeaponList", &weapon_list("weapon_ak47", 2, 90, 0, 1, 28), &mut eng);
    hud.update_client_data(ClientState { weapon_bits: i32::MIN | (1 << 28), ..ClientState::default() }, 90.0, &eng);
    hud.command(&gs_common::token::tokenize("slot1"), &mut eng);
    assert_eq!(eng.server, ["weapon_ak47"]);
    assert_eq!(hud.confirm_selection(false, &mut eng), SelectionOutcome { weaponselect: Some(28), release_attack: false });
}

#[test]
fn text_menus_take_slot_keys_first() {
    let mut eng = Engine::new();
    let mut hud = hud(&mut eng);
    let mut menu = 0b11u16.to_le_bytes().to_vec();
    menu.extend_from_slice(&[0xFF, 0, b'M', 0]);
    hud.msg("ShowMenu", &menu, &mut eng);
    hud.command(&gs_common::token::tokenize("slot2"), &mut eng);
    assert_eq!(eng.client, ["menuselect 2\n"]);
}

/// Radio menus arrive as `#RadioA`..`#RadioC`: the menu shows the titles.txt text.
#[test]
fn text_menus_show_the_titles_entry_of_a_token() {
    let mut eng = Engine::new();
    eng.titles.insert("RadioC".into(), TextMessage { message: "\\yRadio Responses/Reports\\w\n1. \"Affirmative\"".into(), ..TextMessage::default() });
    let mut hud = hud(&mut eng);
    let mut menu = 0b11u16.to_le_bytes().to_vec();
    menu.extend_from_slice(&[0xFF, 0]);
    menu.extend_from_slice(b"#RadioC\0");
    hud.msg("ShowMenu", &menu, &mut eng);
    assert_eq!(hud.menu.text, "\\yRadio Responses/Reports\\w\n1. \"Affirmative\"");
}

#[test]
fn cur_weapon_ignores_ids_below_one_before_the_dead_marker() {
    let mut eng = Engine::new();
    let mut hud = hud(&mut eng);
    hud.msg("WeaponList", &weapon_list("weapon_ak47", 2, 90, 0, 1, 28), &mut eng);
    hud.update_client_data(ClientState { weapon_bits: i32::MIN | (1 << 28), ..ClientState::default() }, 90.0, &eng);
    hud.msg("CurWeapon", &[1, 28, 30], &mut eng);
    assert_eq!(hud.ammo.current(), Some(28));
    hud.command(&gs_common::token::tokenize("slot1"), &mut eng);
    // The server's dead marker reads back as id -1, which the `id < 1` check returns on.
    hud.msg("CurWeapon", &[0, 0xFF, 0xFF], &mut eng);
    assert_eq!(hud.ammo.current(), Some(28));
    assert_eq!(hud.confirm_selection(true, &mut eng).weaponselect, Some(28), "the selection stays open");
}

#[test]
fn fog_message_sets_colour_and_density_until_init_hud_and_overruns_turn_it_off() {
    let mut eng = Engine::new();
    let mut hud = hud(&mut eng);
    let mut fog = vec![120, 130, 140];
    fog.extend_from_slice(&0.002f32.to_le_bytes());
    hud.msg("Fog", &fog, &mut eng);
    assert_eq!(hud.fog.color, [120, 130, 140]);
    assert_eq!(hud.fog.density, 0.002);
    assert!(hud.fog.active(1) && !hud.fog.active(2), "RenderFog is off from waterlevel 2");
    hud.msg("Fog", &fog[..5], &mut eng);
    assert!(!hud.fog.active(0), "a short read zeroes the parameters");
    hud.msg("Fog", &fog, &mut eng);
    hud.msg("InitHUD", &[], &mut eng);
    assert!(!hud.fog.active(0));
}
