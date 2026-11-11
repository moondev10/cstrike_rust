//! Script objects (`settings.scr` / `user.scr`), `CPanelListPanel`, `mpcontrol_t` and the
//! multiplayer-advanced dialog/page (docs/re/gameui-dialogs.md §11, §13;
//! docs/re/vgui-controls-2.md §8.2).

use std::any::Any;

use chrono::{Datelike, Timelike};

use gs_vgui::controls::button::Button;
use gs_vgui::controls::combo_box::ComboBox;
use gs_vgui::controls::frame::{FrameMsg, FrameState};
use gs_vgui::controls::label::{Align, Label, with_label};
use gs_vgui::controls::property::{NoHooks, PageHooks, PropertyPage};
use gs_vgui::controls::scroll_bar::ScrollBar;
use gs_vgui::controls::text_entry::{TextEntry, with_te};
use gs_vgui::controls::toggle::{ToggleButton, ToggleKind, with_toggle};
use gs_vgui::scheme::Scheme;
use gs_vgui::{Color, Control, Msg, PanelId, Surface, Ui};

use crate::options::{c_atof, load_res, set_page_hooks, task_frame};
use crate::{Fs, GameFs, HostRequest, Shared};

// ---------------------------------------------------------------------------------------------
// Script objects (§11)

const SCRIPT_HEADER_LINES: &[&str] = &[
    "// NOTE:  THIS FILE IS AUTOMATICALLY REGENERATED, ",
    "//DO NOT EDIT THIS HEADER, YOUR COMMENTS WILL BE LOST IF YOU DO",
    "// Multiplayer options script",
    "//",
    "// Format:",
    "//  Version [float]",
    "//  Options description followed by ",
    "//  Options defaults",
    "//",
    "// Option description syntax:",
    "//",
    "//  \"cvar\" { \"Prompt\" { type [ type info ] } { default } }",
    "//",
    "//  type = ",
    "//   BOOL   (a yes/no toggle)",
    "//   STRING",
    "//   NUMBER",
    "//   LIST",
    "//",
    "// type info:",
    "// BOOL                 no type info",
    "// NUMBER       min max range, use -1 -1 for no limits",
    "// STRING       no type info",
    "// LIST          delimited list of options value pairs",
    "//",
    "//",
    "// default depends on type",
    "// BOOL is \"0\" or \"1\"",
    "// NUMBER is \"value\"",
    "// STRING is \"value\"",
    "// LIST is \"index\", where index \"0\" is the first element of the list",
    "",
    "",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjType {
    Bool,
    Number,
    List,
    String,
}

#[derive(Debug, Clone)]
pub struct ListItem {
    pub text: String,
    pub value: String,
}

/// `CScriptObject`.
#[derive(Debug, Clone)]
pub struct ScriptObject {
    pub ty: ObjType,
    pub cvar: String,
    pub prompt: String,
    pub items: Vec<ListItem>,
    pub min: f32,
    pub max: f32,
    pub def_value: String,
    pub fdef: f32,
    pub cur_value: String,
    pub fcur: f32,
    pub set_info: bool,
}

/// `RemoveTrailingZeros`.
fn remove_trailing_zeros(s: &mut String) {
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
}

/// `StripQuotesPercent`.
fn strip_quotes_percent(s: &str) -> String {
    s.chars().filter(|&c| c != '"' && c != '%').collect()
}

impl ScriptObject {
    /// `SetCurValue`.
    pub fn set_cur_value(&mut self, s: &str) {
        self.cur_value = s.chars().take(127).collect();
        self.fcur = c_atof(&self.cur_value) as f32;
        if matches!(self.ty, ObjType::Number | ObjType::Bool) {
            remove_trailing_zeros(&mut self.cur_value);
        }
    }
}

/// `CDescription` (`CServerDescription` / `CInfoDescription`).
#[derive(Debug, Clone)]
pub struct Description {
    pub objects: Vec<ScriptObject>,
    /// `SERVER_OPTIONS` or `INFO_OPTIONS`.
    pub kind: &'static str,
}

/// The engine's `COM_ParseFile` over a byte buffer.
struct Tokens<'a> {
    p: &'a [u8],
}

