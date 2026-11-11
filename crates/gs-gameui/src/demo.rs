//! GameUI's demo dialogs (docs/re/gameui-demo.md): the `viewdemo` bar (`CDemoPlayerDialog`), the
//! demo chooser (`CDemoPlayerFileDialog`), the director-command list (`CDemoEventsDialog`) and the
//! command editor (`CDemoEditDialog`), all driving the engine's DemoPlayer module directly.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use gs_hltv::director::{self as dc, DirectorCmd};
use gs_hltv::player::DemoPlayer;
use gs_vgui::controls::button::{Button, with_button};
use gs_vgui::controls::frame::{FrameMsg, FrameState, with_frame};
use gs_vgui::controls::label::{Label, LabelImage, with_label};
use gs_vgui::controls::list_panel::{ListPanel, with_list};
use gs_vgui::controls::slider::Slider;
use gs_vgui::controls::text_entry::{TextEntry, with_te};
use gs_vgui::controls::toggle::{ToggleButton, ToggleKind};
use gs_vgui::scheme::Scheme;
use gs_vgui::{Color, Control, Msg, PanelId, Surface, Ui};

use crate::cvar_controls as cv;
use crate::options::load_res;
use crate::{Fs, HostRequest, Shared};

type Player = Rc<RefCell<DemoPlayer>>;

/// `"%03u:%02u:%02u"` / `"%02u:%02u:%02u"`: minutes, seconds, hundredths (`%u` of the ints).
fn clock(width: usize, secs: i32, hundredths: i32) -> String {
    format!("{:0width$}:{:02}:{:02}", (secs / 60) as u32, (secs % 60) as u32, hundredths as u32)
}

/// `ILocalize::ConstructString` with `%s1..%s3` (a missing format gives an empty string).
fn construct(ui: &Ui, token: &str, args: &[&str]) -> String {
    let mut out = ui.localize.find(token).unwrap_or("").to_owned();
    for (i, a) in args.iter().enumerate() {
        out = out.replacen(&format!("%s{}", i + 1), a, 1);
    }
    out
}

/// `GetFirstFrame()->time` / `GetLastFrame()->time`.
fn first_last(dp: &mut DemoPlayer) -> (Option<f32>, Option<f32>) {
    let first = dp.world.first_frame().and_then(|i| dp.world.frame(i)).map(|f| f.time);
    let last = dp.world.last_frame().and_then(|i| dp.world.frame(i)).map(|f| f.time);
    (first, last)
}

// ---------------------------------------------------------------------------------------------
// CDemoPlayerDialog

/// The ten icon buttons: name, `Resource\icon_*` image.
const ICON_BUTTONS: [(&str, &str); 10] = [
    ("LoadButton", "resource/icon_load"),
    ("PlayButton", "resource/icon_play"),
    ("PauseButton", "resource/icon_pause"),
    ("StepFButton", "resource/icon_stepf"),
    ("StepBButton", "resource/icon_stepb"),
    ("StartButton", "resource/icon_start"),
    ("EndButton", "resource/icon_end"),
    ("SlowerButton", "resource/icon_slower"),
    ("FasterButton", "resource/icon_faster"),
    ("StopButton", "resource/icon_stop"),
];

/// `CDemoPlayerDialog` (§3).
pub struct DemoPlayerDialog {
    pub fs: FrameState,
    host: Shared,
    gfs: Fs,
    /// `m_DemoPlayer` (`None` when LoadModules failed).
    dp: Option<Player>,
    time_label: PanelId,
    time_slider: PanelId,
    buttons: Vec<PanelId>,
    file_dialog: Option<PanelId>,
    events_dialog: Option<PanelId>,
    /// `m_LastSliderValue`.
    last_slider: i32,
}

/// `new CDemoPlayerDialog(basePanel)`.
pub fn create_demo_player_dialog(ui: &mut Ui, parent: PanelId, host: &Shared, gfs: &Fs, s: &mut dyn Surface) -> PanelId {
    let placeholder = ui.embedded();
    let me = ui.create(
        Some(parent),
        "DemoPlayerDialog",
        Box::new(DemoPlayerDialog {
            fs: FrameState::default(),
            host: host.clone(),
            gfs: gfs.clone(),
            dp: None,
            time_label: placeholder,
            time_slider: placeholder,
            buttons: Vec::new(),
            file_dialog: None,
            events_dialog: None,
            last_slider: -1,
        }),
    );
    FrameState::init(ui, me, false);
    ui.set_hd_proportional(me, true);
    with_frame(ui, me, |f, _| f.delete_self_on_close = true);
    // LoadModules: the system's demo player (the engine wrapper and world always exist here).
    let dp = host.borrow().demo_player.0.clone();
    let Some(dp) = dp else {
        host.borrow_mut().requests.push(HostRequest::ConPrintf("CDemoPlayerDialog::LoadModules: couldn't load demo player module.\n".to_owned()));
        ui.set_visible(me, false);
        return me;
    };
    let edit = dp.borrow().core.edit_mode;
    let (w, h) = (ui.psv(me, 464), ui.psv(me, if edit { 128 } else { 96 }));
    ui.set_pos(me, 0, 0);
    ui.set_size(me, w, h);
    FrameState::set_sizeable(ui, me, false);
    with_frame(ui, me, |f, _| f.set_moveable(true));
    ui.set_visible(me, true);
    with_frame(ui, me, |f, ui| f.set_title(ui, me, "#GameUI_DemoPlayer"));
    let time_label = Label::create(ui, Some(me), "TimeLabel", "00:00:00");
    let time_slider = Slider::create(ui, Some(me), "TimeSlider");
    if let Some(sl) = ui.ctrl::<Slider>(time_slider) {
        sl.st.set_range(0, 60);
    }
    ui.add_action_signal_target(time_slider, me);
    let order = ["LoadButton", "PlayButton", "PauseButton", "StepFButton", "StepBButton", "StartButton", "EndButton", "SlowerButton", "FasterButton", "StopButton"];
    let buttons: Vec<PanelId> = order.iter().map(|n| Button::create(ui, Some(me), n, "", None)).collect();
    let master = ToggleButton::create(ui, Some(me), "MasterButton", "Master", ToggleKind::Toggle);
    ui.add_action_signal_target(master, me);
    Button::create(ui, Some(me), "EventsButton", "#GameUI_Events", None);
    Button::create(ui, Some(me), "SaveButton", "#GameUI_Save", None);
    load_res(ui, me, gfs.as_ref(), "Resource/DemoPlayerDialog.res", s);
    let events = create_events_dialog(ui, me, host, gfs, &dp, s);
    ui.add_action_signal_target(events, me);
    if let Some(d) = ui.ctrl::<DemoPlayerDialog>(me) {
        d.dp = Some(dp);
        d.time_label = time_label;
        d.time_slider = time_slider;
        d.buttons = buttons;
        d.events_dialog = Some(events);
        d.file_dialog = None;
    }
    me
}

