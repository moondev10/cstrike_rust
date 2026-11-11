//! The VGUI2 viewport (docs/re/client-hud.md §7): the pending-menu queue, the MOTD and map
//! briefing, the team, class and buy menus laid out from their `.res` files with the
//! `classes/*.res` info panes, and the scoreboard. Panels are drawn with the engine 2D API in
//! the client scheme's fonts and colours; queue, gating, commands and keys follow the original.

use std::collections::VecDeque;

use gs_common::bytestr::truncate_encoded;
use gs_console::keys::{K_KP_ENTER, K_SPACE};
use gs_draw::{FontId, Screen};
use gs_formats::keyvalues;

use super::chat::construct;
use super::health::team_color;
use super::specgui::{atoi, psv};
use super::{Hud, HudEngine, MsgReader};

/// The proportional base the windows' `.res` layouts are written for.
const BASE_SIZE: (i32, i32) = (640, 480);

/// One control of a `.res` layout.
#[derive(Debug, Clone, Default)]
struct Control {
    name: String,
    class: String,
    /// `xpos, ypos, wide, tall`.
    rect: (i32, i32, i32, i32),
    label: String,
    command: String,
    font: String,
    /// `textAlignment`: `center`, or `north*` (top) rather than vertically centred.
    center: bool,
    north: bool,
    dull: bool,
    bright: bool,
    visible: bool,
    image: String,
    image_color: String,
    fill_color: String,
    cost: i32,
    as_restrict: bool,
    de_useonly: bool,
}

/// `LoadControlSettings` on a proportional panel (every viewport window, client.so's ctors call
/// `SetProportional(true)`): `Panel::ApplySettings` scales `xpos`, `ypos`, `wide` and `tall`
/// by the screen height (PSV); an `r`/`c` position prefix counts from the screen's right or
/// bottom edge or its centre; a missing position stays 0 (vgui-controls.md §1.7). Returns the
/// frame's bounds and the controls. A `WizardSubPanel` is placed by its wizard, so only a
/// `Frame` moves its controls.
fn parse_layout(text: &str, (sw, sh): (i32, i32)) -> ((i32, i32, i32, i32), Vec<Control>) {
    let kv = keyvalues::parse(text);
    let mut frame = (0, 0, 0, 0);
    let mut controls = Vec::new();
    let Some((_, root)) = kv.blocks().next() else { return (frame, controls) };
    let coord = |v: &str, extent: i32| match v.as_bytes().first() {
        Some(b'r' | b'R') => extent - psv(atoi(&v[1..]), sh),
        Some(b'c' | b'C') => extent / 2 + psv(atoi(&v[1..]), sh),
        _ => psv(atoi(v), sh),
    };
    for (name, b) in root.blocks() {
        let s = |k: &str| b.last_string(k).unwrap_or_default().to_owned();
        let int = |k: &str| s(k).trim().parse::<i32>().unwrap_or(0);
        let x = b.last_string("xpos").map_or(0, |v| coord(v, sw));
        let y = b.last_string("ypos").map_or(0, |v| coord(v, sh));
        let rect = (x, y, psv(atoi(&s("wide")), sh), psv(atoi(&s("tall")), sh));
        let align = s("textAlignment").to_ascii_lowercase();
        match s("ControlName").as_str() {
            "Frame" => frame = rect,
            "WizardSubPanel" => {}
            class => controls.push(Control {
                name: name.to_owned(),
                class: class.to_owned(),
                rect,
                label: s("labelText"),
                command: s("command"),
                font: s("font"),
                center: align == "center",
                north: align.starts_with("north"),
                dull: int("dulltext") != 0,
                bright: int("brighttext") != 0,
                visible: s("visible") != "0",
                image: s("image"),
                image_color: s("imagecolor"),
                fill_color: s("fillColor"),
                cost: int("cost"),
                as_restrict: int("as_restrict") != 0,
                de_useonly: int("de_useonly") != 0,
            }),
        }
    }
    (frame, controls)
}

/// `GetAlternateProportionalValueFromScaled`: a scaled value normalised back
/// (`GetProportionalNormalizedValue`, single precision) and scaled by the smaller of the two
/// screen/base ratios, so the layout keeps 4:3 on any screen.
fn alt_proportional(v: i32, (sw, sh): (i32, i32)) -> i32 {
    let (bw, bh) = (sw.min(BASE_SIZE.0), sh.min(BASE_SIZE.1));
    let norm = (v as f32 / (sh as f32 / bh as f32)) as i32;
    let ratio = (f64::from(sh) / f64::from(bh)).min(f64::from(sw) / f64::from(bw));
    (f64::from(norm) * ratio) as i32
}

/// `CCSBackGroundPanel` after `Activate` (command-menu.md §10.1): its children re-mapped into
/// the centred 4:3 box, and the offset `CenterWindow` gives every window.
#[derive(Debug, Clone, Default)]
struct Backdrop {
    offset: (i32, i32),
    controls: Vec<Control>,
}

impl Backdrop {
    /// The layout at screen size `screen`: the frame becomes the screen and, while it was not
    /// screen-sized, every child's bounds go through `GetAlternateProportionalValueFromScaled`
    /// and the offset `((SW − alt(frame w))/2, (SH − alt(frame h))/2)`.
    fn layout(text: &str, screen: (i32, i32)) -> Self {
        let (frame, mut controls) = parse_layout(text, screen);
        if (frame.2, frame.3) == screen {
            return Self { offset: (0, 0), controls };
        }
        let alt = |v| alt_proportional(v, screen);
        let offset = ((screen.0 - alt(frame.2)) / 2, (screen.1 - alt(frame.3)) / 2);
        for c in &mut controls {
            c.rect = (alt(c.rect.0) + offset.0, alt(c.rect.1) + offset.1, alt(c.rect.2), alt(c.rect.3));
        }
        Self { offset, controls }
    }

    /// `CounterStrikeViewport::CenterWindow`: `(x + m_offsetX, y + (int)(m_offsetY · 0.67f))`.
    fn place(&self, (x, y): (i32, i32)) -> (i32, i32) {
        (x + self.offset.0, y + (self.offset.1 as f32 * 0.67) as i32)
    }

    fn rect(&self, name: &str) -> Option<(i32, i32, i32, i32)> {
        self.controls.iter().find(|c| c.name.eq_ignore_ascii_case(name)).map(|c| c.rect)
    }
}

/// The hotkey after `&` (lower case) and the label without the marker.
fn hotkey(label: &str) -> (Option<char>, String) {
    match label.find('&') {
        Some(i) => {
            let key = label[i + 1..].chars().next().map(|c| c.to_ascii_lowercase());
            (key, format!("{}{}", &label[..i], &label[i + 1..]))
        }
        None => (None, label.to_owned()),
    }
}

/// Text file contents: UTF-16LE with a BOM, else (lossy) UTF-8; CRs dropped.
fn decode_text(data: &[u8]) -> String {
    let text = match data {
        [0xFF, 0xFE, rest @ ..] => {
            let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&units)
        }
        _ => String::from_utf8_lossy(data).into_owned(),
    };
    text.replace('\r', "")
}

