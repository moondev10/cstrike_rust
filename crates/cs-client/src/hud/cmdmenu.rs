//! The command menu (docs/re/command-menu.md §1–§7): `commandmenu.txt` built into VGUI2
//! cascading popup menus on `+commandmenu`, hidden by `-commandmenu` once open for 0.3 s, used
//! with the mouse only (hover arms and opens sub-menus, a left release fires); and the
//! `BotProgress` panel (§8), drawn as it looks without `CSProgressBar.res`.
//!
//! Not modelled: the width change of menus with `TOGGLE` items on their first hover (§6.1; no
//! shipped menu has one) and `url ` commands opening the system browser.

use gs_common::bytestr::{decode, truncate_encoded};
use gs_common::token::com_parse;
use gs_draw::{FontId, Screen};

use super::msg::MsgReader;
use super::viewport::draw_text;
use super::HudEngine;

/// Menu item height, text inset, and colours (cstrike `ClientScheme.res`).
const ITEM_TALL: i32 = 22;
const TEXT_INSET: i32 = 6;
const MENU_BG: [i32; 4] = [0, 0, 0, 200];
const ARMED_BG: [i32; 4] = [255, 176, 0, 100];
const FG: [u8; 4] = [255, 176, 0, 255];
const BORDER: [i32; 4] = [188, 112, 0, 128];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Action {
    Command(String),
    Toggle(String),
    Submenu(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Item {
    pub(super) label: String,
    pub(super) action: Action,
    pub(super) checked: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Menu {
    pub(super) items: Vec<Item>,
}

/// What a click asks for (`CCommandMenu::OnCommand`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuCommand {
    /// `ShowVGUIMenu(4 / 5 / 6)` for `MAPBRIEFING` / `SERVERINFO` / `CLASSDESC`.
    ShowVgui(i32),
    /// `pfnClientCmd`.
    ClientCmd(String),
}

/// `COM_ParseFile` over the file; an empty string at the end (and for `""`).
struct Tokens<'a> {
    rest: &'a [u8],
    done: bool,
}

impl Tokens<'_> {
    fn next(&mut self) -> String {
        match com_parse(self.rest, false) {
            Some((t, r)) => {
                self.rest = r;
                decode(&t[..t.len().min(1023)])
            }
            None => {
                self.done = true;
                String::new()
            }
        }
    }
}

/// `strtol(s, NULL, 10)`.
fn strtol(s: &str) -> i32 {
    let s = s.trim_start();
    let (neg, d) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let v = d.bytes().take_while(u8::is_ascii_digit).fold(0i32, |a, c| a.wrapping_mul(10).wrapping_add(i32::from(c - b'0')));
    if neg { v.wrapping_neg() } else { v }
}

