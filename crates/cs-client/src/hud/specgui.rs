//! The spectator GUI (docs/re/spectator-gui.md): `CCSSpectatorGUI`'s top bar (team scores,
//! round timer, map / money / HLTV spectator count, the HLTV banner) and blank bottom strip
//! with the observed player's label; the `CBottomBar` opened by DUCK (the player list, the
//! prev/next buttons, the camera-mode and options combos built from `spectcammenu.txt` /
//! `spectatormenu.txt` with the command-menu parser); the PIP outline; and the viewport's
//! show/hide state machine around them. Geometry comes from `Spectator.res` /
//! `BottomSpectator.res` with the vgui2 proportional rules; the widgets (ComboBox with its
//! TextEntry body and drop-down Menu, Button, ScrollBar) behave and paint as the RE notes
//! describe.
//!
//! Which keys vgui2 got while the bar is open is open in the RE (§10.1): here the focused
//! widget's keys, DUCK and the change-team key are taken, other keys run their bindings.

use gs_console::keys::{K_DOWNARROW, K_ENTER, K_ESCAPE, K_KP_ENTER, K_LEFTARROW, K_RIGHTARROW, K_SPACE, K_UPARROW};
use gs_draw::{FontId, Screen};
use gs_formats::keyvalues;

use super::chat::construct;
use super::cmdmenu::{self, Action};
use super::health::team_color;
use super::viewport::make_safe_name;
use super::{Hud, HudEngine};

/// The bar strips and drop-down menus (`0xC4000000`).
const BAR_BG: [i32; 4] = [0, 0, 0, 196];
/// `WindowBG`: combo bodies and their arrow buttons.
const COMBO_BG: [i32; 4] = [0, 0, 0, 200];
/// `ButtonBG`: prev/next idle and depressed.
const BUTTON_BG: [i32; 4] = [0, 0, 0, 64];
/// `SelectionBG`: armed buttons and items, the focused text selection.
const ARMED_BG: [i32; 4] = [255, 176, 0, 100];
/// `BaseText` / `SelectedText`.
const TEXT: [u8; 3] = [255, 176, 0];
/// `BorderBright` / `BorderDark`.
const BORDER: [i32; 4] = [188, 112, 0, 128];
/// `ButtonFocusBorder`.
const FOCUS_DASH: [i32; 4] = [64, 48, 0, 255];
/// `ControlDarkBG`: the scrollbar slider.
const SLIDER_BG: [i32; 4] = [0, 0, 0, 128];
/// Drop-down rows and the scrollbar are not proportional.
const ITEM_TALL: i32 = 22;
const SCROLLBAR: i32 = 18;
/// `ComboBox(…, numLines 10)`: the player list scrolls past this.
const PLAYER_LINES: usize = 10;
/// The `+5` right strip of every menu (`PSV(5)` of a non-proportional panel).
const MENU_STRIP: i32 = 5;

/// Border lines as `(start, end)` offsets of the left, top, right and bottom sides.
const COMBO_BORDER: [(i32, i32); 4] = [(0, 1), (0, 0), (1, 0), (0, 0)];
const BUTTON_BORDER: [(i32, i32); 4] = [(0, 1), (1, 1), (0, 0), (0, 0)];

/// `GetProportionalScaledValue`: `v` scaled by the screen height over 480 lines, truncated.
pub(super) fn psv(v: i32, sh: i32) -> i32 {
    (f64::from(sh) / f64::from(sh.min(480)) * f64::from(v)) as i32
}

/// `atoi`.
pub(super) fn atoi(s: &str) -> i32 {
    let s = s.trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let v = digits.bytes().take_while(u8::is_ascii_digit).fold(0i32, |a, c| a.wrapping_mul(10).wrapping_add(i32::from(c - b'0')));
    if neg { v.wrapping_neg() } else { v }
}

/// `COM_FileBase`: the name without directories and extension.
fn file_base(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rfind('.').map_or(name, |dot| &name[..dot]).to_owned()
}

