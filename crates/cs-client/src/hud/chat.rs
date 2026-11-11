//! Text systems (docs/re/client-hud.md §6.2, §6.3, §6.7): `CHudSayText` (chat lines with
//! colour ranges, wrapping and scrolling), `TextMsg` routing, and the centre print
//! (`CHudVGUI2Print`).

use gs_draw::Screen;

use super::health::team_color;
use super::{Hud, HudEngine, MsgReader, PlayerExtra};

const MAX_LINES: usize = 5;
/// `g_LocationColor` × 255.
const LOCATION_GREEN: [i32; 3] = [0, 204, 0];

/// The team number of player `index` (0 when out of range).
fn team_of(players: &[PlayerExtra], index: i32) -> i32 {
    usize::try_from(index).ok().and_then(|i| players.get(i)).map_or(0, |p| p.teamnumber)
}

/// `ConstructString`: `%s1`..`%s4` replaced by the arguments.
pub(super) fn construct(fmt: &str, args: &[String]) -> String {
    let mut out = fmt.to_owned();
    for (i, a) in args.iter().enumerate().take(4) {
        out = out.replace(&format!("%s{}", i + 1), a);
    }
    out
}

/// C `printf` of `%s` placeholders in order (`%%` → `%`).
fn sprintf(fmt: &str, args: &[String]) -> String {
    let mut out = String::new();
    let mut next = args.iter();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.peek() {
                Some('s') => {
                    chars.next();
                    out.push_str(next.next().map_or("", String::as_str));
                    continue;
                }
                Some('%') => {
                    chars.next();
                    out.push('%');
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

/// Format sanitising: every `%` that is neither `%%` nor one of the first four `%s` becomes
/// a space.
fn sanitise(fmt: &str) -> String {
    let mut out = String::with_capacity(fmt.len());
    let mut chars = fmt.chars().peekable();
    let mut s_count = 0;
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('%') => {
                chars.next();
                out.push_str("%%");
            }
            Some('s') if s_count < 4 => {
                s_count += 1;
                out.push('%');
            }
            _ => out.push(' '),
        }
    }
    out
}

/// One chat line: characters with their colours.
#[derive(Debug, Clone, Default)]
struct Line {
    text: Vec<(char, [i32; 3])>,
}

impl Line {
    fn plain(&self) -> String {
        self.text.iter().map(|(c, _)| *c).collect()
    }
}

#[derive(Default)]
pub(super) struct SayText {
    lines: Vec<Line>,
    scroll_time: f32,
}

impl SayText {
    pub(super) fn clear(&mut self) {
        self.lines.clear();
    }

    /// `MsgFunc_SayText`.
    pub(super) fn msg_say_text(&mut self, r: &mut MsgReader, now: f32, players: &[PlayerExtra], eng: &mut dyn HudEngine) {
        let sender = r.byte();
        let fmt = r.string();
        let mut s1 = r.string();
        if s1.is_empty() && sender > 0 {
            s1 = eng.player_info(sender).map(|p| p.name).unwrap_or_default();
        }
        let args = [s1, r.string(), r.string(), r.string()];
        self.print(&fmt, sender, team_of(players, sender), &args, now, eng);
    }

    /// `SayTextPrint(fmt, sender, s1..s4)`; `team` colours the sender.
    pub(super) fn print(&mut self, fmt: &str, sender: i32, team: i32, args: &[String], now: f32, eng: &mut dyn HudEngine) {
        let fmt = fmt.trim_end_matches(['\r', '\n']);
        let (screen_w, con_color) = {
            let s = eng.screen();
            (s.width as i32, s.con_color.map(i32::from))
        };
        let text = {
            let screen = eng.screen();
            let fmt = match screen.localize.find(fmt).filter(|_| fmt.starts_with('#')) {
                Some(t) => t.to_owned(),
                None => fmt.split(' ').map(|w| if w.starts_with('#') { screen.localize.text(w).to_owned() } else { w.to_owned() }).collect::<Vec<_>>().join(" "),
            };
            let args: Vec<String> = args.iter().map(|a| screen.localize.text(a).to_owned()).collect();
            let bytes: Vec<char> = fmt.chars().collect();
            let positional = bytes.windows(3).any(|w| w[0] == '%' && w[2].is_ascii_digit());
            // A printf-style format takes the args in order: radio lines ("%s (RADIO): %s"
            // from titles.txt) show both the name and the message.
            if positional { construct(&fmt, &args) } else { sprintf(&fmt, &args) }
        };
        let text: String = text.chars().take(255).collect();
        let sender_name = eng.player_info(sender).map(|p| p.name).unwrap_or_default();
        let line = colorize(&text, &sender_name, team_color(team), con_color);
        if self.lines.len() >= MAX_LINES {
            self.scroll(eng);
        }
        let font = eng.screen().font("EngineFont");
        self.push_wrapped(line, screen_w, font, eng.screen());
        if self.lines.len() == 1 {
            self.scroll_time = now + eng.cvar("hud_saytext_time");
        }
    }

    /// Adds `line`, wrapping at the screen width (`width + 9 ≥ W - 40`).
    fn push_wrapped(&mut self, line: Line, screen_w: i32, font: gs_draw::FontId, screen: &mut Screen) {
        let mut width = 0;
        for (i, (c, _)) in line.text.iter().enumerate() {
            width += screen.text_width(font, &c.to_string());
            if width + 9 >= screen_w - 40 {
                let cut = line.text[..i].iter().rposition(|(c, _)| *c == ' ').filter(|&p| p > 0).unwrap_or(i);
                let head = Line { text: line.text[..cut].to_vec() };
                let mut rest = line.text[cut..].to_vec();
                if rest.first().is_none_or(|(c, _)| *c != ' ') {
                    let color = rest.first().map_or([255, 255, 255], |(_, c)| *c);
                    rest.insert(0, (' ', color));
                }
                self.lines.push(head);
                if self.lines.len() >= MAX_LINES {
                    return;
                }
                self.push_wrapped(Line { text: rest }, screen_w, font, screen);
                return;
            }
        }
        self.lines.push(line);
    }

    /// Prints line 0 to the console and shifts the rest up (continuations go with it).
    fn scroll(&mut self, eng: &mut dyn HudEngine) {
        if self.lines.is_empty() {
            return;
        }
        let first = self.lines.remove(0);
        let text: String = first.plain().chars().filter(|&c| c as u32 > 5).collect();
        eng.console_print(&format!("{text}\n"));
        while self.lines.first().is_some_and(|l| l.text.first().is_some_and(|(c, _)| *c == ' ')) {
            let cont = self.lines.remove(0);
            let text: String = cont.plain().chars().filter(|&c| c as u32 > 5).collect();
            eng.console_print(&format!("{text}\n"));
        }
    }

    pub(super) fn draw(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        if eng.cvar("hud_saytext_internal") == 0.0 {
            return;
        }
        if !self.lines.is_empty() && hud.time >= self.scroll_time {
            self.scroll(eng);
            self.scroll_time = hud.time + eng.cvar("hud_saytext_time");
        }
        let observing = hud.state.user1 != 0 || eng.spectate_only();
        let screen = eng.screen();
        let font = screen.font("EngineFont");
        let line_h = screen.font_tall(font);
        // Above the spectator bottom bar (viewport slot 51) while observing or on HLTV.
        let base_y = if observing {
            let h = screen.height as i32;
            h - (h as f32 / h.min(480) as f32 * 64.0) as i32 - 4
        } else {
            screen.height as i32 - (hud.font_height as f32 * 2.75) as i32
        };
        let mut y = base_y - (5 * line_h + line_h / 2);
        for line in &self.lines {
            let mut x = 10;
            for &(c, color) in &line.text {
                if (c as u32) < 32 {
                    continue;
                }
                screen.set_text_color(color[0] as f32 / 255.0, color[1] as f32 / 255.0, color[2] as f32 / 255.0);
                x += screen.vgui2_draw_character(x, y, c, font);
            }
            y += line_h;
        }
    }
}

/// `SayTextLine::Colorize`: 0x02 colours the sender's name; 0x01/0x03/0x04 start default,
/// team and green ranges.
fn colorize(text: &str, name: &str, team: [i32; 3], default: [i32; 3]) -> Line {
    let chars: Vec<char> = text.chars().collect();
    match chars.first() {
        Some('\u{2}') => {
            let end = text[1..].find(name).map(|i| text[..1 + i].chars().count() + name.chars().count());
            let line = chars
                .iter()
                .enumerate()
                .map(|(i, &c)| (c, if end.is_some_and(|e| (1..e).contains(&i)) { team } else { default }))
                .collect();
            Line { text: line }
        }
        Some('\u{1}' | '\u{3}' | '\u{4}') => {
            let mut color = default;
            let line = chars
                .iter()
                .map(|&c| {
                    match c {
                        '\u{1}' => color = default,
                        '\u{3}' => color = team,
                        '\u{4}' => color = LOCATION_GREEN,
                        _ => {}
                    }
                    (c, color)
                })
                .collect();
            Line { text: line }
        }
        _ => Line { text: chars.iter().map(|&c| (c, default)).collect() },
    }
}

/// `CHudVGUI2Print`: centred text for `scr_centertime` seconds.
#[derive(Default)]
pub(super) struct CentrePrint {
    text: String,
    y: i32,
    color: [i32; 3],
    until: f32,
}

impl CentrePrint {
    /// `VGUI2HudPrintArgs`.
    pub(super) fn print(&mut self, fmt: &str, args: &[String], y: i32, color: [i32; 3], now: f32, eng: &mut dyn HudEngine) {
        let screen = eng.screen();
        let args: Vec<String> = args.iter().map(|a| screen.localize.text(a).to_owned()).collect();
        self.text = match screen.localize.find(fmt).filter(|_| fmt.starts_with('#')) {
            Some(t) => construct(t, &args),
            None => sprintf(fmt, &args),
        };
        self.y = y;
        self.color = color;
        let mut time = eng.cvar("scr_centertime");
        if time < 0.0 {
            time = 2.0;
        }
        self.until = now + time;
    }

    pub(super) fn draw(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        if hud.time > self.until || self.text.is_empty() {
            return;
        }
        let screen = eng.screen();
        let font = screen.font("EngineFont");
        let tall = screen.font_tall(font);
        let mut y = self.y;
        for line in self.text.split('\r') {
            let printable: String = line.chars().filter(|c| !c.is_control()).collect();
            let width = screen.text_width(font, &printable);
            let mut x = (screen.width as i32 - width) / 2;
            screen.set_text_color(self.color[0] as f32 / 255.0, self.color[1] as f32 / 255.0, self.color[2] as f32 / 255.0);
            for c in printable.chars() {
                x += screen.vgui2_draw_character(x, y, c, font);
            }
            y += tall;
        }
    }
}

/// `CHudTextMessage::LocaliseTextString`: each `#name` (letters, digits, `_`) with a titles.txt
/// entry becomes that entry's text; an unknown name stays as written, `#` included.
pub(super) fn localise_text_string(text: &str, eng: &dyn HudEngine) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('#') {
        out.push_str(&rest[..i]);
        let name_len = rest[i + 1..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len() - i - 1);
        match eng.text_message(&rest[i + 1..i + 1 + name_len]) {
            Some(t) => out.push_str(&t.message),
            None => out.push_str(&rest[i..i + 1 + name_len]),
        }
        rest = &rest[i + 1 + name_len..];
    }
    out.push_str(rest);
    out
}

