//! `COptionsDialog` and its Multiplayer, Audio and Mouse pages (docs/re/gameui-dialogs.md
//! §1.4, §3, §4, §6, §7), plus `ModInfo`, `CBitmapImagePanel` and `CrosshairImagePanel`.

use std::any::Any;

use gs_vgui::controls::button::Button;
use gs_vgui::controls::combo_box::ComboBox;
use gs_vgui::controls::label::Label;
use gs_vgui::controls::misc::ImagePanel;
use gs_vgui::controls::property::{DialogHooks, NoHooks, PageHooks, PropertyDialog, PropertyDialogParts, PropertyPage, with_property_dialog, with_sheet};
use gs_vgui::controls::text_entry::{TextEntry, with_te};
use gs_vgui::scheme::Scheme;
use gs_vgui::surface::Texture;
use gs_vgui::{Color, Control, Msg, PanelId, Surface, Ui};

use crate::cvar_controls as cv;
use crate::{Fs, GameFs, Shared};

// ---------------------------------------------------------------------------------------------
// ModInfo (§1.4)

/// `ModInfo()`: `liblist.gam` as key/value pairs (`engine.COM_ParseFile` tokens).
#[derive(Debug, Default, Clone)]
pub struct ModInfo(Vec<(String, String)>);

impl ModInfo {
    pub fn load(fs: &dyn GameFs) -> Self {
        let mut out = Vec::new();
        if let Some(data) = fs.read("liblist.gam") {
            let mut p: &[u8] = &data;
            while let Some((k, rest)) = gs_common::token::com_parse(p, false) {
                if k.is_empty() {
                    break;
                }
                let Some((v, rest)) = gs_common::token::com_parse(rest, false) else { break };
                let key = String::from_utf8_lossy(&k).into_owned();
                let value = String::from_utf8_lossy(&v).into_owned();
                match out.iter_mut().find(|(ek, _): &&mut (String, String)| ek.eq_ignore_ascii_case(&key)) {
                    Some(e) => e.1 = value,
                    None => out.push((key, value)),
                }
                p = rest;
            }
        }
        Self(out)
    }

    fn get<'a>(&'a self, key: &str, def: &'a str) -> &'a str {
        self.0.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map_or(def, |(_, v)| v.as_str())
    }

    pub fn game_description(&self) -> &str {
        self.get("game", "Half-Life")
    }
    pub fn is_multiplayer_only(&self) -> bool {
        self.get("type", "").eq_ignore_ascii_case("multiplayer_only")
    }
    pub fn is_single_player_only(&self) -> bool {
        self.get("type", "").eq_ignore_ascii_case("singleplayer_only")
    }
    pub fn no_models(&self) -> bool {
        self.get("nomodels", "0").eq_ignore_ascii_case("1")
    }
    pub fn no_hi_model(&self) -> bool {
        self.get("nohimodel", "0").eq_ignore_ascii_case("1")
    }
    pub fn has_detailed_textures(&self) -> bool {
        self.get("detailed_textures", "0").eq_ignore_ascii_case("1")
    }
    pub fn get_fallback_dir(&self) -> &str {
        self.get("fallback_dir", "")
    }
    pub fn use_fallback_maps(&self) -> bool {
        self.get("fallback_maps", "1").eq_ignore_ascii_case("1")
    }
    pub fn get_mp_filter(&self) -> &str {
        self.get("mpfilter", "")
    }
}

/// C `atof`: the longest leading float, 0 without one.
pub fn c_atof(s: &str) -> f64 {
    let t = s.trim_start();
    let mut end = 0;
    let b = t.as_bytes();
    if end < b.len() && (b[end] == b'+' || b[end] == b'-') {
        end += 1;
    }
    while end < b.len() && b[end].is_ascii_digit() {
        end += 1;
    }
    if end < b.len() && b[end] == b'.' {
        end += 1;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
        }
    }
    if end < b.len() && (b[end] == b'e' || b[end] == b'E') {
        let mut e = end + 1;
        if e < b.len() && (b[e] == b'+' || b[e] == b'-') {
            e += 1;
        }
        if e < b.len() && b[e].is_ascii_digit() {
            while e < b.len() && b[e].is_ascii_digit() {
                e += 1;
            }
            end = e;
        }
    }
    t[..end].parse().unwrap_or(0.0)
}

/// C `atoi`.
pub fn c_atoi(s: &str) -> i32 {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut end = usize::from(!b.is_empty() && (b[0] == b'+' || b[0] == b'-'));
    while end < b.len() && b[end].is_ascii_digit() {
        end += 1;
    }
    t[..end].parse::<i64>().map_or(0, |v| v as i32)
}

/// `LoadControlSettings(res)` through the game filesystem.
pub fn load_res(ui: &mut Ui, me: PanelId, fs: &dyn GameFs, res: &str, s: &mut dyn Surface) {
    match fs.read_text(res) {
        Some(text) => ui.load_control_settings(me, &text, &mut gs_vgui::editable::create_control_by_name, s),
        None => log::warn!("{res}: missing"),
    }
}

/// The task-frame prologue: close sound, title (`CTaskFrame::SetTitle`), destroyed on close.
pub fn task_frame(ui: &mut Ui, me: PanelId, title: &str, delete_on_close: bool) {
    gs_vgui::controls::frame::with_frame(ui, me, |f, ui| {
        f.close_sound = Some("sound/UI/launch_dnmenu1.wav".to_owned());
        f.set_title(ui, me, title);
        f.delete_self_on_close = delete_on_close;
    });
}

/// `Panel::SetHDScaled(true)`.
pub fn set_hd_scaled(ui: &mut Ui, me: PanelId) {
    ui.set_hd_proportional(me, true);
    ui.set_proportional(me, true);
}

/// Installs a page's hooks after its children exist.
pub fn set_page_hooks(ui: &mut Ui, page: PanelId, hooks: Box<dyn PageHooks>) {
    ui.with_ctrl(page, |c, _| {
        if let Some(p) = c.as_any_mut().downcast_mut::<PropertyPage>() {
            p.hooks = hooks;
        }
    });
}