/// `GetSpectatorLabel(mode)`.
fn spectator_label(mode: i32) -> &'static str {
    match mode {
        1 => "#OBS_CHASE_LOCKED",
        2 => "#OBS_CHASE_FREE",
        3 => "#OBS_ROAMING",
        4 => "#OBS_IN_EYE",
        5 => "#OBS_MAP_FREE",
        6 => "#OBS_MAP_CHASE",
        _ => "#OBS_NONE",
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    fn contains(self, (px, py): (i32, i32)) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// One `.res` block.
#[derive(Debug, Clone, Default)]
struct ResCtl {
    name: String,
    class: String,
    xpos: String,
    ypos: String,
    wide: Option<i32>,
    tall: Option<i32>,
    label: String,
    align: String,
    image: String,
    fill: String,
    font: String,
    visible: bool,
}

fn load_res(eng: &mut dyn HudEngine, path: &str) -> Option<Vec<ResCtl>> {
    let Some(data) = eng.read_file(path) else {
        log::warn!("{path} missing");
        return None;
    };
    let kv = keyvalues::parse(&String::from_utf8_lossy(&data));
    let (_, root) = kv.blocks().next()?;
    Some(
        root.blocks()
            .map(|(name, b)| {
                let s = |k: &str| b.last_string(k).unwrap_or_default().to_owned();
                let int = |k: &str| b.last_string(k).map(atoi);
                ResCtl {
                    name: name.to_owned(),
                    class: s("ControlName"),
                    xpos: s("xpos"),
                    ypos: s("ypos"),
                    wide: int("wide"),
                    tall: int("tall"),
                    label: s("labelText"),
                    align: s("textAlignment").to_ascii_lowercase(),
                    image: s("image"),
                    fill: s("fillcolor"),
                    font: s("font"),
                    visible: b.last_string("visible").is_none_or(|v| atoi(v) != 0),
                }
            })
            .collect(),
    )
}

/// `xpos`/`ypos` (`Panel::ApplySettings`): `n` → `PSV(n)`, `rn` → `extent − PSV(n)`,
/// `cn` → `extent/2 + PSV(n)`, against the screen; a missing key keeps 0.
fn res_pos(spec: &str, extent: i32, sh: i32) -> i32 {
    let s = spec.trim();
    if let Some(v) = s.strip_prefix(['r', 'R']) {
        extent - psv(atoi(v), sh)
    } else if let Some(v) = s.strip_prefix(['c', 'C']) {
        extent / 2 + psv(atoi(v), sh)
    } else {
        psv(atoi(s), sh)
    }
}

/// A label's text after the `&` rules (a single `&` hidden, `&&` = `&`, `\r` dropped).
fn amp(text: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                out.push('&');
            }
            '&' | '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

fn fill(screen: &mut Screen, r: Rect, c: [i32; 4]) {
    if r.w > 0 && r.h > 0 {
        screen.fill_rgba_blend(r.x, r.y, r.w, r.h, c[0], c[1], c[2], c[3]);
    }
}

/// A vgui2 line border (`Border::Paint`): left, top, right, bottom.
fn border(screen: &mut Screen, r: Rect, off: [(i32, i32); 4], c: [i32; 4]) {
    let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
    fill(screen, Rect::new(x0, y0 + off[0].0, 1, y1 - off[0].1 - y0 - off[0].0), c);
    fill(screen, Rect::new(x0 + off[1].0, y0, x1 - off[1].1 - x0 - off[1].0, 1), c);
    fill(screen, Rect::new(x1 - 1, y0 + off[2].0, 1, y1 - off[2].1 - y0 - off[2].0), c);
    fill(screen, Rect::new(x0 + off[3].0, y1 - 1, x1 - off[3].1 - x0 - off[3].0, 1), c);
}

/// `Label::DrawDashedLine` (screen coordinates, `x1`/`y1` exclusive).
fn dashed(screen: &mut Screen, (x0, y0, x1, y1): (i32, i32, i32, i32), dash: i32, gap: i32) {
    let step = (dash + gap).max(1);
    if x1 - x0 <= y1 - y0 {
        let mut y = y0;
        loop {
            fill(screen, Rect::new(x0, y, x1 - x0, (y + dash).min(y1) - y), FOCUS_DASH);
            if y + step > y1 {
                break;
            }
            y += step;
        }
    } else {
        let mut x = x0;
        loop {
            fill(screen, Rect::new(x, y0, (x + dash).min(x1) - x, y1 - y0), FOCUS_DASH);
            x += step;
            if x > x1 {
                break;
            }
        }
    }
}

fn glyphs(screen: &mut Screen, font: FontId, x: i32, y: i32, chars: &[char], color: [u8; 3]) -> i32 {
    screen.set_text_color(f32::from(color[0]) / 255.0, f32::from(color[1]) / 255.0, f32::from(color[2]) / 255.0);
    let mut cx = x;
    for &c in chars {
        cx += screen.vgui2_draw_character(cx, y, c, font);
    }
    cx
}

/// `TextImage::RecalculateEllipsesPosition`: where "..." replaces the rest, if anywhere.
/// The first character is never measured.
fn ellipsis_at(screen: &Screen, font: FontId, chars: &[char], draw_w: i32) -> Option<usize> {
    let dots = 3 * screen.char_width(font, '.');
    let mut x = 0;
    for i in 1..chars.len() {
        let len = screen.char_width(font, chars[i]);
        if x + len + dots > draw_w {
            let rem: i32 = chars[i..].iter().map(|&c| screen.char_width(font, c)).sum();
            if x + rem > draw_w {
                return Some(i);
            }
        }
        x += len;
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    West,
    Center,
    East,
}

/// A vgui2 `Label`: its text (`&` rules) aligned in `r`, cut with "..." when too wide; the
/// fallback font, if any, is tried once before cutting.
fn paint_label(screen: &mut Screen, r: Rect, text: &str, align: Align, color: [u8; 3], font: FontId, fallback: Option<FontId>) {
    let chars = amp(text);
    if chars.is_empty() {
        return;
    }
    let mut font = font;
    let mut cut = ellipsis_at(screen, font, &chars, r.w);
    if cut.is_some()
        && let Some(fb) = fallback
    {
        font = fb;
        cut = ellipsis_at(screen, fb, &chars, r.w);
    }
    let full: i32 = chars.iter().map(|&c| screen.char_width(font, c)).sum();
    let tw = full.min(r.w);
    let x = match align {
        Align::West => 0,
        Align::Center => (r.w - tw) / 2,
        Align::East => r.w - tw,
    };
    let y = (r.h - screen.font_tall(font)) / 2;
    let end = glyphs(screen, font, r.x + x, r.y + y, &chars[..cut.unwrap_or(chars.len())], color);
    if cut.is_some() {
        glyphs(screen, font, end, r.y + y, &['.', '.', '.'], color);
    }
}

/// `CBottomBar`'s combos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Combo {
    Settings = 0,
    Player = 1,
    View = 2,
}

const COMBOS: [Combo; 3] = [Combo::Settings, Combo::Player, Combo::View];

impl Combo {
    const fn index(self) -> usize {
        self as usize
    }
}

/// vgui2 key focus inside the GUI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Focus {
    #[default]
    None,
    /// The spectator frame (`Frame::Activate`'s `RequestFocus`) or the bottom bar.
    Frame,
    Combo(Combo),
    /// A combo's open drop-down (or its cascade).
    Menu(Combo),
    /// prev (0) / next (1).
    Button(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Command(String),
    Toggle(String),
    Cascade(usize),
    /// A player-list item: `KeyValues("UserData", "player", name)`.
    Player(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    label: String,
    kind: Kind,
    checked: bool,
}

/// A `ComboBox` (`TextEntry` body + drop-down `Menu`).
#[derive(Debug, Clone, Default)]
struct ComboBox {
    text: String,
    /// The player combo's team colour (`SetFgColor` from `Update`).
    color: Option<[u8; 3]>,
    /// `_select` spans the text (`SelectAllText`).
    selected: bool,
    /// `m_bHighlight`: an item was selected; the next menu close takes the focus.
    highlight: bool,
    /// The drop-down's menus, root first.
    menus: Vec<Vec<Item>>,
    /// The root's `m_iActivatedItem` (row; not reset by `DeleteAllItems`).
    activated: Option<usize>,
    /// The root's scrollbar value.
    scroll: usize,
    /// Menus whose width was recomputed after an arm/disarm (the text-inset quirk).
    relaid: Vec<bool>,
}

/// The open menus from the root: each menu's index and its armed item.
type Chain = Vec<(usize, Option<usize>)>;

/// One open menu's screen placement.
#[derive(Debug, Clone, Copy)]
struct Placed {
    x: i32,
    y: i32,
    /// `_menuWide`: items are `wide − 2` across.
    wide: i32,
    w: i32,
    h: i32,
    scrollbar: bool,
    /// The first visible row and the visible row count.
    first: usize,
    rows: usize,
}

/// The bars' bounds at one screen size (§2).
#[derive(Debug, Clone)]
struct Geometry {
    sw: i32,
    sh: i32,
    /// `PSV(64)`: the top bar, the blank strip and the bottom bar's top.
    bar: i32,
    /// `Spectator.res` blocks' bounds, by block index (before the re-flow).
    top: Vec<Rect>,
    /// settings, player, view.
    combos: [Rect; 3],
    /// prev, next.
    buttons: [Rect; 2],
}

/// `CCSSpectatorGUI` with its `CBottomBar`.
#[derive(Debug, Default)]
pub struct SpectatorGui {
    loaded: bool,
    top_res: Option<Vec<ResCtl>>,
    bottom_res: Option<Vec<ResCtl>>,
    /// Label texts set by name (`SetLabelText`), lower-cased names.
    texts: Vec<(String, String)>,
    /// The re-flowed score-value and extrainfo widths (`SizeControls`); `None` = the .res ones.
    score_w: Option<i32>,
    extra_w: Option<i32>,
    /// `TERScoreValue` was not as wide as `CTScoreValue`: no re-flow ever.
    reflow_off: bool,
    /// The frame (`IsSpectatorGUIVisible`) and the bottom bar (`IsSpectatorBarVisible`).
    pub(super) visible: bool,
    pub(super) bar: bool,
    help_shown: bool,
    /// `insetBox`, `bottombarinset`, `m_bInsetVisible`, their bounds and the top bar's x.
    inset_box: bool,
    bar_inset: bool,
    inset_visible: bool,
    inset: [i32; 4],
    top_x: i32,
    label_visible: bool,
    label_text: String,
    label_color: Option<[u8; 3]>,
    /// The HLTV banner image (`SetImage`) and the panel's visibility.
    banner: Option<String>,
    banner_visible: bool,
    combos: [ComboBox; 3],
    /// The open drop-down: its combo and the menus from the root with their armed items.
    open: Option<(Combo, Chain)>,
    focus: Focus,
    /// prev/next: hovered, held with the mouse captured.
    armed: [bool; 2],
    pressed: [bool; 2],
    /// Dragging the player list's scrollbar nob: the grab offset inside it.
    drag: Option<i32>,
    /// `m_iChooseTeamKey` and whether it is bound to `chooseteam`.
    chooseteam: Option<(u8, bool)>,
    /// `m_pSpecHelp->m_SpecGUI` is set.
    pub(super) help_window: bool,
    /// `OnTick`'s last `g_iUser1`/`g_iUser2`.
    last_users: (i32, i32),
    /// The screen size and the cursor of the last layout.
    screen: (i32, i32),
    cursor: (i32, i32),
}

impl SpectatorGui {
    /// The constructors: the `.res` files, the label texts, the combo texts and their menus
    /// (built once with team 0 and no level, `CreateCommandComboMenu`).
    fn load(&mut self, eng: &mut dyn HudEngine) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        self.top_res = load_res(eng, "resource/UI/Spectator.res");
        self.bottom_res = load_res(eng, "resource/UI/BottomSpectator.res");
        let localize = |eng: &mut dyn HudEngine, s: &str| eng.screen().localize.text(s).to_owned();
        let labels: Vec<(String, String)> = self.top_res.iter().flatten().filter(|c| c.class.eq_ignore_ascii_case("Label")).map(|c| (c.name.to_ascii_lowercase(), c.label.clone())).collect();
        self.texts = labels.into_iter().map(|(n, l)| (n, localize(eng, &l))).collect();
        let menus = |eng: &mut dyn HudEngine, file: &str| -> Vec<Vec<Item>> {
            let Some(data) = eng.read_file(file) else { return vec![Vec::new()] };
            cmdmenu::parse(&data, 0, "", &[])
                .into_iter()
                .map(|m| {
                    m.items
                        .into_iter()
                        .map(|it| {
                            let kind = match it.action {
                                Action::Command(c) => Kind::Command(c),
                                Action::Toggle(c) => Kind::Toggle(c),
                                Action::Submenu(s) => Kind::Cascade(s),
                            };
                            Item { label: it.label, kind, checked: it.checked }
                        })
                        .collect()
                })
                .collect()
        };
        let settings = &mut self.combos[Combo::Settings.index()];
        settings.text = localize(eng, "#SPECT_OPTIONS");
        settings.menus = menus(eng, "spectatormenu.txt");
        let view = &mut self.combos[Combo::View.index()];
        view.text = localize(eng, "#CAM_OPTIONS");
        view.menus = menus(eng, "spectcammenu.txt");
        self.combos[Combo::Player.index()].menus = vec![Vec::new()];
        for c in &mut self.combos {
            c.relaid = vec![false; c.menus.len()];
        }
    }

    fn top(&self, name: &str) -> Option<usize> {
        self.top_res.as_ref()?.iter().position(|c| c.name.eq_ignore_ascii_case(name))
    }

    fn text(&self, name: &str) -> Option<&str> {
        self.texts.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, t)| t.as_str())
    }

    /// `SetLabelText(name, text)`: nothing without such a label.
    fn set_text(&mut self, name: &str, text: String) {
        if let Some(t) = self.texts.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            t.1 = text;
        }
    }

    fn geometry(&self) -> Geometry {
        let (sw, sh) = self.screen;
        let bar = psv(64, sh);
        let top = self
            .top_res
            .iter()
            .flatten()
            .map(|c| Rect::new(res_pos(&c.xpos, sw, sh), res_pos(&c.ypos, sh, sh), psv(c.wide.unwrap_or(64), sh), psv(c.tall.unwrap_or(24), sh)))
            .collect();
        let bar_y = sh - bar;
        let bottom = |name: &str| match self.bottom_res.iter().flatten().find(|c| c.name.eq_ignore_ascii_case(name)) {
            Some(c) => Rect::new(res_pos(&c.xpos, sw, sh), bar_y + res_pos(&c.ypos, sh, sh), psv(c.wide.unwrap_or(64), sh), psv(c.tall.unwrap_or(24), sh)),
            // No settings applied: Panel::Init's 64×24 at the bar's origin.
            None => Rect::new(0, bar_y, 64, 24),
        };
        Geometry {
            sw,
            sh,
            bar,
            top,
            combos: [bottom("settingscombo"), bottom("playercombo"), bottom("viewcombo")],
            buttons: [bottom("playerprev"), bottom("playernext")],
        }
    }

    /// A `Spectator.res` control's bounds after the right-group re-flow (§4.4).
    fn top_rect(&self, geo: &Geometry, index: usize) -> Rect {
        let Some(c) = self.top_res.as_ref().and_then(|r| r.get(index)) else { return Rect::default() };
        let mut r = geo.top[index];
        let res_w = |name: &str| self.top(name).map_or(0, |i| geo.top[i].w);
        let d_extra = self.extra_w.map_or(0, |e| e - res_w("extrainfo"));
        let d_score = self.score_w.map_or(0, |s| s - res_w("CTScoreValue"));
        match c.name.to_ascii_lowercase().as_str() {
            "extrainfo" => {
                r.x -= d_extra;
                r.w += d_extra;
            }
            "timerimage" | "timerlabel" | "dividerbar" => r.x -= d_extra,
            "ctscorevalue" | "terscorevalue" => {
                r.x -= d_extra + d_score;
                r.w += d_score;
            }
            "ctscorelabel" | "terscorelabel" => r.x -= d_extra + d_score,
            _ => {}
        }
        r
    }

    /// `Activate`: the frame and the (blank) player label shown, the bottom bar hidden, the
    /// banner panel shown, the PIP outline back if the inset is on.
    fn activate(&mut self, pip_on: bool, inset: [i32; 4]) {
        self.set_focus(Focus::Frame);
        self.help_shown = false;
        self.visible = true;
        self.label_visible = true;
        self.label_text.clear();
        self.set_bar(false);
        self.banner_visible = true;
        if pip_on {
            self.enable_inset_view(true, inset);
        }
    }

    /// `CBottomBar::SetVisible`; a hidden bar takes its drop-down with it.
    fn set_bar(&mut self, visible: bool) {
        self.bar = visible;
        if !visible {
            self.open = None;
            self.pressed = [false; 2];
            self.drag = None;
            if matches!(self.focus, Focus::Menu(_) | Focus::Combo(_) | Focus::Button(_)) {
                self.set_focus(Focus::Frame);
            }
        }
    }

    /// `CBottomBar::SetPlayerText` → `playercombo->SetText`.
    fn set_combo_text(&mut self, c: Combo, text: String) {
        let combo = &mut self.combos[c.index()];
        combo.text = text;
        combo.selected = false;
    }

    /// `DeactivateGUI`.
    fn deactivate_gui(&mut self) {
        self.label_visible = true;
        self.set_combo_text(Combo::Player, String::new());
        self.inset_box = false;
        self.set_bar(false);
    }

    /// `HideGUI`.
    fn hide_gui(&mut self) {
        self.deactivate_gui();
        self.visible = false;
        self.label_visible = true;
        self.label_text.clear();
        self.inset_box = false;
        self.bar_inset = false;
        self.banner = None;
        self.banner_visible = false;
    }

    /// `EnableInsetView`: the outline panels and the top bar's shift.
    pub(super) fn enable_inset_view(&mut self, on: bool, inset: [i32; 4]) {
        if on {
            self.inset = inset;
            self.top_x = inset[0] + inset[2] + 2;
            self.inset_box = true;
            if self.bar {
                self.bar_inset = true;
                self.inset_visible = true;
            }
        } else {
            self.inset_box = false;
            if self.bar {
                self.bar_inset = false;
                self.inset_visible = false;
            }
            self.top_x = 0;
        }
    }

    /// `SetSpectatorBanner` (director BANNER): the name up to its first ".tga".
    pub(super) fn set_banner(&mut self, image: &str) {
        let name = image.find(".tga").map_or(image, |i| &image[..i]);
        self.banner = Some(name.to_owned());
        self.banner_visible = true;
    }

    /// vgui2 key focus changes: a combo selects its text on gaining focus and drops the
    /// selection on losing it; an open drop-down closes once focus leaves it.
    fn set_focus(&mut self, f: Focus) {
        let old = self.focus;
        if old == f {
            return;
        }
        self.focus = f;
        if let Focus::Combo(c) = old {
            self.combos[c.index()].selected = false;
        }
        if let Focus::Combo(c) = f {
            self.combos[c.index()].selected = true;
        }
        if let Some((open, _)) = self.open
            && f != Focus::Menu(open)
        {
            self.menu_close(open);
        }
    }

    /// `HideMenu` + `ComboBox::OnMenuClose`.
    fn menu_close(&mut self, c: Combo) {
        if !self.open.as_ref().is_some_and(|(o, _)| *o == c) {
            return;
        }
        self.open = None;
        self.drag = None;
        if self.focus == Focus::Menu(c) {
            self.focus = Focus::None;
        }
        let over = self.geometry().combos[c.index()].contains(self.cursor);
        if self.focus == Focus::Combo(c) {
            self.combos[c.index()].selected = true;
        } else if self.combos[c.index()].highlight {
            self.combos[c.index()].highlight = false;
            self.set_focus(Focus::Combo(c));
        } else if over {
            self.set_focus(Focus::Combo(c));
        }
    }

    /// The player list's rows, rebuilt (`DeleteAllItems` + `AddItem`): an open list loses its
    /// armed item.
    fn set_player_items(&mut self, items: Vec<Item>) {
        let combo = &mut self.combos[Combo::Player.index()];
        combo.menus = vec![items];
        combo.relaid = vec![false];
        if let Some((Combo::Player, chain)) = &mut self.open {
            chain.truncate(1);
            chain[0].1 = None;
        }
    }

    /// `menuWide` of a content-sized (cascade) menu: the widest item plus 8, with the text
    /// inset 6 until an item was armed or disarmed, then 0 for checkable items and the check
    /// width + 6 for the others.
    fn content_wide(screen: &mut Screen, items: &[Item], relaid: bool) -> i32 {
        let font = screen.client_font("Default");
        let marlett = screen.client_font("Marlett");
        let check_w = screen.char_width(marlett, 'g');
        let arrow_w = screen.char_width(marlett, '4');
        let any_check = items.iter().any(|i| matches!(i.kind, Kind::Toggle(_)));
        let widest = items
            .iter()
            .map(|it| {
                let t = screen.text_width(font, &cmdmenu::display(screen, &it.label));
                let inset = match (&it.kind, relaid) {
                    (_, false) => 6,
                    (Kind::Toggle(_), true) => 0,
                    (_, true) if any_check => check_w + 6,
                    _ => 6,
                };
                match it.kind {
                    Kind::Toggle(_) => inset + check_w + 12 + t,
                    Kind::Cascade(_) => inset + t + arrow_w,
                    _ => inset + t,
                }
            })
            .max()
            .unwrap_or(0);
        if items.is_empty() { 0 } else { widest + 8 }
    }

    /// The open drop-down's menus: the root fixed to the combo's width above it (`DoMenuLayout`
    /// UP), a cascade beside its armed item (`PositionCascadingMenu`).
    fn place(&self, geo: &Geometry, screen: &mut Screen) -> Vec<Placed> {
        let Some((c, chain)) = &self.open else { return Vec::new() };
        let combo = &self.combos[c.index()];
        let r = geo.combos[c.index()];
        let mut out: Vec<Placed> = Vec::with_capacity(chain.len());
        for (k, &(m, _)) in chain.iter().enumerate() {
            let Some(items) = combo.menus.get(m) else { break };
            let n = items.len();
            let placed = if k == 0 {
                let lines = if *c == Combo::Player { PLAYER_LINES } else { 99 };
                let scrollbar = lines < n;
                let rows = n.min(lines);
                let h = 2 + ITEM_TALL * rows as i32;
                let first = if scrollbar { combo.scroll.min(n - lines) } else { 0 };
                Placed { x: r.x, y: r.y - h - psv(1, geo.sh), wide: r.w - if scrollbar { SCROLLBAR } else { 0 }, w: r.w + MENU_STRIP, h, scrollbar, first, rows }
            } else {
                let (Some(parent), Some(armed)) = (out.get(k - 1).copied(), chain[k - 1].1) else { break };
                let wide = Self::content_wide(screen, items, combo.relaid.get(m).copied().unwrap_or(false));
                let (w, h) = (wide + MENU_STRIP, 2 + ITEM_TALL * n as i32);
                let (ix, iy, iw) = (parent.x + 1, parent.y + 1 + ITEM_TALL * (armed as i32 - parent.first as i32), parent.wide - 2);
                let x = if ix + iw + w > geo.sw { ix - w - 2 } else { ix + iw + 1 };
                Placed { x, y: iy - 1, wide, w, h, scrollbar: false, first: 0, rows: n }
            };
            out.push(placed);
        }
        out
    }

    /// The deepest open menu under `p` and the item there.
    fn hit_menu(placed: &[Placed], p: (i32, i32)) -> Option<(usize, Option<usize>)> {
        let k = (0..placed.len()).rev().find(|&k| {
            let m = placed[k];
            Rect::new(m.x, m.y, m.w, m.h).contains(p)
        })?;
        let m = placed[k];
        let (ix, iy) = (p.0 - m.x - 1, p.1 - m.y - 1);
        let row = (ix >= 0 && ix < m.wide - 2 && iy >= 0 && iy < ITEM_TALL * m.rows as i32).then_some(m.first + (iy / ITEM_TALL) as usize);
        Some((k, row))
    }

    /// The root's scrollbar parts: `(x, y, w, h)` of the bar, its slider and the nob span.
    fn scrollbar(m: &Placed, count: usize, value: usize) -> (Rect, Rect, (i32, i32)) {
        let (sx, sy) = (m.x + 1 + m.w - 1 - SCROLLBAR, m.y + 2);
        let tall = m.h - 2;
        let bar = Rect::new(sx, sy, SCROLLBAR, tall);
        let slider = Rect::new(sx, sy + SCROLLBAR, SCROLLBAR, tall - 2 * SCROLLBAR + 1);
        // ScrollBarSlider::RecomputeNobPosFromValue (range 0..count, window 10).
        let (range, window) = (count as f64, PLAYER_LINES as f64);
        let len = f64::from(slider.h - 1);
        let size = f64::from(SCROLLBAR).max(window / range.max(1.0) * len);
        let first = (len - size) * value as f64 / (range - window).max(1.0);
        let (mut nob0, mut nob1) = (first as f32 as i32, (first + size) as f32 as i32);
        if f64::from(nob1) > len {
            nob0 = (len - size) as f32 as i32;
            nob1 = len as f32 as i32;
        }
        (bar, slider, (nob0, nob1))
    }

    /// Hover: prev/next arm under the cursor; drop-down items arm on entering (closing other
    /// cascades, opening their own) and plain items disarm on leaving; the nob follows a drag.
    fn hover(&mut self, geo: &Geometry, screen: &mut Screen) {
        for i in 0..2 {
            self.armed[i] = self.bar && geo.buttons[i].contains(self.cursor);
        }
        let placed = self.place(geo, screen);
        if let (Some(grab), Some(root)) = (self.drag, placed.first()) {
            let count = self.combos[Combo::Player.index()].menus.first().map_or(0, Vec::len);
            let (_, slider, (nob0, nob1)) = Self::scrollbar(root, count, self.combos[Combo::Player.index()].scroll);
            let travel = (slider.h - 1) - (nob1 - nob0);
            let max = count.saturating_sub(PLAYER_LINES);
            if travel > 0 {
                let pos = (self.cursor.1 - slider.y - grab).clamp(0, travel);
                let value = (f64::from(pos) / f64::from(travel) * max as f64 + 0.5) as usize;
                self.combos[Combo::Player.index()].scroll = value.min(max);
            }
            return;
        }
        let Some((c, mut chain)) = self.open.take() else { return };
        let hit = Self::hit_menu(&placed, self.cursor);
        let combo = &mut self.combos[c.index()];
        let kind_of = |menus: &[Vec<Item>], m: usize, i: usize| menus.get(m).and_then(|menu| menu.get(i)).map(|it| it.kind.clone());
        for (k, link) in chain.iter_mut().enumerate() {
            let (m, armed) = *link;
            if let Some(i) = armed
                && !matches!(kind_of(&combo.menus, m, i), Some(Kind::Cascade(_)))
                && hit != Some((k, Some(i)))
            {
                link.1 = None;
                if let Some(r) = combo.relaid.get_mut(m) {
                    *r = true;
                }
            }
        }
        if let Some((k, Some(i))) = hit
            && !(chain[k].1 == Some(i) && chain.len() > k + 1)
        {
            let m = chain[k].0;
            chain.truncate(k + 1);
            chain[k].1 = Some(i);
            if let Some(r) = combo.relaid.get_mut(m) {
                *r = true;
            }
            if let Some(Kind::Cascade(sub)) = kind_of(&combo.menus, m, i) {
                chain.push((sub, None));
            }
        }
        self.open = Some((c, chain));
    }
}

