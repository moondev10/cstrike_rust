//! Content page — `COptionsSubAdvanced` and `CContentControlDialog`
//! (docs/re/gameui-dialogs.md §10).

use std::any::Any;

use md5::{Digest, Md5};

use gs_vgui::controls::button::Button;
use gs_vgui::controls::frame::{FrameMsg, FrameState};
use gs_vgui::controls::label::{Label, with_label};
use gs_vgui::controls::property::{NoHooks, PageHooks, PropertyPage};
use gs_vgui::controls::text_entry::{TextEntry, with_te};
use gs_vgui::controls::toggle::{ToggleButton, ToggleKind, with_toggle};
use gs_vgui::scheme::Scheme;
use gs_vgui::{Control, Msg, PanelId, Surface, Ui};

use crate::cvar_controls as cv;
use crate::options::{ModInfo, load_res, set_page_hooks, task_frame};
use crate::{Fs, Shared};

/// The registry value holding the gore lock's hash.
pub const GORE_KEY: &str = "Software\\Valve\\Half-Life\\Settings\\User Token 2";

/// `HashPassword`: 32 lowercase hex digits of the MD5.
fn hash_password(pw: &str) -> String {
    Md5::digest(pw.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn registry_get(host: &Shared, key: &str) -> String {
    host.borrow().registry.get(key).cloned().unwrap_or_default().chars().take(63).collect()
}

// ---------------------------------------------------------------------------------------------
// CContentControlDialog

pub struct ContentControlDialog {
    pub fs: FrameState,
    host: Shared,
    gore_pw: String,
    status: PanelId,
    prompt: PanelId,
    reentry_prompt: PanelId,
    explain: PanelId,
    password: PanelId,
    password2: PanelId,
}

fn with_ccd<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut ContentControlDialog, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<ContentControlDialog>().map(|d| f(d, ui))).flatten()
}

impl ContentControlDialog {
    fn create(ui: &mut Ui, parent: Option<PanelId>, host: &Shared, fs: &Fs, s: &mut dyn Surface) -> PanelId {
        let placeholder = ui.embedded();
        let me = ui.create(
            parent,
            "ContentControlDialog",
            Box::new(ContentControlDialog { fs: FrameState::default(), host: host.clone(), gore_pw: String::new(), status: placeholder, prompt: placeholder, reentry_prompt: placeholder, explain: placeholder, password: placeholder, password2: placeholder }),
        );
        FrameState::init(ui, me, false);
        ui.set_bounds(me, 0, 0, 372, 160);
        FrameState::set_sizeable(ui, me, false);
        task_frame(ui, me, "#GameUI_ContentLock", false);
        let status = Label::create(ui, Some(me), "ContentStatus", "");
        let password = TextEntry::create(ui, Some(me), "Password");
        Button::create(ui, Some(me), "Ok", "#GameUI_OK", Some("Ok"));
        let prompt = Label::create(ui, Some(me), "PasswordPrompt", "#GameUI_PasswordPrompt");
        let reentry_prompt = Label::create(ui, Some(me), "PasswordReentryPrompt", "#GameUI_PasswordReentryPrompt");
        let password2 = TextEntry::create(ui, Some(me), "Password2");
        let explain = Label::create(ui, Some(me), "ContentControlExplain", "");
        Button::create(ui, Some(me), "Cancel", "#GameUI_Cancel", Some("Cancel"));
        let gore_pw = registry_get(host, GORE_KEY);
        load_res(ui, me, fs.as_ref(), "Resource\\ContentControlDialog.res", s);
        with_ccd(ui, me, |d, _| {
            d.gore_pw = gore_pw;
            d.status = status;
            d.prompt = prompt;
            d.reentry_prompt = reentry_prompt;
            d.explain = explain;
            d.password = password;
            d.password2 = password2;
        });
        me
    }

    fn set_label(ui: &mut Ui, id: PanelId, text: &str) {
        with_label(ui, id, |l, ui| l.set_text(ui, text));
    }

    fn entry_text(ui: &mut Ui, id: PanelId) -> String {
        gs_common::bytestr::ansi_cap(&with_te(ui, id, |te, _| te.text()).unwrap_or_default(), 256)
    }

    /// `UpdateContentControlStatus`.
    fn update_status(&self, ui: &mut Ui) {
        let locked = !self.gore_pw.is_empty();
        Self::set_label(ui, self.status, if locked { "#GameUI_ContentStatusEnabled" } else { "#GameUI_ContentStatusDisabled" });
        Self::set_label(ui, self.prompt, if locked { "#GameUI_PasswordDisablePrompt" } else { "#GameUI_PasswordPrompt" });
        ui.set_visible(self.reentry_prompt, !locked);
        ui.set_visible(self.password2, !locked);
    }