// ---------------------------------------------------------------------------------------------
// CBitmapImagePanel (§4.9)

/// GameUI's `CBitmapImagePanel`: a texture file stretched over the panel.
#[derive(Debug, Default)]
pub struct BitmapImagePanel {
    texture: String,
    tex: Option<Texture>,
    uploaded: bool,
    reload: bool,
}

impl BitmapImagePanel {
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str, texture: Option<&str>) -> PanelId {
        let me = ui.create(parent, name, Box::new(BitmapImagePanel { texture: texture.unwrap_or("").to_owned(), ..Default::default() }));
        ui.set_bounds(me, 0, 0, 100, 100);
        me
    }

    /// `SetImage(path)`: a loaded texture is reloaded from the file.
    pub fn set_image(ui: &mut Ui, me: PanelId, path: &str) {
        if let Some(p) = ui.ctrl::<BitmapImagePanel>(me) {
            p.texture = path.to_owned();
            p.reload = p.uploaded;
        }
    }
}

impl Control for BitmapImagePanel {
    fn class(&self) -> &'static str {
        "CBitmapImagePanel"
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if self.texture.is_empty() {
            return;
        }
        if !self.uploaded || self.reload {
            self.tex = if self.reload { s.reload_texture(&self.texture, true) } else { s.texture(&self.texture, true) };
            self.uploaded = true;
            self.reload = false;
        }
        if let Some(t) = self.tex {
            let (w, h) = ui.size(me);
            s.draw_set_color(Color::WHITE);
            s.draw_textured_rect(t, 0, 0, w, h);
        }
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// CrosshairImagePanel (§4.11)

pub struct CrosshairImage {
    base: Box<dyn Control>,
    rgb: (i32, i32, i32),
    bar: i32,
    gap: i32,
    additive: Option<PanelId>,
}

fn crosshair_div(sw: i32, size: i32) -> i32 {
    let auto = if sw <= 640 {
        1024
    } else if sw < 1024 {
        800
    } else {
        640
    };
    match size {
        0 => auto,
        1 => 1024,
        2 => 800,
        3 => 640,
        _ => 1024,
    }
}

impl CrosshairImage {
    fn create(ui: &mut Ui, parent: PanelId, additive: Option<PanelId>) -> PanelId {
        let me = ImagePanel::create(ui, Some(parent), "CrosshairImage");
        set_hd_scaled(ui, me);
        let sw = ui.screen.0;
        let div = crosshair_div(sw, 0);
        ui.replace_ctrl(me, |base| Box::new(CrosshairImage { base, rgb: (50, 250, 50), bar: sw * 9 / div, gap: sw * 5 / div, additive }));
        me
    }

