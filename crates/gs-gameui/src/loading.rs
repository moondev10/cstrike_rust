//! `CLoadingDialog` (docs/re/gameui.md §6).

use std::any::Any;

use gs_vgui::controls::button::Button;
use gs_vgui::controls::frame::{FrameMsg, FrameState, with_frame};
use gs_vgui::controls::label::{Label, TextColorState};
use gs_vgui::controls::misc::ProgressBar;
use gs_vgui::keys::KEY_ESCAPE;
use gs_vgui::scheme::Scheme;
use gs_vgui::{Control, Msg, PanelId, Surface, Ui};

use crate::{HostRequest, Shared};

/// `CLoadingDialog`.
#[derive(Debug)]
pub struct LoadingDialog {
    pub fs: FrameState,
    host: Shared,
    pub progress: PanelId,
    pub progress2: PanelId,
    pub info: PanelId,
    pub time_remaining: PanelId,
    pub cancel: PanelId,
    range: (i32, i32),
    showing_secondary: bool,
    fraction: f32,
    secondary: f32,
    secondary_last_update: f32,
    secondary_start: f32,
    pub http_download: bool,
    pub showing_vac: bool,
    pub consume_next_error: bool,
}

/// Runs `f` on the loading dialog `id`.
pub fn with_loading<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut LoadingDialog, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<LoadingDialog>().map(|d| f(d, ui))).flatten()
}

fn set_label(ui: &mut Ui, id: PanelId, text: &str) {
    ui.with_ctrl(id, |c, ui| {
        if let Some(l) = c.as_any_mut().downcast_mut::<Label>() {
            l.st.set_text(ui, text);
        }
    });
    ui.invalidate_layout(id);
}

fn set_button(ui: &mut Ui, id: PanelId, text: &str, cmd: &str) {
    ui.with_ctrl(id, |c, ui| {
        if let Some(b) = c.as_any_mut().downcast_mut::<Button>() {
            b.st.label.set_text(ui, text);
            b.st.set_command(cmd);
        }
    });
    ui.invalidate_layout(id);
}

impl LoadingDialog {
    /// `new CLoadingDialog(staticPanel)`: `res` is `Resource/LoadingDialogNoBanner.res` (no
    /// `-steam`) or `Resource/LoadingDialog.res`.
    pub fn create(ui: &mut Ui, host: Shared, parent: PanelId, res: Option<&str>, s: &mut dyn Surface) -> PanelId {
        let placeholder = ui.embedded();
        let me = ui.create(
            Some(parent),
            "LoadingDialog",
            Box::new(LoadingDialog {
                fs: FrameState::default(),
                host,
                progress: placeholder,
                progress2: placeholder,
                info: placeholder,
                time_remaining: placeholder,
                cancel: placeholder,
                range: (0, 0),
                showing_secondary: false,
                fraction: 0.0,
                secondary: 0.0,
                secondary_last_update: 0.0,
                secondary_start: 0.0,
                http_download: false,
                showing_vac: false,
                consume_next_error: false,
            }),
        );
        FrameState::init(ui, me, true);
        ui.set_hd_proportional(me, true);
        ui.set_proportional(me, true);
        let (w, h) = (ui.psv(me, 416), ui.psv(me, 100));
        ui.set_size(me, w, h);
        with_frame(ui, me, |f, ui| f.set_title(ui, me, "#GameUI_Loading"));
        let progress = ProgressBar::create(ui, Some(me), "Progress");
        let progress2 = ProgressBar::create(ui, Some(me), "Progress2");
        let info = Label::create(ui, Some(me), "InfoLabel", "");
        let cancel = Button::create(ui, Some(me), "CancelButton", "#GameUI_Cancel", Some("Cancel"));
        let time_remaining = Label::create(ui, Some(me), "TimeRemainingLabel", "");
        let b = |ui: &mut Ui, p: PanelId, x: i32, y: i32, w: i32, h: i32| {
            let (x, y, w, h) = (ui.psv(me, x), ui.psv(me, y), ui.psv(me, w), ui.psv(me, h));
            ui.set_bounds(p, x, y, w, h);
        };
        b(ui, info, 20, 32, 392, 24);
        b(ui, progress, 20, 64, 300, 24);
        b(ui, cancel, 330, 64, 72, 24);
        if let Some(l) = ui.ctrl::<Label>(info) {
            l.st.color_state = TextColorState::Dull;
        }
        with_frame(ui, me, |f, ui| {
            f.set_close_button_visible(ui, false);
            f.set_moveable(false);
        });
        FrameState::set_sizeable(ui, me, false);
        ui.set_visible(progress2, false);
        with_loading(ui, me, |d, _| {
            d.progress = progress;
            d.progress2 = progress2;
            d.info = info;
            d.time_remaining = time_remaining;
            d.cancel = cancel;
        });
        if let Some(text) = res {
            ui.load_control_settings(me, text, &mut gs_vgui::editable::create_control_by_name, s);
        }
        me
    }

    /// `Open`: paint restricted to the dialog, shown, progress and status visible.
    pub fn open(&mut self, ui: &mut Ui, me: PanelId) {
        self.fs.set_title(ui, me, "#GameUI_Loading");
        ui.restrict_paint_to(Some(me));
        FrameState::activate(ui, me);
        ui.set_visible(self.progress, true);
        ui.set_visible(self.info, true);
        set_label(ui, self.info, "");
        set_button(ui, self.cancel, "#GameUI_Cancel", "Cancel");
    }

    pub fn set_progress_range(&mut self, min: i32, max: i32) {
        self.range = (min, max);
    }

