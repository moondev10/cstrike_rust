//! `MessageBox` and `QueryBox` (docs/re/vgui-controls.md §10).

use std::any::Any;

use crate::controls::button::Button;
use crate::controls::frame::{FrameMsg, FrameState, with_frame};
use crate::controls::label::Label;
use crate::keys::KEY_ESCAPE;
use crate::msg::Msg;
use crate::scheme::Scheme;
use crate::surface::Surface;
use crate::ui::{Control, PanelId, Ui};

/// `vgui2::MessageBox`, or a `QueryBox` when `cancel` exists.
#[derive(Debug)]
pub struct MessageBox {
    pub fs: FrameState,
    label: Option<PanelId>,
    ok: Option<PanelId>,
    cancel: Option<PanelId>,
    frame_over: Option<PanelId>,
    /// MessageBox's `m_OkCommand` (posted on close).
    ok_close_command: Option<Msg>,
    /// QueryBox's `m_pOkCommand` / `m_pCancelCommand`.
    ok_command: Option<Msg>,
    cancel_command: Option<Msg>,
}

impl MessageBox {
    fn build(ui: &mut Ui, title: &str, text: &str, parent: Option<PanelId>, query: bool) -> PanelId {
        let me = ui.create(
            parent,
            "",
            Box::new(MessageBox { fs: FrameState::default(), label: None, ok: None, cancel: None, frame_over: None, ok_close_command: None, ok_command: None, cancel_command: None }),
        );
        FrameState::init(ui, me, false);
        with_frame(ui, me, |f, ui| {
            f.set_title(ui, me, title);
            f.set_close_button_visible(ui, false);
        });
        FrameState::set_sizeable(ui, me, false);
        let label = Label::create(ui, Some(me), "", text);
        let ok = Button::create(ui, Some(me), "OKButton", "#MessageBox_OK", Some(if query { "OK" } else { "Close" }));
        ui.add_action_signal_target(ok, me);
        let cancel = query.then(|| {
            ui.set_hd_proportional(me, true);
            ui.set_proportional(me, true);
            let c = Button::create(ui, Some(me), "CancelButton", "#QueryBox_Cancel", Some("Cancel"));
            ui.add_action_signal_target(c, me);
            if let Some(n) = ui.node_mut(ok) {
                n.tab_position = 1;
            }
            if let Some(n) = ui.node_mut(c) {
                n.tab_position = 2;
            }
            c
        });
        if let Some(m) = ui.ctrl::<MessageBox>(me) {
            m.label = Some(label);
            m.ok = Some(ok);
            m.cancel = cancel;
        }
        me
    }

    /// `new MessageBox(title, text, parent)`.
    pub fn create(ui: &mut Ui, title: &str, text: &str, parent: Option<PanelId>) -> PanelId {
        Self::build(ui, title, text, parent, false)
    }

    /// `new QueryBox(title, text, parent)`.
    pub fn create_query(ui: &mut Ui, title: &str, text: &str, parent: Option<PanelId>) -> PanelId {
        Self::build(ui, title, text, parent, true)
    }

    /// `SetOKButtonText`.
    pub fn set_ok_button_text(ui: &mut Ui, me: PanelId, text: &str) {
        let Some(ok) = ui.ctrl::<MessageBox>(me).and_then(|m| m.ok) else { return };
        ui.with_ctrl(ok, |c, ui| {
            if let Some(b) = c.as_any_mut().downcast_mut::<Button>() {
                b.st.label.set_text(ui, text);
            }
        });
        ui.invalidate_layout(me);
    }

    /// `QueryBox::SetOKCommand` / `SetCancelCommand`.
    pub fn set_commands(ui: &mut Ui, me: PanelId, ok: Option<Msg>, cancel: Option<Msg>) {
        if let Some(m) = ui.ctrl::<MessageBox>(me) {
            if ok.is_some() {
                m.ok_command = ok;
            }
            if cancel.is_some() {
                m.cancel_command = cancel;
            }
        }
    }

    /// `MessageBox::SetCommand(cmd)`: posted when the box closes.
    pub fn set_command(ui: &mut Ui, me: PanelId, cmd: &str) {
        if let Some(m) = ui.ctrl::<MessageBox>(me) {
            m.ok_close_command = Some(Msg::new("Command").str("command", cmd));
        }
    }

    /// `ShowWindow(frameOver)`.
    pub fn show_window(ui: &mut Ui, me: PanelId, frame_over: Option<PanelId>) {
        let ok = ui.ctrl::<MessageBox>(me).and_then(|m| {
            m.frame_over = frame_over;
            m.ok
        });
        ui.set_visible(me, true);
        ui.set_enabled(me, true);
        ui.move_to_front(me);
        match ok.filter(|&o| ui.is_visible(o)) {
            Some(o) => ui.request_focus(o),
            None => ui.request_focus(me),
        }
        ui.invalidate_layout(me);
    }

    /// `DoModal(frameOver)`.
    pub fn do_modal(ui: &mut Ui, me: PanelId, frame_over: Option<PanelId>) {
        Self::show_window(ui, me, frame_over);
        ui.set_app_modal(Some(me));
    }