/// The text an HTML page shows: `head`/`style`/`script` dropped, `br` and block ends as line
/// breaks, whitespace collapsed, the common entities decoded.
fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut skip: Option<String> = None;
    let mut rest = html;
    let push_text = |out: &mut String, text: &str| {
        let text = text.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&");
        for c in text.chars() {
            if c.is_whitespace() {
                if !out.is_empty() && !out.ends_with([' ', '\n']) {
                    out.push(' ');
                }
            } else {
                out.push(c);
            }
        }
    };
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            if skip.is_none() {
                push_text(&mut out, rest);
            }
            break;
        };
        if skip.is_none() {
            push_text(&mut out, &rest[..lt]);
        }
        // The tag runs to its `>`; an unterminated one takes the rest of the page.
        let (tag, after) = match rest[lt + 1..].find('>') {
            Some(g) => (&rest[lt + 1..lt + 1 + g], lt + 2 + g),
            None => (&rest[lt + 1..], rest.len()),
        };
        rest = &rest[after..];
        let closing = tag.starts_with('/');
        let name: String = tag.trim_start_matches('/').chars().take_while(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
        if let Some(s) = &skip {
            if closing && name == *s {
                skip = None;
            }
            continue;
        }
        match name.as_str() {
            "head" | "style" | "script" | "title" if !closing => skip = Some(name),
            "br" => {
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push('\n');
            }
            "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "table" => {
                while out.ends_with(' ') {
                    out.pop();
                }
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            _ => {}
        }
    }
    out.trim().to_owned()
}

/// `CClientMOTD::Activate`'s message handling: a URL (`http://`, case-sensitive) is navigated
/// to and shows nothing until the page arrives; a page with the blocked patterns loads
/// nothing; anything else is written to `motd_temp.html` and rendered as HTML. The text is the
/// panel's own rendering, blank when the engine's browser shows the page.
fn motd_activate(msg: &str, eng: &mut dyn HudEngine) -> String {
    if msg.starts_with("http://") && msg.len() > 7 {
        eng.motd_navigate(msg);
        String::new()
    } else if msg.contains("img src=\"view-source:") || msg.contains("<style>;@/*") {
        String::new()
    } else {
        eng.motd_temp_file(Some(msg));
        if eng.motd_shows_pages() { String::new() } else { html_to_text(msg) }
    }
}

/// Greedy word wrap of `text` (with `\n` breaks) to `width` pixels.
fn wrap(screen: &mut Screen, font: FontId, text: &str, width: i32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
            if !line.is_empty() && screen.text_width(font, &candidate) > width {
                lines.push(std::mem::replace(&mut line, word.to_owned()));
            } else {
                line = candidate;
            }
        }
        lines.push(line);
    }
    lines
}

/// Draws `text` as-is in `font`; returns the advance.
pub(super) fn draw_text(screen: &mut Screen, font: FontId, x: i32, y: i32, text: &str, color: [u8; 4]) -> i32 {
    screen.set_text_color(f32::from(color[0]) / 255.0, f32::from(color[1]) / 255.0, f32::from(color[2]) / 255.0);
    let mut cx = x;
    for ch in text.chars().filter(|c| !c.is_control()) {
        cx += screen.vgui2_draw_character(cx, y, ch, font);
    }
    cx - x
}

/// A client-scheme colour; the fallbacks are the shipped `ClientScheme.res` values.
fn color(screen: &Screen, name: &str, fallback: [u8; 4]) -> [u8; 4] {
    screen.client_color(name).unwrap_or(fallback)
}

fn fill(screen: &mut Screen, (x, y, w, h): (i32, i32, i32, i32), c: [u8; 4]) {
    screen.fill_rgba_blend(x, y, w, h, i32::from(c[0]), i32::from(c[1]), i32::from(c[2]), i32::from(c[3]));
}

/// A 1-pixel outline.
fn outline(screen: &mut Screen, (x, y, w, h): (i32, i32, i32, i32), c: [u8; 4]) {
    fill(screen, (x, y, w, 1), c);
    fill(screen, (x, y + h - 1, w, 1), c);
    fill(screen, (x, y + 1, 1, h - 2), c);
    fill(screen, (x + w - 1, y + 1, 1, h - 2), c);
}

/// `.res` → `_TER.res` / `_CT.res` by team (`CBuySubMenu::OnCommand`).
fn team_file(file: &str, team: i32) -> String {
    file.replace(".res", if team == 1 { "_TER.res" } else { "_CT.res" })
}

/// A button of the open page as laid out this frame.
#[derive(Debug, Clone)]
struct Button {
    name: String,
    class: String,
    text: String,
    key: Option<char>,
    command: String,
    /// Screen pixels.
    rect: (i32, i32, i32, i32),
    enabled: bool,
    center: bool,
}

impl Button {
    fn contains(&self, (x, y): (i32, i32)) -> bool {
        let (bx, by, bw, bh) = self.rect;
        (bx..bx + bw).contains(&x) && (by..by + bh).contains(&y)
    }
}

/// The panel shown for the queue front.
#[derive(Debug, Clone)]
struct OpenMenu {
    id: i32,
    /// Screen position of the layout origin.
    origin: (i32, i32),
    controls: Vec<Control>,
    /// The `Message` / `MapInfo` pane text.
    body: String,
    /// The MOTD / briefing title.
    title: Option<String>,
    /// The info pane (`classes/<button>.res`) of the hovered `MouseOverPanelButton`.
    info: Option<(String, Vec<Control>)>,
}

/// The local state the panels gate on.
pub struct ViewState {
    /// `g_iTeamNumber`.
    pub local_team: i32,
    pub local_dead: bool,
    /// `g_iFreezeTimeOver`.
    pub freeze_over: bool,
    /// `CHudAccountBalance::GetBalance`.
    pub money: i32,
    /// `pfnGetLevelName` (`maps/<name>.bsp`).
    pub level_name: String,
}

/// What the viewport asks of the spectator GUI, handled in order by the next `OnTick`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SpecEvent {
    /// `HideSpectatorGUI` (a VGUI menu was displayed).
    Hide,
    /// `ShowSpectatorGUI` (`jointeam 6`).
    Show,
    /// `CSpecHelpWindow::OnClose`.
    HelpClosed,
}

/// `MakeSafeName`: `&` doubled, a leading `#` replaced by `*`.
pub(super) fn make_safe_name(name: &str) -> String {
    let mut out = name.replace('&', "&&");
    if out.starts_with('#') {
        out.replace_range(..1, "*");
    }
    out
}

