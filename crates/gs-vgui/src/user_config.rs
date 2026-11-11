//! The vgui user config (`ISystem::SetUserConfigFile` / `GetUserConfigFileData`,
//! docs/re/vgui2-core.md §6.3) and the panels that read and write it: `EditablePanel`'s
//! `LoadUserConfig` / `SaveUserConfig` with the child recursion, `Frame`'s position and size
//! (docs/re/vgui-controls.md §8: moveable → xpos/ypos, sizeable → wide/tall, clamped to the
//! workspace, centred without a position) and `ListPanel`'s `%d_hidden` columns
//! (docs/re/vgui-controls-2.md §6).

use crate::controls::frame::FrameState;
use crate::controls::list_panel::with_list;
use crate::keyvalues::KeyValues;
use crate::ui::{PanelId, Ui};

impl Ui {
    /// `SetUserConfigFile`'s load: the file's keys merge into the config (created on first use).
    pub fn load_user_config_file(&mut self, data: &[u8]) {
        let kv = self.user_config.get_or_insert_with(|| KeyValues::new("UserConfigData"));
        kv.load_from_buffer(data);
    }

    /// `SaveUserConfigFile`'s text (`None` without a config).
    pub fn user_config_file_text(&self) -> Option<Vec<u8>> {
        self.user_config.as_ref().map(KeyValues::save_to_buffer)
    }

    /// `GetUserConfigFileData(name, id)`: `"name_id"` for a non-zero id, created if absent.
    pub fn user_config_file_data(&mut self, name: &str, id: i32) -> Option<&mut KeyValues> {
        let key = if id != 0 { format!("{name}_{id}") } else { name.to_owned() };
        self.user_config.as_mut().map(|c| c.find_or_create(&key))
    }

    /// `EditablePanel::LoadUserConfig(name, id)`.
    pub fn load_user_config(&mut self, panel: PanelId, name: &str, id: i32) {
        let data = self.user_config_file_data(name, id).map(|d| d.clone());
        if let Some(n) = self.node_mut(panel) {
            n.config = Some((name.to_owned(), id));
        }
        if let Some(d) = data {
            self.apply_user_config_settings(panel, &d);
        }
    }

    /// `EditablePanel::SaveUserConfig` (nothing without a `LoadUserConfig`).
    pub fn save_user_config(&mut self, panel: PanelId) {
        let flags = self.with_ctrl(panel, |c, _| c.frame().map(|f| (f.moveable, f.sizeable))).flatten();
        self.save_user_config_as(panel, flags);
    }

    /// `SaveUserConfig` of a frame whose control is busy (its own `OnClose`).
    pub fn save_frame_user_config(&mut self, panel: PanelId, fs: &FrameState) {
        self.save_user_config_as(panel, Some((fs.moveable, fs.sizeable)));
    }

    fn save_user_config_as(&mut self, panel: PanelId, frame: Option<(bool, bool)>) {
        let Some((name, id)) = self.node(panel).and_then(|n| n.config.clone()) else { return };
        let Some(mut data) = self.user_config_file_data(&name, id).map(|d| d.clone()) else { return };
        self.get_user_config_settings(panel, frame, &mut data);
        if let Some(d) = self.user_config_file_data(&name, id) {
            *d = data;
        }
    }

    /// `HasUserConfigSettings`: frames and list panels.
    fn has_user_config_settings(&mut self, panel: PanelId) -> bool {
        self.with_ctrl(panel, |c, _| c.frame().is_some() || c.list_panel().is_some()).unwrap_or(false)
    }

    /// `ApplyUserConfigSettings(data)` by class.
    pub fn apply_user_config_settings(&mut self, panel: PanelId, data: &KeyValues) {
        let frame = self
            .with_ctrl(panel, |c, ui| {
                let pos = c.default_screen_position(ui);
                c.frame().map(|f| (f.moveable, f.sizeable, pos))
            })
            .flatten();
        if let Some((moveable, sizeable, default_pos)) = frame {
            self.frame_apply_user_config(panel, moveable, sizeable, default_pos, data);
        } else if with_list(self, panel, |l, ui| l.apply_user_config_settings(ui, panel, data)).is_some() {
            return;
        }
        if self.node(panel).is_some_and(|n| n.editable) {
            for child in self.children(panel) {
                let name = self.node(child).map(|n| n.name.clone()).unwrap_or_default();
                if !name.is_empty() && self.has_user_config_settings(child) {
                    let sub = data.find_key(&name).cloned().unwrap_or_else(|| KeyValues::new(&name));
                    self.apply_user_config_settings(child, &sub);
                }
            }
        }
    }

    fn get_user_config_settings(&mut self, panel: PanelId, frame: Option<(bool, bool)>, data: &mut KeyValues) {
        if let Some((moveable, sizeable)) = frame {
            if moveable {
                let (x, y) = self.pos(panel);
                data.set_int("xpos", x);
                data.set_int("ypos", y);
            }
            if sizeable {
                let (w, t) = self.size(panel);
                data.set_int("wide", w);
                data.set_int("tall", t);
            }
        } else if with_list(self, panel, |l, _| l.get_user_config_settings(data)).is_some() {
            return;
        }
        if self.node(panel).is_some_and(|n| n.editable) {
            for child in self.children(panel) {
                let name = self.node(child).map(|n| n.name.clone()).unwrap_or_default();
                if name.is_empty() || !self.has_user_config_settings(child) {
                    continue;
                }
                let child_frame = self.with_ctrl(child, |c, _| c.frame().map(|f| (f.moveable, f.sizeable))).flatten();
                let sub = data.find_or_create(&name);
                let mut copy = sub.clone();
                self.get_user_config_settings(child, child_frame, &mut copy);
                *data.find_or_create(&name) = copy;
            }
        }
    }

    /// `Frame::ApplyUserConfigSettings`.
    fn frame_apply_user_config(&mut self, panel: PanelId, moveable: bool, sizeable: bool, default_pos: Option<(i32, i32, i32, i32)>, data: &KeyValues) {
        let (wx, wy) = (0, 0);
        let (ww, wt) = self.screen;
        let (mut x, mut y) = self.pos(panel);
        let (mut wide, mut tall) = self.size(panel);
        let mut no_settings = false;
        if moveable {
            if data.find_key("xpos").is_none() {
                no_settings = true;
            }
            x = data.get_int("xpos", x);
            y = data.get_int("ypos", y);
        }
        if sizeable {
            wide = data.get_int("wide", wide).min(ww);
            tall = data.get_int("tall", tall).min(wt);
        }
        if no_settings && let Some(d) = default_pos {
            (x, y, wide, tall) = d;
            no_settings = false;
        }
        if x + wide > ww {
            x = wx + ww - wide;
        }
        if y + tall > wt {
            y = wy + wt - tall;
        }
        x = x.max(wx);
        y = y.max(wy);
        self.set_bounds(panel, x, y, wide, tall);
        if no_settings {
            FrameState::move_to_center_of_screen(self, panel);
        }
    }
}