impl Hud {
    /// `InsetValues`: the PIP inset rectangle.
    pub(super) fn inset_values(&self) -> [i32; 4] {
        self.overview.window
    }

    /// `TeamFortressViewport::UpdateSpectatorPanel` and the CS wrapper's `UpdateTimer`:
    /// nothing but the settings without a player target or in free look.
    pub(super) fn update_spectator_panel(&mut self, eng: &mut dyn HudEngine) {
        self.spectator_gui.load(eng);
        let mode = self.state.user1;
        self.check_settings(eng);
        let target = self.state.user2;
        if (1..=64).contains(&target) && mode != 3 {
            self.spectator_update(target, mode, eng);
            self.spectator_player_list(eng);
        }
        self.spectator_timer();
    }

    /// `CSpectatorGUI::Update` + `CCSSpectatorGUI::Update`.
    fn spectator_update(&mut self, target: i32, mode: i32, eng: &mut dyn HudEngine) {
        let label = eng.screen().localize.text(spectator_label(mode)).to_owned();
        self.spectator_gui.set_combo_text(Combo::View, label);
        if !self.spectator_gui.help_shown && !self.spectator_gui.bar && mode != 0 {
            self.spectator_gui.help_shown = true;
            self.duck_message("#Spec_Duck", eng);
        }
        let team = usize::try_from(target).ok().and_then(|i| self.players.get(i)).map_or(0, |p| p.teamnumber);
        let color = team_color(team).map(|v| v.clamp(0, 255) as u8);
        let text = self.player_label_text(target, eng);
        let gui = &mut self.spectator_gui;
        gui.combos[Combo::Player.index()].color = Some(color);
        gui.label_color = Some(color);
        gui.set_combo_text(Combo::Player, text.clone());
        gui.label_text = text;
        let extra = if eng.spectate_only() {
            let n = self.observer.spectator_number;
            format!("{} {n}", eng.screen().localize.find("#Spectators").unwrap_or(""))
        } else {
            let fmt = eng.screen().localize.find("#Spec_Map").unwrap_or("").to_owned();
            construct(&fmt, &[file_base(&eng.level_name())])
        };
        self.spectator_gui.set_text("extrainfo", extra);
        let local_team = self.local_index(eng).map_or(0, |i| self.players[i].teamnumber);
        if local_team == 1 || local_team == 2 {
            let money = format!("${}", self.money());
            self.spectator_gui.set_text("extrainfo", money);
        }
        self.spectator_size_controls(eng);
    }