impl DemoPlayerDialog {
    /// `OnCommand` (§3.4); every branch ends in `Frame::OnCommand`.
    fn on_command(&mut self, ui: &mut Ui, me: PanelId, cmd: &str, s: &mut dyn Surface) {
        if let Some(p) = self.dp.clone() {
            match cmd {
                "pause" => self.pause(&p),
                "load" => self.open_file_dialog(ui, me, s),
                "play" => {
                    p.borrow_mut().set_paused(false);
                    self.host.borrow_mut().cvar_set("spec_autodirector", "1");
                }
                "stepf" => Self::step_frame(&p, 1),
                "stepb" => Self::step_frame(&p, -1),
                "start" => {
                    let mut dp = p.borrow_mut();
                    if let (Some(t), _) = first_last(&mut dp) {
                        dp.set_world_time(f64::from(t - 0.01), false);
                        dp.set_time_scale(1.0);
                        dp.set_paused(true);
                    }
                }
                "end" => {
                    let mut dp = p.borrow_mut();
                    if let (_, Some(t)) = first_last(&mut dp) {
                        dp.set_world_time(f64::from(t), false);
                        dp.set_paused(true);
                        self.host.borrow_mut().client_cmd("stopsound");
                    }
                }
                "slower" => {
                    let sc = p.borrow().time_scale();
                    if 0.25 < sc {
                        p.borrow_mut().set_time_scale(sc * 0.5);
                    }
                }
                "faster" => {
                    let sc = p.borrow().time_scale();
                    if sc < 4.0 {
                        p.borrow_mut().set_time_scale(sc * 2.0);
                    }
                }
                "stop" => {
                    p.borrow_mut().stop();
                    self.host.borrow_mut().client_cmd("stopdemo");
                    self.update_ui(ui, me);
                }
                "events" => {
                    let ev = match self.events_dialog.filter(|&d| ui.exists(d)) {
                        Some(d) => d,
                        None => {
                            let d = create_events_dialog(ui, me, &self.host, &self.gfs, &p, s);
                            ui.add_action_signal_target(d, me);
                            self.events_dialog = Some(d);
                            d
                        }
                    };
                    ui.send(ev, &Msg::new("Activate"), None, s);
                    ui.post(ev, Msg::new("UpdateCmdList"), Some(me), 0.0);
                }
                "save" => {
                    // SaveGame(NULL): over m_FileName; the result is ignored.
                    let _saved = p.borrow_mut().save_game(None);
                }
                _ => {}
            }
        }
        self.fs.on_command(ui, me, cmd);
    }

    /// `Pause()`.
    fn pause(&self, p: &Player) {
        p.borrow_mut().set_paused(true);
        self.host.borrow_mut().client_cmd("stopsound");
    }

    /// `StepFrame(dir)`: the frame after/before the one at the world time (the original
    /// dereferences a missing frame; here nothing happens).
    fn step_frame(p: &Player, dir: i32) {
        let mut dp = p.borrow_mut();
        let t = dp.world_time() as f32;
        let Some(seq) = dp.world.frame_by_time(f64::from(t)).and_then(|i| dp.world.frame(i)).map(|f| f.seqnr) else { return };
        let Some(time) = dp.world.frame_by_seq(seq.wrapping_add_signed(dir)).and_then(|i| dp.world.frame(i)).map(|f| f.time) else { return };
        dp.set_world_time(f64::from(time), false);
        dp.set_paused(true);
    }

    /// `OpenFileDialog`.
    fn open_file_dialog(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        let d = match self.file_dialog.filter(|&d| ui.exists(d)) {
            Some(d) => d,
            None => {
                let d = create_file_dialog(ui, me, &self.gfs, s);
                ui.add_action_signal_target(d, me);
                self.file_dialog = Some(d);
                d
            }
        };
        ui.send(d, &Msg::new("Activate"), None, s);
    }

    /// `OnDemoSelected(demoname)`: `viewdemo` the name up to the first `;`/`\n` (269 bytes).
    fn on_demo_selected(&mut self, name: &str) {
        let Some(p) = self.dp.clone() else { return };
        let mut buf: String = name.chars().take(269).collect();
        if let Some(i) = buf.find([';', '\n']) {
            buf.truncate(i);
        }
        let cmd: String = format!("viewdemo {buf}\n").chars().take(269).collect();
        p.borrow_mut().stop();
        self.host.borrow_mut().client_cmd(cmd.trim_end_matches('\n'));
    }