    /// `SetProgressPoint`: `p / (max - min)` (min is not subtracted from `p`).
    pub fn set_progress_point(&mut self, ui: &mut Ui, p: i32) {
        self.fraction = p as f32 / (self.range.1 - self.range.0) as f32;
        ProgressBar::set_progress(ui, self.progress, self.fraction);
    }

    /// `SetSecondaryProgress`: the dual-progress layout on the first value ≤ 0.99.
    pub fn set_secondary_progress(&mut self, ui: &mut Ui, me: PanelId, f: f32, dual_res: Option<&str>, s: &mut dyn Surface) {
        let now = self.host.borrow().time as f32;
        if !self.showing_secondary {
            if f > 0.99 {
                return;
            }
            if let Some(text) = dual_res {
                ui.load_control_settings(me, text, &mut gs_vgui::editable::create_control_by_name, s);
            }
            self.showing_secondary = true;
            ProgressBar::set_progress(ui, self.progress, self.fraction);
            ui.set_visible(self.progress2, true);
            self.secondary_start = now;
        }
        if f > self.secondary {
            ProgressBar::set_progress(ui, self.progress2, f);
            self.secondary = f;
            self.secondary_last_update = now;
        } else if f < self.secondary {
            ProgressBar::set_progress(ui, self.progress2, f);
            self.secondary = f;
            self.secondary_last_update = now;
            self.secondary_start = now;
        }
    }

    /// `SetControlString("SecondaryProgressLabel", t)`.
    pub fn set_secondary_text(ui: &mut Ui, me: PanelId, text: &str) {
        let label = ui.children(me).into_iter().find(|&c| ui.node(c).is_some_and(|n| n.name.eq_ignore_ascii_case("SecondaryProgressLabel")));
        if let Some(l) = label {
            set_label(ui, l, text);
        }
    }

    pub fn set_status_text(&self, ui: &mut Ui, text: &str) {
        set_label(ui, self.info, text);
    }

    /// `SetupControlSettingsForErrorDisplay(res)`.
    fn setup_error(&mut self, ui: &mut Ui, me: PanelId, res: Option<&str>, s: &mut dyn Surface) {
        self.fs.set_title(ui, me, "#GameUI_Disconnected");
        if let Some(text) = res {
            ui.load_control_settings(me, text, &mut gs_vgui::editable::create_control_by_name, s);
        }
        ui.restrict_paint_to(Some(me));
        FrameState::activate(ui, me);
        ui.set_visible(self.progress, false);
        ui.set_visible(self.info, true);
        set_button(ui, self.cancel, "#GameUI_Close", "Close");
        set_label(ui, self.info, "");
        ui.request_focus(self.cancel);
    }

    /// `DisplayGenericError(reason, ext)`: skipped once after a specific error.
    pub fn display_generic_error(&mut self, ui: &mut Ui, me: PanelId, reason: &str, ext: &str, res: Option<&str>, s: &mut dyn Surface) {
        if self.consume_next_error {
            self.consume_next_error = false;
            return;
        }
        self.setup_error(ui, me, res, s);
        if ext.is_empty() {
            set_label(ui, self.info, reason);
        } else {
            let loc = |t: &str| t.strip_prefix('#').and_then(|k| ui.localize.find(k)).map_or_else(|| t.to_owned(), str::to_owned);
            let text = loc(reason).replace("%s1", &loc(ext));
            if let Some(l) = ui.ctrl::<Label>(self.info) {
                l.st.text.set_text_raw(&text);
            }
            ui.invalidate_layout(self.info);
        }
    }
}

impl Control for LoadingDialog {
    fn class(&self) -> &'static str {
        "CLoadingDialog"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("KeyCodePressed") && msg.get_int("code") == KEY_ESCAPE {
            self.command(ui, me, "Cancel");
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
    fn think(&mut self, ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        if !self.showing_secondary {
            return;
        }
        let now = self.host.borrow().time as f32;
        let text = if self.secondary >= 1.0 {
            "complete".to_owned()
        } else {
            ProgressBar::time_remaining(ui, self.secondary_start, now, self.secondary, self.secondary_last_update, true).unwrap_or_default()
        };
        if let Some(l) = ui.ctrl::<Label>(self.time_remaining) {
            l.st.text.set_text_raw(&text);
        }
        ui.invalidate_layout(self.time_remaining);
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.fs.apply_scheme(ui, me, scheme, s);
    }
    /// `PerformLayout`: always re-centred.
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        FrameState::move_to_center_of_screen(ui, me);
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

impl LoadingDialog {
    /// `OnCommand`: Cancel disconnects (or stops the HTTP download) and closes; the error
    /// layouts close through Frame's `Close`.
    fn command(&mut self, ui: &mut Ui, me: PanelId, cmd: &str) {
        if cmd.eq_ignore_ascii_case("Cancel") {
            let c = if self.http_download { "httpstop\n" } else { "disconnect\n" };
            self.host.borrow_mut().requests.push(HostRequest::ClientCmd(c.to_owned()));
            self.on_close(ui, me);
        } else if cmd.eq_ignore_ascii_case("Close") {
            self.on_close(ui, me);
        } else {
            self.fs.on_command(ui, me, cmd);
        }
    }

    /// `OnClose`: paint unrestricted, Frame's close, deleted.
    fn on_close(&mut self, ui: &mut Ui, me: PanelId) {
        ui.restrict_paint_to(None);
        self.fs.close(ui, me);
        ui.mark_for_deletion(me);
    }
}