    /// `GetPlayerLabelText`: `Name (health)` while the last `SpecHealth*` was positive, else
    /// `Name`; at most 39 characters.
    fn player_label_text(&self, target: i32, eng: &mut dyn HudEngine) -> String {
        let name = eng.player_info(target).map_or_else(String::new, |p| make_safe_name(&p.name));
        let health = usize::try_from(target).ok().and_then(|i| self.players.get(i)).map_or(0, |p| p.spec_health);
        let localize = &eng.screen().localize;
        let text = if health > 0 {
            localize.find("#Spec_PlayerItem_Team").map(|f| construct(f, &[name, health.to_string()]))
        } else {
            localize.find("#Spec_PlayerItem").map(|f| construct(f, &[name]))
        };
        text.unwrap_or_else(|| "Unable to find #Spec_PlayerItem*".to_owned()).chars().take(39).collect()
    }

    /// `CCSSpectatorGUI::UpdateSpectatorPlayerList`: the player list (while the bar is open),
    /// then the team scores, re-flowed once both teams are known.
    pub(super) fn spectator_player_list(&mut self, eng: &mut dyn HudEngine) {
        if self.spectator_gui.bar {
            let local_team = self.local_index(eng).map_or(0, |i| self.players[i].teamnumber);
            let any_team = local_team == 0 || eng.spectate_only() || self.viewport.force_camera == 0;
            let mut items = Vec::new();
            for i in 1..64 {
                let Some(info) = eng.player_info(i) else { continue };
                let Some(extra) = usize::try_from(i).ok().and_then(|i| self.players.get(i)) else { continue };
                if info.name.is_empty() || info.thisplayer || extra.dead || !(any_team || extra.teamnumber == local_team) {
                    continue;
                }
                let localize = &eng.screen().localize;
                let team = localize.find(&format!("#{}", extra.teamname)).map_or_else(|| extra.teamname.clone(), str::to_owned);
                let fmt = localize.find("#Spec_PlayerItem_Team").unwrap_or("");
                let label: String = construct(fmt, &[info.name.clone(), team]).chars().take(79).collect();
                items.push(Item { label, kind: Kind::Player(info.name), checked: false });
            }
            self.spectator_gui.set_player_items(items);
        }
        let ter = self.viewport.team_score("TERRORIST");
        let ct = self.viewport.team_score("CT");
        if let Some(s) = ter {
            self.spectator_gui.set_text("TERScoreValue", s.to_string());
        }
        if let Some(s) = ct {
            self.spectator_gui.set_text("CTScoreValue", s.to_string());
        }
        if ter.is_some() && ct.is_some() {
            self.spectator_size_controls(eng);
        }
    }

    /// `CCSSpectatorGUI::UpdateTimer`: blank once the bomb is planted.
    fn spectator_timer(&mut self) {
        let text = if self.show_timer {
            let s = ((self.round_time_end() - self.time) as i32).max(0);
            format!("{}:{:02}", s / 60, s % 60)
        } else {
            String::new()
        };
        self.spectator_gui.set_text("timerlabel", text);
    }

    /// `CCSSpectatorGUI::SizeControls`: the score values and extrainfo grow leftward to their
    /// text, never below the .res widths.
    fn spectator_size_controls(&mut self, eng: &mut dyn HudEngine) {
        let gui = &mut self.spectator_gui;
        let names = ["CTScoreLabel", "CTScoreValue", "TERScoreLabel", "TERScoreValue", "timerimage", "timerlabel", "DividerBar", "extrainfo"];
        if names.iter().any(|n| gui.top(n).is_none()) || gui.reflow_off {
            return;
        }
        let screen = eng.screen();
        gui.screen = (screen.width as i32, screen.height as i32);
        let geo = gui.geometry();
        let res_w = |name: &str| gui.top(name).map_or(0, |i| geo.top[i].w);
        if gui.score_w.is_none() && gui.extra_w.is_none() && res_w("TERScoreValue") != res_w("CTScoreValue") {
            gui.reflow_off = true;
            return;
        }
        let content = |screen: &mut Screen, name: &str| -> i32 {
            let font = gui.top(name).and_then(|i| gui.top_res.as_ref()?.get(i)).map_or("Default", |c| if c.font.is_empty() { "Default" } else { c.font.as_str() });
            let font = screen.client_font(font);
            amp(gui.text(name).unwrap_or("")).iter().map(|&c| screen.char_width(font, c)).sum()
        };
        let score = res_w("CTScoreValue").max(content(screen, "CTScoreValue")).max(content(screen, "TERScoreValue"));
        let extra = res_w("extrainfo").max(content(screen, "extrainfo"));
        gui.score_w = Some(score);
        gui.extra_w = Some(extra);
    }