    /// `UpdateCrosshair(r, g, b, size)`.
    fn update(ui: &mut Ui, me: PanelId, rgb: (i32, i32, i32), size: i32) {
        let sw = ui.screen.0;
        ui.with_ctrl(me, |c, _| {
            if let Some(ci) = c.as_any_mut().downcast_mut::<CrosshairImage>() {
                ci.rgb = rgb;
                let div = crosshair_div(sw, size);
                ci.bar = sw * 9 / div;
                ci.gap = sw * 5 / div;
            }
        });
    }
}

impl Control for CrosshairImage {
    fn class(&self) -> &'static str {
        "CrosshairImagePanel"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        self.base.on_message(ui, me, msg, from, s)
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.base.apply_scheme(ui, me, scheme, s);
    }
    fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &gs_formats::keyvalues::Block, s: &mut dyn Surface) {
        self.base.apply_settings(ui, me, kv, s);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.base.perform_layout(ui, me, s);
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.base.paint_background(ui, me, s);
    }
    /// Four engine fills: additive when the translucency box is checked.
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let Some(add) = self.additive else { return };
        let (w, h) = ui.size(me);
        let (cx, cy) = (w / 2, h / 2);
        let additive = cv::check_selected(ui, add);
        let (r, g, b) = self.rgb;
        let c = Color::rgba(r as u8, g as u8, b as u8, 255);
        let (bar, gap) = (self.bar, self.gap);
        s.engine_fill_rgba(cx - bar - gap + 1, cy, bar, 1, c, additive);
        s.engine_fill_rgba(cx + gap, cy, bar, 1, c, additive);
        s.engine_fill_rgba(cx, cy - bar - gap + 1, 1, bar, c, additive);
        s.engine_fill_rgba(cx, cy + gap, 1, bar, c, additive);
    }
    fn paint_border(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.base.paint_border(ui, me, s);
    }
    fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId, w: i32, h: i32) {
        self.base.on_size_changed(ui, me, w, h);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// COptionsDialog (§3)

struct OptionsHooks;

impl DialogHooks for OptionsHooks {
    /// `Activate`: `Frame::Activate(); EnableApplyButton(false)`.
    fn on_activate(&mut self, ui: &mut Ui, _me: PanelId, d: &mut PropertyDialogParts) {
        ui.set_enabled(d.apply, false);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `new COptionsDialog(parent)`: Counter-Strike's tab order is Multiplayer, Audio, Video,
/// Mouse, Keyboard, Voice, Content.
pub fn create_options_dialog(ui: &mut Ui, parent: PanelId, host: &Shared, fs: &Fs, s: &mut dyn Surface) -> PanelId {
    let dlg = PropertyDialog::create(ui, Some(parent), "OptionsDialog", Box::new(OptionsHooks));
    set_hd_scaled(ui, dlg);
    let (w, h) = (ui.psv(dlg, 522), ui.psv(dlg, 444));
    ui.set_bounds(dlg, 0, 0, w, h);
    task_frame(ui, dlg, "#GameUI_Options", true);
    let mi = ModInfo::load(fs.as_ref());
    let half_life = "Half-Life".eq_ignore_ascii_case(mi.game_description());
    let (mp, sp) = (mi.is_multiplayer_only(), mi.is_single_player_only());
    if mp && !sp {
        let p = create_multiplayer_page(ui, dlg, host, fs, &mi, s);
        PropertyDialog::add_page(ui, dlg, p, "#GameUI_Multiplayer");
    }
    let p = create_audio_page(ui, dlg, host, fs, &mi, s);
    PropertyDialog::add_page(ui, dlg, p, "#GameUI_Audio");
    let p = crate::options_video::create_video_page(ui, dlg, host, fs, s);
    PropertyDialog::add_page(ui, dlg, p, "#GameUI_Video");
    let p = create_mouse_page(ui, dlg, host, fs, s);
    PropertyDialog::add_page(ui, dlg, p, "#GameUI_Mouse");
    let p = crate::options_keyboard::create_keyboard_page(ui, dlg, host, fs, s);
    PropertyDialog::add_page(ui, dlg, p, "#GameUI_Keyboard");
    if !sp || half_life {
        let p = crate::options_voice::create_voice_page(ui, dlg, host, fs, s);
        PropertyDialog::add_page(ui, dlg, p, "#GameUI_Voice");
    }
    let p = crate::options_content::create_content_page(ui, dlg, host, fs, s);
    PropertyDialog::add_page(ui, dlg, p, "#GameUI_Content");
    if sp && !half_life {
        let p = crate::advanced::create_multiplayer_advanced_page(ui, dlg, host, fs, s);
        PropertyDialog::add_page(ui, dlg, p, "#GameUI_AdvancedNoEllipsis");
    }
    if half_life || (!mp && !sp) {
        let p = create_multiplayer_page(ui, dlg, host, fs, &mi, s);
        PropertyDialog::add_page(ui, dlg, p, "#GameUI_Multiplayer");
    }
    // SetApplyButtonVisible(true); sheet tab width Scale(72).
    let parts = with_property_dialog(ui, dlg, |d, _| d.parts);
    if let Some(parts) = parts {
        ui.set_visible(parts.apply, true);
        let tw = ui.psv(dlg, 72);
        with_sheet(ui, parts.sheet, |sh, ui| sh.set_tab_width(ui, parts.sheet, tw));
    }
    dlg
}

/// `OnControlModified` of the pages using `OnControlModified@0x10032090`.
fn apply_button_enable(ui: &mut Ui, page: PanelId) {
    ui.post_action_signal(page, Msg::new("ApplyButtonEnable"));
}

// ---------------------------------------------------------------------------------------------
// Multiplayer page (§4)

const LOGO_COLORS: [(&str, i32, i32, i32); 9] = [
    ("#Valve_Orange", 255, 120, 24),
    ("#Valve_Yellow", 225, 180, 24),
    ("#Valve_Blue", 0, 60, 255),
    ("#Valve_Ltblue", 0, 167, 255),
    ("#Valve_Green", 0, 167, 0),
    ("#Valve_Red", 255, 43, 0),
    ("#Valve_Brown", 123, 73, 0),
    ("#Valve_Ltgray", 100, 100, 100),
    ("#Valve_Dkgray", 36, 36, 36),
];

const CROSSHAIR_COLORS: [(&str, i32, i32, i32); 5] =
    [("#Valve_Green", 50, 250, 50), ("#Valve_Red", 250, 50, 50), ("#Valve_Blue", 50, 50, 250), ("#Valve_Yellow", 250, 250, 50), ("#Valve_Ltblue", 50, 250, 250)];

/// `ColorForName`: the first entry whose name is a case-insensitive prefix of `name`.
fn color_for_name(name: &str) -> (i32, i32, i32) {
    LOGO_COLORS
        .iter()
        .find(|(n, ..)| name.len() >= n.len() && name.as_bytes()[..n.len()].eq_ignore_ascii_case(n.as_bytes()))
        .map_or((0, 0, 0), |&(_, r, g, b)| (r, g, b))
}

struct MultiplayerPage {
    host: Shared,
    fs: Fs,
    model_image: PanelId,
    player_model: PanelId,
    logo_image: PanelId,
    spray_list: PanelId,
    logo_name: String,
    crosshair_size: Option<PanelId>,
    translucency: Option<PanelId>,
    crosshair_color: Option<PanelId>,
    rgb: (i32, i32, i32),
    spray_color: PanelId,
    name_entry: PanelId,
    primary: PanelId,
    secondary: PanelId,
    hq_models: PanelId,
    crosshair_image: Option<PanelId>,
    topcolor: i32,
    bottomcolor: i32,
    advanced: Option<PanelId>,
}

fn create_multiplayer_page(ui: &mut Ui, dlg: PanelId, host: &Shared, fs: &Fs, mi: &ModInfo, s: &mut dyn Surface) -> PanelId {
    let page = PropertyPage::create(ui, Some(dlg), "OptionsSubMultiplayer", true, Box::new(NoHooks));
    Button::create(ui, Some(page), "Advanced", "#GameUI_AdvancedEllipsis", Some("Advanced"));
    Label::create(ui, Some(page), "NameLabel", "#GameUI_PlayerName");
    let name_entry = cv::create_text(ui, Some(page), "NameEntry", "name", host.clone(), s);
    ui.with_ctrl(name_entry, |c, _| {
        if let Some(te) = c.as_any_mut().downcast_mut::<cv::CvarControl>().and_then(|cc| cc.base_mut().as_any_mut().downcast_mut::<TextEntry>()) {
            te.set_catch_enter_key(true);
        }
    });
    let primary = cv::create_slider(ui, Some(page), "Primary Color Slider", 0.0, 255.0, "topcolor", false, host.clone());
    let secondary = cv::create_slider(ui, Some(page), "Secondary Color Slider", 0.0, 255.0, "bottomcolor", false, host.clone());
    let hq_models = cv::create_check(ui, Some(page), "High Quality Models", "#GameUI_HighModels", Some("cl_himodels"), false, host.clone(), s);
    let player_model = cv::create_combo(ui, Some(page), "Player model", host.clone());
    for id in [Some("GAMECONFIG"), Some("GAME"), None] {
        fs.remove("models/player/remapped.bmp", id);
    }
    let cur: String = host.borrow().cvar_string("model").unwrap_or("").chars().take(63).collect();
    let (mut i, mut sel) = (0, 0);
    for (name, dir) in fs.find("models/player/*.*", None) {
        if dir && !name.starts_with('.') && !cv::combo_has_item(ui, player_model, &name) {
            if cur.eq_ignore_ascii_case(&name) {
                sel = i;
            }
            cv::combo_add_item(ui, player_model, &name, &format!("model {name}\n"));
            i += 1;
        }
    }
    cv::combo_set_initial_item(ui, player_model, sel, s);
    let spray_list = cv::create_combo(ui, Some(page), "SpraypaintList", host.clone());
    init_logo_list(ui, spray_list, host, fs.as_ref(), s);
    let spray_color = cv::create_combo(ui, Some(page), "SpraypaintColor", host.clone());
    let lc = host.borrow().cvar_string("cl_logocolor").map(str::to_owned);
    let mut sel = 0;
    for (k, &(name, ..)) in LOGO_COLORS.iter().enumerate() {
        if lc.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(name)) {
            sel = k as i32;
        }
        cv::combo_add_item(ui, spray_color, name, &format!("cl_logocolor {name}\n"));
    }
    cv::combo_set_initial_item(ui, spray_color, sel, s);
    ui.add_action_signal_target(spray_color, page);
    let model_image = BitmapImagePanel::create(ui, Some(page), "ModelImage", None);
    ui.add_action_signal_target(model_image, page);
    let logo_image = BitmapImagePanel::create(ui, Some(page), "LogoImage", None);
    ui.add_action_signal_target(logo_image, page);
    let mut mp = MultiplayerPage {
        host: host.clone(),
        fs: fs.clone(),
        model_image,
        player_model,
        logo_image,
        spray_list,
        logo_name: String::new(),
        crosshair_size: None,
        translucency: None,
        crosshair_color: None,
        rgb: (-1, -1, -1),
        spray_color,
        name_entry,
        primary,
        secondary,
        hq_models,
        crosshair_image: None,
        topcolor: 30,
        bottomcolor: 6,
        advanced: None,
    };
    let gd = fs.game_dir();
    if ["czero", "terror", "cstrike", "cstrike_beta"].iter().any(|g| g.eq_ignore_ascii_case(&gd)) {
        mp.rgb = (50, 250, 50);
        let cc = ComboBox::create(ui, Some(page), "CrosshairColorComboBox", 6, false);
        mp.crosshair_color = Some(cc);
        mp.init_crosshair_color_entries(ui, page, s);
        let size = cv::create_combo(ui, Some(page), "CrosshairSizeComboBox", host.clone());
        mp.crosshair_size = Some(size);
        let tr = cv::create_check(ui, Some(page), "CrosshairTranslucencyCheckbox", "#GameUI_Translucent", Some("cl_crosshair_translucent"), false, host.clone(), s);
        mp.translucency = Some(tr);
        cv::reset(ui, size, s);
        for (t, c) in [("#GameUI_Auto", "auto"), ("#GameUI_Small", "small"), ("#GameUI_Medium", "medium"), ("#GameUI_Large", "large")] {
            cv::combo_add_item(ui, size, t, &format!("cl_crosshair_size {c}"));
        }
        let cs = host.borrow().cvar_string("cl_crosshair_size").map(str::to_ascii_lowercase);
        let sel = match cs.as_deref() {
            Some("small") => 1,
            Some("medium") => 2,
            Some("large") => 3,
            _ => 0,
        };
        cv::combo_set_initial_item(ui, size, sel, s);
        let img = CrosshairImage::create(ui, page, Some(tr));
        mp.crosshair_image = Some(img);
        mp.update_crosshair_from_color_combo(ui, page);
    }
    for (name, text) in [("OK", "#GameUI_OK"), ("Apply", "#GameUI_Apply"), ("Cancel", "#GameUI_Cancel")] {
        Button::create(ui, Some(page), name, text, None);
    }
    load_res(ui, page, fs.as_ref(), "Resource/OptionsSubMultiplayer.res", s);
    for name in ["OK", "Apply", "Cancel"] {
        if let Some(b) = ui.find_child_by_name(page, name) {
            ui.set_enabled(b, false);
            ui.set_visible(b, false);
        }
    }
    if let Some(img) = mp.crosshair_image {
        let n = ui.psv(img, 64);
        ui.with_ctrl(img, |c, _| {
            if let Some(ci) = c.as_any_mut().downcast_mut::<CrosshairImage>()
                && let Some(ip) = ci.base.as_any_mut().downcast_mut::<ImagePanel>()
            {
                ip.image_size = Some((n, n));
            }
        });
    }
    if mi.no_models() {
        for p in [Some(mp.model_image), Some(mp.player_model), Some(mp.primary), Some(mp.secondary), ui.find_child_by_name(page, "Label1"), ui.find_child_by_name(page, "Colors")].into_iter().flatten() {
            ui.set_visible(p, false);
        }
    }
    if mi.no_hi_model() {
        ui.set_visible(mp.hq_models, false);
    }
    set_page_hooks(ui, page, Box::new(mp));
    page
}

/// `InitLogoList`: `logos/*.bmp` without `remapped.bmp`, sorted case-insensitively.
fn init_logo_list(ui: &mut Ui, combo: PanelId, host: &Shared, fs: &dyn GameFs, s: &mut dyn Surface) {
    for id in [Some("GAMECONFIG"), Some("GAME"), None] {
        fs.remove("logos/remapped.bmp", id);
    }
    let cur = host.borrow().cvar_string("cl_logofile").unwrap_or("").to_owned();
    let mut tree: Vec<String> = Vec::new();
    for (f, _) in fs.find("logos/*.bmp", None) {
        if f.eq_ignore_ascii_case("remapped.bmp") || f.is_empty() || f.starts_with('.') {
            continue;
        }
        let mut name = f.clone();
        if name.len() >= 4 {
            name.truncate(name.len() - 4);
        }
        let key = name.to_ascii_lowercase();
        if let Err(pos) = tree.binary_search_by(|t| t.to_ascii_lowercase().cmp(&key)) {
            tree.insert(pos, name);
        }
    }
    for (i, name) in tree.iter().enumerate() {
        cv::combo_add_item(ui, combo, name, "");
        if name.eq_ignore_ascii_case(&cur) {
            cv::combo_set_initial_item(ui, combo, i as i32, s);
        }
    }
}

impl MultiplayerPage {
    /// `InitCrosshairColorEntries`.
    fn init_crosshair_color_entries(&mut self, ui: &mut Ui, page: PanelId, s: &mut dyn Surface) {
        let mut rgb = (50, 250, 50);
        if let Some(cs) = self.host.borrow().cvar_string("cl_crosshair_color") {
            let mut toks = Vec::new();
            let mut p: &[u8] = cs.as_bytes();
            for _ in 0..3 {
                match gs_common::token::com_parse(p, false) {
                    Some((t, rest)) => {
                        toks.push(String::from_utf8_lossy(&t).into_owned());
                        p = rest;
                    }
                    None => toks.push(String::new()),
                }
            }
            rgb = (c_atoi(&toks[0]), c_atoi(&toks[1]), c_atoi(&toks[2]));
        }
        if let Some(cc) = self.crosshair_color {
            let mut sel = 0;
            for (k, &(name, r, g, b)) in CROSSHAIR_COLORS.iter().enumerate() {
                ComboBox::add_item(ui, cc, name, Some(Msg::new("data").int("red", r).int("green", g).int("blue", b)));
                if rgb == (r, g, b) {
                    sel = k as i32;
                }
            }
            ComboBox::activate_item_by_row(ui, cc, sel, s);
        }
        self.update_crosshair_from_color_combo(ui, page);
    }

    /// `UpdateCrosshairFromColorCombo`.
    fn update_crosshair_from_color_combo(&mut self, ui: &mut Ui, page: PanelId) {
        let Some(cc) = self.crosshair_color else { return };
        let ud = ComboBox::active_item_user_data(ui, cc);
        let get = |k: &str| ud.as_ref().map_or(0, |m| m.get_int(k));
        let rgb = (get("red"), get("green"), get("blue"));
        if rgb != self.rgb {
            self.rgb = rgb;
            self.on_control_modified(ui, page);
        }
        if let (Some(img), Some(size)) = (self.crosshair_image, self.crosshair_size) {
            let active = cv::combo_active_item(ui, size);
            CrosshairImage::update(ui, img, self.rgb, active);
        }
    }

    /// `OnControlModified`: `ApplyButtonEnable` to the parent (the sheet).
    fn on_control_modified(&self, ui: &mut Ui, page: PanelId) {
        if let Some(p) = ui.parent(page) {
            ui.post(p, Msg::new("ApplyButtonEnable"), None, 0.0);
        }
        ui.invalidate_layout(page);
    }

    /// `RemapModel`.
    fn remap_model(&self, ui: &mut Ui) {
        let t = gs_common::bytestr::ansi_cap(&cv::combo_text(ui, self.player_model), 256);
        if t.is_empty() {
            return;
        }
        remap_model_palette(self.fs.as_ref(), &t, self.topcolor, self.bottomcolor);
        BitmapImagePanel::set_image(ui, self.model_image, "models/player/remapped");
    }

    /// `OnTextChanged(panel)`.
    fn on_text_changed(&mut self, ui: &mut Ui, page: PanelId, panel: Option<PanelId>) {
        if panel == Some(self.name_entry) {
            return;
        }
        self.remap_model(ui);
        let t = gs_common::bytestr::ansi_cap(&cv::combo_text(ui, self.spray_list), 256);
        if !t.is_empty() {
            let img = if self.fs.exists(&format!("logos/{t}.tga")) {
                ui.set_enabled(self.spray_color, false);
                Some(format!("logos/{t}"))
            } else {
                match cv::combo_active_command(ui, self.spray_color).filter(|c| !c.is_empty()) {
                    Some(cmd) => {
                        let (r, g, b) = color_for_name(cmd.get(13..).unwrap_or(""));
                        remap_logo(self.fs.as_ref(), &t, r, g, b);
                        ui.set_enabled(self.spray_color, true);
                        Some("logos/remapped".to_owned())
                    }
                    None => None,
                }
            };
            if let Some(img) = img {
                BitmapImagePanel::set_image(ui, self.logo_image, &img);
            }
        }
        self.update_crosshair_from_color_combo(ui, page);
    }
}

impl PageHooks for MultiplayerPage {
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) -> bool {
        if msg.is("TextChanged") {
            self.on_text_changed(ui, me, msg.get_panel("panel"));
        } else if msg.is("SliderMoved") {
            self.topcolor = cv::slider_value(ui, self.primary) as i32;
            self.bottomcolor = cv::slider_value(ui, self.secondary) as i32;
            self.remap_model(ui);
            self.update_crosshair_from_color_combo(ui, me);
        } else if msg.is("ControlModified") {
            self.on_control_modified(ui, me);
        } else if msg.is("Command") {
            let cmd = msg.get_str("command");
            if cmd.eq_ignore_ascii_case("Advanced") {
                let alive = self.advanced.filter(|&a| ui.exists(a));
                let dlg = match alive {
                    Some(a) => a,
                    None => {
                        let a = crate::advanced::create_multiplayer_advanced_dialog(ui, me, &self.host, &self.fs, s);
                        self.advanced = Some(a);
                        a
                    }
                };
                gs_vgui::controls::frame::with_frame(ui, dlg, |_, ui| ui.post(dlg, Msg::new("Activate"), None, 0.0));
            } else if cmd.eq_ignore_ascii_case("ShowKeyboard") {
                self.host.borrow_mut().client_cmd("show_osk 609 273");
            }
            return false;
        } else {
            return false;
        }
        true
    }

    /// `OnApplyChanges` (exact order).
    fn on_apply_changes(&mut self, ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        cv::apply_changes(ui, self.primary, false);
        cv::apply_changes(ui, self.secondary, false);
        cv::apply_changes(ui, self.player_model, false);
        cv::apply_changes(ui, self.spray_list, false);
        self.logo_name = gs_common::bytestr::ansi_cap(&cv::combo_text(ui, self.spray_list), 32);
        cv::apply_changes(ui, self.spray_color, false);
        cv::apply_changes(ui, self.hq_models, false);
        cv::apply_changes(ui, self.name_entry, false);
        if let Some(c) = self.crosshair_size {
            cv::apply_changes(ui, c, false);
        }
        if let Some(c) = self.translucency {
            cv::apply_changes(ui, c, false);
        }
        let (r, g, b) = self.rgb;
        if r != -1 && g != -1 && b != -1 {
            self.host.borrow_mut().cvar_set("cl_crosshair_color", &format!("{r} {g} {b}"));
        }
        let Some(cmd) = cv::combo_active_command(ui, self.spray_color).filter(|c| !c.is_empty()) else { return };
        self.host.borrow_mut().client_cmd(&format!("cl_logofile {}\n", self.logo_name).chars().take(0x1FE).collect::<String>());
        let tga = format!("logos/{}.tga", self.logo_name);
        if self.fs.exists(&tga) {
            if let Some(wad) = logo_wad_from_tga(self.fs.as_ref(), &tga) {
                self.fs.write_config("tempdecal.wad", &wad);
            }
            return;
        }
        let (r, g, b) = color_for_name(cmd.get(13..).unwrap_or(""));
        let Some(data) = self.fs.read("logos/remapped.bmp") else { return };
        if data.len() >= 14 && data[..2] == *b"BM"
            && let Some(wad) = logo_wad_from_bmp(&data[14..], r, g, b)
        {
            self.fs.write_config("tempdecal.wad", &wad);
        }
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `RemapLogo`: `logos/<name>.bmp` with biClrUsed 256 and a black→colour ramp palette written
/// to `logos/remapped.bmp` (an empty file when the source cannot be read).
fn remap_logo(fs: &dyn GameFs, name: &str, r: i32, g: i32, b: i32) {
    let mut buf = Vec::new();
    if let Some(src) = fs.read(&format!("logos/{name}.bmp")) {
        let hdr_len = src.len().min(14);
        buf.extend_from_slice(&src[..hdr_len]);
        if hdr_len == 14 && src[..2] == *b"BM" {
            let mut data = src[14..].to_vec();
            if let Some(c) = data.get_mut(0x20..0x24) {
                c.copy_from_slice(&256u32.to_le_bytes());
            }
            for i in 0..256usize {
                let f = i as f32 / 256.0;
                let o = 0x28 + i * 4;
                if let Some(p) = data.get_mut(o..o + 3) {
                    p[0] = (b as f32 * f) as i32 as u8;
                    p[1] = (g as f32 * f) as i32 as u8;
                    p[2] = (r as f32 * f) as i32 as u8;
                }
            }
            buf.extend_from_slice(&data);
        }
    }
    fs.remove("logos/remapped.bmp", Some("GAMECONFIG"));
    fs.write_config("logos/remapped.bmp", &buf);
}

/// `RemapModelPalette`: the model's preview bitmap with the top/bottom colour hue ramps.
fn remap_model_palette(fs: &dyn GameFs, model: &str, top: i32, bottom: i32) {
    let mut buf = Vec::new();
    let path: String = format!("models/player/{model}/{model}.bmp").chars().take(255).collect();
    if let Some(src) = fs.read(&path) {
        let hdr_len = src.len().min(14);
        buf.extend_from_slice(&src[..hdr_len]);
        if hdr_len == 14 && src[..2] == *b"BM" {
            let mut data = src[14..].to_vec();
            if let Some(pal) = data.get_mut(0x28..0x28 + 1024) {
                if top < 223 {
                    remap_palette_hue(pal, top, 192, 223);
                }
                if bottom < 191 {
                    remap_palette_hue(pal, bottom, 160, 191);
                }
            }
            buf.extend_from_slice(&data);
        }
    }
    fs.remove("models/player/remapped.bmp", Some("GAMECONFIG"));
    fs.write_config("models/player/remapped.bmp", &buf);
}

/// `RemapPaletteHue` (float math, truncation; black gives NaN → 0).
fn remap_palette_hue(pal: &mut [u8], top: i32, start: usize, end: usize) {
    let hue = (f64::from(top) * 1.411_764_705_882_353) as f32;
    let to_byte = |v: f32| if (-2_147_483_648.0..2_147_483_648.0).contains(&(v * 255.0)) { (v * 255.0) as i32 as u8 } else { 0 };
    for i in start..end {
        let (c0, c1, c2) = (pal[4 * i], pal[4 * i + 1], pal[4 * i + 2]);
        let mx = f32::from(c0.max(c1).max(c2)) / 255.0;
        let mn = f32::from(c0.min(c1).min(c2)) / 255.0;
        let s = (mx - mn) / mx;
        let lo = (1.0 - s) * mx;
        let d = mx - lo;
        let (o0, o1, o2) = if hue <= 120.0 {
            if hue < 60.0 { (lo, d * hue / (120.0 - hue) + lo, mx) } else { (lo, mx, (120.0 - hue) * d / hue + lo) }
        } else if hue <= 240.0 {
            if hue < 180.0 { ((hue - 120.0) * d / (240.0 - hue) + lo, mx, lo) } else { (mx, (240.0 - hue) * d / (hue - 120.0) + lo, lo) }
        } else if hue < 300.0 {
            (mx, lo, (hue - 240.0) * d / (360.0 - hue) + lo)
        } else {
            ((360.0 - hue) * d / (hue - 240.0) + lo, lo, mx)
        };
        pal[4 * i] = to_byte(o0);
        pal[4 * i + 1] = to_byte(o1);
        pal[4 * i + 2] = to_byte(o2);
    }
}

/// A one-lump `WAD3` with `LOGO`.
fn wad3(mips: &[u8], disk: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + mips.len() + 32);
    out.extend_from_slice(b"WAD3");
    out.extend_from_slice(&1i32.to_le_bytes());
    out.extend_from_slice(&((mips.len() + 12) as i32).to_le_bytes());
    out.extend_from_slice(mips);
    out.extend_from_slice(&12i32.to_le_bytes());
    out.extend_from_slice(&(disk as i32).to_le_bytes());
    out.extend_from_slice(&(disk as i32).to_le_bytes());
    out.extend_from_slice(&[0x40, 0, 0, 0]);
    let mut name = [0u8; 16];
    name[..4].copy_from_slice(b"LOGO");
    out.extend_from_slice(&name);
    out
}

/// The miptex header, the four mip levels (top-left sample of each block) and the palette
/// tail; `None` when a side is not a multiple of 16.
fn mip_tex(mip0: &[u8], w: usize, h: usize, palette_tail: [u8; 3]) -> Option<Vec<u8>> {
    if (w | h) & 0xF != 0 {
        return None;
    }
    let mut out = Vec::new();
    let mut name = [0u8; 16];
    name[..4].copy_from_slice(b"LOGO");
    out.extend_from_slice(&name);
    out.extend_from_slice(&(w as u32).to_le_bytes());
    out.extend_from_slice(&(h as u32).to_le_bytes());
    let offs_at = out.len();
    out.extend_from_slice(&[0u8; 16]);
    let mut offs = [0u32; 4];
    offs[0] = out.len() as u32;
    out.extend_from_slice(mip0);
    for (k, scale) in [2usize, 4, 8].into_iter().enumerate() {
        offs[k + 1] = out.len() as u32;
        for y in (0..h).step_by(scale) {
            for x in (0..w).step_by(scale) {
                out.push(mip0[y * w + x]);
            }
        }
    }
    for (k, o) in offs.iter().enumerate() {
        out[offs_at + k * 4..offs_at + k * 4 + 4].copy_from_slice(&o.to_le_bytes());
    }
    out.extend_from_slice(&256u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 765]);
    out.extend_from_slice(&palette_tail);
    Some(out)
}

/// `UpdateLogoWAD_BMP`: an 8-bit BMP (after its file header) → tempdecal.wad bytes.
fn logo_wad_from_bmp(bmp: &[u8], r: i32, g: i32, b: i32) -> Option<Vec<u8>> {
    let rd = |o: usize| bmp.get(o..o + 4).map(|v| i32::from_le_bytes([v[0], v[1], v[2], v[3]]));
    let (w, h) = (usize::try_from(rd(4)?).ok()?, usize::try_from(rd(8)?).ok()?);
    let pixels = bmp.get(0x428..)?;
    let mut mip0 = vec![0u8; w * h];
    for y in 0..h {
        let src = pixels.get((h - 1 - y) * w..(h - y) * w)?;
        mip0[y * w..(y + 1) * w].copy_from_slice(src);
    }
    let mut mips = mip_tex(&mip0, w, h, [r as u8, g as u8, b as u8])?;
    if w != h || ![16, 32, 64].contains(&w) {
        return None;
    }
    while mips.len() % 4 != 0 {
        mips.push(0);
    }
    let disk = mips.len();
    Some(wad3(&mips, disk))
}

/// `UpdateLogoWAD_TGA`: the three channel mips (R, G, B) of a coloured spray.
fn logo_wad_from_tga(fs: &dyn GameFs, path: &str) -> Option<Vec<u8>> {
    let data = fs.read(path)?;
    let (w, h, rgba) = load_tga_gameui(&data)?;
    let mut all = Vec::new();
    let mut first = 0;
    for (k, (ch, tail)) in [(0usize, [255u8, 0, 0]), (1, [0, 255, 0]), (2, [0, 0, 255])].into_iter().enumerate() {
        let mip0: Vec<u8> = rgba
            .chunks_exact(4)
            .map(|p| {
                let (c, a) = (f32::from(p[ch]), f32::from(p[3]));
                let v = 255.0 / a * c;
                if (-2_147_483_648.0..2_147_483_648.0).contains(&v) { v as i32 as u8 } else { 0 }
            })
            .collect();
        let mut m = mip_tex(&mip0, w, h, tail)?;
        while m.len() % 4 != 0 {
            m.push(0);
        }
        if k == 0 {
            first = m.len();
        }
        all.extend_from_slice(&m);
    }
    if w != h || ![16, 32, 64].contains(&w) {
        return None;
    }
    Some(wad3(&all, first))
}

/// GameUI's `LoadTGA`: types 2/10, no colour map, 24/32 bpp; rows written from the bottom up;
/// RGBA out.
fn load_tga_gameui(d: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let hdr = d.get(..18)?;
    let (id_len, cmap, ty) = (hdr[0] as usize, hdr[1], hdr[2]);
    let w = usize::from(u16::from_le_bytes([hdr[12], hdr[13]]));
    let h = usize::from(u16::from_le_bytes([hdr[14], hdr[15]]));
    let bpp = hdr[16];
    if (ty != 2 && ty != 10) || cmap != 0 || (bpp != 24 && bpp != 32) {
        return None;
    }
    let px = usize::from(bpp / 8);
    let mut src = d.get(18 + id_len..)?.iter().copied();
    let mut out = vec![0u8; w * h * 4];
    let read = |src: &mut dyn Iterator<Item = u8>| -> Option<[u8; 4]> {
        let b = src.next()?;
        let g = src.next()?;
        let r = src.next()?;
        let a = if px == 4 { src.next()? } else { 255 };
        Some([r, g, b, a])
    };
    if ty == 2 {
        for row in (0..h).rev() {
            for x in 0..w {
                let p = read(&mut src)?;
                out[(row * w + x) * 4..(row * w + x) * 4 + 4].copy_from_slice(&p);
            }
        }
    } else {
        let (mut row, mut x) = (h as isize - 1, 0usize);
        while row >= 0 {
            let head = src.next()?;
            let n = usize::from(head & 0x7F) + 1;
            let run = if head & 0x80 != 0 { Some(read(&mut src)?) } else { None };
            for _ in 0..n {
                let p = match run {
                    Some(p) => p,
                    None => read(&mut src)?,
                };
                let o = (row as usize * w + x) * 4;
                out[o..o + 4].copy_from_slice(&p);
                x += 1;
                if x == w {
                    x = 0;
                    row -= 1;
                    if row < 0 {
                        break;
                    }
                }
            }
        }
    }
    Some((w, h, out))
}

// ---------------------------------------------------------------------------------------------
// Audio page (§7)

struct AudioPage {
    sfx: PanelId,
    suit: PanelId,
    mp3: PanelId,
    quality: PanelId,
}

fn create_audio_page(ui: &mut Ui, dlg: PanelId, host: &Shared, fs: &Fs, mi: &ModInfo, s: &mut dyn Surface) -> PanelId {
    let page = PropertyPage::create(ui, Some(dlg), "OptionsSubAudio", true, Box::new(NoHooks));
    let sfx = cv::create_slider(ui, Some(page), "SFX Slider", 0.0, 2.0, "volume", false, host.clone());
    let suit = cv::create_slider(ui, Some(page), "Suit Slider", 0.0, 2.0, "suitvolume", false, host.clone());
    let mp3 = cv::create_slider(ui, Some(page), "MP3 Volume", 0.0, 1.0, "mp3volume", false, host.clone());
    let quality = cv::create_combo(ui, Some(page), "Sound Quality", host.clone());
    cv::combo_add_item(ui, quality, "#GameUI_High", "hisound 1");
    cv::combo_add_item(ui, quality, "#GameUI_Low", "hisound 0");
    let hs = host.borrow().cvar_float("hisound");
    cv::combo_set_initial_item(ui, quality, i32::from(hs == 0.0), s);
    load_res(ui, page, fs.as_ref(), "Resource\\OptionsSubAudio.res", s);
    if mi.is_multiplayer_only() {
        for n in ["suit label", "Suit Slider"] {
            if let Some(p) = ui.find_child_by_name(page, n) {
                ui.set_visible(p, false);
            }
        }
    }
    set_page_hooks(ui, page, Box::new(AudioPage { sfx, suit, mp3, quality }));
    page
}

impl PageHooks for AudioPage {
    fn on_apply_changes(&mut self, ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        for c in [self.sfx, self.suit, self.mp3, self.quality] {
            cv::apply_changes(ui, c, false);
        }
    }
    fn on_reset_data(&mut self, ui: &mut Ui, _me: PanelId, s: &mut dyn Surface) {
        for c in [self.sfx, self.suit, self.mp3, self.quality] {
            cv::reset(ui, c, s);
        }
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _s: &mut dyn Surface) -> bool {
        if msg.is("ControlModified") {
            apply_button_enable(ui, me);
            return true;
        }
        false
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// Mouse page (§6)

struct MousePage {
    reverse: PanelId,
    low_latency: PanelId,
    filter: PanelId,
    slider: PanelId,
    raw: PanelId,
    label: PanelId,
    auto_aim: PanelId,
}

fn create_mouse_page(ui: &mut Ui, dlg: PanelId, host: &Shared, fs: &Fs, s: &mut dyn Surface) -> PanelId {
    let page = PropertyPage::create(ui, Some(dlg), "OptionsSubMouse", true, Box::new(NoHooks));
    let reverse = cv::create_check(ui, Some(page), "ReverseMouse", "#GameUI_ReverseMouse", Some("m_pitch"), true, host.clone(), s);
    let low_latency = cv::create_check(ui, Some(page), "LowLatency", "#GameUI_LowLatency", Some("gl_lowlatency"), false, host.clone(), s);
    let filter = cv::create_check(ui, Some(page), "MouseFilter", "#GameUI_MouseFilter", Some("m_filter"), false, host.clone(), s);
    let slider = cv::create_slider(ui, Some(page), "Slider", 0.2, 20.0, "sensitivity", true, host.clone());
    let raw = cv::create_check(ui, Some(page), "RawInput", "#GameUI_RawInput", Some("m_rawinput"), false, host.clone(), s);
    let label = TextEntry::create(ui, Some(page), "SensitivityLabel");
    ui.add_action_signal_target(label, page);
    let auto_aim = cv::create_check(ui, Some(page), "Auto-Aim", "#GameUI_AutoAim", Some("sv_aim"), false, host.clone(), s);
    load_res(ui, page, fs.as_ref(), "Resource\\OptionsSubMouse.res", s);
    let text = format!(" {:.1}", f64::from(host.borrow().cvar_float("sensitivity")));
    with_te(ui, label, |te, ui| te.set_text(ui, label, &text.chars().take(63).collect::<String>(), s));
    set_page_hooks(ui, page, Box::new(MousePage { reverse, low_latency, filter, slider, raw, label, auto_aim }));
    page
}

impl PageHooks for MousePage {
    fn apply_scheme(&mut self, ui: &mut Ui, _me: PanelId, scheme: &Scheme, _s: &mut dyn Surface) {
        let b = scheme.border("ButtonDepressedBorder");
        if let Some(n) = ui.node_mut(self.label) {
            n.border = b;
        }
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) -> bool {
        if msg.is("ControlModified") {
            apply_button_enable(ui, me);
            if msg.get_panel("panel") == Some(self.slider) && cv::has_been_modified(ui, self.slider) {
                let t = format!(" {:.1}", f64::from(cv::slider_value(ui, self.slider)));
                let label = self.label;
                with_te(ui, label, |te, ui| te.set_text(ui, label, &t, s));
            }
            return true;
        }
        if msg.is("TextChanged") {
            if msg.get_panel("panel") == Some(self.label) {
                let text = gs_common::bytestr::ansi_cap(&with_te(ui, self.label, |te, _| te.text()).unwrap_or_default(), 64);
                let f = c_atof(&text) as f32;
                if f >= 1.0 {
                    cv::set_slider_value(ui, self.slider, f);
                    apply_button_enable(ui, me);
                }
            }
            return true;
        }
        false
    }
    fn on_apply_changes(&mut self, ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        for c in [self.reverse, self.low_latency, self.filter, self.slider, self.auto_aim, self.raw] {
            cv::apply_changes(ui, c, false);
        }
    }
    fn on_reset_data(&mut self, ui: &mut Ui, _me: PanelId, s: &mut dyn Surface) {
        for c in [self.reverse, self.low_latency, self.filter, self.slider, self.raw, self.auto_aim] {
            cv::reset(ui, c, s);
        }
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