impl Tokens<'_> {
    fn next(&mut self) -> String {
        match gs_common::token::com_parse(self.p, false) {
            Some((t, rest)) => {
                self.p = rest;
                String::from_utf8_lossy(&t).into_owned()
            }
            None => String::new(),
        }
    }
}

impl Description {
    pub fn new(kind: &'static str) -> Self {
        Self { objects: Vec::new(), kind }
    }

    /// `ReadFromBuffer`: errors go to the console (no newline) and stop the load.
    fn read_from_buffer(&mut self, data: &[u8], host: &Shared) -> bool {
        let err = |m: String| host.borrow_mut().requests.push(HostRequest::ConPrintf(m));
        let mut t = Tokens { p: data };
        let tok = t.next();
        if !tok.eq_ignore_ascii_case("VERSION") {
            err(format!("Expecting 'VERSION', got '{tok}'"));
            return false;
        }
        let tok = t.next();
        if tok.is_empty() {
            err("Expecting version #".to_owned());
            return false;
        }
        let v = c_atof(&tok) as f32;
        if v != 1.0 {
            err(format!("Version mismatch, expecting {:.6}, got {:.6}", 1.0, f64::from(v)));
            return false;
        }
        let tok = t.next();
        if !tok.eq_ignore_ascii_case("DESCRIPTION") {
            err(format!("Expecting 'DESCRIPTION', got '{tok}'"));
            return false;
        }
        let tok = t.next();
        if tok.is_empty() {
            err(format!("Expecting '{}'", self.kind));
            return false;
        }
        if !tok.eq_ignore_ascii_case(self.kind) {
            err(format!("Expecting {}, got {tok}", self.kind));
            return false;
        }
        let tok = t.next();
        if tok != "{" {
            err(format!("Expecting '{{', got '{tok}'"));
            return false;
        }
        loop {
            let save = t.p;
            let tok = t.next();
            if tok.is_empty() || tok == "}" {
                break;
            }
            t.p = save;
            match Self::read_object(&mut t, &err) {
                Some(o) => self.objects.push(o),
                None => return false,
            }
        }
        true
    }