/// `TeamFortressViewport` / `CounterStrikeViewport` state.
pub struct Viewport {
    /// `m_PendingDialogs`.
    queue: VecDeque<i32>,
    open: Option<OpenMenu>,
    /// `BackgroundPanel.res` (loaded with the first menu) and its layout for the last shown one.
    background_res: Option<String>,
    backdrop: Backdrop,
    server_name: String,
    /// `m_szMOTD`: the chunks' bytes, joined before decoding (a UTF-8 character may span two).
    motd: Vec<u8>,
    /// `m_iGotAllMOTD`: the last chunk ended the previous MOTD.
    got_all_motd: bool,
    allow_spectators: bool,
    /// `m_sMapName` (from `VGUIMenu 4`).
    map_name: String,
    /// `m_sTeamNames` with their `TeamScore`, in arrival order (index 0 = team 1).
    teams: Vec<(String, i32)>,
    /// `GetForceCamera` from `ForceCam`.
    pub force_camera: i32,
    pub(super) scoreboard: bool,
    cursor: (i32, i32),
    pub(super) spec_events: Vec<SpecEvent>,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
            open: None,
            background_res: None,
            backdrop: Backdrop::default(),
            server_name: String::new(),
            motd: Vec::new(),
            got_all_motd: true,
            allow_spectators: false,
            map_name: String::new(),
            teams: Vec::new(),
            force_camera: 0,
            scoreboard: false,
            cursor: (0, 0),
            spec_events: Vec::new(),
        }
    }
}

impl Viewport {
    /// `m_szServerName` (`IGameClientExports::GetServerHostName`).
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// `AllowedToPrintText`: false while the front menu is 2, 3, 5 or 6.
    pub fn allowed_to_print(&self) -> bool {
        !matches!(self.queue.front(), Some(2 | 3 | 5 | 6))
    }

    /// A VGUI menu is up or queued (`IsAnyVGUIMenuActive` for the menus built here).
    pub fn menu_active(&self) -> bool {
        !self.queue.is_empty()
    }

    /// `TeamScore` of `TERRORIST` and `CT`.
    pub fn team_scores(&self) -> (i32, i32) {
        let score = |name: &str| self.teams.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map_or(0, |(_, s)| *s);
        (score("TERRORIST"), score("CT"))
    }