    /// `CBottomBar::SetVisible`'s one-time `m_iChooseTeamKey` lookup.
    fn spectator_chooseteam_key(&mut self, eng: &mut dyn HudEngine) {
        if self.spectator_gui.chooseteam.is_none() {
            self.spectator_gui.chooseteam = eng.key_for_binding("chooseteam").map(|k| (k, true)).or_else(|| eng.key_for_binding("changeteam").map(|k| (k, false)));
        }
    }

    /// `ShowSpectatorGUI` (viewport slot 20; `spec_menu 1`, OnTick, `jointeam 6`).
    pub(super) fn show_spectator_gui(&mut self, eng: &mut dyn HudEngine) {
        self.spectator_gui.load(eng);
        self.spectator_gui.help_window = true;
        self.spectator_activate(eng);
    }

    fn spectator_activate(&mut self, eng: &mut dyn HudEngine) {
        let pip = eng.cvar("spec_pip") != 0.0;
        let inset = self.inset_values();
        self.spectator_gui.activate(pip, inset);
        self.spectator_chooseteam_key(eng);
    }

    /// `HideSpectatorGUI` (viewport slot 22): the duck hint for HLTV if it was up.
    pub(super) fn hide_spectator_gui(&mut self, eng: &mut dyn HudEngine) {
        if self.spectator_gui.visible && eng.spectate_only() {
            self.duck_message("#Spec_Duck", eng);
        }
        self.spectator_gui.hide_gui();
        self.spectator_chooseteam_key(eng);
    }

    /// `DeactivateSpectatorGUI` (viewport slot 23, `spec_menu 0`).
    pub(super) fn deactivate_spectator_gui(&mut self, eng: &mut dyn HudEngine) {
        self.spectator_gui.deactivate_gui();
        self.spectator_chooseteam_key(eng);
        self.duck_message("#Spec_Duck", eng);
    }

    /// `ShowSpectatorGUIBar` (viewport slot 21, DUCK in `HandleButtonsDown`): the bar with the
    /// player list; the label and the player combo stay blank until the next update.
    pub(super) fn show_spectator_gui_bar(&mut self, eng: &mut dyn HudEngine) {
        self.spectator_gui.load(eng);
        self.spectator_gui.bar = true;
        self.spectator_chooseteam_key(eng);
        self.spectator_player_list(eng);
        let gui = &mut self.spectator_gui;
        gui.label_visible = false;
        gui.label_text.clear();
        gui.set_combo_text(Combo::Player, String::new());
    }

    /// The bar has the mouse: the cursor shows and mouse look stops.
    pub(super) fn spectator_cursor(&self) -> bool {
        self.spectator_gui.visible && self.spectator_gui.bar
    }

    /// The viewport's `OnTick` for the spectator GUI: the viewport's requests, a panel update
    /// on a mode or target change, the GUI shown while observing with no VGUI menu up (hidden
    /// otherwise), and the timer.
    pub(super) fn spectator_tick(&mut self, eng: &mut dyn HudEngine) {
        self.spectator_gui.load(eng);
        for event in std::mem::take(&mut self.viewport.spec_events) {
            match event {
                super::viewport::SpecEvent::Hide => {
                    if self.spectator_gui.visible {
                        self.hide_spectator_gui(eng);
                    }
                }
                super::viewport::SpecEvent::Show => self.show_spectator_gui(eng),
                // CSpecHelpWindow::OnClose: m_SpecGUI->Activate().
                super::viewport::SpecEvent::HelpClosed => {
                    if self.spectator_gui.help_window {
                        self.spectator_activate(eng);
                    }
                }
            }
        }
        let users = (self.state.user1, self.state.user2);
        if users != self.spectator_gui.last_users {
            self.spectator_gui.last_users = users;
            self.update_spectator_panel(eng);
        }
        if self.state.user1 != 0 {
            if !self.spectator_gui.visible && !self.viewport.menu_active() && !self.bot_progress.visible() {
                self.show_spectator_gui(eng);
            }
        } else if self.spectator_gui.visible {
            self.hide_spectator_gui(eng);
        }
        if self.spectator_gui.visible {
            self.spectator_timer();
        }
    }

    /// `CheckSettings`' last step and the USE button: `EnableInsetView`.
    pub(super) fn spectator_inset(&mut self, on: bool) {
        let inset = self.inset_values();
        self.spectator_gui.enable_inset_view(on, inset);
    }

    // ------------------------------------------------------------------ input

    /// `DoClick`: the drop-down toggles; opening pre-arms the item showing the combo's text,
    /// refreshes the check marks (`CommandComboBox::OnShowMenu`) and takes the focus.
    fn spectator_do_click(&mut self, c: Combo, eng: &mut dyn HudEngine) {
        if self.spectator_gui.open.as_ref().is_some_and(|(o, _)| *o == c) {
            self.spectator_gui.menu_close(c);
            return;
        }
        let pip = eng.cvar("spec_pip") != 0.0;
        let scores = self.viewport.scoreboard;
        let gui = &mut self.spectator_gui;
        let (text, combo) = (gui.combos[c.index()].text.clone(), &mut gui.combos[c.index()]);
        let Some(root) = combo.menus.first() else { return };
        let armed = root.iter().position(|it| eng.screen().localize.text(&it.label) == text);
        if let Some(row) = armed
            && c == Combo::Player
            && root.len() > PLAYER_LINES
            && (row >= combo.scroll + PLAYER_LINES || row < combo.scroll)
        {
            combo.scroll = row.min(root.len() - PLAYER_LINES);
        }
        if c != Combo::Player {
            for menu in &mut combo.menus {
                for it in menu.iter_mut() {
                    if let Kind::Toggle(cmd) = &it.kind {
                        it.checked = if cmd.contains("spec_mode") {
                            pip
                        } else if cmd.contains("togglescores") {
                            scores
                        } else {
                            let name: String = format!("{cmd}_internal").chars().take(63).collect();
                            eng.cvar(&name) > 0.0
                        };
                    }
                }
            }
        }
        gui.set_focus(Focus::Menu(c));
        gui.open = Some((c, vec![(0, armed)]));
        gui.combos[c.index()].selected = false;
    }

    /// An item fires (release over it, ENTER): the menus close; a player item sets the combo
    /// text and follows that player if the text changed; a command item runs its command
    /// (`CommandComboBox::OnCommand`, the settings combo's also reaching `CBottomBar::OnCommand`).
    fn spectator_fire(&mut self, c: Combo, m: usize, i: usize, eng: &mut dyn HudEngine) {
        let gui = &mut self.spectator_gui;
        let Some(item) = gui.combos[c.index()].menus.get(m).and_then(|menu| menu.get(i)).cloned() else { return };
        match item.kind {
            Kind::Cascade(_) => {}
            Kind::Player(name) => {
                gui.combos[c.index()].activated = Some(i);
                gui.open = None;
                if gui.focus == Focus::Menu(c) {
                    gui.focus = Focus::None;
                }
                let changed = gui.combos[c.index()].text != item.label;
                if changed {
                    gui.set_combo_text(c, item.label);
                }
                gui.open = Some((c, Vec::new()));
                gui.menu_close(c);
                gui.combos[c.index()].highlight = true;
                if changed {
                    self.find_player(&name, eng);
                }
            }
            Kind::Command(cmd) | Kind::Toggle(cmd) => {
                if let Some(it) = gui.combos[c.index()].menus.get_mut(m).and_then(|menu| menu.get_mut(i))
                    && matches!(it.kind, Kind::Toggle(_))
                {
                    it.checked = !it.checked;
                }
                if m == 0 {
                    gui.combos[c.index()].activated = Some(i);
                }
                gui.menu_close(c);
                gui.combos[c.index()].highlight = true;
                if c == Combo::Settings {
                    self.spectator_bar_command(&cmd, eng);
                }
                if cmd.eq_ignore_ascii_case("ButtonClicked") {
                    self.spectator_do_click(c, eng);
                } else {
                    eng.client_cmd(&cmd);
                }
            }
        }
    }

    /// `CBottomBar::OnCommand`: only the prev/next commands (▶ sends `follownext 1`).
    fn spectator_bar_command(&mut self, cmd: &str, eng: &mut dyn HudEngine) {
        if cmd.eq_ignore_ascii_case("playernext") {
            self.find_next_player(true, eng);
        } else if cmd.eq_ignore_ascii_case("playerprev") {
            self.find_next_player(false, eng);
        }
    }