    /// `MessageBox::OnClose`: Frame's, the close command, then delete.
    fn on_close(&mut self, ui: &mut Ui, me: PanelId) {
        self.fs.close(ui, me);
        if let Some(c) = self.ok_close_command.clone() {
            ui.post_action_signal(me, c);
        }
        ui.mark_for_deletion(me);
    }

    fn on_command(&mut self, ui: &mut Ui, me: PanelId, cmd: &str) {
        if self.cancel.is_some() && (cmd.eq_ignore_ascii_case("OK") || cmd.eq_ignore_ascii_case("Cancel")) {
            self.on_close(ui, me);
            let c = if cmd.eq_ignore_ascii_case("OK") { self.ok_command.clone() } else { self.cancel_command.clone() };
            if let Some(c) = c {
                ui.post_action_signal(me, c);
            }
            ui.mark_for_deletion(me);
            return;
        }
        if cmd.eq_ignore_ascii_case("Close") {
            self.on_close(ui, me);
        } else {
            self.fs.on_command(ui, me, cmd);
        }
    }

    fn content(ui: &mut Ui, p: PanelId, s: &mut dyn Surface) -> (i32, i32) {
        ui.with_ctrl(p, |c, ui| {
            if let Some(l) = c.as_any_mut().downcast_mut::<Label>() {
                l.st.content_size(ui, p, s)
            } else if let Some(b) = c.as_any_mut().downcast_mut::<Button>() {
                b.st.label.content_size(ui, p, s)
            } else {
                (0, 0)
            }
        })
        .unwrap_or((0, 0))
    }
}

impl Control for MessageBox {
    fn class(&self) -> &'static str {
        if self.cancel.is_some() { "QueryBox" } else { "MessageBox" }
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if self.cancel.is_some() && msg.is("KeyCodeTyped") && msg.get_int("code") == KEY_ESCAPE {
            self.on_command(ui, me, "Cancel");
            return true;
        }
        if msg.is("ShutdownRequest") {
            ui.post(me, Msg::new("Close"), None, 0.0);
            return true;
        }
        if msg.is("Close") {
            self.on_close(ui, me);
            return true;
        }
        match self.fs.on_message(ui, me, msg, s) {
            FrameMsg::Handled => true,
            FrameMsg::Unhandled => false,
            FrameMsg::Command(c) => {
                self.on_command(ui, me, &c);
                true
            }
        }
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.fs.apply_scheme(ui, me, scheme, s);
        let Some(label) = self.label else { return };
        let (lw, lh) = Self::content(ui, label, s);
        ui.set_size(label, lw, lh);
        let (w, h) = (lw + ui.psv(me, 100), lh + ui.psv(me, 100));
        ui.set_size(me, w, h);
        let (x, y) = match self.frame_over {
            Some(f) => {
                let (fx, fy) = ui.pos(f);
                let (fw, fh) = ui.size(f);
                (fx + (fw - w) / 2, fy + (fh - h) / 2)
            }
            None => {
                let (sw, sh) = ui.screen;
                ((sw - w) / 2, (sh - h) / 2)
            }
        };
        ui.set_pos(me, x, y);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let (Some(label), Some(ok)) = (self.label, self.ok) else { return };
        let (x, y, cw, ct) = self.fs.client_area(ui, me, s);
        let (cw, ct) = (cw + x, ct + y);
        let (bw, bh) = ui.size(me);
        let (ow, oh) = ui.size(ok);
        let (cbw, cbh) = Self::content(ui, ok, s);
        let btn_w = (cbw + ui.psv(me, 10)).max(ow);
        let btn_h = (cbh + ui.psv(me, 10)).max(oh);
        ui.set_size(ok, btn_w, btn_h);
        let box_w = (2 * btn_w + 100).max(bw).max(ui.size(label).0 + ui.psv(me, 100));
        ui.set_size(me, box_w, bh);
        let lw = ui.size(label).0;
        let ly = y + ui.psv(me, 15);
        ui.set_pos(label, x + cw / 2 - lw / 2, ly);
        let (okw, okh) = ui.size(ok);
        let oy = ct - okh - ui.psv(me, 15);
        ui.set_pos(ok, x + cw / 2 - okw / 2, oy);
        self.fs.perform_layout(ui, me);
        if let Some(cancel) = self.cancel {
            let (x, y, cw, ct) = self.fs.client_area(ui, me, s);
            let (cw, ct) = (cw + x, ct + y);
            let (cbw, cbh) = Self::content(ui, cancel, s);
            let (cw0, ch0) = ui.size(cancel);
            let (ccw, cch) = ((cbw + ui.psv(me, 10)).max(cw0), (cbh + ui.psv(me, 10)).max(ch0));
            ui.set_size(cancel, ccw, cch);
            let (okw, okh) = ui.size(ok);
            ui.set_pos(ok, x + cw / 2 - okw - ui.psv(me, 1), ct - okh - ui.psv(me, 15));
            ui.set_pos(cancel, x + cw / 2 + ui.psv(me, 16), ct - cch - ui.psv(me, 15));
        }
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