    /// The `TeamScore` of the team named `name`, if one arrived.
    pub fn team_score(&self, name: &str) -> Option<i32> {
        self.teams.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, s)| *s)
    }

    /// `m_sTeamNames[1..]`, in arrival order (`GetNumberOfTeams` is their count).
    pub fn team_names(&self) -> Vec<String> {
        self.teams.iter().map(|(n, _)| n.clone()).collect()
    }

    /// A menu with mouse and keyboard input is up (the scoreboard has neither).
    pub fn cursor_visible(&self) -> bool {
        self.open.is_some() && !self.scoreboard
    }

    pub fn set_cursor(&mut self, x: i32, y: i32) {
        self.cursor = (x, y);
    }

    /// The viewport's user messages (§7.2); `false` for others.
    pub fn msg(&mut self, name: &str, r: &mut MsgReader, hud: &Hud, eng: &mut dyn HudEngine) -> bool {
        match name {
            "VGUIMenu" => {
                let menu = r.byte();
                if menu == 4 {
                    self.map_name = truncate_encoded(&r.string(), 63).into_owned();
                }
                self.show(menu, hud, eng);
            }
            "MOTD" => {
                let last = r.byte() != 0;
                if self.got_all_motd {
                    self.motd.clear();
                }
                self.got_all_motd = last;
                self.motd.extend_from_slice(r.string_bytes());
                // m_szMOTD[1536]: strncat of the server's bytes.
                self.motd.truncate(1535);
                // The text is kept; an HLTV client never shows it.
                if last && !eng.spectate_only() {
                    self.show(5, hud, eng);
                }
            }
            "ServerName" => self.server_name = truncate_encoded(&r.string(), 31).into_owned(),
            "AllowSpec" => self.allow_spectators = r.byte() != 0,
            "TeamScore" => {
                let team = r.string();
                let score = r.short();
                let found = self.teams.iter().position(|(n, _)| n.get(..team.len()).is_some_and(|p| p.eq_ignore_ascii_case(&team)));
                match found {
                    Some(i) => self.teams[i].1 = score,
                    // m_sTeamNames has 4 usable slots.
                    None if self.teams.len() < 4 => self.teams.push((team, score)),
                    None => log::debug!("TeamScore: no slot for team {team}"),
                }
            }
            "ForceCam" => {
                let (camera, chase, fade) = (r.byte(), r.byte(), r.byte());
                self.force_camera = if fade != 0 {
                    2
                } else if chase != 0 {
                    chase
                } else {
                    camera
                };
            }
            "BuyClose" => self.hide(28, hud, eng),
            // MsgFunc_SpecFade's shared empty body.
            "Spectator" => {}
            _ => return false,
        }
        true
    }

    /// `ShowVGUIMenu(id)`: nothing during a demo playback or (but the scoreboard) intermission;
    /// shown now when the queue is empty, else queued once.
    pub fn show(&mut self, id: i32, hud: &Hud, eng: &mut dyn HudEngine) {
        if eng.demo_playing_back() || (hud.intermission && id != 5) || self.queue.contains(&id) {
            return;
        }
        self.queue.push_back(id);
        if self.queue.len() == 1 {
            self.display(id, hud, eng);
        }
    }

    /// `CounterStrikeViewport::HideVGUIMenu(id)`; the queue advances in the same frame. The buy
    /// panel serves both 28 and 34.
    pub fn hide(&mut self, id: i32, hud: &Hud, eng: &mut dyn HudEngine) {
        let buy = |m: i32| m == 28 || m == 34;
        if self.open.as_ref().is_some_and(|m| m.id == id || (buy(id) && buy(m.id))) {
            self.close(hud, eng);
        }
    }

    /// `HideAllVGUIMenu` (from `OnLevelChange`): every panel hidden and the queue cleared.
    pub fn hide_all(&mut self) {
        self.open = None;
        self.queue.clear();
    }

    /// The MOTD's navigation finished: the page replaces the blank panel if it is still up.
    pub fn motd_page(&mut self, html: &str) {
        if let Some(menu) = self.open.as_mut().filter(|m| m.id == 5) {
            menu.body = html_to_text(html);
        }
    }

    /// The MOTD's `Message` control on screen while the MOTD is up (not swapped for the
    /// scoreboard), for a browser showing the page there.
    pub fn motd_rect(&self) -> Option<(i32, i32, i32, i32)> {
        if self.scoreboard {
            return None;
        }
        let menu = self.open.as_ref().filter(|m| m.id == 5)?;
        let c = menu.controls.iter().find(|c| c.class == "HTML" && c.visible)?;
        Some((menu.origin.0 + c.rect.0, menu.origin.1 + c.rect.1, c.rect.2, c.rect.3))
    }

    /// The front panel closed (`OnClose`; the buy menu unpauses, the spectator help brings
    /// the spectator GUI back): pop it and show the next.
    fn close(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        let closed = self.open.take().map(|m| m.id);
        if matches!(closed, Some(28 | 34)) {
            eng.server_cmd("unpause\n");
        }
        if closed == Some(9) {
            self.spec_events.push(SpecEvent::HelpClosed);
        }
        self.queue.pop_front();
        if let Some(&next) = self.queue.front() {
            self.display(next, hud, eng);
        }
    }

    /// `OnTick`: the hovered `MouseOverPanelButton`'s info pane.
    pub fn tick(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        if !self.cursor_visible() {
            return;
        }
        let state = hud.view_state(eng);
        let Some(menu) = &self.open else { return };
        let buttons = self.buttons(menu, &state, eng.screen());
        let hovered = buttons.iter().filter(|b| b.class == "MouseOverPanelButton").find(|b| b.contains(self.cursor)).map(|b| b.name.clone());
        let current = menu.info.as_ref().map(|(n, _)| n.clone());
        // The first button's pane shows on activate; a pane stays until another is entered.
        let wanted = hovered.or(current).or_else(|| buttons.iter().find(|b| b.class == "MouseOverPanelButton").map(|b| b.name.clone()));
        if let Some(name) = wanted
            && menu.info.as_ref().is_none_or(|(n, _)| *n != name)
        {
            let data = eng.read_file(&format!("classes/{name}.res")).or_else(|| eng.read_file("classes/default.res"));
            let screen = (eng.screen().width as i32, eng.screen().height as i32);
            let controls = data.map(|d| parse_layout(&decode_text(&d), screen).1).unwrap_or_default();
            if let Some(menu) = &mut self.open {
                menu.info = Some((name, controls));
            }
        }
    }

    /// `DisplayVGUIMenu(id)`.
    fn display(&mut self, id: i32, hud: &Hud, eng: &mut dyn HudEngine) {
        let state = hud.view_state(eng);
        let level = state.level_name.trim_start_matches("maps/").trim_end_matches(".bsp").to_owned();
        let map = if self.map_name.is_empty() { level.clone() } else { self.map_name.clone() };
        let read_text = |eng: &mut dyn HudEngine, file: &str| eng.read_file(file).map(|d| decode_text(&d));
        let (file, body, title) = match id {
            2 => {
                let info = read_text(eng, &format!("maps/{map}.txt")).or_else(|| read_text(eng, "maps/default.txt"));
                ("resource/UI/TeamMenu.res".to_owned(), info.unwrap_or_default(), None)
            }
            4 => {
                let text = read_text(eng, &format!("maps/{map}.txt")).unwrap_or_default();
                ("resource/UI/TextWindow.res".to_owned(), text, Some(truncate_encoded(&map, 31).into_owned()))
            }
            5 => {
                let title = if self.server_name.is_empty() { "Half-Life".to_owned() } else { self.server_name.clone() };
                ("resource/UI/MOTD.res".to_owned(), motd_activate(&gs_common::bytestr::decode(&self.motd), eng), Some(title))
            }
            // CSpecHelpWindow (a CVGUITextWindow).
            9 => {
                let localize = &eng.screen().localize;
                let text = localize.text("#Spec_Help_Text").to_owned();
                ("resource/UI/TextWindow.res".to_owned(), text, Some(localize.text("#Spec_Help_Title").to_owned()))
            }
            26 => ("resource/UI/ClassMenu_TER.res".to_owned(), String::new(), None),
            27 => ("resource/UI/ClassMenu_CT.res".to_owned(), String::new(), None),
            28 => {
                eng.server_cmd("setpause\n");
                ("resource/UI/MainBuyMenu.res".to_owned(), String::new(), None)
            }
            34 => {
                // Picked by the current team; the original keeps the team at CBuyMenu creation.
                eng.server_cmd("setpause\n");
                (team_file("resource/UI/BuyEquipment.res", state.local_team), String::new(), None)
            }
            _ => {
                // No panel: the next OnTick finds it inactive and pops it.
                log::debug!("VGUI menu {id} has no panel");
                self.queue.retain(|&q| q != id);
                if let Some(&next) = self.queue.front() {
                    self.display(next, hud, eng);
                }
                return;
            }
        };
        if self.background_res.is_none() {
            self.background_res = eng.read_file("resource/UI/BackgroundPanel.res").map(|d| decode_text(&d));
        }
        let Some(data) = eng.read_file(&file) else {
            log::warn!("VGUI menu {id}: {file} missing");
            self.queue.retain(|&q| q != id);
            return;
        };
        // CCSBackGroundPanel::Activate lays the backdrop out for the current screen; then
        // CenterWindow places the window by its offset.
        let screen = (eng.screen().width as i32, eng.screen().height as i32);
        self.backdrop = self.background_res.as_deref().map(|t| Backdrop::layout(t, screen)).unwrap_or_default();
        let (frame, controls) = parse_layout(&decode_text(&data), screen);
        let origin = self.backdrop.place((frame.0, frame.1));
        self.open = Some(OpenMenu { id, origin, controls, body, title, info: None });
        // Every panel shown hides the spectator GUI (HideSpectatorGUI).
        self.spec_events.push(SpecEvent::Hide);
    }

    /// The open page's visible buttons with their commands and enabled state (re-evaluated
    /// every layout pass, like `BuyMouseOverPanelButton::PerformLayout`).
    fn buttons(&self, menu: &OpenMenu, state: &ViewState, screen: &Screen) -> Vec<Button> {
        let level = state.level_name.to_ascii_lowercase();
        let as_map = level.starts_with("maps/as_");
        let de_map = level.starts_with("maps/de_");
        let mut out = Vec::new();
        for c in &menu.controls {
            if !matches!(c.class.as_str(), "Button" | "MouseOverPanelButton") {
                continue;
            }
            let name = c.name.to_ascii_lowercase();
            let visible = match (menu.id, name.as_str()) {
                (2, "specbutton") => self.allow_spectators && (state.local_team == 0 || !state.freeze_over || state.local_dead),
                (2, "vipbutton") => as_map && state.local_team == 2,
                (2, "cancelbutton") => state.local_team != 0,
                // czero-only models.
                (26 | 27, "militia" | "spetsnaz") => false,
                // g_iPlayerClass is never non-zero (§7.5).
                (26 | 27, "cancelbutton") => false,
                _ => c.visible,
            };
            if !visible {
                continue;
            }
            let command = match (menu.id, name.as_str()) {
                (28, "pistols") => "Resource/UI/BuyPistols.res".to_owned(),
                (28, "shotguns") => "Resource/UI/BuyShotguns.res".to_owned(),
                (28, "submachineguns") => "Resource/UI/BuySubMachineGuns.res".to_owned(),
                (28, "rifles") => "Resource/UI/BuyRifles.res".to_owned(),
                (28, "machineguns") => "Resource/UI/BuyMachineGuns.res".to_owned(),
                (28, "primaryammo") => "primammo".to_owned(),
                (28, "secammo") => "secammo".to_owned(),
                (28, "equipment") => "Resource/UI/BuyEquipment.res".to_owned(),
                (28, "autobuybutton") => "autobuy".to_owned(),
                (28, "rebuybutton") => "rebuy".to_owned(),
                (_, "cancelbutton") => "vguicancel".to_owned(),
                (5, "ok") => "okay".to_owned(),
                _ => c.command.clone(),
            };
            let enabled = !((c.cost != 0 && c.cost > state.money) || (as_map && c.as_restrict) || (!de_map && c.de_useonly));
            let (key, text) = hotkey(screen.localize.text(&c.label));
            let rect = (menu.origin.0 + c.rect.0, menu.origin.1 + c.rect.1, c.rect.2, c.rect.3);
            out.push(Button { name: c.name.clone(), class: c.class.clone(), text, key, command, rect, enabled, center: c.center });
        }
        out
    }

    /// A key press while a menu has the keyboard (the engine's VGUI2 key filter): hotkeys,
    /// the panels' `OnKeyCodePressed` defaults, and the `showscores` swap. `true` = consumed.
    pub fn key(&mut self, key: u8, binding: &str, hud: &Hud, eng: &mut dyn HudEngine) -> bool {
        if !self.cursor_visible() {
            return false;
        }
        let Some(id) = self.open.as_ref().map(|m| m.id) else { return false };
        let binding = binding.to_ascii_lowercase();
        if matches!(binding.as_str(), "+showscores" | "+score") && matches!(id, 2 | 5 | 26 | 27) {
            // The menu stays at the queue front and returns when the board closes.
            if eng.max_clients() > 1 {
                self.scoreboard = true;
            }
            return true;
        }
        let default = match (id, key) {
            (2, K_KP_ENTER) => Some("jointeam 5"),
            (2, _) if binding == "+jump" => Some("jointeam 5"),
            (26 | 27, K_KP_ENTER) => Some("joinclass 6"),
            (5, K_KP_ENTER | K_SPACE) => Some("okay"),
            _ => None,
        };
        if let Some(command) = default {
            if command != "okay" {
                eng.client_cmd(&format!("{command}\n"));
            } else if id == 5 {
                // CClientMOTD::OnCommand("okay"): the temp page goes.
                eng.motd_temp_file(None);
            }
            self.close(hud, eng);
            return true;
        }
        let ch = char::from(key).to_ascii_lowercase();
        if key < 128 && ch.is_ascii_alphanumeric() {
            let state = hud.view_state(eng);
            let button = self.open.as_ref().and_then(|m| self.buttons(m, &state, eng.screen()).into_iter().find(|b| b.enabled && b.key == Some(ch)));
            if let Some(b) = button {
                self.press(&b, hud, eng);
            }
        }
        true
    }

    /// `IViewPort::SlotInput(slot)` (0-based): while a menu has input, `slotN` presses the
    /// button whose hotkey is digit N (slot10 → 0). `true` = consumed.
    pub fn slot_input(&mut self, slot: usize, hud: &Hud, eng: &mut dyn HudEngine) -> bool {
        if !self.cursor_visible() {
            return false;
        }
        let key = char::from_digit(((slot + 1) % 10) as u32, 10);
        let state = hud.view_state(eng);
        let button = self.open.as_ref().and_then(|m| self.buttons(m, &state, eng.screen()).into_iter().find(|b| b.enabled && b.key == key));
        if let Some(b) = button {
            self.press(&b, hud, eng);
        }
        true
    }

    /// A left click at screen `(x, y)`; `true` when a menu took it.
    pub fn click(&mut self, x: i32, y: i32, hud: &Hud, eng: &mut dyn HudEngine) -> bool {
        if !self.cursor_visible() {
            return false;
        }
        let state = hud.view_state(eng);
        let button = self.open.as_ref().and_then(|m| self.buttons(m, &state, eng.screen()).into_iter().find(|b| b.enabled && b.contains((x, y))));
        if let Some(b) = button {
            self.press(&b, hud, eng);
        }
        true
    }

    /// The open panel's `OnCommand`.
    fn press(&mut self, b: &Button, hud: &Hud, eng: &mut dyn HudEngine) {
        let Some(id) = self.open.as_ref().map(|m| m.id) else { return };
        if (id == 28 || id == 34) && b.command.to_ascii_lowercase().contains(".res") {
            // CBuySubMenu: the team's sub-page, then wizard "next".
            let file = team_file(&b.command, hud.view_state(eng).local_team);
            match eng.read_file(&file) {
                Some(data) => {
                    let screen = (eng.screen().width as i32, eng.screen().height as i32);
                    let (_, controls) = parse_layout(&decode_text(&data), screen);
                    if let Some(menu) = &mut self.open {
                        menu.controls = controls;
                        menu.info = None;
                    }
                }
                None => log::warn!("buy menu page {file} missing"),
            }
            return;
        }
        if id == 28 || id == 34 {
            eng.server_cmd("client_buy_close\n");
        }
        // CCSTeamMenu::OnCommand: jointeam 6 also brings up the spectator GUI.
        if id == 2 && b.command.eq_ignore_ascii_case("jointeam 6") {
            self.spec_events.push(SpecEvent::Show);
        }
        if id == 5 && b.command == "okay" {
            eng.motd_temp_file(None);
        }
        self.close(hud, eng);
        if b.command != "vguicancel" && b.command != "okay" {
            eng.client_cmd(&format!("{}\n", b.command));
        }
    }

    /// `__CmdFunc_ClientBuy` (`buy`): needs the buy zone and a living player.
    pub fn buy(&mut self, hud: &Hud, old_style: bool, eng: &mut dyn HudEngine) {
        if !hud.in_buy_zone() || hud.view_state(eng).local_dead {
            return;
        }
        if old_style {
            eng.server_cmd("buy\n");
        } else {
            self.show(28, hud, eng);
            eng.server_cmd("client_buy_open\n");
        }
    }

    pub fn draw(&self, hud: &Hud, eng: &mut dyn HudEngine) {
        if self.scoreboard {
            self.draw_scoreboard(hud, eng);
        } else if let Some(menu) = &self.open {
            let state = hud.view_state(eng);
            self.draw_menu(menu, &state, eng);
        }
    }

    /// `CCSBackGroundPanel::PaintBackground` (hardware path), then its children: `ViewportBG`
    /// around the rounded corners and down to and from the title gap, which stays clear (a
    /// centred 4:3 box, not a full-screen dim); the corner images tinted 255 255 255 200.
    fn draw_backdrop(&self, eng: &mut dyn HudEngine) {
        let b = &self.backdrop;
        let corners = ["TopLeftPanel", "TopRightPanel", "BottomLeftPanel", "BottomRightPanel"].map(|n| b.rect(n));
        if let ([Some(tl), Some(tr), Some(bl), Some(br)], Some(gap)) = (corners, b.rect("GapPanel")) {
            let screen = eng.screen();
            let bg = color(screen, "ViewportBG", [0, 0, 0, 200]);
            let mut span = |(x0, y0): (i32, i32), (x1, y1): (i32, i32)| fill(screen, (x0, y0, x1 - x0, y1 - y0), bg);
            if gap.2 > gap.3 {
                span((tl.0 + tl.2, tl.1), (tr.0, tr.1 + tr.3));
                span((tl.0, tl.1 + tl.3), (tr.0 + tr.2, gap.1));
                span((gap.0, gap.1 + gap.3), (br.0 + br.2, br.1));
                span((bl.0 + bl.2, bl.1), (br.0, br.1 + br.3));
            } else {
                // A gap taller than wide (no shipped layout): one fill corner to corner, as the
                // software path paints.
                span((tl.0, tl.1), (br.0 + br.2, br.1 + br.3));
            }
            let images = ["gfx/vgui/round_corner_nw", "gfx/vgui/round_corner_ne", "gfx/vgui/round_corner_sw", "gfx/vgui/round_corner_se"];
            for (rect, image) in [tl, tr, bl, br].into_iter().zip(images) {
                if let Some(img) = eng.load_image(image) {
                    eng.screen().draw_image(img, rect.0, rect.1, rect.2, rect.3, [255, 255, 255, 200]);
                }
            }
        }
        for c in &b.controls {
            draw_control(c, (0, 0), None, eng);
        }
    }

    fn draw_menu(&self, menu: &OpenMenu, state: &ViewState, eng: &mut dyn HudEngine) {
        // CCSBackGroundPanel behind every menu.
        self.draw_backdrop(eng);
        for c in &menu.controls {
            let text = match c.class.as_str() {
                "Label" if c.name.eq_ignore_ascii_case("serverName") => menu.title.clone(),
                "HTML" | "TextEntry" => Some(menu.body.clone()),
                _ => None,
            };
            match c.class.as_str() {
                "Button" | "MouseOverPanelButton" => {}
                // The info pane container is hidden in the class menu layouts; the hovered
                // button's pane is shown inside it.
                "Panel" if c.name.eq_ignore_ascii_case("ClassInfo") || c.name.eq_ignore_ascii_case("ItemInfo") => {
                    if let Some((_, info)) = &menu.info {
                        let at = (menu.origin.0 + c.rect.0, menu.origin.1 + c.rect.1);
                        for ic in info.iter().filter(|c| c.visible) {
                            draw_control(ic, at, None, eng);
                        }
                    }
                }
                _ if c.visible => draw_control(c, menu.origin, text.as_deref(), eng),
                _ => {}
            }
        }
        // A text window's title is its frame title (no label in TextWindow.res).
        if menu.id == 4
            && let Some(title) = &menu.title
        {
            let screen = eng.screen();
            let sh = screen.height as i32;
            let font = screen.client_font_proportional("Title");
            let c = color(screen, "TitleText", [255, 174, 0, 255]);
            draw_text(screen, font, menu.origin.0 + psv(170, sh), menu.origin.1 + psv(40, sh), title, c);
        }
        let screen = eng.screen();
        let buttons = self.buttons(menu, state, screen);
        let font = screen.client_font_proportional("Default");
        let tall = screen.font_tall(font);
        let fg = color(screen, "FgColor", [255, 176, 0, 255]);
        let disabled = color(screen, "DisabledFgColor1", [80, 48, 0, 255]);
        let bg = color(screen, "ButtonBgColor", [0, 0, 0, 64]);
        let armed = color(screen, "ButtonArmedBgColor", [255, 176, 0, 100]);
        let border = color(screen, "BorderBright", [188, 112, 0, 128]);
        for b in &buttons {
            let hover = b.enabled && b.contains(self.cursor);
            fill(screen, b.rect, if hover { armed } else { bg });
            outline(screen, b.rect, border);
            let (x, y, bw, bh) = b.rect;
            let tx = if b.center { x + (bw - screen.text_width(font, &b.text)) / 2 } else { x + 6 };
            draw_text(screen, font, tx, y + (bh - tall) / 2, &b.text, if b.enabled { fg } else { disabled });
        }
    }

    /// `CCSClientScoreBoardDialog` (§7.7). The dialog is proportional (`SetProportional(true)`
    /// before `ScoreBoard.res` loads), so the `.res` geometry, the column widths and the line
    /// spacing are `GetProportionalScaledValue`d by the screen height, matching the fonts the
    /// scheme picks by height. Its `SectionedListPanel` lays out from (5, 5): every visible
    /// section is a `DefaultVerySmall` header (tall + 7, divider 2 px above its bottom), then its
    /// rows (`OverrideLineSpacing`), then 8 px.
    fn draw_scoreboard(&self, hud: &Hud, eng: &mut dyn HudEngine) {
        let max = eng.max_clients();
        let local = hud.local_index(eng);
        let local_team = local.map_or(0, |l| hud.players[l].teamnumber);
        let local_dead = local.is_some_and(|l| hud.players[l].dead);
        let mut rows: Vec<Row> = Vec::new();
        // Slot 64 is never shown.
        for i in 1..(max.clamp(0, 64) as usize).min(hud.players.len()) {
            let Some(info) = eng.player_info(i as i32) else { continue };
            let p = &hud.players[i];
            let section = if p.teamnumber == 1 || p.teamnumber == 2 { p.teamnumber } else { 3 };
            let attr = if p.dead {
                "#Cstrike_DEAD"
            } else if p.has_c4 && (local_team <= 1 || local_dead) {
                "#Cstrike_BOMB"
            } else if p.vip && (local_team == 0 || local_team == 2 || local_dead) {
                "#Cstrike_VIP"
            } else if p.has_defuse_kit && (local_team == 0 || local_team == 2 || local_dead) {
                "#Cstrike_DEFUSE_KIT"
            } else {
                ""
            };
            let name = make_safe_name(&info.name);
            rows.push(Row { section, index: i, name, attr: attr.to_owned(), me: info.thisplayer, ping: info.ping, bot: info.bot });
        }
        rows.sort_by(|a, b| {
            let (pa, pb) = (&hud.players[a.index], &hud.players[b.index]);
            a.section.cmp(&b.section).then(pb.frags.cmp(&pa.frags)).then(pa.deaths.cmp(&pb.deaths)).then(a.index.cmp(&b.index))
        });
        let on = |name: &str| eng.cvar(name) as i32 != 0;
        let show_health = on("scoreboard_showhealth") && hud.players.iter().any(|p| p.health >= 0);
        let show_money = on("scoreboard_showmoney") && hud.players.iter().any(|p| p.account >= 0);
        let short = on("scoreboard_shortheaders");
        let show_avatars = on("scoreboard_showavatars");
        let screen = eng.screen();
        let sh = screen.height as i32;
        let p = |v: i32| super::specgui::psv(v, sh);
        let n = rows.len();
        // UpdatePlayerInfo's OverrideLineSpacing (scaled on a proportional list).
        let row_h = p(match n {
            0..=17 => 21,
            18..=23 => 15,
            _ => 11,
        });
        // UpdateOptionalColumns: the avatar column keeps its (unscaled) width without Steam;
        // the name column gets the rest of its scaled base.
        let avatar = if show_avatars {
            let (few, some, many) = if sh <= 599 {
                (36, 26, 19)
            } else if sh < 768 {
                (40, 28, 21)
            } else {
                (56, 38, 29)
            };
            match n {
                0..=17 => few,
                18..=23 => some,
                _ => many,
            }
        } else {
            0
        };
        let name_base = match (show_health, short) {
            (true, false) => 209,
            (true, true) => 239,
            (false, false) => 250,
            (false, true) => 274,
        } + if show_money { 0 } else { 57 };
        // avatar, name, spacer1, class, health, account, frags, deaths, ping; the last five
        // are COLUMN_RIGHT.
        let widths = [
            avatar,
            p(name_base) - avatar,
            5,
            p(56),
            if show_health { p(if short { 38 } else { 41 }) } else { 0 },
            if show_money { p(57) } else { 0 },
            p(if short { 38 } else { 45 }),
            p(if short { 38 } else { 46 }),
            p(if short { 38 } else { 50 }),
        ];
        let mut col = [0; 9];
        for i in 1..9 {
            col[i] = col[i - 1] + widths[i - 1];
        }
        let font = screen.client_font_proportional("Default");
        let head_font = screen.client_font_proportional("DefaultVerySmall");
        let name_fallback = screen.client_font_proportional("DefaultVerySmallFallBack");
        let head_tall = screen.font_tall(head_font);
        let sec_tall = head_tall + 7;
        let members = |s: i32| rows.iter().filter(|r| r.section == s).count() as i32;
        // LayoutPanels' content height: the always-visible column-title section, then each
        // populated team section.
        let content = 5 + sec_tall + 8 + [1, 2, 3].iter().filter(|&&s| members(s) > 0).map(|&s| sec_tall + members(s) * row_h + 8).sum::<i32>();
        let (x0, y0, width) = (screen.width as i32 / 2 + p(-260), p(48), p(520));
        let height = p(340).max(content);
        fill(screen, (x0, y0, width, height), color(screen, "ListBG", [0, 0, 0, 128]));
        let localize = |screen: &Screen, s: &str| screen.localize.text(s).to_owned();
        // The bright ServerName label (3, 2), north-west.
        let server = format!(" {}", self.server_name);
        draw_text(screen, font, x0 + p(3), y0 + p(2), &server, color(screen, "BrightControlText", [255, 176, 0, 255]));
        // List children sit at x = 5, 10 px narrower than the list.
        let (lx, lw) = (x0 + 5, width - 10);
        let divider = color(screen, "SectionDividerColor", [188, 112, 0, 128]);
        // A cell: left-aligned at its column, COLUMN_RIGHT ones against its right edge; text is
        // centred vertically in the row.
        let cell = |screen: &mut Screen, f: FontId, i: usize, top: i32, h: i32, text: &str, c: [u8; 4]| {
            let ty = top + (h - screen.font_tall(f)) / 2;
            let x = if i >= 4 { lx + col[i] + widths[i] - screen.text_width(f, text) } else { lx + col[i] };
            draw_text(screen, f, x, ty, text, c);
        };
        let header = |screen: &mut Screen, top: i32| fill(screen, (lx + 1, top + sec_tall - 2, lw - 3, 1), divider);
        // Section 0: the column titles.
        let mut y = y0 + 5;
        let title = |screen: &Screen, full: &str, letter: &str| if short { letter.to_owned() } else { localize(screen, full) };
        let head_color = color(screen, "SectionTextColor", [255, 176, 0, 255]);
        for (i, show, full, letter) in [
            (4, show_health, "#CStrike_HEALTH", "H"),
            (5, show_money, "#CStrike_ACCOUNT", "$"),
            (6, true, "#PlayerScore", "S"),
            (7, true, "#PlayerDeath", "D"),
            (8, true, "#PlayerPing", "P"),
        ] {
            if show {
                let t = title(screen, full, letter);
                cell(screen, head_font, i, y, sec_tall, &t, head_color);
            }
        }
        header(screen, y);
        y += sec_tall + 8;
        for section in [1, 2, 3] {
            let members: Vec<&Row> = rows.iter().filter(|r| r.section == section).collect();
            if members.is_empty() {
                continue;
            }
            let c = match section {
                1 => [255, 64, 64, 255],
                2 => [153, 204, 255, 255],
                _ => [204, 204, 204, 255],
            };
            // UpdateTeamInfo: team name, player count, team score and average ping.
            let count = members.len();
            let team = match section {
                1 => localize(screen, "#Cstrike_ScoreBoard_Ter"),
                2 => localize(screen, "#Cstrike_ScoreBoard_CT"),
                _ => localize(screen, "#Spectators"),
            };
            let name = if section == 3 {
                team
            } else {
                let fmt = localize(screen, if count == 1 { "#Cstrike_ScoreBoard_Player" } else { "#Cstrike_ScoreBoard_Players" });
                construct(&fmt, &[team, count.to_string()])
            };
            cell(screen, head_font, 1, y, sec_tall, &name, c);
            if section != 3 {
                let wanted = if section == 1 { "TERRORIST" } else { "CT" };
                if let Some((_, score)) = self.teams.iter().find(|(n, _)| n.eq_ignore_ascii_case(wanted)) {
                    cell(screen, head_font, 6, y, sec_tall, &score.to_string(), c);
                }
                let pings: Vec<i32> = members.iter().map(|r| r.ping).collect();
                let avg = pings.iter().sum::<i32>() / pings.len().max(1) as i32;
                if avg > 0 {
                    cell(screen, head_font, 8, y, sec_tall, &avg.to_string(), c);
                }
            }
            header(screen, y);
            y += sec_tall;
            for row in members {
                let p = &hud.players[row.index];
                let rc = if section == 3 {
                    [204, 204, 204, 255]
                } else {
                    let t = team_color(p.teamnumber);
                    [t[0] as u8, t[1] as u8, t[2] as u8, 255]
                };
                // The local player's row is the selected item of an unfocused list.
                if row.me {
                    fill(screen, (lx, y, lw, row_h), color(screen, "SelectionBG2", [255, 255, 255, 25]));
                }
                // The name's TextImage falls back to DefaultVerySmallFallBack when it overflows.
                let name_font = if screen.text_width(font, &row.name) > widths[1] - 2 { name_fallback } else { font };
                cell(screen, name_font, 1, y, row_h, &row.name, rc);
                let attr = localize(screen, &row.attr);
                cell(screen, font, 3, y, row_h, &attr, rc);
                // Spectator rows show only name and class.
                if section != 3 {
                    if show_health && p.health >= 0 && !p.dead {
                        cell(screen, font, 4, y, row_h, &p.health.to_string(), rc);
                    }
                    if show_money && p.account >= 0 {
                        cell(screen, font, 5, y, row_h, &format!("${}", p.account), rc);
                    }
                    cell(screen, font, 6, y, row_h, &p.frags.to_string(), rc);
                    cell(screen, font, 7, y, row_h, &p.deaths.to_string(), rc);
                    let ping = if row.ping > 0 {
                        row.ping.to_string()
                    } else if row.bot {
                        "BOT".to_owned()
                    } else {
                        String::new()
                    };
                    cell(screen, font, 8, y, row_h, &ping, rc);
                }
                y += row_h;
            }
            y += 8;
        }
    }
}