    /// A mouse press at `(x, y)` (`left`: the left button); `true` when a GUI popup took it.
    pub(super) fn spectator_press(&mut self, x: i32, y: i32, left: bool, eng: &mut dyn HudEngine) -> bool {
        if !self.spectator_cursor() {
            return false;
        }
        let screen = eng.screen();
        let gui = &mut self.spectator_gui;
        gui.screen = (screen.width as i32, screen.height as i32);
        gui.cursor = (x, y);
        let geo = gui.geometry();
        let placed = gui.place(&geo, screen);
        if let Some((k, _)) = SpectatorGui::hit_menu(&placed, (x, y)) {
            // Items fire on release; the player list's scrollbar acts on press.
            if left
                && k == 0
                && placed[0].scrollbar
            {
                let combo = &mut gui.combos[Combo::Player.index()];
                let count = combo.menus.first().map_or(0, Vec::len);
                let max = count.saturating_sub(PLAYER_LINES);
                let (bar, slider, (nob0, nob1)) = SpectatorGui::scrollbar(&placed[0], count, combo.scroll);
                if bar.contains((x, y)) {
                    if y < slider.y {
                        combo.scroll = combo.scroll.saturating_sub(1);
                    } else if y >= bar.y + bar.h - SCROLLBAR {
                        combo.scroll = (combo.scroll + 1).min(max);
                    } else if y < slider.y + nob0 {
                        combo.scroll = combo.scroll.saturating_sub(PLAYER_LINES);
                    } else if y >= slider.y + nob1 {
                        combo.scroll = (combo.scroll + PLAYER_LINES).min(max);
                    } else {
                        gui.drag = Some(y - slider.y - nob0);
                    }
                    if let Some((_, chain)) = &mut gui.open {
                        chain.truncate(1);
                    }
                }
            }
            return true;
        }
        let inset = Rect::new(gui.inset[0], gui.inset[1], gui.inset[2], gui.inset[3]);
        if gui.bar_inset && inset.contains((x, y)) {
            if left {
                eng.client_cmd("spec_toggleinset\n");
            }
            return true;
        }
        if !Rect::new(0, geo.sh - geo.bar, geo.sw, psv(66, geo.sh)).contains((x, y)) {
            return false;
        }
        for c in COMBOS {
            let r = geo.combos[c.index()];
            if !r.contains((x, y)) {
                continue;
            }
            let arrow = Rect::new(r.x + r.w - 1 - psv(20, geo.sh), r.y + psv(2, geo.sh), psv(16, geo.sh), r.h - 1 - psv(2, geo.sh));
            if arrow.contains((x, y)) {
                // ComboBoxButton: left only, no focus request.
                if left {
                    self.spectator_do_click(c, eng);
                }
            } else {
                self.spectator_gui.set_focus(Focus::Combo(c));
                self.spectator_do_click(c, eng);
            }
            return true;
        }
        for i in 0..2 {
            if geo.buttons[i].contains((x, y)) {
                if left {
                    self.spectator_gui.set_focus(Focus::Button(i));
                    self.spectator_gui.pressed[i] = true;
                }
                return true;
            }
        }
        // [INFERENCE] the bar's frame takes the click and the focus.
        self.spectator_gui.set_focus(Focus::Frame);
        true
    }

    /// A left-button release: a captured prev/next fires if still under the cursor; else an
    /// item under the cursor fires.
    pub(super) fn spectator_release(&mut self, x: i32, y: i32, eng: &mut dyn HudEngine) -> bool {
        let gui = &mut self.spectator_gui;
        gui.cursor = (x, y);
        gui.drag = None;
        if let Some(i) = gui.pressed.iter().position(|&p| p) {
            gui.pressed = [false; 2];
            if gui.bar && gui.geometry().buttons[i].contains((x, y)) {
                self.find_next_player(i == 1, eng);
            }
            return true;
        }
        if !self.spectator_cursor() {
            return false;
        }
        let geo = self.spectator_gui.geometry();
        let placed = self.spectator_gui.place(&geo, eng.screen());
        let Some((k, Some(i))) = SpectatorGui::hit_menu(&placed, (x, y)) else { return false };
        let Some((c, chain)) = &self.spectator_gui.open else { return false };
        let (c, m) = (*c, chain[k].0);
        self.spectator_fire(c, m, i, eng);
        true
    }

    /// The mouse wheel over the open player list (`Menu::OnMouseWheeled`: `value -= delta`).
    pub(super) fn spectator_wheel(&mut self, delta: i32, eng: &mut dyn HudEngine) -> bool {
        if !self.spectator_cursor() {
            return false;
        }
        let geo = self.spectator_gui.geometry();
        let placed = self.spectator_gui.place(&geo, eng.screen());
        let gui = &mut self.spectator_gui;
        let over = SpectatorGui::hit_menu(&placed, gui.cursor).is_some();
        if over
            && let Some((Combo::Player, chain)) = &mut gui.open
            && placed.first().is_some_and(|p| p.scrollbar)
        {
            let combo = &mut gui.combos[Combo::Player.index()];
            let max = combo.menus.first().map_or(0, Vec::len).saturating_sub(PLAYER_LINES);
            combo.scroll = (combo.scroll as i32 - delta).clamp(0, max as i32) as usize;
            chain.truncate(1);
        }
        over
    }

    /// A key press while the bar is open; `true` when the GUI took it. DUCK closes the bar
    /// (`CSpectatorGUI::OnKeyCodePressed`), the change-team key hides the GUI and sends
    /// `chooseteam`/`changeteam`; the focused widget gets its keys.
    pub(super) fn spectator_key(&mut self, key: u8, eng: &mut dyn HudEngine) -> bool {
        if !self.spectator_cursor() {
            return false;
        }
        if let Some((k, choose)) = self.spectator_gui.chooseteam
            && k == key
        {
            self.hide_spectator_gui(eng);
            eng.client_cmd(if choose { "chooseteam\n" } else { "changeteam\n" });
            return true;
        }
        if eng.key_for_binding("duck") == Some(key) {
            let gui = &mut self.spectator_gui;
            gui.set_bar(false);
            if gui.inset_visible {
                gui.bar_inset = false;
                gui.inset_box = true;
            }
            gui.label_visible = true;
            return true;
        }
        let enter = matches!(key, K_ENTER | K_KP_ENTER);
        match self.spectator_gui.focus {
            Focus::Menu(c) => self.spectator_menu_key(c, key, eng),
            Focus::Combo(c) => match key {
                K_UPARROW | K_DOWNARROW => {
                    self.spectator_step(c, if key == K_UPARROW { -1 } else { 1 }, eng);
                    true
                }
                _ if enter || key == K_SPACE => {
                    self.spectator_do_click(c, eng);
                    true
                }
                _ if key.is_ascii_alphanumeric() => {
                    self.spectator_typed(c, char::from(key), eng);
                    true
                }
                _ => false,
            },
            Focus::Button(i) if enter || key == K_SPACE => {
                self.find_next_player(i == 1, eng);
                true
            }
            _ => false,
        }
    }

    /// `Menu::OnKeyCodeTyped` / `OnKeyTyped` on the deepest open menu.
    fn spectator_menu_key(&mut self, c: Combo, key: u8, eng: &mut dyn HudEngine) -> bool {
        let gui = &mut self.spectator_gui;
        let Some((_, chain)) = &mut gui.open else { return false };
        let Some(&(m, armed)) = chain.last() else { return false };
        let items = &gui.combos[c.index()].menus[m];
        let n = items.len();
        match key {
            K_ESCAPE => gui.menu_close(c),
            K_UPARROW | K_DOWNARROW if n > 0 => {
                let next = match (armed, key == K_UPARROW) {
                    (None, true) => n - 1,
                    (None, false) => 0,
                    (Some(i), true) => (i + n - 1) % n,
                    (Some(i), false) => (i + 1) % n,
                };
                if let Some(last) = chain.last_mut() {
                    last.1 = Some(next);
                }
                if c == Combo::Player && m == 0 && n > PLAYER_LINES {
                    let combo = &mut gui.combos[c.index()];
                    if next >= combo.scroll + PLAYER_LINES || next < combo.scroll {
                        combo.scroll = next.min(n - PLAYER_LINES);
                    }
                }
            }
            K_RIGHTARROW => {
                if let Some(i) = armed
                    && let Kind::Cascade(sub) = items[i].kind
                {
                    chain.push((sub, None));
                }
            }
            K_LEFTARROW if chain.len() > 1 => {
                chain.pop();
            }
            K_ENTER | K_KP_ENTER => {
                if let Some(i) = armed {
                    self.spectator_fire(c, m, i, eng);
                }
            }
            _ if key.is_ascii_alphanumeric() && n > 0 => {
                let ch = char::from(key).to_ascii_lowercase();
                let start = armed.map_or(0, |i| i + 1);
                let screen = eng.screen();
                let found = (0..n).map(|k| (start + k) % n).find(|&k| screen.localize.text(&items[k].label).chars().next().is_some_and(|f| f.to_lowercase().eq(std::iter::once(ch))));
                if let (Some(k), Some(last)) = (found, chain.last_mut()) {
                    last.1 = Some(k);
                }
            }
            _ => return false,
        }
        true
    }

    /// `ComboBox::MoveAlongMenuItemList`: from the item showing the combo's text to its
    /// neighbour, which is set as the text and fired.
    fn spectator_step(&mut self, c: Combo, dir: i32, eng: &mut dyn HudEngine) {
        let gui = &self.spectator_gui;
        let Some(root) = gui.combos[c.index()].menus.first() else { return };
        let text = &gui.combos[c.index()].text;
        let Some(row) = root.iter().position(|it| eng.screen().localize.text(&it.label) == text) else { return };
        let j = (row as i32 + dir).clamp(0, root.len() as i32 - 1) as usize;
        self.spectator_select(c, j, eng);
    }