    /// `CScriptObject::ReadFromBuffer`.
    fn read_object(t: &mut Tokens<'_>, err: &dyn Fn(String)) -> Option<ScriptObject> {
        let cvar: String = t.next().chars().take(63).collect();
        let brace = |t: &mut Tokens<'_>| {
            let tok = t.next();
            if tok == "{" {
                Some(())
            } else {
                err(format!("Expecting '{{', got '{tok}'"));
                None
            }
        };
        brace(t)?;
        let prompt: String = t.next().chars().take(255).collect();
        brace(t)?;
        let tname = t.next();
        let ty = match tname.to_ascii_uppercase().as_str() {
            "BOOL" => ObjType::Bool,
            "NUMBER" => ObjType::Number,
            "LIST" => ObjType::List,
            "STRING" => ObjType::String,
            _ => {
                err(format!("Type '{tname}' unknown"));
                return None;
            }
        };
        let close = |t: &mut Tokens<'_>| {
            let tok = t.next();
            if tok == "}" {
                Some(())
            } else {
                err(format!("Expecting '}}', got '{tok}'"));
                None
            }
        };
        let (mut min, mut max, mut items) = (0.0, 0.0, Vec::new());
        match ty {
            ObjType::Bool | ObjType::String => close(t)?,
            ObjType::Number => {
                min = c_atof(&t.next()) as f32;
                max = c_atof(&t.next()) as f32;
                close(t)?;
            }
            ObjType::List => loop {
                let save = t.p;
                let tok = t.next();
                if tok == "}" || tok.is_empty() {
                    break;
                }
                t.p = save;
                let text: String = t.next().chars().take(127).collect();
                let value: String = t.next().chars().take(255).collect();
                items.push(ListItem { text, value });
            },
        }
        brace(t)?;
        let mut def_value: String = t.next().chars().take(127).collect();
        let fdef = c_atof(&def_value) as f32;
        if ty == ObjType::Number {
            remove_trailing_zeros(&mut def_value);
        }
        let mut cur_value = def_value.clone();
        let fcur = c_atof(&cur_value) as f32;
        if matches!(ty, ObjType::Number | ObjType::Bool) {
            remove_trailing_zeros(&mut cur_value);
        }
        let tok = t.next();
        if tok != "}" {
            err(format!("Expecting '{{', got '{tok}'"));
            return None;
        }
        let mut tok = t.next();
        let mut set_info = false;
        if tok.eq_ignore_ascii_case("SetInfo") {
            set_info = true;
            tok = t.next();
        }
        if tok != "}" {
            err(format!("Expecting '}}', got '{tok}'"));
            return None;
        }
        Some(ScriptObject { ty, cvar, prompt, items, min, max, def_value, fdef, cur_value, fcur, set_info })
    }

    /// `InitFromFile(name)` (`GAMECONFIG` only).
    pub fn init_from_file(&mut self, fs: &dyn GameFs, name: &str, host: &Shared) -> bool {
        let Some(data) = fs.read_in(name, "GAMECONFIG") else { return false };
        self.read_from_buffer(&data, host);
        true
    }

    /// `CInfoDescription::InitFromFile`: plus the objects of `user_default.scr` not present.
    pub fn init_info_from_file(&mut self, fs: &dyn GameFs, name: &str, host: &Shared) -> bool {
        let r = self.init_from_file(fs, name, host);
        if let Some(data) = fs.read("user_default.scr") {
            let mut defaults = Description::new(self.kind);
            defaults.read_from_buffer(&data, host);
            for o in defaults.objects {
                if !self.objects.iter().any(|x| x.cvar.eq_ignore_ascii_case(&o.cvar)) {
                    self.objects.push(o);
                }
            }
        }
        r
    }

    /// `InitFromCurrentCvars`.
    pub fn init_from_current_cvars(&mut self, host: &Shared) {
        let h = host.borrow();
        for o in &mut self.objects {
            let v = if o.set_info {
                h.userinfo.iter().find(|(k, _)| k.eq_ignore_ascii_case(&o.cvar)).map(|(_, v)| v.clone()).unwrap_or_default()
            } else {
                h.cvar_string(&o.cvar).unwrap_or("").to_owned()
            };
            if !v.is_empty() {
                let v: String = v.chars().take(127).collect();
                o.cur_value = v.clone();
                o.def_value = v;
                o.fcur = c_atof(&o.cur_value) as f32;
                o.fdef = o.fcur;
            }
        }
    }

    /// `UpdateEngine`: one console command per object.
    pub fn update_engine(&self, host: &Shared) {
        let mut h = host.borrow_mut();
        for o in &self.objects {
            let value = match o.ty {
                ObjType::Bool => if o.fcur == 0.0 { "0" } else { "1" }.to_owned(),
                ObjType::Number => {
                    let mut v = o.fcur;
                    // `!(v > min)` / `!(max > v)`: NaN takes the bound too.
                    if o.min != -1.0 && v.partial_cmp(&o.min) != Some(std::cmp::Ordering::Greater) {
                        v = o.min;
                    }
                    if o.max != -1.0 && o.max.partial_cmp(&v) != Some(std::cmp::Ordering::Greater) {
                        v = o.max;
                    }
                    format!("{:.6}", f64::from(v))
                }
                ObjType::String => strip_quotes_percent(&format!("\"{}\"", o.cur_value)),
                ObjType::List => match o.items.get(o.fcur as i32 as usize).filter(|_| o.fcur >= 0.0) {
                    Some(it) => strip_quotes_percent(&it.value),
                    None => "0.0".to_owned(),
                },
            };
            let cmd = if o.set_info { format!("setinfo {} \"{}\"\n", o.cvar, value) } else { format!("{} \"{}\"\n", o.cvar, value) };
            h.client_cmd(&cmd);
        }
    }

    /// `WriteToScriptFile`: the header, then every object (names, prompts, list texts and
    /// string values stripped of quotes and `%` in place).
    pub fn write_to_script(&mut self) -> String {
        let mut out = String::new();
        for (i, l) in SCRIPT_HEADER_LINES.iter().enumerate() {
            let l = if i == 2 && self.kind == "INFO_OPTIONS" { "// User options script" } else { l };
            out.push_str(l);
            out.push_str("\r\n");
        }
        let (title, desc) = if self.kind == "INFO_OPTIONS" {
            ("// Half-Life User Info Configuration Layout Script (stores last settings chosen, too)\r\n", "DESCRIPTION INFO_OPTIONS\r\n{\r\n")
        } else {
            ("// Half-Life Server Configuration Layout Script (stores last settings chosen, too)\r\n", "DESCRIPTION SERVER_OPTIONS\r\n{\r\n")
        };
        out.push_str(title);
        out.push_str(&format!("// File generated:  {} {}\r\n", file_time(), am_pm()));
        out.push_str("//\r\n//\r\n// Cvar\t-\tSetting\r\n\r\n");
        out.push_str("VERSION 1.0\r\n\r\n");
        out.push_str(desc);
        for o in &mut self.objects {
            o.cvar = strip_quotes_percent(&o.cvar);
            o.prompt = strip_quotes_percent(&o.prompt);
            out.push_str(&format!("\t\"{}\"\r\n\t{{\r\n\t\t\"{}\"\r\n", o.cvar, o.prompt));
            match o.ty {
                ObjType::Bool => out.push_str(&format!("\t\t{{ BOOL }}\r\n\t\t{{ \"{}\" }}\r\n", i32::from(o.fcur as i32 != 0))),
                ObjType::Number => out.push_str(&format!("\t\t{{ NUMBER {:.6} {:.6} }}\r\n\t\t{{ \"{:.6}\" }}\r\n", f64::from(o.min), f64::from(o.max), f64::from(o.fcur))),
                ObjType::String => {
                    o.cur_value = strip_quotes_percent(&o.cur_value);
                    out.push_str(&format!("\t\t{{ STRING }}\r\n\t\t{{ \"{}\" }}\r\n", o.cur_value));
                }
                ObjType::List => {
                    out.push_str("\t\t{\r\n\t\t\tLIST\r\n");
                    for it in &mut o.items {
                        it.text = strip_quotes_percent(&it.text);
                        it.value = strip_quotes_percent(&it.value);
                        out.push_str(&format!("\t\t\t\"{}\" \"{}\"\r\n", it.text, it.value));
                    }
                    out.push_str(&format!("\t\t}}\r\n\t\t{{ \"{:.6}\" }}\r\n", f64::from(o.fcur)));
                }
            }
            if o.set_info {
                out.push_str("\t\tSetInfo\r\n");
            }
            out.push_str("\t}\r\n\r\n");
        }
        out.push_str("}\r\n");
        out
    }
}