    /// `Activate`.
    fn activate(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        FrameState::activate(ui, me);
        let (p1, p2) = (self.password, self.password2);
        with_te(ui, p1, |te, ui| te.set_text(ui, p1, "", s));
        ui.request_focus(p1);
        with_te(ui, p2, |te, ui| te.set_text(ui, p2, "", s));
        Self::set_label(ui, self.explain, "");
        self.update_status(ui);
        ui.set_app_modal(Some(me));
    }

    /// `OnClose`: hidden (not deleted), then `ContentControlClose`.
    fn on_close(&mut self, ui: &mut Ui, me: PanelId) {
        self.fs.close(ui, me);
        ui.post_action_signal(me, Msg::new("ContentControlClose"));
    }

    fn command(&mut self, ui: &mut Ui, me: PanelId, cmd: &str) {
        if cmd.eq_ignore_ascii_case("Ok") {
            let p1 = Self::entry_text(ui, self.password);
            let p2 = Self::entry_text(ui, self.password2);
            let mut host = self.host.borrow_mut();
            if !self.gore_pw.is_empty() {
                if p1.is_empty() {
                    drop(host);
                    Self::set_label(ui, self.explain, "#GameUI_MustEnterPassword");
                    return;
                }
                if !hash_password(&p1).eq_ignore_ascii_case(&self.gore_pw) {
                    drop(host);
                    Self::set_label(ui, self.explain, "#GameUI_IncorrectPassword");
                    return;
                }
                self.gore_pw.clear();
                for c in ["violence_hblood", "violence_hgibs", "violence_ablood", "violence_agibs"] {
                    host.cvar_set_value(c, 1.0);
                }
            } else {
                if p1 != p2 {
                    drop(host);
                    Self::set_label(ui, self.explain, "#GameUI_PasswordsDontMatch");
                    return;
                }
                if p1.is_empty() {
                    drop(host);
                    Self::set_label(ui, self.explain, "#GameUI_MustEnterPassword");
                    return;
                }
                for c in ["violence_hblood", "violence_hgibs", "violence_ablood", "violence_agibs"] {
                    host.cvar_set_value(c, 0.0);
                }
                self.gore_pw = hash_password(&p1);
            }
            drop(host);
            self.on_close(ui, me);
        } else if cmd.eq_ignore_ascii_case("Cancel") || cmd.eq_ignore_ascii_case("Close") {
            self.on_close(ui, me);
        } else {
            self.fs.on_command(ui, me, cmd);
        }
    }
}