    /// `UpdateUI` (§3.5).
    fn update_ui(&mut self, ui: &mut Ui, me: PanelId) {
        let Some(p) = self.dp.clone() else { return };
        let mut dp = p.borrow_mut();
        let file: String = dp.core.file_name.chars().take(127).collect();
        let title = if !dp.is_active() {
            "#GameUI_DemoPlayer".to_owned()
        } else if dp.is_loading() {
            construct(ui, "GameUI_LoadingFilename", &[&file])
        } else {
            file
        };
        self.fs.set_title(ui, me, &title);
        let slider = self.time_slider;
        if let (Some(first), Some(last)) = first_last(&mut dp) {
            if let Some(sl) = ui.ctrl::<Slider>(slider) {
                sl.st.set_range(first as i32, last as i32);
            }
            ui.invalidate_layout(slider);
        }
        let t = dp.world_time();
        let sc = dp.time_scale();
        let v = ui.ctrl::<Slider>(slider).map_or(0, |sl| sl.st.value());
        if v != self.last_slider {
            dp.set_world_time(f64::from(v), false);
            if self.last_slider > 0 {
                dp.set_paused(true);
                self.host.borrow_mut().client_cmd("stopsound");
            }
            self.last_slider = v;
        } else {
            self.last_slider = t as i32;
            ui.with_ctrl(slider, |c, ui| {
                if let Some(sl) = c.as_any_mut().downcast_mut::<Slider>() {
                    sl.st.set_value(ui, slider, t as i32, true);
                }
            });
        }
        let status = if !dp.is_active() {
            "Valve_Stopped"
        } else if dp.is_paused() {
            "Valve_Paused"
        } else {
            "Valve_Playing"
        };
        drop(dp);
        let speed = match sc {
            0.25 => "Valve_x_1_4",
            0.5 => "Valve_x_1_2",
            1.0 => "Valve_x_1",
            2.0 => "Valve_x_2",
            4.0 => "Valve_x_4",
            _ => "Valve_x_1",
        };
        let time: String = clock(2, t as i32, (t * 100.0) as i32 % 100).chars().take(30).collect();
        let speed = ui.localize.find(speed).unwrap_or("").to_owned();
        let status = ui.localize.find(status).unwrap_or("").to_owned();
        let out: String = construct(ui, "Valve_ViewDemoStatus", &[&time, &speed, &status]).chars().take(127).collect();
        let label = self.time_label;
        with_label(ui, label, |l, ui| l.set_text(ui, &out));
        ui.invalidate_layout(label);
    }

    /// `ReceiveSignal` (§3.6): the event list follows the module in edit mode.
    fn receive_signal(&mut self, ui: &mut Ui, me: PanelId, signal: i32) {
        let Some(p) = self.dp.clone() else { return };
        let edit = p.borrow().core.edit_mode;
        let target = self.events_dialog.filter(|&d| ui.exists(d));
        match signal {
            1 | 2 => {
                if edit && let Some(ev) = target {
                    ui.post(ev, Msg::new(if signal == 1 { "UpdateCmdList" } else { "UpdateLastCmd" }), Some(me), 0.0);
                }
            }
            _ => self.host.borrow_mut().requests.push(HostRequest::ConPrintf(format!("CDemoPlayerDialog::ReceiveSignal: unknown signal {signal}.\n"))),
        }
    }
}

