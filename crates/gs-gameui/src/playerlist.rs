//! `CPlayerListDialog` (docs/re/gameui-dialogs.md §14): the players of the current game with
//! voice mute.

use std::any::Any;

use gs_vgui::controls::button::{Button, with_button};
use gs_vgui::controls::frame::{FrameMsg, FrameState};
use gs_vgui::controls::list_panel::{ListPanel, with_list};
use gs_vgui::scheme::Scheme;
use gs_vgui::{Control, Msg, PanelId, Surface, Ui};

use crate::options::load_res;
use crate::{Fs, Shared};

pub struct PlayerListDialog {
    pub fs: FrameState,
    host: Shared,
    list: PanelId,
    mute: PanelId,
}

pub fn create_player_list_dialog(ui: &mut Ui, parent: PanelId, host: &Shared, fs: &Fs, s: &mut dyn Surface) -> PanelId {
    let placeholder = ui.embedded();
    let me = ui.create(Some(parent), "PlayerListDialog", Box::new(PlayerListDialog { fs: FrameState::default(), host: host.clone(), list: placeholder, mute: placeholder }));
    FrameState::init(ui, me, false);
    let (mw, mh) = (ui.psv(me, 300), ui.psv(me, 240));
    ui.set_min_size(me, mw, mh);
    let (w, h) = (ui.psv(me, 320), ui.psv(me, 240));
    ui.set_size(me, w, h);
    // GameClientExports exists in cstrike: "#GameUI_PlayerListDialogTitle" with the host name.
    let server = host.borrow().server_host_name.clone();
    let fmt = ui.localize.find("GameUI_PlayerListDialogTitle").unwrap_or("").to_owned();
    let title = fmt.replacen("%s1", &server.chars().take(255).collect::<String>(), 1);
    gs_vgui::controls::frame::with_frame(ui, me, |f, _| f.title.set_text_raw(&title));
    let mute = Button::create(ui, Some(me), "MuteButton", "", None);
    let list = ListPanel::create(ui, Some(me), "PlayerList");
    let (c128, c80) = (ui.psv(me, 128), ui.psv(me, 80));
    with_list(ui, list, |l, ui| {
        l.add_column_header(ui, list, 0, "Name", "#GameUI_PlayerName", c128, 0);
        l.add_column_header(ui, list, 1, "Properties", "#GameUI_Properties", c80, 0);
        l.set_empty_list_text(ui, "#GameUI_NoOtherPlayersInGame");
    });
    load_res(ui, me, fs.as_ref(), "Resource/PlayerListDialog.res", s);
    ui.with_ctrl(me, |c, _| {
        if let Some(d) = c.as_any_mut().downcast_mut::<PlayerListDialog>() {
            d.list = list;
            d.mute = mute;
        }
    });
    me
}

impl PlayerListDialog {
    /// `Activate`: the list rebuilt from the player infos.
    fn activate(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        FrameState::activate(ui, me);
        let list = self.list;
        let host = self.host.clone();
        with_list(ui, list, |l, ui| {
            l.delete_all_items(ui, list);
            let max = host.borrow().max_clients;
            for i in 1..=max {
                let n = host.borrow().player_info_value(i, "name").unwrap_or_default();
                if !n.is_empty() {
                    l.add_item(ui, list, &Msg::new(&i.to_string()).str("Name", &n).int("index", i), 0, false, false);
                }
            }
        });
        self.refresh(ui);
        let first = with_list(ui, list, |l, _| l.item_id_from_row(0)).unwrap_or(-1);
        with_list(ui, list, |l, ui| l.set_single_selected_item(ui, list, first));
        self.on_item_selected(ui, s);
    }

    /// `RefreshPlayerProperties`.
    fn refresh(&mut self, ui: &mut Ui) {
        let list = self.list;
        let host = self.host.clone();
        with_list(ui, list, |l, ui| {
            let count = l.item_count();
            for id in 0..=count {
                let Some(idx) = l.item(id).map(|kv| kv.get_int("index")) else { continue };
                let h = host.borrow();
                let props = match h.player_info_value(idx, "name") {
                    None => "Disconnected".to_owned(),
                    Some(n) => {
                        if let Some(kv) = l.item_mut(id) {
                            kv.set("name", gs_vgui::Value::Str(n));
                        }
                        let muted = h.is_muted(idx);
                        if h.player_info_value(idx, "*bot").is_some_and(|b| b.eq_ignore_ascii_case("1")) {
                            "CPU Player".to_owned()
                        } else if muted {
                            "Muted".to_owned()
                        } else {
                            String::new()
                        }
                    }
                };
                if let Some(kv) = l.item_mut(id) {
                    kv.set("properties", gs_vgui::Value::Str(props));
                }
            }
            ui.invalidate_layout(list);
        });
    }

    /// `OnItemSelected`.
    fn on_item_selected(&mut self, ui: &mut Ui, _s: &mut dyn Surface) {
        self.refresh(ui);
        let list = self.list;
        let sel = with_list(ui, list, |l, _| (l.selected_items_count(), l.item(l.selected_item(0)).cloned())).unwrap_or((0, None));
        let mute = self.mute;
        match sel {
            (n, _) if n <= 0 => {
                ui.set_enabled(mute, false);
                with_button(ui, mute, |b, ui| b.label.set_text(ui, "#GameUI_MuteIngameVoice"));
            }
            (_, kv) => {
                let h = self.host.borrow();
                let idx = kv.as_ref().map_or(0, |k| k.get_int("index"));
                let name = h.player_info_value(idx, "name");
                let bot = kv.is_some() && h.player_info_value(idx, "*bot").is_some_and(|b| b == "1") || name.is_none();
                let text = if !bot && h.is_muted(idx) { "#GameUI_UnmuteIngameVoice" } else { "#GameUI_MuteIngameVoice" };
                drop(h);
                with_button(ui, mute, |b, ui| b.label.set_text(ui, text));
                ui.set_enabled(mute, !bot);
            }
        }
    }
}

impl Control for PlayerListDialog {
    fn class(&self) -> &'static str {
        "CPlayerListDialog"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("Activate") {
            self.activate(ui, me, s);
            return true;
        }
        if msg.is("ItemSelected") {
            self.on_item_selected(ui, s);
            return true;
        }
        match self.fs.on_message(ui, me, msg, s) {
            FrameMsg::Handled => true,
            FrameMsg::Unhandled => false,
            FrameMsg::Command(c) => {
                if c.eq_ignore_ascii_case("Mute") {
                    let list = self.list;
                    let idx = with_list(ui, list, |l, _| l.item(l.selected_item(0)).map(|kv| kv.get_int("index"))).flatten();
                    if let Some(idx) = idx {
                        let muted = self.host.borrow().is_muted(idx);
                        self.host.borrow_mut().set_muted(idx, !muted);
                        self.refresh(ui);
                        self.on_item_selected(ui, s);
                    }
                } else {
                    self.fs.on_command(ui, me, &c);
                }
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