impl Control for ContentControlDialog {
    fn class(&self) -> &'static str {
        "CContentControlDialog"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("Activate") {
            self.activate(ui, me, s);
            return true;
        }
        match self.fs.on_message(ui, me, msg, s) {
            FrameMsg::Handled => true,
            FrameMsg::Unhandled => false,
            FrameMsg::Command(c) => {
                self.command(ui, me, &c);
                true
            }
        }
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.fs.apply_scheme(ui, me, scheme, s);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        self.fs.perform_layout(ui, me);
    }
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.fs.paint_background(ui, me, s);
    }
    fn frame(&mut self) -> Option<&mut FrameState> {
        Some(&mut self.fs)
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// COptionsSubAdvanced

struct ContentPage {
    host: Shared,
    dlg: PanelId,
    lock: PanelId,
    original_models: PanelId,
    addons: PanelId,
    detail: PanelId,
}

pub fn create_content_page(ui: &mut Ui, dlg: PanelId, host: &Shared, fs: &Fs, s: &mut dyn Surface) -> PanelId {
    let page = PropertyPage::create(ui, Some(dlg), "OptionsSubAdvanced", true, Box::new(NoHooks));
    let lock = ToggleButton::create(ui, Some(page), "ContentlockButton", "#GameUI_ContentLock", ToggleKind::Check);
    with_toggle(ui, lock, |t, _| t.st.set_command("ContentControl"));
    let ccd = ContentControlDialog::create(ui, Some(dlg), host, fs, s);
    ui.add_action_signal_target(ccd, page);
    let (sw, sh) = ui.screen;
    let (w, h) = ui.size(ccd);
    ui.set_pos(ccd, (sw - w) / 2, (sh - h) / 2);
    let locked = with_ccd(ui, ccd, |d, _| !d.gore_pw.is_empty()).unwrap_or(false);
    with_toggle(ui, lock, |t, ui| t.set_selected(ui, lock, locked, s));
    let (hd, addons) = {
        let h = host.borrow();
        (h.hd_models, h.addons_folder)
    };
    let detail = cv::create_check(ui, Some(page), "DetailTextures", "#GameUI_DetailTextures", Some("r_detailtextures"), false, host.clone(), s);
    let original_models = ToggleButton::create(ui, Some(page), "OriginalModels", "#GameUI_OriginalModels", ToggleKind::Check);
    with_toggle(ui, original_models, |t, ui| t.set_selected(ui, original_models, !hd, s));
    let addons_cb = ToggleButton::create(ui, Some(page), "AddonsFolder", "#GameUI_AddonsFolder", ToggleKind::Check);
    with_toggle(ui, addons_cb, |t, ui| t.set_selected(ui, addons_cb, addons, s));
    load_res(ui, page, fs.as_ref(), "Resource\\OptionsSubAdvanced.res", s);
    let supported = host.borrow().cvar_float("r_detailtexturessupported") > 0.0;
    if !ModInfo::load(fs.as_ref()).has_detailed_textures() {
        ui.set_visible(detail, false);
    } else if !supported {
        ui.set_enabled(detail, false);
    }
    set_page_hooks(ui, page, Box::new(ContentPage { host: host.clone(), dlg: ccd, lock, original_models, addons: addons_cb, detail }));
    page
}

impl ContentPage {
    fn selected(ui: &mut Ui, id: PanelId) -> bool {
        with_toggle(ui, id, |t, _| t.is_selected()).unwrap_or(false)
    }
    fn gore_pw(&self, ui: &mut Ui) -> String {
        with_ccd(ui, self.dlg, |d, _| d.gore_pw.clone()).unwrap_or_default()
    }
}

impl PageHooks for ContentPage {
    /// `OnApplyChanges`.
    fn on_apply_changes(&mut self, ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        let pw = self.gore_pw(ui);
        self.host.borrow_mut().requests.push(crate::HostRequest::SetRegistryString { key: GORE_KEY.to_owned(), value: pw.clone() });
        self.host.borrow_mut().registry.insert(GORE_KEY.to_owned(), pw);
        with_ccd(ui, self.dlg, |d, ui| d.update_status(ui));
        let (hd, addons) = {
            let h = self.host.borrow();
            (h.hd_models, h.addons_folder)
        };
        let mut restart = false;
        let v = !Self::selected(ui, self.original_models);
        if hd != v {
            self.host.borrow_mut().client_cmd(&format!("_sethdmodels {}\n", i32::from(v)));
            restart = true;
        }
        let v = Self::selected(ui, self.addons);
        if addons != v {
            self.host.borrow_mut().client_cmd(&format!("_setaddons_folder {}\n", i32::from(v)));
            restart = true;
        }
        cv::apply_changes(ui, self.detail, false);
        if restart {
            self.host.borrow_mut().client_cmd("_restart\n");
        }
    }

    /// `OnResetData`.
    fn on_reset_data(&mut self, ui: &mut Ui, _me: PanelId, s: &mut dyn Surface) {
        let pw = registry_get(&self.host, GORE_KEY);
        with_ccd(ui, self.dlg, |d, _| d.gore_pw = pw.clone());
        let lock = self.lock;
        with_toggle(ui, lock, |t, ui| t.set_selected(ui, lock, !pw.is_empty(), s));
        let (hd, addons) = {
            let h = self.host.borrow();
            (h.hd_models, h.addons_folder)
        };
        let om = self.original_models;
        with_toggle(ui, om, |t, ui| t.set_selected(ui, om, !hd, s));
        let ad = self.addons;
        with_toggle(ui, ad, |t, ui| t.set_selected(ui, ad, addons, s));
        cv::reset(ui, self.detail, s);
    }

    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) -> bool {
        if msg.is("CheckButtonChecked") {
            let state = msg.get_int("state") != 0;
            let p = msg.get_panel("panel");
            let (hd, addons) = {
                let h = self.host.borrow();
                (h.hd_models, h.addons_folder)
            };
            let modified = (p == Some(self.original_models) && state == hd) || (p == Some(self.addons) && state != addons) || p == Some(self.detail);
            if modified {
                ui.post_action_signal(me, Msg::new("ApplyButtonEnable"));
            }
            return true;
        }
        if msg.is("ContentControlClose") {
            let enabled = !self.gore_pw(ui).is_empty();
            let lock = self.lock;
            with_toggle(ui, lock, |t, ui| t.set_selected(ui, lock, enabled, s));
            return true;
        }
        if msg.is("Command") && msg.get_str("command").eq_ignore_ascii_case("ContentControl") {
            let pw = self.gore_pw(ui);
            if !Self::selected(ui, self.lock) && pw.is_empty() {
                let reset = registry_get(&self.host, GORE_KEY);
                with_ccd(ui, self.dlg, |d, _| d.gore_pw = reset);
            } else {
                let d = self.dlg;
                with_ccd(ui, d, |dd, ui| dd.activate(ui, d, s));
            }
            return true;
        }
        false
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