/// `asctime` of the local time with the hour made 12-hour (`>12 → −12`, `0 → 12`), first 19
/// characters.
fn file_time() -> String {
    let now = chrono::Local::now();
    let h = now.hour();
    let h12 = if h > 12 {
        h - 12
    } else if h == 0 {
        12
    } else {
        h
    };
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!(
        "{} {}{:3} {:02}:{:02}:{:02}",
        DAYS[now.weekday().num_days_from_sunday() as usize],
        MONTHS[now.month0() as usize],
        now.day(),
        h12,
        now.minute(),
        now.second()
    )
}

/// `"PM"` only for hours after 12 (noon stays `"AM"`).
fn am_pm() -> &'static str {
    if chrono::Local::now().hour() > 12 { "PM" } else { "AM" }
}

// ---------------------------------------------------------------------------------------------
// CPanelListPanel (vgui-controls-2.md §8.2)

pub struct PanelListPanel {
    items: Vec<PanelId>,
    vbar: PanelId,
    embedded: PanelId,
}

impl std::fmt::Debug for PanelListPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CPanelListPanel").field("items", &self.items).finish_non_exhaustive()
    }
}

impl PanelListPanel {
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str) -> PanelId {
        let placeholder = ui.embedded();
        let me = ui.create(parent, name, Box::new(PanelListPanel { items: Vec::new(), vbar: placeholder, embedded: placeholder }));
        let s100 = ui.psv(me, 100);
        ui.set_bounds(me, 0, 0, s100, s100);
        let vbar = ScrollBar::create(ui, Some(me), "CPanelListPanelVScroll", true);
        let s20 = ui.psv(me, 20);
        ui.set_bounds(vbar, 0, 0, s20, s20);
        ui.set_visible(vbar, false);
        ui.add_action_signal_target(vbar, me);
        let embedded = ui.create(Some(me), "PanelListEmbedded", Box::new(gs_vgui::controls::misc::Divider));
        ui.set_bounds(embedded, 0, 0, s20, s20);
        if let Some(n) = ui.node_mut(embedded) {
            n.paint_background_enabled = false;
            n.paint_border_enabled = false;
        }
        if let Some(p) = ui.ctrl::<PanelListPanel>(me) {
            p.vbar = vbar;
            p.embedded = embedded;
        }
        me
    }

    /// `AddItem(panel)`.
    pub fn add_item(ui: &mut Ui, me: PanelId, panel: PanelId) {
        ui.invalidate_layout(me);
        let Some(embedded) = ui.ctrl::<PanelListPanel>(me).map(|p| p.embedded) else { return };
        ui.set_parent(panel, Some(embedded));
        if let Some(p) = ui.ctrl::<PanelListPanel>(me) {
            p.items.push(panel);
        }
    }

    pub fn items(ui: &mut Ui, me: PanelId) -> Vec<PanelId> {
        ui.ctrl::<PanelListPanel>(me).map(|p| p.items.clone()).unwrap_or_default()
    }

    /// `DeleteAllItems`.
    pub fn delete_all_items(ui: &mut Ui, me: PanelId) {
        let items = ui.ctrl::<PanelListPanel>(me).map(|p| std::mem::take(&mut p.items)).unwrap_or_default();
        for i in items {
            ui.delete(i);
        }
        ui.invalidate_layout(me);
    }
}

