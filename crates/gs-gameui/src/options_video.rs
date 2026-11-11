//! Video page — `COptionsSubVideo` (docs/re/gameui-dialogs.md §8).

use std::any::Any;

use gs_vgui::controls::combo_box::ComboBox;
use gs_vgui::controls::property::{NoHooks, PageHooks, PropertyPage};
use gs_vgui::controls::toggle::{ToggleButton, ToggleKind, with_toggle};
use gs_vgui::{Msg, PanelId, Surface, Ui};

use crate::cvar_controls as cv;
use crate::options::{load_res, set_page_hooks};
use crate::{Fs, Shared};

/// The 0x90-byte settings struct: `w, h, bpp, windowed, renderer[128]`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Settings {
    w: i32,
    h: i32,
    bpp: i32,
    windowed: bool,
    renderer: String,
}

struct VideoPage {
    host: Shared,
    orig: Settings,
    current: Settings,
    brightness: PanelId,
    gamma: PanelId,
    resolution: PanelId,
    aspect: PanelId,
    renderer: PanelId,
    windowed: PanelId,
    vsync: PanelId,
    widescreen_fov: PanelId,
    texture_filtering: PanelId,
    use_shaders: PanelId,
    aspect_names: [String; 2],
    /// The renderers offered: `_setrenderer` name and combo text.
    renderers: Vec<(String, String)>,
    widescreen: bool,
    renderer_index: i32,
}

/// `localize()->Find(token)` converted to ANSI (31 chars).
fn localized(ui: &Ui, token: &str) -> String {
    let w = token.strip_prefix('#').and_then(|k| ui.localize.find(k)).unwrap_or("");
    w.chars().take(31).collect()
}

pub fn create_video_page(ui: &mut Ui, dlg: PanelId, host: &Shared, fs: &Fs, s: &mut dyn Surface) -> PanelId {
    let page = PropertyPage::create(ui, Some(dlg), "OptionsSubVideo", true, Box::new(NoHooks));
    let brightness = cv::create_slider(ui, Some(page), "Brightness", 0.0, 2.0, "brightness", false, host.clone());
    let gamma = cv::create_slider(ui, Some(page), "Gamma", 1.0, 3.0, "gamma", false, host.clone());
    // GetCurrentRenderer: the running backend; the original's two renderers when the engine
    // names no backends.
    let (orig, renderers) = {
        let h = host.borrow();
        let name = if h.renderer.is_empty() { "gl".to_owned() } else { h.renderer.clone() };
        let renderers = if h.renderers.is_empty() {
            vec![("software".to_owned(), localized(ui, "#GameUI_Software")), ("gl".to_owned(), localized(ui, "#GameUI_OpenGL"))]
        } else {
            h.renderers.clone()
        };
        (Settings { w: h.current_mode.width, h: h.current_mode.height, bpp: h.current_mode.bpp, windowed: h.windowed, renderer: name }, renderers)
    };
    let current = orig.clone();
    let resolution = ComboBox::create(ui, Some(page), "Resolution", 6, false);
    let aspect = ComboBox::create(ui, Some(page), "AspectRatio", 2, false);
    let vsync = cv::create_check(ui, Some(page), "Vsync", "#GameUI_VSync", Some("gl_vsync"), false, host.clone(), s);
    let aspect_names = [localized(ui, "#GameUI_AspectNormal"), localized(ui, "#GameUI_AspectWide")];
    let id_n = ComboBox::add_item(ui, aspect, &aspect_names[0], None);
    let id_w = ComboBox::add_item(ui, aspect, &aspect_names[1], None);
    let widescreen = current.w * 3 - current.h * 4 != 0;
    ComboBox::activate_item(ui, aspect, if widescreen { id_w } else { id_n }, s);
    let renderer = ComboBox::create(ui, Some(page), "Renderer", renderers.len().max(2) as i32, false);
    for (_, label) in &renderers {
        ComboBox::add_item(ui, renderer, label, None);
    }
    let renderer_index = renderer_row(&renderers, &current.renderer);
    ComboBox::activate_item_by_row(ui, renderer, renderer_index, s);
    let windowed = ToggleButton::create(ui, Some(page), "Windowed", "#GameUI_Windowed", ToggleKind::Check);
    with_toggle(ui, windowed, |t, ui| t.set_selected(ui, windowed, current.windowed, s));
    let widescreen_fov = cv::create_check(ui, Some(page), "WidescreenFOV", "#GameUI_WidescreenFOV", Some("gl_widescreen_yfov"), false, host.clone(), s);
    ui.set_enabled(widescreen_fov, false);
    ui.set_visible(widescreen_fov, false);
    let texture_filtering = cv::create_check(ui, Some(page), "TextureFiltering", "#GameUI_TextureFiltering", Some("r_prefertexturefiltering"), false, host.clone(), s);
    let use_shaders = cv::create_check(ui, Some(page), "UseShaders", "#GameUI_UseShaders", Some("gl_use_shaders"), false, host.clone(), s);
    load_res(ui, page, fs.as_ref(), "Resource\\OptionsSubVideo.res", s);
    let mut vp = VideoPage {
        host: host.clone(),
        orig,
        current,
        brightness,
        gamma,
        resolution,
        aspect,
        renderer,
        windowed,
        vsync,
        widescreen_fov,
        texture_filtering,
        use_shaders,
        aspect_names,
        renderers,
        widescreen,
        renderer_index,
    };
    vp.prepare_resolution_list(ui, true, s);
    set_page_hooks(ui, page, Box::new(vp));
    page
}