/// Paints one non-button control at `origin`: label text (`text` overrides the label), body
/// panes, images, fills and dividers.
fn draw_control(c: &Control, origin: (i32, i32), text: Option<&str>, eng: &mut dyn HudEngine) {
    let rect = (origin.0 + c.rect.0, origin.1 + c.rect.1, c.rect.2, c.rect.3);
    match c.class.as_str() {
        "Label" | "HTML" | "TextEntry" => {
            let screen = eng.screen();
            let font = screen.client_font_proportional(if c.font.is_empty() { "Default" } else { &c.font });
            let tall = screen.font_tall(font).max(1);
            let fg = if c.dull {
                color(screen, "LabelDimText", [255, 176, 0, 164])
            } else if c.bright {
                color(screen, "BrightControlText", [255, 176, 0, 255])
            } else {
                color(screen, "FgColor", [255, 176, 0, 255])
            };
            let text = text.map_or_else(|| screen.localize.text(&c.label).to_owned(), str::to_owned);
            let lines = wrap(screen, font, &text, rect.2);
            let body = c.class != "Label";
            let total = lines.len() as i32 * tall;
            let mut y = if body || c.north { rect.1 } else { rect.1 + (rect.3 - total) / 2 };
            for line in &lines {
                if body && y + tall > rect.1 + rect.3 {
                    break;
                }
                let x = if c.center { rect.0 + (rect.2 - screen.text_width(font, line)) / 2 } else { rect.0 };
                draw_text(screen, font, x, y, line, fg);
                y += tall;
            }
        }
        "ImagePanel" | "BitmapImagePanel" => {
            if !c.fill_color.is_empty() {
                let screen = eng.screen();
                let c = color(screen, &c.fill_color, [0, 0, 0, 200]);
                fill(screen, rect, c);
            }
            if !c.image.is_empty()
                && let Some(image) = eng.load_image(&c.image)
            {
                let screen = eng.screen();
                let tint = if c.image_color.is_empty() { [255; 4] } else { color(screen, &c.image_color, [255; 4]) };
                screen.draw_image(image, rect.0, rect.1, rect.2, rect.3, tint);
            }
        }
        "Divider" => {
            let screen = eng.screen();
            let c = color(screen, "BorderDark", [188, 112, 0, 128]);
            outline(screen, rect, c);
        }
        // The background's separator line.
        "Panel" if c.name.eq_ignore_ascii_case("GapPanel") => {
            let screen = eng.screen();
            let c = color(screen, "BorderBright", [188, 112, 0, 128]);
            fill(screen, rect, c);
        }
        _ => {}
    }
}