impl Control for DemoPlayerDialog {
    fn class(&self) -> &'static str {
        "CDemoPlayerDialog"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("Activate") {
            FrameState::activate(ui, me);
            return true;
        }
        if msg.is("DemoSelected") {
            self.on_demo_selected(&msg.get_str("demoname"));
            return true;
        }
        if msg.is("DemoPlayerSignal") {
            self.receive_signal(ui, me, msg.get_int("signal"));
            return true;
        }
        match self.fs.on_message(ui, me, msg, s) {
            FrameMsg::Handled => true,
            FrameMsg::Unhandled => false,
            FrameMsg::Command(c) => {
                self.on_command(ui, me, &c, s);
                true
            }
        }
    }
    /// `OnThink`: the bar follows a loaded demo.
    fn think(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        if self.dp.as_ref().is_some_and(|p| p.borrow().is_active()) {
            self.update_ui(ui, me);
        }
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.fs.apply_scheme(ui, me, scheme, s);
        for (&b, (_, image)) in self.buttons.iter().zip(ICON_BUTTONS) {
            let tex = s.texture(image, false);
            let size = tex.map_or((0, 0), |t| (t.width, t.height));
            with_button(ui, b, |st, _| st.label.set_image_at(0, LabelImage::Texture { name: image.to_owned(), tex, size, color: Color::WHITE }, 0));
        }
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
// CDemoPlayerFileDialog

/// `CDemoPlayerFileDialog` (§4).
pub struct DemoFileDialog {
    pub fs: FrameState,
    list: PanelId,
}

fn create_file_dialog(ui: &mut Ui, parent: PanelId, gfs: &Fs, s: &mut dyn Surface) -> PanelId {
    let placeholder = ui.embedded();
    let me = ui.create(Some(parent), "DemoPlayerFileDialog", Box::new(DemoFileDialog { fs: FrameState::default(), list: placeholder }));
    FrameState::init(ui, me, false);
    ui.set_hd_proportional(me, true);
    with_frame(ui, me, |f, _| f.delete_self_on_close = true);
    let (w, h) = (ui.psv(me, 278), ui.psv(me, 380));
    ui.set_pos(me, 0, 0);
    ui.set_size(me, w, h);
    FrameState::set_sizeable(ui, me, false);
    with_frame(ui, me, |f, ui| f.set_title(ui, me, "#GameUI_LoadDemo"));
    let list = ListPanel::create(ui, Some(me), "DemoList");
    Button::create(ui, Some(me), "LoadButton", "#GameUI_Load", None);
    Button::create(ui, Some(me), "CancelButton", "#GameUI_Cancel", None);
    load_res(ui, me, gfs.as_ref(), "Resource/DemoPlayerFileDialog.res", s);
    let wide = ui.size(list).0;
    with_list(ui, list, |l, ui| l.add_column_header(ui, list, 0, "demoname", "#GameUI_DemoFile", wide, 0));
    if let Some(d) = ui.ctrl::<DemoFileDialog>(me) {
        d.list = list;
    }
    scan_demo_files(ui, list, gfs);
    me
}

/// `ScanDemoFiles` (§4.2): `*.dem` over every search path, de-duplicated and sorted
/// case-insensitively; the first row selected.
fn scan_demo_files(ui: &mut Ui, list: PanelId, gfs: &Fs) {
    let mut names: Vec<String> = Vec::new();
    for (name, dir) in gfs.find("*.dem", None) {
        if !dir && !names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            names.push(name);
        }
    }
    names.sort_by_key(|n| n.to_ascii_lowercase());
    with_list(ui, list, |l, ui| {
        l.delete_all_items(ui, list);
        for n in &names {
            l.add_item(ui, list, &Msg::new("data").str("demoname", n), 0, false, false);
        }
        if l.item_count() > 0 {
            let id = l.item_id_from_row(0);
            l.set_single_selected_item(ui, list, id);
        }
    });
}

impl DemoFileDialog {
    /// `OnCommand` (§4.3).
    fn on_command(&mut self, ui: &mut Ui, me: PanelId, cmd: &str) {
        if cmd == "load" {
            let list = self.list;
            let name = with_list(ui, list, |l, _| {
                let id = l.selected_item(0);
                if l.is_valid_item_id(id) { l.item(id).map(|kv| kv.get_str("demoname")) } else { None }
            })
            .flatten();
            if let Some(name) = name {
                ui.post_action_signal(me, Msg::new("DemoSelected").str("demoname", &name));
            }
            self.fs.close(ui, me);
        }
        self.fs.on_command(ui, me, cmd);
    }
}