    /// `ComboBox::OnKeyTyped`: the next item starting with `ch` (wrapping from the active
    /// one; the active one again without a match), set as the text and fired.
    fn spectator_typed(&mut self, c: Combo, ch: char, eng: &mut dyn HudEngine) {
        let gui = &self.spectator_gui;
        let Some(root) = gui.combos[c.index()].menus.first() else { return };
        let count = root.len();
        if count == 0 {
            return;
        }
        let start = gui.combos[c.index()].activated.unwrap_or(0).min(count - 1);
        let ch = ch.to_ascii_lowercase();
        let screen = eng.screen();
        let mut i = (start + 1) % count;
        while i != start && screen.localize.text(&root[i].label).chars().next().is_none_or(|f| f.to_ascii_lowercase() != ch) {
            i = (i + 1) % count;
        }
        self.spectator_select(c, i, eng);
    }

    /// `OnSetText(item)` + `SelectAllText` + `ActivateItem(item)`.
    fn spectator_select(&mut self, c: Combo, i: usize, eng: &mut dyn HudEngine) {
        let Some(item) = self.spectator_gui.combos[c.index()].menus.first().and_then(|m| m.get(i)).cloned() else { return };
        let text = eng.screen().localize.text(&item.label).to_owned();
        let combo = &mut self.spectator_gui.combos[c.index()];
        if combo.text != text {
            combo.text = text;
            match c {
                // CBottomBar::OnTextChanged.
                Combo::Settings => combo.text = eng.screen().localize.text("#SPECT_OPTIONS").to_owned(),
                Combo::Player => {
                    if let Some(Kind::Player(name)) = combo.menus.first().and_then(|m| m.get(combo.activated.unwrap_or(i))).map(|it| it.kind.clone()) {
                        self.find_player(&name, eng);
                    }
                }
                Combo::View => {}
            }
        }
        let combo = &mut self.spectator_gui.combos[c.index()];
        combo.selected = true;
        combo.activated = Some(i);
        if let Kind::Command(cmd) | Kind::Toggle(cmd) = item.kind {
            if c == Combo::Settings {
                self.spectator_bar_command(&cmd, eng);
            }
            eng.client_cmd(&cmd);
        }
        self.spectator_gui.combos[c.index()].highlight = true;
    }

    // ------------------------------------------------------------------ paint

    /// The GUI over the HUD: the frame (bars, banner, PIP outline, labels), then the bottom bar
    /// popup (combos, buttons), its PIP outline, then any open drop-down.
    pub(super) fn spectator_draw(&mut self, eng: &mut dyn HudEngine) {
        let gui = &mut self.spectator_gui;
        if !gui.visible {
            return;
        }
        let banner = gui.banner.clone().filter(|_| gui.banner_visible).and_then(|b| eng.load_image(&b));
        let timer = gui.top_res.iter().flatten().find(|c| c.name.eq_ignore_ascii_case("timerimage")).map(|c| c.image.clone()).and_then(|i| eng.load_image(&i));
        let icon_down = eng.load_image("resource/icon_down");
        let icon_up = eng.load_image("resource/icon_up");
        let screen = eng.screen();
        gui.screen = (screen.width as i32, screen.height as i32);
        gui.cursor = self.cursor;
        let geo = gui.geometry();
        gui.hover(&geo, screen);
        let (sw, sh, bar) = (geo.sw, geo.sh, geo.bar);
        fill(screen, Rect::new(gui.top_x, 0, sw, bar), BAR_BG);
        if let Some(img) = banner {
            // Its own size, cut to the 256 × PSV(64) panel.
            let (w, h) = (img.width as i32, img.height as i32);
            let (cw, ch) = (w.min(256), h.min(bar));
            if cw > 0 && ch > 0 {
                screen.draw_image_uv(img, gui.top_x, 0, cw, ch, [0.0, 0.0, cw as f32 / w as f32, ch as f32 / h as f32], [255; 4]);
            }
        }
        fill(screen, Rect::new(0, sh - bar, sw, bar), BAR_BG);
        if gui.inset_box {
            Self::inset_outline(screen, gui.inset);
        }
        let default = screen.client_font("Default");
        let fallback = screen.client_font("DefaultVerySmallFallBack");
        for (index, c) in gui.top_res.iter().flatten().enumerate() {
            if !c.visible || c.name.eq_ignore_ascii_case("playerlabel") {
                continue;
            }
            let r = gui.top_rect(&geo, index);
            match c.class.as_str() {
                "Label" => {
                    let font = if c.font.is_empty() { default } else { screen.client_font(&c.font) };
                    let align = match c.align.as_str() {
                        "east" | "north-east" | "south-east" => Align::East,
                        "center" | "north" | "south" => Align::Center,
                        _ => Align::West,
                    };
                    let text = gui.text(&c.name).unwrap_or("").to_owned();
                    paint_label(screen, r, &text, align, TEXT, font, None);
                }
                "ImagePanel" => {
                    if !c.fill.is_empty() {
                        let f = screen.client_color(&c.fill).unwrap_or([188, 112, 0, 128]);
                        fill(screen, r, f.map(i32::from));
                    }
                    if c.name.eq_ignore_ascii_case("timerimage")
                        && let Some(img) = timer
                    {
                        screen.draw_image(img, r.x, r.y, r.w, r.h, [255; 4]);
                    }
                }
                _ => {}
            }
        }
        if gui.label_visible
            && let Some(i) = gui.top("playerlabel")
        {
            let r = gui.top_rect(&geo, i);
            let color = gui.label_color.unwrap_or(TEXT);
            paint_label(screen, r, &gui.label_text, Align::Center, color, default, Some(fallback));
        }
        if gui.bar {
            for c in COMBOS {
                Self::paint_combo(screen, gui, c, geo.combos[c.index()], sh, icon_down);
            }
            Self::paint_buttons(screen, gui, &geo);
            if gui.bar_inset {
                Self::inset_outline(screen, gui.inset);
            }
            Self::paint_menus(screen, gui, &geo, icon_up, icon_down);
        }
    }

    /// `InsetBox::Paint`: four lines in the current draw colour, which depends on vgui2's paint
    /// order (spectator-overview.md open question 1; white as recommended there). The right and
    /// bottom lines lie outside the panel and are clipped.
    fn inset_outline(screen: &mut Screen, r: [i32; 4]) {
        fill(screen, Rect::new(r[0], r[1], r[2], 1), [255; 4]);
        fill(screen, Rect::new(r[0], r[1], 1, r[3]), [255; 4]);
    }

    /// A combo: the `TextEntry` body and text (ellipsis, fallback font, selection boxes), the
    /// arrow button with `icon_down`, the `ComboBoxBorder`.
    fn paint_combo(screen: &mut Screen, gui: &SpectatorGui, c: Combo, r: Rect, sh: i32, icon: Option<gs_draw::Image>) {
        let combo = &gui.combos[c.index()];
        fill(screen, r, COMBO_BG);
        let font = screen.client_font("Default");
        let f = screen.font_tall(font);
        let y = r.y + r.h / 2 - f / 2;
        let draw_w = r.w - psv(16, sh);
        let chars: Vec<char> = combo.text.chars().filter(|c| !c.is_control()).collect();
        let fits = |screen: &Screen, font: FontId| -> Option<usize> {
            let mut sum = 3;
            for (k, &ch) in chars.iter().enumerate() {
                sum += screen.char_width(font, ch);
                if sum > draw_w {
                    return Some(k);
                }
            }
            None
        };
        let mut use_font = font;
        let mut over = fits(screen, font);
        if over.is_some() && c == Combo::Player {
            use_font = screen.client_font("DefaultVerySmallFallBack");
            over = fits(screen, use_font);
        }
        let len = chars.len();
        let mut end = len;
        if let Some(mut i) = over.map(|i| i as i32) {
            let mut dots = 3 * screen.char_width(use_font, '.');
            while dots > 0 && i >= 0 {
                dots -= screen.char_width(use_font, chars[i as usize]);
                i -= 1;
            }
            end = (i + 1) as usize;
        }
        if len - end == 1 || len - end == 2 {
            end = len.saturating_sub(3);
        }
        let focused = matches!(gui.focus, Focus::Combo(f) | Focus::Menu(f) if f == c);
        let mut color = combo.color.unwrap_or(TEXT);
        let t = screen.font_tall(use_font);
        let mut x = r.x + 3;
        let shown = chars[..end].iter().copied().chain(if end < len { "...".chars().collect::<Vec<_>>() } else { Vec::new() });
        for ch in shown {
            let w = screen.char_width(use_font, ch);
            if combo.selected {
                fill(screen, Rect::new(x, y, w, t + 1), if focused { ARMED_BG } else { [255, 255, 255, 25] });
                color = TEXT;
            }
            glyphs(screen, use_font, x, y, &[ch], color);
            x += w;
        }
        let b = Rect::new(r.x + r.w - 1 - psv(20, sh), r.y + psv(2, sh), psv(16, sh), r.h - 1 - psv(2, sh));
        fill(screen, b, COMBO_BG);
        if let Some(img) = icon {
            let (iw, ih) = (psv(9, sh), psv(6, sh));
            screen.draw_image(img, b.x + b.w - iw, b.y + (b.h - f) / 2 + ((f - ih) >> 1), iw, ih, [255; 4]);
        }
        border(screen, r, COMBO_BORDER, BORDER);
    }