/// `MsgFunc_TextMsg`: destinations 1 notify, 2 console, 3 talk, 4 centre, 5 radio.
pub(super) fn msg_text_msg(
    r: &mut MsgReader,
    chat: &mut SayText,
    centre: &mut CentrePrint,
    players: &[PlayerExtra],
    now: f32,
    eng: &mut dyn HudEngine,
) {
    let mut dest = r.byte();
    let sender = if dest == 5 { r.string().trim().parse::<i32>().unwrap_or(0) } else { -1 };
    let mut msg = gs_common::bytestr::truncate_encoded(&r.string(), 127).into_owned();
    if let Some(name) = msg.strip_prefix('#')
        && let Some(t) = eng.text_message(name)
    {
        if t.effect < 0 {
            dest = -t.effect;
        }
        msg = t.message;
    }
    let args: Vec<String> = (0..4)
        .map(|_| {
            let a = r.string();
            let a = match a.strip_prefix('#').and_then(|n| eng.text_message(n)) {
                Some(t) => t.message,
                None => a,
            };
            gs_common::bytestr::truncate_encoded(a.trim_end_matches(['\n', '\r']), 127).into_owned()
        })
        .collect();
    let fmt = sanitise(&msg);
    match dest {
        1 => eng.console_print(&format!("\u{1}{}", sprintf(&fmt, &args).replace('\r', "\n"))),
        2 => eng.console_print(&sprintf(&fmt, &args).replace('\r', "\n")),
        3 => chat.print(&fmt, -1, 0, &args, now, eng),
        5 => chat.print(&fmt, sender, team_of(players, sender), &args, now, eng),
        4 => {
            let fmt = fmt.trim_end_matches(['\n', '\r']).to_owned();
            let y = (0.35 * eng.screen().height as f32) as i32;
            centre.print(&fmt, &args, y, [255, 180, 30], now, eng);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use gs_draw::titles::TextMessage;

    use super::*;

    #[test]
    fn formats_follow_printf_and_construct_string() {
        let args = ["Bob".to_owned(), "hi".to_owned(), String::new(), String::new()];
        assert_eq!(construct("\u{2}%s1 :  %s2", &args), "\u{2}Bob :  hi");
        assert_eq!(sprintf("%s joined 100%%", &args), "Bob joined 100%");
        assert_eq!(sanitise("%d %s %s %s %s %s"), " d %s %s %s %s  s");
    }

    #[test]
    fn colour_ranges_follow_control_characters() {
        let line = colorize("\u{2}Bob :  hi", "Bob", [1, 1, 1], [9, 9, 9]);
        let colours: Vec<[i32; 3]> = line.text.iter().map(|(_, c)| *c).collect();
        assert_eq!(&colours[..5], &[[9, 9, 9], [1, 1, 1], [1, 1, 1], [1, 1, 1], [9, 9, 9]]);
        let line = colorize("\u{1}a\u{3}b\u{4}c", "", [1, 1, 1], [9, 9, 9]);
        let colours: Vec<[i32; 3]> = line.text.iter().map(|(_, c)| *c).collect();
        assert_eq!(colours, [[9, 9, 9], [9, 9, 9], [1, 1, 1], [1, 1, 1], LOCATION_GREEN, LOCATION_GREEN]);
    }

    #[test]
    fn radio_lines_show_the_name_and_the_message() {
        let mut eng = crate::hud::tests::Engine::new();
        for (name, message) in [("Game_radio", "%s (RADIO): %s"), ("Fire_in_the_hole", "Fire in the hole!")] {
            eng.titles.insert(name.to_owned(), TextMessage { message: message.to_owned(), ..TextMessage::default() });
        }
        let mut data = vec![5];
        for s in ["1", "#Game_radio", "me", "#Fire_in_the_hole"] {
            data.extend(s.bytes());
            data.push(0);
        }
        let (mut chat, mut centre) = (SayText::default(), CentrePrint::default());
        msg_text_msg(&mut MsgReader::new(&data), &mut chat, &mut centre, &vec![PlayerExtra::default(); 34], 0.0, &mut eng);
        assert_eq!(chat.lines.first().map(Line::plain).as_deref(), Some("me (RADIO): Fire in the hole!"));
    }
}