/// One scoreboard row.
struct Row {
    /// 1 T, 2 CT, 3 spectators/unassigned.
    section: i32,
    index: usize,
    name: String,
    attr: String,
    me: bool,
    ping: i32,
    bot: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_read_controls_and_hotkeys() {
        let team_menu = "\"Resource/UI/TeamMenu.res\" { \"TeamMenu\" { \"ControlName\" \"Frame\" \"xpos\" \"76\" \"ypos\" \"0\" \"wide\" \"552\" \"tall\" \"448\" }\n\
             \"terbutton\" { \"ControlName\" \"Button\" \"xpos\" \"0\" \"ypos\" \"116\" \"wide\" \"148\" \"tall\" \"20\" \"visible\" \"1\" \"labelText\" \"#Cstrike_Terrorist_Forces\" \"command\" \"jointeam 1\" } }";
        let (frame, controls) = parse_layout(team_menu, (640, 480));
        assert_eq!(frame, (76, 0, 552, 448));
        assert_eq!(controls.len(), 1);
        assert_eq!((controls[0].rect, controls[0].command.as_str()), ((0, 116, 148, 20), "jointeam 1"));
        // Proportional: every value scaled by SH/480 (×3 at 1440 lines), whatever the width.
        let (frame, controls) = parse_layout(team_menu, (2560, 1440));
        assert_eq!((frame, controls[0].rect), ((228, 0, 1656, 1344), (0, 348, 444, 60)));
        assert_eq!(hotkey("&1 TERRORIST FORCES"), (Some('1'), "1 TERRORIST FORCES".to_owned()));
        assert_eq!(hotkey("&OK"), (Some('o'), "OK".to_owned()));
        assert_eq!(team_file("Resource/UI/BuyRifles.res", 1), "Resource/UI/BuyRifles_TER.res");
        assert_eq!(team_file("Resource/UI/BuyRifles.res", 2), "Resource/UI/BuyRifles_CT.res");
    }