    /// prev/next: the state's fill, the Marlett ◀/▶ left-aligned, the `ButtonBorder`, and the
    /// dashed focus rectangle while focused.
    fn paint_buttons(screen: &mut Screen, gui: &SpectatorGui, geo: &Geometry) {
        let marlett = screen.client_font_proportional("Marlett");
        let m = screen.font_tall(marlett);
        let d = psv(1, geo.sh);
        for (i, glyph) in ['3', '4'].into_iter().enumerate() {
            let r = geo.buttons[i];
            let depressed = gui.armed[i] && gui.pressed[i];
            fill(screen, r, if gui.armed[i] && !depressed { ARMED_BG } else { BUTTON_BG });
            glyphs(screen, marlett, r.x + psv(6, geo.sh), r.y + (r.h - m) / 2, &[glyph], TEXT);
            border(screen, r, BUTTON_BORDER, BORDER);
            if gui.focus == Focus::Button(i) {
                let (x0, y0, x1, y1) = (r.x + 3, r.y + 3, r.x + r.w - 4, r.y + r.h - 2);
                dashed(screen, (x0, y0, x1, y0 + d), d, d);
                dashed(screen, (x0, y1 - d, x1, y1), d, d);
                dashed(screen, (x0, y0, x0 + d, y1), d, d);
                dashed(screen, (x1 - d, y0, x1, y1), d, d);
            }
        }
    }

    /// The open drop-down chain: bg, the scrollbar divider, items (armed highlight, check box,
    /// text, cascade arrow), the scrollbar, the `MenuBorder`.
    fn paint_menus(screen: &mut Screen, gui: &SpectatorGui, geo: &Geometry, icon_up: Option<gs_draw::Image>, icon_down: Option<gs_draw::Image>) {
        let Some((c, chain)) = &gui.open else { return };
        let combo = &gui.combos[c.index()];
        let placed = gui.place(geo, screen);
        let font = screen.client_font("Default");
        let fallback = screen.client_font("DefaultVerySmallFallBack");
        let marlett = screen.client_font("Marlett");
        let (tall, mt) = (screen.font_tall(font), screen.font_tall(marlett));
        let (check_w, arrow_w) = (screen.char_width(marlett, 'g'), screen.char_width(marlett, '4'));
        for (k, p) in placed.iter().enumerate() {
            let (m, armed) = chain[k];
            let items = &combo.menus[m];
            fill(screen, Rect::new(p.x, p.y, p.w, p.h), BAR_BG);
            if p.scrollbar {
                fill(screen, Rect::new(p.x + 1 + p.w - SCROLLBAR, p.y + 1, 1, p.h - 2), BORDER);
            }
            let any_check = items.iter().any(|i| matches!(i.kind, Kind::Toggle(_)));
            for row in 0..p.rows {
                let i = p.first + row;
                let Some(item) = items.get(i) else { break };
                let r = Rect::new(p.x + 1, p.y + 1 + ITEM_TALL * row as i32, p.wide - 2, ITEM_TALL);
                if armed == Some(i) {
                    fill(screen, r, ARMED_BG);
                }
                let toggle = matches!(item.kind, Kind::Toggle(_));
                let cascade = matches!(item.kind, Kind::Cascade(_));
                let max_h = if toggle || cascade { tall.max(mt) } else { tall };
                let ty0 = (ITEM_TALL - max_h) / 2;
                let glyph_y = r.y + ty0 + ((max_h - mt) >> 1);
                if toggle {
                    // MenuItemCheckImage: 'g' in the item's bg (unarmed: 0 0 0 0), then the check.
                    if armed == Some(i) {
                        screen.draw_char_rgba(r.x + 6, glyph_y, 'g', marlett, ARMED_BG.map(|v| v as u8));
                    }
                    if item.checked {
                        glyphs(screen, marlett, r.x + 6, glyph_y + 2, &['a'], TEXT);
                    }
                }
                let tx = if any_check { 6 + check_w } else { 6 };
                let avail = r.w - tx - if cascade { arrow_w } else { 0 };
                let text_r = Rect::new(r.x + tx, r.y + ty0 + ((max_h - tall) >> 1), avail, tall);
                let fb = (*c == Combo::Player).then_some(fallback);
                let label = screen.localize.text(&item.label).to_owned();
                paint_label(screen, text_r, &label, Align::West, TEXT, font, fb);
                if cascade {
                    glyphs(screen, marlett, r.x + r.w - arrow_w, glyph_y, &['4'], TEXT);
                }
            }
            if k == 0 && p.scrollbar {
                let count = items.len();
                let (bar, slider, (nob0, nob1)) = SpectatorGui::scrollbar(p, count, combo.scroll.min(count - PLAYER_LINES));
                fill(screen, Rect::new(slider.x, slider.y, slider.w - 1, slider.h - 1), SLIDER_BG);
                border(screen, Rect::new(slider.x, slider.y + nob0, slider.w - 1, nob1 - nob0), BUTTON_BORDER, BORDER);
                for (b, icon) in [(Rect::new(bar.x, bar.y, SCROLLBAR - 1, SCROLLBAR), icon_up), (Rect::new(bar.x, bar.y + bar.h - SCROLLBAR, SCROLLBAR - 1, SCROLLBAR), icon_down)] {
                    let b = Rect::new(b.x, b.y, b.w, b.h.min(p.y + p.h - 1 - b.y));
                    border(screen, b, COMBO_BORDER, BORDER);
                    if let Some(img) = icon {
                        // a_center puts the 9-wide icon at x 11; the button cuts it at 17.
                        let shown = (b.w - 11).clamp(0, img.width as i32);
                        if shown > 0 {
                            let uv = [0.0, 0.0, shown as f32 / img.width as f32, 1.0];
                            screen.draw_image_uv(img, b.x + 11, b.y + (SCROLLBAR - img.height as i32) / 2, shown, img.height as i32, uv, [255; 4]);
                        }
                    }
                }
            }
            border(screen, Rect::new(p.x, p.y, p.w, p.h), COMBO_BORDER, BORDER);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proportional_values_follow_the_screen_height() {
        let table = [(480, [1, 64, 66]), (600, [1, 80, 82]), (768, [1, 102, 105]), (960, [2, 128, 132]), (1024, [2, 136, 140]), (1080, [2, 144, 148])];
        for (sh, [one, bar, frame]) in table {
            assert_eq!([psv(1, sh), psv(64, sh), psv(66, sh)], [one, bar, frame], "SH {sh}");
        }
        // Negative offsets truncate toward zero: prev/next are not mirror images.
        assert_eq!((res_pos("c-175", 1280, 1024), res_pos("c155", 1280, 1024)), (267, 970));
        assert_eq!(res_pos("r233", 1024, 768), 652);
    }

    #[test]
    fn the_bars_and_bottom_controls_follow_the_res_rules() {
        let mut gui = SpectatorGui { screen: (1280, 1024), ..SpectatorGui::default() };
        let ctl = |name: &str, x: &str, y: &str, w: i32, h: i32| ResCtl { name: name.into(), xpos: x.into(), ypos: y.into(), wide: Some(w), tall: Some(h), ..ResCtl::default() };
        gui.bottom_res = Some(vec![ctl("settingscombo", "5", "30", 135, 22), ctl("playercombo", "c-150", "30", 300, 22), ctl("viewcombo", "r140", "30", 135, 22), ctl("playerprev", "c-175", "30", 20, 22), ctl("playernext", "c155", "30", 20, 22)]);
        let geo = gui.geometry();
        assert_eq!(geo.bar, 136);
        assert_eq!(geo.combos, [Rect::new(10, 952, 288, 46), Rect::new(320, 952, 640, 46), Rect::new(982, 952, 288, 46)]);
        assert_eq!(geo.buttons, [Rect::new(267, 952, 42, 46), Rect::new(970, 952, 42, 46)]);
    }

    #[test]
    fn labels_cut_with_dots_and_never_measure_the_first_character() {
        assert_eq!(amp("a&b&&c\r"), ['a', 'b', '&', 'c']);
        assert_eq!(file_base("maps/de_dust2.bsp"), "de_dust2");
        assert_eq!(spectator_label(4), "#OBS_IN_EYE");
        assert_eq!(spectator_label(0), "#OBS_NONE");
    }

    #[test]
    fn focus_moves_close_the_drop_down_and_a_selection_takes_the_focus_on_the_next_close() {
        let mut gui = SpectatorGui { screen: (640, 480), bar: true, visible: true, ..SpectatorGui::default() };
        gui.combos[Combo::View.index()].menus = vec![vec![Item { label: "x".into(), kind: Kind::Command("spec_mode 4".into()), checked: false }]];
        gui.open = Some((Combo::View, vec![(0, None)]));
        gui.focus = Focus::Menu(Combo::View);
        gui.set_focus(Focus::Button(0));
        assert!(gui.open.is_none(), "focus left the menu");
        assert!(!gui.combos[Combo::View.index()].selected);
        gui.combos[Combo::View.index()].highlight = true;
        gui.open = Some((Combo::View, vec![(0, None)]));
        gui.focus = Focus::Menu(Combo::View);
        gui.menu_close(Combo::View);
        assert_eq!(gui.focus, Focus::Combo(Combo::View), "m_bHighlight requests the focus");
        assert!(gui.combos[Combo::View.index()].selected, "OnSetFocus selects all");
        assert!(!gui.combos[Combo::View.index()].highlight);
    }

    #[test]
    fn the_inset_moves_the_top_bar_and_shows_the_bar_outline_only_with_the_bar() {
        let mut gui = SpectatorGui::default();
        gui.enable_inset_view(true, [4, 4, 600, 450]);
        assert_eq!((gui.top_x, gui.inset_box, gui.bar_inset, gui.inset_visible), (606, true, false, false));
        gui.bar = true;
        gui.enable_inset_view(true, [4, 4, 600, 450]);
        assert!(gui.bar_inset && gui.inset_visible);
        gui.deactivate_gui();
        assert!(!gui.inset_box && gui.bar_inset, "DeactivateGUI hides only the frame's outline");
        gui.set_banner("gfx/temp/0123.tga");
        assert_eq!(gui.banner.as_deref(), Some("gfx/temp/0123"));
        gui.hide_gui();
        assert!(gui.banner.is_none() && !gui.bar_inset);
    }
}