/// `CreateCommandMenu`: the item forms (§2.3) and the main loop (§2.4), with the parser state
/// fresh for every build and MAP skips stopping at the end of the data.
pub(super) fn parse(data: &[u8], team: i32, level: &str, team_names: &[String]) -> Vec<Menu> {
    let mut menus = vec![Menu::default()];
    let mut t = Tokens { rest: data, done: false };
    let mut tok = t.next();
    if tok.is_empty() {
        return menus;
    }
    let (mut current, mut stack, mut depth) = (0usize, Vec::new(), 0i32);
    loop {
        if tok != "}" {
            loop {
                let first = truncate_encoded(&tok, 31).into_owned();
                if first == "CUSTOM" {
                    let _key = t.next();
                    let label = truncate_encoded(&t.next(), 31).into_owned();
                    let command = truncate_encoded(&t.next(), 127).into_owned();
                    let look = t.next();
                    custom_button(&mut menus, current, label, &command, team, team_names);
                    tok = look;
                    if tok == "{" {
                        tok = t.next();
                    }
                } else {
                    let (mut item_team, mut map, mut toggle) = (-1, None, None);
                    if first == "MAP" {
                        map = Some(truncate_encoded(&t.next(), 255).into_owned());
                        let _key = t.next();
                    } else if let Some(n) = first.strip_prefix("TEAM") {
                        item_team = strtol(n);
                        let _key = t.next();
                    } else if first.starts_with("TOGGLE") {
                        let state = t.next();
                        toggle = Some(state.get(..4).is_some_and(|s| s.eq_ignore_ascii_case("true")));
                        let _key = t.next();
                    }
                    let label = truncate_encoded(&t.next(), 31).into_owned();
                    let command = truncate_encoded(&t.next(), 127).into_owned();
                    if command.starts_with('{') {
                        let other_map = map.as_deref().is_some_and(|m| !m.is_empty() && format!("maps/{m}.bsp") != level);
                        if other_map {
                            let mut d = 1;
                            while d > 0 && !t.done {
                                match t.next().as_str() {
                                    "{" => d += 1,
                                    "}" => d -= 1,
                                    _ => {}
                                }
                            }
                        } else {
                            let sub = menus.len();
                            menus.push(Menu::default());
                            menus[current].items.push(Item { label, action: Action::Submenu(sub), checked: false });
                            stack.push(current);
                            current = sub;
                            depth += 1;
                        }
                    } else if item_team == -1 {
                        let item = match toggle {
                            Some(checked) => Item { label, action: Action::Toggle(command), checked },
                            None => Item { label, action: Action::Command(command), checked: false },
                        };
                        menus[current].items.push(item);
                    } else if item_team == team {
                        menus[current].items.push(Item { label, action: Action::Command(command), checked: false });
                    }
                    tok = t.next();
                }
                if tok.is_empty() || tok == "}" {
                    break;
                }
            }
        }
        if current != 0 {
            if depth <= 0 {
                current = 0;
            } else {
                current = stack.pop().unwrap_or(0);
                depth -= 1;
            }
        }
        tok = t.next();
        if tok.is_empty() {
            return menus;
        }
    }
}

/// `CreateCustomButton` (§3).
fn custom_button(menus: &mut Vec<Menu>, current: usize, label: String, command: &str, team: i32, team_names: &[String]) {
    match command {
        "!MAPBRIEFING" | "!CLASSDESC" | "!SERVERINFO" => {
            menus[current].items.push(Item { label, action: Action::Command(command[1..].to_owned()), checked: false });
        }
        "!CHANGETEAM" | "!CHANGECLASS" => {
            let sub = menus.len();
            menus.push(Menu::default());
            menus[current].items.push(Item { label, action: Action::Submenu(sub), checked: false });
            if command == "!CHANGETEAM" {
                // m_sTeamNames[0] is NULL in CS: the labels are off by one.
                let name = |i: usize| if i == 0 { String::new() } else { team_names.get(i - 1).cloned().unwrap_or_default() };
                let n = team_names.len() as i32;
                let mut add = |label: String, cmd: &str| menus[sub].items.push(Item { label, action: Action::Command(cmd.to_owned()), checked: false });
                for k in 0..4 {
                    if n > k - 1 && team != k {
                        add(name(k as usize), &format!("jointeam {}", k + 1));
                    }
                }
                add("#Team_AutoAssign".to_owned(), "jointeam 5");
                add("#Menu_Spectate".to_owned(), "spectate");
            }
        }
        _ => {}
    }
}

/// The text a label shows: `#tokens` localised (else raw, `#` included); `&` hidden, `&&` = `&`.
pub(super) fn display(screen: &Screen, label: &str) -> String {
    let text = screen.localize.text(label);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                chars.next();
                out.push('&');
            }
            continue;
        }
        if c != '\r' {
            out.push(c);
        }
    }
    out
}

/// One open menu's placement.
#[derive(Debug, Clone, Copy)]
struct Placed {
    x: i32,
    y: i32,
    /// `menuWide` (items are `wide - 2` across) and the panel size.
    wide: i32,
    w: i32,
    h: i32,
}

