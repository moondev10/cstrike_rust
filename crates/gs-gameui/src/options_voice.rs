//! Voice page — `COptionsSubVoice` with `IVoiceTweak` (docs/re/gameui-dialogs.md §9,
//! docs/re/voice-engine.md §9).

use std::any::Any;

use gs_vgui::controls::button::{Button, with_button};
use gs_vgui::controls::label::Label;
use gs_vgui::controls::misc::ImagePanel;
use gs_vgui::controls::property::{NoHooks, PageHooks, PropertyPage};
use gs_vgui::controls::slider::Slider;
use gs_vgui::controls::toggle::{ToggleButton, ToggleKind, with_toggle};
use gs_vgui::{Msg, PanelId, Surface, Ui};

use crate::cvar_controls as cv;
use crate::options::{load_res, set_page_hooks};
use crate::{Fs, Shared};

/// The engine's `g_VoiceTweakAPI`: without mixer controls nothing starts and every control
/// reads 0.
struct VoiceTweak {
    host: Shared,
}

impl VoiceTweak {
    fn start(&self) -> i32 {
        let mut h = self.host.borrow_mut();
        if !h.voice_mixer {
            return 0;
        }
        h.requests.push(crate::HostRequest::VoiceTweak(true));
        1
    }
    fn end(&self) {
        self.host.borrow_mut().requests.push(crate::HostRequest::VoiceTweak(false));
    }
    /// Controls 0 (mic volume) and 2 (mic boost) are the Windows mixer's, which this client
    /// does not drive; 1 is `voice_scale`.
    fn set_control_float(&self, c: i32, v: f32) {
        let mut h = self.host.borrow_mut();
        if h.voice_mixer && c == 1 {
            h.requests.push(crate::HostRequest::VoiceScale(v));
        }
    }
    fn get_control_float(&self, c: i32) -> f32 {
        let h = self.host.borrow();
        match (h.voice_mixer, c) {
            (false, _) => 0.0,
            (true, 1) => h.cvar_float("voice_scale"),
            (true, _) => 1.0,
        }
    }
    fn speaking_volume(&self) -> i32 {
        self.host.borrow().voice_speaking_volume
    }
}

struct VoicePage {
    host: Shared,
    tweak: VoiceTweak,
    mic_boost: PanelId,
    mic_meter2: PanelId,
    test: PanelId,
    mic_label: PanelId,
    voice_volume: PanelId,
    receive_label: PanelId,
    voice_receive: PanelId,
    voice_enable: PanelId,
    mic_volume_value: i32,
    mic_boost_selected: bool,
    receive_volume: f32,
    receive_slider_value: i32,
    voice_on: bool,
}

fn slider_value(ui: &mut Ui, id: PanelId) -> i32 {
    ui.ctrl::<Slider>(id).map_or(0, |s| s.st.value())
}

fn set_slider(ui: &mut Ui, id: PanelId, v: i32) {
    ui.with_ctrl(id, |c, ui| {
        if let Some(sl) = c.as_any_mut().downcast_mut::<Slider>() {
            sl.st.set_value(ui, id, v, true);
        }
    });
}

pub fn create_voice_page(ui: &mut Ui, dlg: PanelId, host: &Shared, fs: &Fs, s: &mut dyn Surface) -> PanelId {
    let page = PropertyPage::create(ui, Some(dlg), "OptionsSubVoice", true, Box::new(NoHooks));
    let _mic_meter = ImagePanel::create(ui, Some(page), "MicMeter");
    let mic_meter2 = ImagePanel::create(ui, Some(page), "MicMeter2");
    let receive_label = Label::create(ui, Some(page), "ReceiveLabel", "#GameUI_VoiceReceiveVolume");
    let voice_receive = cv::create_slider(ui, Some(page), "VoiceReceive", 0.0, 1.0, "voice_scale", false, host.clone());
    let mic_label = Label::create(ui, Some(page), "MicrophoneLabel", "#GameUI_VoiceTransmitVolume");
    let voice_volume = Slider::create(ui, Some(page), "VoiceVolume");
    if let Some(sl) = ui.ctrl::<Slider>(voice_volume) {
        sl.st.set_range(0, 100);
    }
    ui.add_action_signal_target(voice_volume, page);
    let voice_enable = cv::create_check(ui, Some(page), "voice_modenable", "#GameUI_EnableVoice", Some("voice_modenable"), false, host.clone(), s);
    let mic_boost = ToggleButton::create(ui, Some(page), "MicBoost", "#GameUI_BoostMicrophone", ToggleKind::Check);
    ui.add_action_signal_target(mic_boost, page);
    let test = Button::create(ui, Some(page), "TestMicrophone", "#GameUI_TestMicrophone", None);
    load_res(ui, page, fs.as_ref(), "Resource\\OptionsSubVoice.res", s);
    if let Some(t) = s.texture("resource/mic_meter_live", false) {
        let (w, h) = (ui.psv(mic_meter2, t.width), ui.psv(mic_meter2, t.height));
        if let Some(ip) = ui.ctrl::<ImagePanel>(mic_meter2) {
            ip.image_size = Some((w, h));
        }
    }
    ui.set_visible(mic_meter2, false);
    let vp = VoicePage {
        host: host.clone(),
        tweak: VoiceTweak { host: host.clone() },
        mic_boost,
        mic_meter2,
        test,
        mic_label,
        voice_volume,
        receive_label,
        voice_receive,
        voice_enable,
        mic_volume_value: 0,
        mic_boost_selected: false,
        receive_volume: 0.0,
        receive_slider_value: 0,
        voice_on: false,
    };
    set_page_hooks(ui, page, Box::new(vp));
    page
}