/// The combo row of the renderer named `name` (0 when it is not offered).
fn renderer_row(renderers: &[(String, String)], name: &str) -> i32 {
    renderers.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)).map_or(0, |i| i as i32)
}

impl VideoPage {
    /// `PrepareResolutionList(filter)`.
    fn prepare_resolution_list(&mut self, ui: &mut Ui, filter: bool, s: &mut dyn Surface) {
        let modes = self.host.borrow().video_modes.clone();
        ComboBox::delete_all_items(ui, self.resolution);
        let sw = gs_common::bytestr::ansi_cap(&ComboBox::text(ui, self.renderer), 256);
        let software = sw.eq_ignore_ascii_case("software");
        let (mut sel, mut added, mut found_wide) = (-1, 0, false);
        for m in &modes {
            let text = format!("{} x {}", m.width, m.height);
            if filter && modes.len() > 1 {
                if m.width < 640 || m.height < 480 {
                    continue;
                }
                if software && (m.width > 1599 || m.height > 1199) {
                    continue;
                }
            }
            let mut id = -1;
            if m.width * 3 - m.height * 4 == 0 {
                if !self.widescreen {
                    id = ComboBox::add_item(ui, self.resolution, &text, None);
                    added += 1;
                }
            } else {
                if self.widescreen {
                    id = ComboBox::add_item(ui, self.resolution, &text, None);
                    added += 1;
                }
                found_wide = true;
            }
            if m.width == self.current.w && m.height == self.current.h {
                sel = id;
            }
        }
        ui.set_enabled(self.aspect, found_wide);
        ComboBox::activate_item(ui, self.resolution, if sel != -1 { sel } else { 0 }, s);
        if added == 0 && !modes.is_empty() {
            self.widescreen = !self.widescreen;
            let next = (ComboBox::active_item(ui, self.aspect) + 1) % 2;
            ComboBox::activate_item(ui, self.aspect, next, s);
            self.prepare_resolution_list(ui, false, s);
        }
    }

    fn on_control_modified(ui: &mut Ui, me: PanelId) {
        ui.post_action_signal(me, Msg::new("ApplyButtonEnable"));
    }
}