/// `CCommandMenu`.
#[derive(Debug, Default)]
pub struct CommandMenu {
    /// `m_iTeamNumber`, `m_szMap`, `m_flCommandMenuOpened`.
    team: i32,
    map: String,
    opened: f32,
    visible: bool,
    /// The built menus (root first); empty while `m_pMenu` is NULL.
    menus: Vec<Menu>,
    /// The open menus from the root and their armed items.
    chain: Vec<(usize, Option<usize>)>,
}

impl CommandMenu {
    /// `IsCommandMenuVisible`.
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// `ShowCommandMenu` (`+commandmenu`): rebuilt only when the team or the level changed.
    pub fn show(&mut self, team: i32, team_names: &[String], hud_time: f32, eng: &mut dyn HudEngine) {
        if self.visible {
            return;
        }
        let mut rebuild = false;
        if self.team != team {
            self.team = team;
            rebuild = true;
        }
        let level = eng.level_name();
        if level != self.map {
            self.map = truncate_encoded(&level, 255).into_owned();
            rebuild = true;
        }
        if rebuild {
            self.menus = eng.read_file("commandmenu.txt").map(|d| parse(&d, self.team, &self.map, team_names)).unwrap_or_default();
            self.chain.clear();
        }
        if !self.menus.is_empty() {
            self.visible = true;
            self.chain = vec![(0, None)];
            self.opened = hud_time;
        }
    }

    /// `HideCommandMenu` (`-commandmenu`, `HideAllVGUIMenu`): only once open for 0.3 s.
    pub fn hide(&mut self, hud_time: f32) {
        if !self.menus.is_empty() && f64::from(self.opened) + 0.3 <= f64::from(hud_time) {
            self.visible = false;
            self.chain.clear();
        }
    }

    fn fonts(screen: &mut Screen) -> (FontId, FontId) {
        (screen.client_font("Default"), screen.client_font("Marlett"))
    }

    /// `menuWide`: the widest item's content (`6 + text`, + the arrow for a cascade,
    /// `6 + check + 12 + text` for a checkable item) plus 8; 0 for an empty menu.
    fn menu_wide(&self, m: usize, screen: &mut Screen) -> i32 {
        let (font, marlett) = Self::fonts(screen);
        let arrow_w = screen.char_width(marlett, '4');
        let check_w = screen.char_width(marlett, 'g');
        let items = &self.menus[m].items;
        if items.is_empty() {
            return 0;
        }
        let widest = items
            .iter()
            .map(|it| {
                let t = screen.text_width(font, &display(screen, &it.label));
                match it.action {
                    Action::Command(_) => TEXT_INSET + t,
                    Action::Submenu(_) => TEXT_INSET + t + arrow_w,
                    Action::Toggle(_) => TEXT_INSET + check_w + 12 + t,
                }
            })
            .max()
            .unwrap_or(0);
        widest + 8
    }

    /// The open menus' rectangles: the root at the left edge, vertically centred; each
    /// sub-menu beside its armed item (flipped left when it would leave the screen).
    fn layout(&self, screen: &mut Screen) -> Vec<Placed> {
        let (sw, sh) = (screen.width as i32, screen.height as i32);
        let mut placed: Vec<Placed> = Vec::with_capacity(self.chain.len());
        for (k, &(m, _)) in self.chain.iter().enumerate() {
            let wide = self.menu_wide(m, screen);
            let (w, h) = (wide + 5, ITEM_TALL * self.menus[m].items.len() as i32 + 2);
            let (x, y) = match k.checked_sub(1).and_then(|p| placed.get(p).map(|pp| (pp, self.chain[p].1))) {
                Some((parent, Some(armed))) => {
                    let (ix, iy, iw) = (parent.x + 1, parent.y + 1 + ITEM_TALL * armed as i32, parent.wide - 2);
                    if ix + iw + w > sw { (ix - w - 2, iy - 1) } else { (ix + iw + 1, iy - 1) }
                }
                _ => (0, ((sh - h) / 2).max(0)),
            };
            placed.push(Placed { x, y, wide, w, h });
        }
        placed
    }