impl VoicePage {
    /// `RestoreSettings`.
    fn restore_settings(&self, ui: &mut Ui, s: &mut dyn Surface) {
        self.tweak.set_control_float(0, self.mic_volume_value as f32 / 100.0);
        self.tweak.set_control_float(2, if self.mic_boost_selected { 1.0 } else { 0.0 });
        self.host.borrow_mut().cvar_set_value("voice_scale", self.receive_volume);
        cv::reset(ui, self.voice_receive, s);
        cv::set_slider_position(ui, self.voice_receive, self.receive_slider_value);
    }

    fn set_test_controls_enabled(&self, ui: &mut Ui, enabled: bool) {
        for p in [self.voice_receive, self.voice_volume, self.voice_enable, self.mic_boost, self.mic_label, self.receive_label] {
            ui.set_enabled(p, enabled);
        }
    }

    /// `EndTestMicrophone` (the "stop" branch).
    fn end_test(&mut self, ui: &mut Ui, s: &mut dyn Surface) {
        if !self.voice_on {
            return;
        }
        self.tweak.end();
        self.restore_settings(ui, s);
        let test = self.test;
        with_button(ui, test, |b, ui| b.label.set_text(ui, "#GameUI_TestMicrophone"));
        self.voice_on = false;
        self.set_test_controls_enabled(ui, true);
        ui.set_visible(self.mic_meter2, false);
    }
}

impl PageHooks for VoicePage {
    fn on_reset_data(&mut self, ui: &mut Ui, _me: PanelId, s: &mut dyn Surface) {
        set_slider(ui, self.voice_volume, (self.tweak.get_control_float(0) * 100.0) as i32);
        self.mic_volume_value = slider_value(ui, self.voice_volume);
        let boost = self.tweak.get_control_float(2) != 0.0;
        let mb = self.mic_boost;
        with_toggle(ui, mb, |t, ui| t.set_selected(ui, mb, boost, s));
        self.mic_boost_selected = with_toggle(ui, mb, |t, _| t.is_selected()).unwrap_or(false);
        cv::reset(ui, self.voice_receive, s);
        self.receive_volume = cv::slider_value(ui, self.voice_receive);
        cv::reset(ui, self.voice_enable, s);
    }
    fn on_apply_changes(&mut self, ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        self.mic_volume_value = slider_value(ui, self.voice_volume);
        self.tweak.set_control_float(0, self.mic_volume_value as f32 / 100.0);
        self.mic_boost_selected = with_toggle(ui, self.mic_boost, |t, _| t.is_selected()).unwrap_or(false);
        self.tweak.set_control_float(2, if self.mic_boost_selected { 1.0 } else { 0.0 });
        cv::apply_changes(ui, self.voice_receive, false);
        self.receive_volume = cv::slider_value(ui, self.voice_receive);
        cv::apply_changes(ui, self.voice_enable, false);
    }
    fn on_page_hide(&mut self, ui: &mut Ui, _me: PanelId, s: &mut dyn Surface) {
        self.end_test(ui, s);
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, s: &mut dyn Surface) -> bool {
        if msg.is("SliderMoved") {
            if slider_value(ui, self.voice_volume) != self.mic_volume_value {
                ui.post_action_signal(me, Msg::new("ApplyButtonEnable"));
            }
        } else if msg.is("CheckButtonChecked") {
            let sel = with_toggle(ui, self.mic_boost, |t, _| t.is_selected()).unwrap_or(false);
            if sel != self.mic_boost_selected {
                ui.post_action_signal(me, Msg::new("ApplyButtonEnable"));
            }
        } else if msg.is("ControlModified") {
            ui.post_action_signal(me, Msg::new("ApplyButtonEnable"));
        } else if msg.is("Command") && msg.get_str("command").eq_ignore_ascii_case("TestMicrophone") {
            if self.voice_on {
                self.end_test(ui, s);
                return true;
            }
            self.voice_on = true;
            self.tweak.set_control_float(0, slider_value(ui, self.voice_volume) as f32 / 100.0);
            let boost = with_toggle(ui, self.mic_boost, |t, _| t.is_selected()).unwrap_or(false);
            self.tweak.set_control_float(2, if boost { 1.0 } else { 0.0 });
            self.receive_slider_value = cv::slider_position(ui, self.voice_receive);
            cv::apply_changes(ui, self.voice_receive, false);
            if self.tweak.start() != 0 {
                let test = self.test;
                with_button(ui, test, |b, ui| b.label.set_text(ui, "#GameUI_StopTestMicrophone"));
                self.set_test_controls_enabled(ui, false);
                ui.set_visible(self.mic_meter2, true);
            } else {
                self.restore_settings(ui, s);
                self.voice_on = false;
            }
        } else {
            return false;
        }
        true
    }
    /// `OnThink`: the live meter width while testing.
    fn think(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        if !self.voice_on {
            return;
        }
        let v = self.tweak.speaking_volume();
        let w = ui.psv(me, v * 160 / 32768);
        let (bar, gap) = (ui.psv(me, 8), ui.psv(me, 1));
        let (_, h2) = ui.size(self.mic_meter2);
        let n = bar - gap + w;
        ui.set_size(self.mic_meter2, n - n % bar.max(1), h2);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