impl PageHooks for VideoPage {
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) -> bool {
        if msg.is("ControlModified") {
            Self::on_control_modified(ui, me);
        } else if msg.is("TextChanged") {
            let p = msg.get_panel("panel");
            let text = msg.get_str("text");
            if p == Some(self.resolution) {
                if text != format!("{} x {}", self.current.w, self.current.h) {
                    Self::on_control_modified(ui, me);
                }
            } else if p == Some(self.renderer) {
                self.prepare_resolution_list(ui, true, s);
                if self.renderers.get(self.renderer_index as usize).is_none_or(|(_, label)| text != *label) {
                    Self::on_control_modified(ui, me);
                }
            } else if p == Some(self.aspect) && text != self.aspect_names[usize::from(self.widescreen)] {
                self.widescreen = !self.widescreen;
                self.prepare_resolution_list(ui, true, s);
            }
        } else if msg.is("CheckButtonChecked") {
            let state = msg.get_int("state");
            let p = msg.get_panel("panel");
            if (p == Some(self.windowed) && state != i32::from(self.current.windowed)) || p == Some(self.vsync) {
                Self::on_control_modified(ui, me);
            }
        } else {
            return false;
        }
        true
    }

    /// `OnApplyChanges`.
    fn on_apply_changes(&mut self, ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        let gamma_changed = cv::has_been_modified(ui, self.gamma);
        cv::apply_changes(ui, self.brightness, false);
        cv::apply_changes(ui, self.gamma, false);
        for c in [self.widescreen_fov, self.texture_filtering, self.use_shaders] {
            cv::apply_changes(ui, c, false);
        }
        let t = gs_common::bytestr::ansi_cap(&ComboBox::text(ui, self.resolution), 256);
        let mut it = t.split(" x ");
        if let (Some(w), Some(h)) = (it.next(), it.next()) {
            self.current.w = crate::options::c_atoi(w);
            self.current.h = crate::options::c_atoi(h);
        }
        self.current.bpp = 32;
        let r = gs_common::bytestr::ansi_cap(&ComboBox::text(ui, self.renderer), 256);
        if let Some((name, _)) = self.renderers.iter().find(|(_, label)| r.eq_ignore_ascii_case(label)) {
            self.current.renderer = name.clone();
        }
        if self.current.renderer.eq_ignore_ascii_case(&self.orig.renderer) && self.orig.bpp == 16 {
            self.current.bpp = 16;
        }
        self.current.windowed = with_toggle(ui, self.windowed, |t, _| t.is_selected()).unwrap_or(false);
        cv::apply_changes(ui, self.vsync, false);
        if self.orig != self.current || gamma_changed {
            let c = &self.current;
            let mut host = self.host.borrow_mut();
            host.client_cmd(&format!("_setvideomode {} {} {}\n", c.w, c.h, c.bpp));
            host.client_cmd(&format!("_setrenderer {} {}\n", c.renderer, if c.windowed { "windowed" } else { "fullscreen" }));
            host.client_cmd("_restart\n");
        }
    }

    /// `OnResetData`.
    fn on_reset_data(&mut self, ui: &mut Ui, _me: PanelId, s: &mut dyn Surface) {
        self.current = self.orig.clone();
        cv::reset(ui, self.brightness, s);
        cv::reset(ui, self.gamma, s);
        let w = self.current.windowed;
        let wid = self.windowed;
        with_toggle(ui, wid, |t, ui| t.set_selected(ui, wid, w, s));
        for c in [self.texture_filtering, self.vsync, self.use_shaders] {
            cv::reset(ui, c, s);
        }
        self.renderer_index = renderer_row(&self.renderers, &self.current.renderer);
        ComboBox::activate_item_by_row(ui, self.renderer, self.renderer_index, s);
        let modes = self.host.borrow().video_modes.clone();
        if let Some(m) = modes.iter().find(|m| m.width == self.current.w && m.height == self.current.h) {
            let t = format!("{} x {}", m.width, m.height);
            let r = self.resolution;
            ui.with_ctrl(r, |c, ui| {
                if let Some(cb) = c.as_any_mut().downcast_mut::<ComboBox>() {
                    cb.te.set_text(ui, r, &t, s);
                }
            });
        }
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