    /// The deepest open menu and item under `(x, y)`.
    fn hit(&self, placed: &[Placed], x: i32, y: i32) -> Option<(usize, Option<usize>)> {
        let k = (0..placed.len()).rev().find(|&k| {
            let p = placed[k];
            x >= p.x && x < p.x + p.w && y >= p.y && y < p.y + p.h
        })?;
        let p = placed[k];
        let count = self.menus[self.chain[k].0].items.len() as i32;
        let (ix, iy) = (x - p.x - 1, y - p.y - 1);
        let item = (ix >= 0 && ix < p.wide - 2 && iy >= 0 && iy < ITEM_TALL * count).then_some((iy / ITEM_TALL) as usize);
        Some((k, item))
    }

    /// Cursor movement: a plain item left behind disarms; entering an item arms it, closes
    /// the other sub-menus and opens its own at once.
    pub fn hover(&mut self, x: i32, y: i32, screen: &mut Screen) {
        if !self.visible {
            return;
        }
        let placed = self.layout(screen);
        let hit = self.hit(&placed, x, y);
        for k in 0..self.chain.len() {
            let (m, armed) = self.chain[k];
            if let Some(i) = armed
                && !matches!(self.menus[m].items[i].action, Action::Submenu(_))
                && hit != Some((k, Some(i)))
            {
                self.chain[k].1 = None;
            }
        }
        let Some((k, Some(i))) = hit else { return };
        let m = self.chain[k].0;
        if self.chain[k].1 == Some(i) && self.chain.len() > k + 1 {
            return;
        }
        self.chain.truncate(k + 1);
        self.chain[k].1 = Some(i);
        if let Action::Submenu(sub) = self.menus[m].items[i].action {
            self.chain.push((sub, None));
        }
    }

    /// A left-button release: an item under the cursor fires (the menus close; a checkable
    /// item flips afterwards); a cascade item does nothing.
    pub fn release(&mut self, x: i32, y: i32, screen: &mut Screen) -> Option<MenuCommand> {
        if !self.visible {
            return None;
        }
        let placed = self.layout(screen);
        let (k, Some(i)) = self.hit(&placed, x, y)? else { return None };
        let m = self.chain[k].0;
        let command = match &self.menus[m].items[i].action {
            Action::Command(c) | Action::Toggle(c) => c.clone(),
            Action::Submenu(_) => return None,
        };
        if matches!(self.menus[m].items[i].action, Action::Toggle(_)) {
            let item = &mut self.menus[m].items[i];
            item.checked = !item.checked;
        }
        self.visible = false;
        self.chain.clear();
        Some(match command.as_str() {
            "MAPBRIEFING" => MenuCommand::ShowVgui(4),
            "CLASSDESC" => MenuCommand::ShowVgui(6),
            "SERVERINFO" => MenuCommand::ShowVgui(5),
            _ => MenuCommand::ClientCmd(command),
        })
    }