impl Control for DemoFileDialog {
    fn class(&self) -> &'static str {
        "CDemoPlayerFileDialog"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("Activate") {
            FrameState::activate(ui, me);
            return true;
        }
        if msg.is("ItemDoubleClicked") {
            self.on_command(ui, me, "load");
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
// CDemoEventsDialog

/// `CDemoEventsDialog` (§5).
pub struct DemoEventsDialog {
    pub fs: FrameState,
    host: Shared,
    dp: Player,
    list: PanelId,
    edit: PanelId,
}

fn create_events_dialog(ui: &mut Ui, parent: PanelId, host: &Shared, gfs: &Fs, dp: &Player, s: &mut dyn Surface) -> PanelId {
    let placeholder = ui.embedded();
    let me = ui.create(Some(parent), "DemoEventsDialog", Box::new(DemoEventsDialog { fs: FrameState::default(), host: host.clone(), dp: dp.clone(), list: placeholder, edit: placeholder }));
    FrameState::init(ui, me, false);
    ui.set_hd_proportional(me, true);
    let (w, h) = (ui.psv(me, 464), ui.psv(me, 380));
    ui.set_pos(me, 0, 0);
    ui.set_size(me, w, h);
    FrameState::set_sizeable(ui, me, false);
    with_frame(ui, me, |f, ui| f.set_title(ui, me, "#GameUI_EventList"));
    let list = ListPanel::create(ui, Some(me), "EventList");
    Button::create(ui, Some(me), "AddButton", "#GameUI_Add", None);
    Button::create(ui, Some(me), "RemoveButton", "#GameUI_Remove", None);
    Button::create(ui, Some(me), "ModifyButton", "#GameUI_Modify", None);
    Button::create(ui, Some(me), "GotoButton", "#GameUI_Goto", None);
    load_res(ui, me, gfs.as_ref(), "Resource/DemoEventsDialog.res", s);
    let (c32, c128) = (ui.psv(me, 32), ui.psv(me, 128));
    with_list(ui, list, |l, ui| {
        l.add_column_header(ui, list, 0, "time", "#GameUI_Time", c32, 0);
        l.add_column_header(ui, list, 1, "type", "#GameUI_Type", c128, 0);
    });
    let edit = create_edit_dialog(ui, me, host, gfs, dp, s);
    ui.send(edit, &Msg::new("Activate"), None, s);
    if let Some(d) = ui.ctrl::<DemoEventsDialog>(me) {
        d.list = list;
        d.edit = edit;
    }
    me
}

impl DemoEventsDialog {
    /// `OnUpdateCmdList` (§5.2).
    fn update_cmd_list(&mut self, ui: &mut Ui) {
        let (cmds, last) = {
            let dp = self.dp.borrow();
            (dp.commands(), dp.last_command_index())
        };
        let list = self.list;
        with_list(ui, list, |l, ui| {
            l.delete_all_items(ui, list);
            for c in &cmds {
                let t = c.time;
                let time = clock(3, t as i32, (t * 100.0) as i32 % 100);
                let kv = Msg::new(&c.index.to_string()).str("time", &time).str("type", &c.to_text().unwrap_or_default());
                l.add_item(ui, list, &kv, 0, false, false);
            }
            let row = if last != 0 { last - 1 } else { 0 };
            let id = l.item_id_from_row(row);
            l.set_single_selected_item(ui, list, id);
        });
    }

    /// `OnCommand` (§5.3): the selected row number is taken as the command index.
    fn on_command(&mut self, ui: &mut Ui, me: PanelId, cmd: &str, s: &mut dyn Surface) {
        let list = self.list;
        let id = with_list(ui, list, |l, _| {
            let id = l.selected_item(0);
            (id >= 0).then(|| l.item_current_row(id) + 1)
        })
        .flatten();
        let selected = id.and_then(|index| self.dp.borrow().commands().into_iter().find(|c| c.index == index));
        match cmd {
            "goto" => {
                if let Some(c) = &selected {
                    let mut dp = self.dp.borrow_mut();
                    dp.set_world_time(f64::from(c.time), false);
                    dp.execute_director_cmd(c);
                    dp.set_paused(true);
                }
            }
            "remove" => {
                if let Some(c) = &selected {
                    self.dp.borrow_mut().remove_command(c.index);
                }
            }
            "modify" => {
                if let Some(c) = selected {
                    self.open_editor(ui, c, s);
                }
            }
            "add" => {
                let time = {
                    let mut dp = self.dp.borrow_mut();
                    dp.set_paused(true);
                    dp.world_time() as f32
                };
                let (pos, ang) = {
                    let h = self.host.borrow();
                    (h.view_origin, h.view_angles)
                };
                let mut c = DirectorCmd { time, ..DirectorCmd::default() };
                c.set_camera_data(pos, ang, 90.0, 0);
                c.index = -1;
                self.open_editor(ui, c, s);
            }
            _ => {}
        }
        self.fs.on_command(ui, me, cmd);
    }

    /// `m_EditDialog->SetCommand(cmd); Activate()`.
    fn open_editor(&mut self, ui: &mut Ui, cmd: DirectorCmd, s: &mut dyn Surface) {
        let edit = self.edit;
        if let Some(d) = ui.ctrl::<DemoEditDialog>(edit) {
            d.cmd = cmd;
        }
        ui.with_ctrl(edit, |c, ui| {
            if let Some(d) = c.as_any_mut().downcast_mut::<DemoEditDialog>() {
                d.update_ui(ui, s);
            }
        });
        ui.send(edit, &Msg::new("Activate"), None, s);
    }
}

impl Control for DemoEventsDialog {
    fn class(&self) -> &'static str {
        "CDemoEventsDialog"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("Activate") {
            FrameState::activate(ui, me);
            return true;
        }
        if msg.is("UpdateCmdList") || msg.is("UpdateLastCmd") {
            self.update_cmd_list(ui);
            return true;
        }
        match self.fs.on_message(ui, me, msg, s) {
            FrameMsg::Handled => true,
            FrameMsg::Unhandled => false,
            FrameMsg::Command(c) => {
                self.on_command(ui, me, &c, s);
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
// CDemoEditDialog

/// `CDemoEditDialog` (§6).
pub struct DemoEditDialog {
    pub fs: FrameState,
    host: Shared,
    dp: Player,
    /// `m_Cmd`.
    cmd: DirectorCmd,
    /// `m_Updating`.
    updating: bool,
    type_box: PanelId,
    time_entry: PanelId,
    get_view: PanelId,
    labels: Vec<PanelId>,
    entries: Vec<PanelId>,
}

fn create_edit_dialog(ui: &mut Ui, parent: PanelId, host: &Shared, gfs: &Fs, dp: &Player, s: &mut dyn Surface) -> PanelId {
    let placeholder = ui.embedded();
    let me = ui.create(
        Some(parent),
        "DemoEditDialog",
        Box::new(DemoEditDialog {
            fs: FrameState::default(),
            host: host.clone(),
            dp: dp.clone(),
            cmd: DirectorCmd::default(),
            updating: false,
            type_box: placeholder,
            time_entry: placeholder,
            get_view: placeholder,
            labels: Vec::new(),
            entries: Vec::new(),
        }),
    );
    FrameState::init(ui, me, false);
    ui.set_hd_proportional(me, true);
    let (w, h) = (ui.psv(me, 336), ui.psv(me, 380));
    ui.set_pos(me, 0, 0);
    ui.set_size(me, w, h);
    FrameState::set_sizeable(ui, me, false);
    with_frame(ui, me, |f, ui| f.set_title(ui, me, "#GameUI_EditDirectorCommand"));
    let type_box = cv::create_combo(ui, Some(me), "TypeBox", host.clone());
    for name in &dc::NAMES[dc::CAMERA as usize..=dc::CAMPATH as usize] {
        cv::combo_add_item(ui, type_box, name, "");
    }
    cv::combo_set_initial_item(ui, type_box, 0, s);
    let time_entry = TextEntry::create(ui, Some(me), "TimeEntry");
    Button::create(ui, Some(me), "OkButton", "#GameUI_OK", None);
    Button::create(ui, Some(me), "CancelButton", "#GameUI_Cancel", None);
    Button::create(ui, Some(me), "GetTimeButton", "#GameUI_GetTime", None);
    let get_view = Button::create(ui, Some(me), "GetViewButton", "#GameUI_GetView", None);
    let mut y = ui.psv(me, 96);
    let (mut labels, mut entries) = (Vec::new(), Vec::new());
    for i in 0..8 {
        let l = Label::create(ui, Some(me), &format!("DataLabel{i}"), "");
        let (x, lw, rh) = (ui.psv(me, 10), ui.psv(me, 96), ui.psv(me, 24));
        ui.set_pos(l, x, y);
        ui.set_size(l, lw, rh);
        let e = TextEntry::create(ui, Some(me), &format!("DataEntry{i}"));
        let (x, ew) = (ui.psv(me, 104), ui.psv(me, 224));
        ui.set_pos(e, x, y);
        ui.set_size(e, ew, rh);
        y += ui.psv(me, 30);
        labels.push(l);
        entries.push(e);
    }
    load_res(ui, me, gfs.as_ref(), "Resource/DemoEditDialog.res", s);
    ui.with_ctrl(me, |c, ui| {
        if let Some(d) = c.as_any_mut().downcast_mut::<DemoEditDialog>() {
            d.type_box = type_box;
            d.time_entry = time_entry;
            d.get_view = get_view;
            d.labels = labels;
            d.entries = entries;
            d.cmd.kind = 0;
            d.cmd.time = 0.0;
            d.cmd.data.clear();
            d.update_ui(ui, s);
        }
    });
    me
}

/// One `sscanf` conversion over `s` from `*pos` (leading whitespace skipped; `None` on failure).
fn scan_num(s: &[u8], pos: &mut usize, float: bool, hex: bool) -> Option<f64> {
    let mut i = *pos;
    while s.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    let start = i;
    if matches!(s.get(i), Some(b'+' | b'-')) {
        i += 1;
    }
    if hex && s.get(i) == Some(&b'0') && matches!(s.get(i + 1), Some(b'x' | b'X')) && s.get(i + 2).is_some_and(u8::is_ascii_hexdigit) {
        i += 2;
    }
    let digits_start = i;
    let digit = |c: &u8| if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() };
    while s.get(i).is_some_and(digit) {
        i += 1;
    }
    let mut any = i > digits_start;
    if float {
        if s.get(i) == Some(&b'.') {
            i += 1;
            while s.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
                any = true;
            }
        }
        if any && matches!(s.get(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(s.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if s.get(j).is_some_and(u8::is_ascii_digit) {
                while s.get(j).is_some_and(u8::is_ascii_digit) {
                    j += 1;
                }
                i = j;
            }
        }
    }
    if !any {
        return None;
    }
    *pos = i;
    let text = std::str::from_utf8(&s[start..i]).ok()?;
    if hex {
        let neg = text.starts_with('-');
        let body = text.trim_start_matches(['+', '-']).trim_start_matches("0x").trim_start_matches("0X");
        let v = u64::from_str_radix(body, 16).ok()? as u32;
        Some(f64::from(if neg { v.wrapping_neg() } else { v }))
    } else if float {
        text.parse::<f64>().ok()
    } else {
        text.parse::<i64>().ok().map(|v| f64::from(v as i32))
    }
}

/// `sscanf(text, "%d %d …")` / `"%f %f …"`: `n` conversions, stopping at the first failure (the
/// unconverted ones keep the original's uninitialised stack values; 0 here).
fn scan(text: &str, n: usize, float: bool) -> Vec<f64> {
    let b = text.as_bytes();
    let mut pos = 0;
    let mut out = vec![0.0; n];
    for v in &mut out {
        match scan_num(b, &mut pos, float, false) {
            Some(x) => *v = x,
            None => break,
        }
    }
    out
}

impl DemoEditDialog {
    fn row_text(&self, ui: &mut Ui, i: usize) -> String {
        let e = self.entries[i];
        gs_common::bytestr::ansi_cap(&with_te(ui, e, |te, _| te.text()).unwrap_or_default(), 1024)
    }
    fn row_int(&self, ui: &mut Ui, i: usize) -> i32 {
        scan(&self.row_text(ui, i), 1, false)[0] as i32
    }
    fn row_float(&self, ui: &mut Ui, i: usize) -> f32 {
        scan(&self.row_text(ui, i), 1, true)[0] as f32
    }
    fn row_vec(&self, ui: &mut Ui, i: usize) -> [f32; 3] {
        let v = scan(&self.row_text(ui, i), 3, true);
        [v[0] as f32, v[1] as f32, v[2] as f32]
    }

    /// One row: label text and entry text, both shown.
    fn row(&self, ui: &mut Ui, i: usize, label: &str, text: &str, s: &mut dyn Surface) {
        let (l, e) = (self.labels[i], self.entries[i]);
        with_label(ui, l, |st, ui| st.set_text(ui, label));
        ui.set_visible(l, true);
        with_te(ui, e, |te, ui| te.set_text(ui, e, text, s));
        ui.set_visible(e, true);
    }

    /// `UpdateUI` (§6.3).
    fn update_ui(&mut self, ui: &mut Ui, s: &mut dyn Surface) {
        self.updating = true;
        ui.set_visible(self.get_view, false);
        for i in 0..8 {
            let (l, e) = (self.labels[i], self.entries[i]);
            with_label(ui, l, |st, ui| st.set_text(ui, ""));
            ui.set_visible(l, false);
            with_te(ui, e, |te, ui| te.set_text(ui, e, "", s));
            ui.set_visible(e, false);
        }
        let c = self.cmd.clone();
        if (1..=15).contains(&c.kind) {
            cv::combo_set_initial_item(ui, self.type_box, c.kind - dc::CAMERA, s);
            let te = self.time_entry;
            let t = c.time;
            let text = clock(3, t as i32, (t * 100.0) as i32 % 100);
            with_te(ui, te, |entry, ui| entry.set_text(ui, te, &text, s));
            let v3 = |v: [f32; 3]| format!("{:.1} {:.1} {:.1}", v[0], v[1], v[2]);
            match c.kind {
                dc::EVENT => {
                    let (e1, e2, flags) = c.event_data();
                    self.row(ui, 0, "Target 1", &e1.to_string(), s);
                    self.row(ui, 1, "Target 2", &e2.to_string(), s);
                    self.row(ui, 2, "Flags", &flags.to_string(), s);
                }
                dc::MODE => self.row(ui, 0, "View Mode", &c.mode_data().to_string(), s),
                dc::CAMERA | dc::CAMPATH => {
                    let (pos, ang, fov, last) = c.camera_data();
                    self.row(ui, 0, "Position", &v3(pos), s);
                    self.row(ui, 1, "View Angle", &v3(ang), s);
                    self.row(ui, 2, "FOV", &format!("{fov:.1}"), s);
                    self.row(ui, 3, if c.kind == dc::CAMERA { "Target" } else { "Flags" }, &last.to_string(), s);
                    ui.set_visible(self.get_view, true);
                }
                dc::TIMESCALE => self.row(ui, 0, "Time Scale", &format!("{:.2}", c.time_scale_data()), s),
                dc::MESSAGE => {
                    let (effect, color, f, text) = c.message_data();
                    self.row(ui, 0, "Effect", &effect.to_string(), s);
                    self.row(ui, 1, "Color", &format!("{color:x}"), s);
                    self.row(ui, 2, "Position", &format!("{:.2} {:.2}", f[0], f[1]), s);
                    self.row(ui, 3, "Fade In", &format!("{:.1}", f[2]), s);
                    self.row(ui, 4, "Fade Out", &format!("{:.1}", f[3]), s);
                    self.row(ui, 5, "Hold Time", &format!("{:.1}", f[4]), s);
                    self.row(ui, 6, "FX Time", &format!("{:.1}", f[5]), s);
                    self.row(ui, 7, "Text", &text, s);
                }
                dc::SOUND => {
                    let (name, vol) = c.sound_data();
                    self.row(ui, 0, "Name", &name, s);
                    self.row(ui, 1, "Volume", &format!("{vol:.2}"), s);
                }
                dc::BANNER => self.row(ui, 0, "File", &c.string_data(), s),
                dc::STUFFTEXT => self.row(ui, 0, "Command", &c.string_data(), s),
                dc::CHASE => {
                    let (e1, e2, dist, flags) = c.chase_data();
                    self.row(ui, 0, "Target 1", &e1.to_string(), s);
                    self.row(ui, 1, "Target 2", &e2.to_string(), s);
                    self.row(ui, 2, "Distance", &format!("{dist:.1}"), s);
                    self.row(ui, 3, "Flags", &flags.to_string(), s);
                }
                dc::INEYE => self.row(ui, 0, "Player", &c.in_eye_data().to_string(), s),
                dc::MAP => {
                    let (e, angle, dist) = c.map_data();
                    self.row(ui, 0, "Player", &e.to_string(), s);
                    self.row(ui, 1, "Angle", &format!("{angle:.1}"), s);
                    self.row(ui, 2, "Distance", &format!("{dist:.1}"), s);
                }
                // STATUS reads its data and shows nothing; START / WAYPOINTS have no rows.
                _ => {}
            }
        }
        self.updating = false;
    }

    /// `ReadData` (§6.4).
    fn read_data(&mut self, ui: &mut Ui) {
        let te = self.time_entry;
        let text = with_te(ui, te, |e, _| e.text()).unwrap_or_default().replace(':', " ");
        let t = scan(&text, 3, false);
        let (m, sec, h) = (t[0] as i32, t[1] as i32, t[2] as i32);
        self.cmd.time = m.wrapping_mul(60).wrapping_add(sec) as f32 + h as f32 / 100.0;
        match self.cmd.kind {
            dc::EVENT => {
                let v = (self.row_int(ui, 0), self.row_int(ui, 1), self.row_int(ui, 2));
                self.cmd.set_event_data(v.0, v.1, v.2);
            }
            dc::MODE => {
                let m = self.row_int(ui, 0);
                self.cmd.set_mode_data(m);
            }
            dc::CAMERA | dc::CAMPATH => {
                let (pos, ang, fov, last) = (self.row_vec(ui, 0), self.row_vec(ui, 1), self.row_float(ui, 2), self.row_int(ui, 3));
                if self.cmd.kind == dc::CAMERA {
                    self.cmd.set_camera_data(pos, ang, fov, last);
                } else {
                    self.cmd.set_way_point_data(pos, ang, fov, last);
                }
            }
            dc::TIMESCALE => {
                let v = self.row_float(ui, 0);
                self.cmd.set_time_scale_data(v);
            }
            dc::MESSAGE => {
                let effect = self.row_int(ui, 0);
                let color_text = self.row_text(ui, 1);
                let color = scan_num(color_text.as_bytes(), &mut 0, false, true).unwrap_or(0.0) as u32;
                let p = scan(&self.row_text(ui, 2), 2, true);
                let f = [self.row_float(ui, 3), self.row_float(ui, 4), self.row_float(ui, 5), self.row_float(ui, 6)];
                let text = self.row_text(ui, 7);
                self.cmd.set_message_data(effect, color, [p[0] as f32, p[1] as f32], f[0], f[1], f[2], f[3], &text);
            }
            dc::SOUND => {
                let (name, vol) = (self.row_text(ui, 0), self.row_float(ui, 1));
                self.cmd.set_sound_data(&name, vol);
            }
            dc::STATUS => self.cmd.set_status_data(0, 0, 0),
            dc::BANNER => {
                let f = self.row_text(ui, 0);
                self.cmd.set_banner_data(&f);
            }
            dc::STUFFTEXT => {
                let t = self.row_text(ui, 0);
                self.cmd.set_stuff_text_data(&t);
            }
            dc::CHASE => {
                let v = (self.row_int(ui, 0), self.row_int(ui, 1), self.row_float(ui, 2), self.row_int(ui, 3));
                self.cmd.set_chase_data(v.0, v.1, v.2, v.3);
            }
            dc::INEYE => {
                let e = self.row_int(ui, 0);
                self.cmd.set_in_eye_data(e);
            }
            dc::MAP => {
                let v = (self.row_int(ui, 0), self.row_float(ui, 1), self.row_float(ui, 2));
                self.cmd.set_map_data(v.0, v.1, v.2);
            }
            _ => {}
        }
    }

    /// `OnControlModified` (§6.5): picking a type replaces the payload with its defaults.
    fn on_control_modified(&mut self, ui: &mut Ui, s: &mut dyn Surface) {
        if self.updating {
            return;
        }
        let (time, index) = (self.cmd.time, self.cmd.index);
        self.cmd = DirectorCmd { time, index, ..DirectorCmd::default() };
        let name = gs_common::bytestr::ansi_cap(&cv::combo_text(ui, self.type_box), 256);
        let found = dc::NAMES[..15].iter().position(|n| *n == name).map(|i| i as i32);
        match found {
            Some(i) if i == self.cmd.kind => return,
            Some(i) => self.cmd.kind = i,
            None => {}
        }
        let c = &mut self.cmd;
        match c.kind {
            dc::EVENT => c.set_event_data(1, 0, 0),
            dc::MODE => c.set_mode_data(4),
            dc::CAMERA => c.set_camera_data([0.0; 3], [0.0; 3], 90.0, 0),
            dc::TIMESCALE => c.set_time_scale_data(1.0),
            dc::MESSAGE => c.set_message_data(0, 0xFF, [0.5, 0.9], 1.0, 1.0, 5.0, 0.0, "Add your message here"),
            dc::SOUND => c.set_sound_data("barney/heybuddy.wav", 1.0),
            dc::STATUS => c.set_status_data(0, 0, 0),
            dc::BANNER => c.set_banner_data("gfx/temp/file.tga"),
            dc::STUFFTEXT => c.set_stuff_text_data("echo Enter command here"),
            dc::CHASE => c.set_chase_data(1, 0, 96.0, 0),
            dc::INEYE => c.set_in_eye_data(1),
            dc::MAP => c.set_map_data(1, 90.0, 128.0),
            dc::CAMPATH => c.set_way_point_data([0.0; 3], [0.0; 3], 90.0, 0),
            _ => {}
        }
        self.update_ui(ui, s);
    }

    /// `OnCommand` (§6.6).
    fn on_command(&mut self, ui: &mut Ui, me: PanelId, cmd: &str, s: &mut dyn Surface) {
        match cmd {
            "ok" => {
                self.read_data(ui);
                {
                    let mut dp = self.dp.borrow_mut();
                    if self.cmd.index > 0 {
                        dp.remove_command(self.cmd.index);
                    }
                    dp.add_command(&self.cmd);
                }
                self.fs.on_command(ui, me, "Close");
            }
            "getview" => {
                if matches!(self.cmd.kind, dc::CAMERA | dc::CAMPATH) {
                    let (_, _, fov, last) = self.cmd.camera_data();
                    let (pos, ang) = {
                        let h = self.host.borrow();
                        (h.view_origin, h.view_angles)
                    };
                    if self.cmd.kind == dc::CAMERA {
                        self.cmd.set_camera_data(pos, ang, fov, last);
                    } else {
                        self.cmd.set_way_point_data(pos, ang, fov, last);
                    }
                    self.update_ui(ui, s);
                }
            }
            "gettime" => {
                self.cmd.time = self.dp.borrow().world_time() as f32;
                self.update_ui(ui, s);
            }
            _ => {}
        }
        self.fs.on_command(ui, me, cmd);
    }
}

impl Control for DemoEditDialog {
    fn class(&self) -> &'static str {
        "CDemoEditDialog"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("Activate") {
            FrameState::activate(ui, me);
            return true;
        }
        if msg.is("ControlModified") {
            self.on_control_modified(ui, s);
            return true;
        }
        match self.fs.on_message(ui, me, msg, s) {
            FrameMsg::Handled => true,
            FrameMsg::Unhandled => false,
            FrameMsg::Command(c) => {
                self.on_command(ui, me, &c, s);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sscanf_conversions() {
        assert_eq!(scan("12 -3 x", 3, false), vec![12.0, -3.0, 0.0]);
        assert_eq!(scan(" 1.5 2e1 .5", 3, true), vec![1.5, 20.0, 0.5]);
        assert_eq!(scan_num(b"0xff", &mut 0, false, true), Some(255.0));
        assert_eq!(scan_num(b"zz", &mut 0, false, true), None);
    }

    #[test]
    fn clock_format() {
        assert_eq!(clock(2, 187, 25), "03:07:25");
        assert_eq!(clock(3, 61, 5), "001:01:05");
        assert_eq!(clock(2, 0, -1), "00:00:4294967295");
    }
}