    /// The backdrop keeps a 4:3 box centred on any screen and every window moves with it:
    /// on 16:9 the box is as tall as the screen, on 5:4 as wide.
    #[test]
    fn backdrop_centres_a_4_3_box_and_offsets_the_windows() {
        let bg = "\"Resource/UI/BackgroundPanel.res\" { \"ViewPortBackGround\" { \"ControlName\" \"Frame\" \"xpos\" \"0\" \"ypos\" \"0\" \"wide\" \"640\" \"tall\" \"480\" }\n\
                  \"TopLeftPanel\" { \"ControlName\" \"Panel\" \"xpos\" \"20\" \"ypos\" \"20\" \"wide\" \"10\" \"tall\" \"10\" } }";
        let wide = Backdrop::layout(bg, (2560, 1440));
        assert_eq!((wide.offset, wide.rect("TopLeftPanel")), ((320, 0), Some((380, 60, 30, 30))));
        assert_eq!(wide.place((228, 0)), (548, 0));
        let tall = Backdrop::layout(bg, (1280, 1024));
        // PSV(480) is 1023 here: (double)1024/480·480 truncates.
        assert_eq!((tall.offset, tall.rect("TopLeftPanel")), ((1, 33), Some((39, 71, 18, 18))));
        assert_eq!(tall.place((162, 0)), (163, 22), "CenterWindow takes 0.67 of the vertical offset");
        let native = Backdrop::layout(bg, (640, 480));
        assert_eq!((native.offset, native.rect("TopLeftPanel")), ((0, 0), Some((20, 20, 10, 10))));
    }