    /// The open menus, root first: background, the armed item's highlight, text and arrows,
    /// then the border lines (L, T, R, B).
    pub fn draw(&self, eng: &mut dyn HudEngine) {
        if !self.visible {
            return;
        }
        let screen = eng.screen();
        let placed = self.layout(screen);
        let (font, marlett) = Self::fonts(screen);
        let tall = screen.font_tall(font);
        let arrow_w = screen.char_width(marlett, '4');
        let arrow_h = screen.font_tall(marlett);
        for (k, p) in placed.iter().enumerate() {
            let (m, armed) = self.chain[k];
            let fill = |s: &mut Screen, x: i32, y: i32, w: i32, h: i32, c: [i32; 4]| s.fill_rgba_blend(x, y, w, h, c[0], c[1], c[2], c[3]);
            fill(screen, p.x, p.y, p.w, p.h, MENU_BG);
            for (i, item) in self.menus[m].items.iter().enumerate() {
                let (ix, iy, iw) = (p.x + 1, p.y + 1 + ITEM_TALL * i as i32, p.wide - 2);
                if armed == Some(i) {
                    fill(screen, ix, iy, iw, ITEM_TALL, ARMED_BG);
                }
                let text = display(screen, &item.label);
                let max_h = tall.max(arrow_h);
                let y0 = (ITEM_TALL - max_h) / 2;
                let mut tx = ix + TEXT_INSET;
                if let Action::Toggle(_) = item.action {
                    let iy0 = iy + y0 + ((max_h - arrow_h) >> 1);
                    if armed == Some(i) {
                        let c = ARMED_BG.map(|v| v as u8);
                        draw_text(screen, marlett, tx, iy0, "g", c);
                    }
                    if item.checked {
                        draw_text(screen, marlett, tx, iy0 + 2, "a", FG);
                    }
                    tx += screen.char_width(marlett, 'g');
                }
                draw_text(screen, font, tx, iy + (ITEM_TALL - tall) / 2, &text, FG);
                if let Action::Submenu(_) = item.action {
                    draw_text(screen, marlett, ix + iw - arrow_w, iy + y0 + ((max_h - arrow_h) >> 1), "4", FG);
                }
            }
            fill(screen, p.x, p.y, 1, p.h - 1, BORDER);
            fill(screen, p.x, p.y, p.w, 1, BORDER);
            fill(screen, p.x + p.w - 1, p.y + 1, 1, p.h - 1, BORDER);
            fill(screen, p.x, p.y + p.h - 1, p.w, 1, BORDER);
        }
    }
}

/// `CCSProgressBar` (the `BotProgress` panel).
#[derive(Debug, Default)]
pub struct ProgressBar {
    visible: bool,
    title: String,
    text: String,
    num_ticks: i32,
    tick: i32,
}

impl ProgressBar {
    /// `IsProgressBarVisible` (also counts as an active VGUI menu).
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// `__MsgFunc_BotProgress`: 0 update (starting a hidden bar), 1 start, 2 stop.
    pub fn msg(&mut self, r: &mut MsgReader) {
        match r.byte() {
            0 => {
                let tick = r.byte();
                let text = r.string();
                if !self.visible {
                    self.start(&text);
                }
                self.text = text;
                self.tick = tick;
            }
            1 => {
                let title = r.string();
                self.start(&title);
            }
            2 => self.visible = false,
            _ => {}
        }
    }

    /// `StartProgressBar(title, 100, 0, false)` and `Frame::Activate`.
    fn start(&mut self, title: &str) {
        title.clone_into(&mut self.title);
        self.text.clear();
        self.num_ticks = 100;
        self.tick = 0;
        self.visible = true;
    }

    /// Hidden on every level change (`UpdateScheme`).
    pub fn level_change(&mut self) {
        self.visible = false;
    }