impl Control for PanelListPanel {
    fn class(&self) -> &'static str {
        "CPanelListPanel"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, _s: &mut dyn Surface) -> bool {
        if msg.is("ScrollBarSliderMoved") {
            ui.invalidate_layout(me);
            return true;
        }
        if msg.is("MouseWheeled") {
            let v = ScrollBar::value(ui, self.vbar);
            let step = ui.psv(me, 15);
            ScrollBar::set_value(ui, self.vbar, v - msg.get_int("delta") * step);
            return true;
        }
        false
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, _s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let border = scheme.border("ButtonDepressedBorder");
        let bg = scheme.color("LabelBgColor", scheme.color("WindowDisabledBgColor", Color::TRANSPARENT));
        if let Some(n) = ui.node_mut(me) {
            n.border = border;
            n.bg = bg;
        }
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, _s: &mut dyn Surface) {
        let (wide, tall) = ui.size(me);
        let vpix: i32 = self.items.iter().map(|&p| ui.size(p).1).sum::<i32>() + ui.psv(me, 5);
        let (s24, s20, s18, s2, s8, s36) = (ui.psv(me, 24), ui.psv(me, 20), ui.psv(me, 18), ui.psv(me, 2), ui.psv(me, 8), ui.psv(me, 36));
        ui.set_visible(self.vbar, true);
        ui.set_enabled(self.vbar, false);
        ScrollBar::set_range(ui, self.vbar, 0, vpix - tall + s24);
        ScrollBar::set_range_window(ui, self.vbar, s24);
        ScrollBar::set_button_pressed_scroll_value(ui, self.vbar, s24);
        ui.set_pos(self.vbar, wide - s20, 0);
        ui.set_size(self.vbar, s18, tall - s2);
        ui.invalidate_layout(self.vbar);
        let v = ScrollBar::value(ui, self.vbar);
        ui.set_pos(self.embedded, 0, -v);
        ui.set_size(self.embedded, wide - s20, vpix);
        let mut y = 0;
        for &p in &self.items {
            let h = ui.size(p).1;
            ui.set_bounds(p, s8, y, wide - s36, h);
            y += h;
        }
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// mpcontrol_t

#[derive(Debug)]
pub struct MpControl {
    pub ty: ObjType,
    pub control: Option<PanelId>,
    pub prompt: Option<PanelId>,
    pub obj: usize,
}

impl Control for MpControl {
    fn class(&self) -> &'static str {
        "mpcontrol_t"
    }
    /// `OnSizeChanged`: label left of the middle (+20), control right of it.
    fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId, w: i32, h: i32) {
        match (self.prompt, self.control) {
            (Some(l), c) => {
                let mid = w / 2;
                if let Some(c) = c {
                    ui.set_bounds(c, mid + 20, 4, mid - 20, h - 8);
                }
                ui.set_bounds(l, 0, 4, mid + 20, h - 8);
            }
            (None, Some(c)) => ui.set_bounds(c, 0, 4, w, h - 8),
            (None, None) => {}
        }
        ui.invalidate_layout(me);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// The names of a control set: the advanced dialog's or the create-server gameplay page's.
pub struct ControlNames {
    pub item: &'static str,
    pub check: &'static str,
    pub text: &'static str,
    pub combo: &'static str,
}

/// `CreateControls` / `LoadGameOptionsList`: one `mpcontrol_t` per object. `default_host`
/// replaces the `hostname` default (the create-server page).
pub fn create_controls(ui: &mut Ui, list: PanelId, desc: &Description, names: &ControlNames, default_host: Option<&str>, s: &mut dyn Surface) -> Vec<PanelId> {
    let mut out = Vec::new();
    for (i, o) in desc.objects.iter().enumerate() {
        let item_name = if names.item.is_empty() { o.cvar.as_str() } else { names.item };
        let mp = ui.create(Some(list), item_name, Box::new(MpControl { ty: o.ty, control: None, prompt: None, obj: i }));
        if let Some(n) = ui.node_mut(mp) {
            n.paint_background_enabled = false;
        }
        let control = match o.ty {
            ObjType::Bool => {
                let c = ToggleButton::create(ui, Some(mp), names.check, &o.prompt, ToggleKind::Check);
                with_toggle(ui, c, |t, ui| t.set_selected(ui, c, o.fdef != 0.0, s));
                c
            }
            ObjType::Number | ObjType::String => {
                let c = TextEntry::create(ui, Some(mp), names.text);
                let text = match default_host {
                    Some(h) if o.cvar == "hostname" => h.to_owned(),
                    _ => o.def_value.clone(),
                };
                with_te(ui, c, |te, ui| te.insert_string(ui, c, &text, s));
                c
            }
            ObjType::List => {
                let c = ComboBox::create(ui, Some(mp), names.combo, 5, false);
                for it in &o.items {
                    ComboBox::add_item(ui, c, &it.text, None);
                }
                ComboBox::activate_item_by_row(ui, c, o.fdef as i32, s);
                c
            }
        };
        let prompt = (o.ty != ObjType::Bool).then(|| {
            let l = Label::create(ui, Some(mp), "DescLabel", "");
            let inset = ui.psv(list, 5);
            with_label(ui, l, |st, ui| {
                st.align = Align::West;
                st.set_text_inset(ui, l, inset, 0);
                st.set_text(ui, &o.prompt);
            });
            l
        });
        if let Some(m) = ui.ctrl::<MpControl>(mp) {
            m.control = Some(control);
            m.prompt = prompt;
        }
        let (w, h) = (ui.psv(list, 100), ui.psv(list, 28));
        ui.set_size(mp, w, h);
        PanelListPanel::add_item(ui, list, mp);
        out.push(mp);
    }
    out
}

/// `GatherCurrentValues` of the advanced dialog: the controls' values into the objects.
pub fn gather_current_values(ui: &mut Ui, desc: &mut Description, controls: &[PanelId]) {
    for &mp in controls {
        let Some((obj, control)) = ui.ctrl::<MpControl>(mp).map(|m| (m.obj, m.control)) else { continue };
        let Some(o) = desc.objects.get(obj) else { continue };
        let value = match control {
            None => o.def_value.clone(),
            Some(c) => match o.ty {
                ObjType::Bool => if with_toggle(ui, c, |t, _| t.is_selected()).unwrap_or(false) { "1" } else { "0" }.to_owned(),
                ObjType::Number | ObjType::String => gs_common::bytestr::ansi_cap(&with_te(ui, c, |te, _| te.text()).unwrap_or_default(), 256),
                ObjType::List => {
                    let idx = ComboBox::active_item(ui, c);
                    match usize::try_from(idx).ok().and_then(|i| o.items.get(i)) {
                        Some(it) => it.value.clone(),
                        None => {
                            log::warn!("Couldn't find string in list, using default value");
                            o.def_value.clone()
                        }
                    }
                }
            },
        };
        let v = strip_quotes_percent(&value);
        if let Some(o) = desc.objects.get_mut(obj) {
            o.set_cur_value(&v);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// CMultiplayerAdvancedDialog / CMultiplayerAdvancedPage (§13)

const ADVANCED_NAMES: ControlNames = ControlNames { item: "mpcontrol_t", check: "DescCheckButton", text: "DescTextEntry", combo: "DescComboBox" };

/// `CInfoDescription` from `user.scr` + current cvars, and its controls.
fn advanced_setup(ui: &mut Ui, owner: PanelId, host: &Shared, fs: &Fs, s: &mut dyn Surface) -> (PanelId, Description, Vec<PanelId>) {
    let list = PanelListPanel::create(ui, Some(owner), "PanelListPanel");
    let mut desc = Description::new("INFO_OPTIONS");
    desc.init_info_from_file(fs.as_ref(), "user.scr", host);
    desc.init_from_current_cvars(host);
    let controls = create_controls(ui, list, &desc, &ADVANCED_NAMES, None, s);
    (list, desc, controls)
}

/// `SaveValues`: gather, engine commands, then `user.scr` rewritten.
fn save_values(ui: &mut Ui, desc: &mut Description, controls: &[PanelId], host: &Shared, fs: &Fs) {
    gather_current_values(ui, desc, controls);
    desc.update_engine(host);
    let text = desc.write_to_script();
    fs.write_config("user.scr", text.as_bytes());
}

pub struct AdvancedDialog {
    pub fs: FrameState,
    host: Shared,
    gamefs: Fs,
    desc: Description,
    controls: Vec<PanelId>,
}

pub fn create_multiplayer_advanced_dialog(ui: &mut Ui, parent: PanelId, host: &Shared, fs: &Fs, s: &mut dyn Surface) -> PanelId {
    let me = ui.create(Some(parent), "MultiplayerAdvancedDialog", Box::new(AdvancedDialog { fs: FrameState::default(), host: host.clone(), gamefs: fs.clone(), desc: Description::new("INFO_OPTIONS"), controls: Vec::new() }));
    FrameState::init(ui, me, false);
    let (w, h) = (ui.psv(me, 372), ui.psv(me, 160));
    ui.set_bounds(me, 0, 0, w, h);
    FrameState::set_sizeable(ui, me, false);
    task_frame(ui, me, "#GameUI_MultiplayerAdvanced", true);
    Button::create(ui, Some(me), "Cancel", "#GameUI_Cancel", Some("Close"));
    Button::create(ui, Some(me), "OK", "#GameUI_OK", Some("Ok"));
    let (_, desc, controls) = advanced_setup(ui, me, host, fs, s);
    load_res(ui, me, fs.as_ref(), "Resource\\MultiplayerAdvancedDialog.res", s);
    ui.with_ctrl(me, |c, _| {
        if let Some(d) = c.as_any_mut().downcast_mut::<AdvancedDialog>() {
            d.desc = desc;
            d.controls = controls;
        }
    });
    me
}

impl Control for AdvancedDialog {
    fn class(&self) -> &'static str {
        "CMultiplayerAdvancedDialog"
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, s: &mut dyn Surface) -> bool {
        if msg.is("Activate") {
            FrameState::activate(ui, me);
            return true;
        }
        match self.fs.on_message(ui, me, msg, s) {
            FrameMsg::Handled => true,
            FrameMsg::Unhandled => false,
            FrameMsg::Command(c) => {
                if c.eq_ignore_ascii_case("Ok") {
                    save_values(ui, &mut self.desc, &self.controls, &self.host, &self.gamefs);
                    self.fs.close(ui, me);
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

struct AdvancedPage {
    host: Shared,
    fs: Fs,
    desc: Description,
    controls: Vec<PanelId>,
}

/// The page form (single-player-only mods; not added for Counter-Strike).
pub fn create_multiplayer_advanced_page(ui: &mut Ui, dlg: PanelId, host: &Shared, fs: &Fs, s: &mut dyn Surface) -> PanelId {
    let page = PropertyPage::create(ui, Some(dlg), "MultiplayerAdvancedDialog", true, Box::new(NoHooks));
    let (_, desc, controls) = advanced_setup(ui, page, host, fs, s);
    load_res(ui, page, fs.as_ref(), "Resource\\MultiplayerAdvancedPage.res", s);
    set_page_hooks(ui, page, Box::new(AdvancedPage { host: host.clone(), fs: fs.clone(), desc, controls }));
    page
}

impl PageHooks for AdvancedPage {
    fn on_apply_changes(&mut self, ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {
        save_values(ui, &mut self.desc, &self.controls, &self.host, &self.fs);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