    #[test]
    fn motd_urls_navigate_pages_go_through_the_temp_file_and_blocked_ones_load_nothing() {
        let mut eng = crate::hud::tests::Engine::new();
        let page = "<!DOCTYPE HTML>\n<html><head><title>Cstrike MOTD</title><style>body { color: #FFB000; }</style></head>\n\
                    <body>\nYou are playing Counter-Strike v1.6<br>\nVisit   the site &amp; more<br><a href=\"x\">link</a></body></html>";
        assert_eq!(motd_activate(page, &mut eng), "You are playing Counter-Strike v1.6\nVisit the site & more\nlink");
        assert_eq!(motd_activate("http://example.com/motd", &mut eng), "", "blank until the page arrives");
        assert_eq!(motd_activate("https://example.com/motd", &mut eng), "https://example.com/motd", "only http:// navigates");
        assert_eq!(motd_activate("<img src=\"view-source:x\">", &mut eng), "");
        assert_eq!(eng.motd, [format!("temp {page}"), "navigate http://example.com/motd".to_owned(), "temp https://example.com/motd".to_owned()]);
        assert_eq!(decode_text(&[0xFF, 0xFE, b'h', 0, b'i', 0, b'\r', 0, b'\n', 0]), "hi\n");
    }

    /// A server's page ending inside a tag whose last character is multi-byte crashed the client.
    #[test]
    fn motd_text_survives_an_unterminated_tag_ending_in_a_multibyte_character() {
        assert_eq!(html_to_text("ab<x\u{e9}"), "ab");
        assert_eq!(html_to_text("ab<br\u{10fffd}cd"), "ab");
        assert_eq!(html_to_text("a<br>b<"), "a\nb");
    }
}