    /// A 64×24 rounded box at the top-left of an invisible 128×66 frame centred on screen:
    /// translucent black with an orange outline, the bar filling `tick·64/numTicks`, the title
    /// and status cut to 64 px, the corner images over everything.
    pub fn draw(&self, eng: &mut dyn HudEngine) {
        if !self.visible {
            return;
        }
        let corners = ["nw", "ne", "sw", "se"].map(|c| (eng.load_image(&format!("gfx/vgui/round_corner_{c}")), eng.load_image(&format!("gfx/vgui/round_corner_small_{c}"))));
        let screen = eng.screen();
        let (fx, fy) = (screen.width as i32 / 2 - 64, screen.height as i32 / 2 - 33);
        let rect = |s: &mut Screen, x0: i32, y0: i32, x1: i32, y1: i32, c: [i32; 4]| s.fill_rgba_blend(fx + x0, fy + y0, x1 - x0, y1 - y0, c[0], c[1], c[2], c[3]);
        let bg = [0, 0, 0, 128];
        rect(screen, 0, 10, 10, 13, bg);
        rect(screen, 10, 0, 53, 23, bg);
        rect(screen, 53, 10, 63, 13, bg);
        rect(screen, 0, 10, 1, 14, BORDER);
        rect(screen, 10, 0, 54, 1, BORDER);
        rect(screen, 10, 23, 54, 24, BORDER);
        rect(screen, 63, 10, 64, 14, BORDER);
        let font = screen.client_font("Default");
        let tall = screen.font_tall(font);
        for text in [&self.title, &self.text] {
            let localized = screen.localize.text(text).to_owned();
            let shown = ellipsis(screen, font, &localized, 64);
            draw_text(screen, font, fx, fy + (24 - tall) / 2, &shown, FG);
        }
        // ProgressBarBorder, then the bar with its ButtonBorder.
        let lines = |s: &mut Screen, w: i32| {
            rect(s, 0, 0, 1, 23, BORDER);
            rect(s, 1, 0, w - 1, 1, BORDER);
            rect(s, w - 1, 0, w, 24, BORDER);
            rect(s, 0, 23, w, 24, BORDER);
        };
        lines(screen, 64);
        if self.num_ticks != 0 {
            let w = self.tick * 64 / self.num_ticks;
            rect(screen, 0, 0, w, 24, ARMED_BG);
            if w > 0 {
                lines(screen, w);
            }
        }
        for ((big, small), (x, y)) in corners.into_iter().zip([(0, 0), (53, 0), (0, 13), (53, 13)]) {
            if let Some(img) = big {
                screen.draw_image(img, fx + x, fy + y, 10, 10, [255, 255, 255, 128]);
            }
            if let Some(img) = small {
                screen.draw_image(img, fx + x, fy + y, 10, 10, [188, 112, 0, 128]);
            }
        }
    }
}

/// `TextImage` ellipsis: the text cut so it and "..." fit `width`; the first character stays.
fn ellipsis(screen: &mut Screen, font: FontId, text: &str, width: i32) -> String {
    if screen.text_width(font, text) <= width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 1 {
        n -= 1;
        let s: String = chars[..n].iter().collect::<String>() + "...";
        if screen.text_width(font, &s) <= width {
            return s;
        }
    }
    chars.first().map_or_else(String::new, |c| format!("{c}..."))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &[u8] = br##"
// comment
"1" "#Cstrike_Help" {
    CUSTOM "1" "Map Description" "!MAPBRIEFING"
    "2" "Time Left" "timeleft"
}
"2" "Change Team" "chooseteam"
"3" "Team Message" {
    MAP "de_dust2" "1" "dust2" {
        "1" "Go A" "say_team go a"
    }
    MAP "cs_747" "2" "747" {
        "1" "x" "y"
    }
}
TEAM2 "4" "CT only" "say ct"
TEAM1 "5" "T only" "say t"
"##;

    #[test]
    fn the_shipped_forms_build_the_menu_tree() {
        let menus = parse(FILE, 2, "maps/de_dust2.bsp", &[]);
        let root: Vec<&str> = menus[0].items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(root, ["#Cstrike_Help", "Change Team", "Team Message", "CT only"]);
        assert_eq!(menus[1].items[0].action, Action::Command("MAPBRIEFING".into()));
        let Action::Submenu(team_msg) = menus[0].items[2].action else { panic!("submenu") };
        assert_eq!(menus[team_msg].items.len(), 1, "only the current map's block");
        let Action::Submenu(dust) = menus[team_msg].items[0].action else { panic!("submenu") };
        assert_eq!(menus[dust].items[0].action, Action::Command("say_team go a".into()));
    }

    #[test]
    fn an_empty_token_where_an_item_should_start_closes_the_open_menu() {
        let menus = parse(br#""1" "s" { "1" "a" "x" "" "2" "b" "y" }"#, 0, "", &[]);
        assert_eq!(menus[1].items.len(), 1, "the sub-menu ends at the empty token");
        let root: Vec<&str> = menus[0].items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(root, ["s", "b"], "the next item lands in the parent");
    }

    #[test]
    fn an_unterminated_map_skip_stops_at_the_end() {
        let menus = parse(b"MAP \"other\" \"1\" \"m\" { \"1\" \"a\" \"x\"", 0, "maps/de_dust2.bsp", &[]);
        assert!(menus[0].items.is_empty());
    }
}
