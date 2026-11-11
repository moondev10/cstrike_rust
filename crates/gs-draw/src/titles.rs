//! `titles.txt` (docs/re/client-hud.md §2.3.6): `client_textmessage_t` records parsed like
//! `TextMessageParse`, with sticky `$` directives and raw message bodies.

/// `client_textmessage_t`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextMessage {
    pub effect: i32,
    /// `r1 g1 b1 a1` (alpha is never set by the parser).
    pub color1: [u8; 4],
    pub color2: [u8; 4],
    pub x: f32,
    pub y: f32,
    pub fadein: f32,
    pub fadeout: f32,
    pub holdtime: f32,
    pub fxtime: f32,
    pub name: String,
    pub message: String,
}

/// More messages abort the parse (`messageCount>=MAX_MESSAGES`).
const MAX_MESSAGES: usize = 1000;

/// The parsed table (`gMessageTable`).
#[derive(Debug, Clone, Default)]
pub struct Titles {
    messages: Vec<TextMessage>,
}

/// Up to `n` whitespace-separated floats (missing ones are 0).
fn floats<const N: usize>(s: &str) -> [f32; N] {
    let mut out = [0.0; N];
    for (o, v) in out.iter_mut().zip(s.split_whitespace()) {
        *o = v.parse().unwrap_or(0.0);
    }
    out
}

/// Applies a `$directive` line to the template; unknown ones are logged.
fn directive(line: &str, t: &mut TextMessage) {
    let body = &line[1..];
    let starts = |k: &str| body.get(..k.len()).is_some_and(|p| p.eq_ignore_ascii_case(k));
    let rest = |k: &str| &body[k.len()..];
    let rgb = |v: [f32; 3]| [v[0] as u8, v[1] as u8, v[2] as u8];
    if starts("position") {
        let [x, y] = floats::<2>(rest("position"));
        (t.x, t.y) = (x, y);
    } else if starts("effect") {
        t.effect = floats::<1>(rest("effect"))[0] as i32;
    } else if starts("fxtime") {
        t.fxtime = floats::<1>(rest("fxtime"))[0];
    } else if starts("color2") {
        let c = rgb(floats::<3>(rest("color2")));
        t.color2[..3].copy_from_slice(&c);
    } else if starts("color") {
        let c = rgb(floats::<3>(rest("color")));
        t.color1[..3].copy_from_slice(&c);
    } else if starts("fadein") {
        t.fadein = floats::<1>(rest("fadein"))[0];
    } else if starts("fadeout") {
        t.fadeout = floats::<3>(rest("fadeout"))[0];
    } else if starts("holdtime") {
        t.holdtime = floats::<3>(rest("holdtime"))[0];
    } else {
        log::debug!("Unknown token: {line}");
    }
}

impl Titles {
    /// `TextMessageParse`; a stray brace aborts and keeps nothing.
    pub fn parse(text: &str) -> Self {
        let mut template = TextMessage::default();
        let mut messages = Vec::new();
        let mut name = String::new();
        let mut body: Option<Vec<&str>> = None;
        for (number, raw) in text.split('\n').enumerate() {
            let line = raw.trim_matches([' ', '\t', '\r']);
            match &mut body {
                None => {
                    if line.is_empty() || line.starts_with("//") {
                        continue;
                    }
                    if line.starts_with('$') {
                        directive(line, &mut template);
                    } else if line.starts_with('{') {
                        body = Some(Vec::new());
                    } else if line.starts_with('}') {
                        log::debug!("Unexpected '}}' found, line {}", number + 1);
                        return Self::default();
                    } else {
                        name = gs_common::bytestr::truncate_encoded(line, 511).into_owned();
                    }
                }
                Some(lines) => {
                    if line.starts_with('{') {
                        log::debug!("Unexpected '{{' found, line {}", number + 1);
                        return Self::default();
                    }
                    if line.starts_with('}') {
                        let message = lines.join("\n");
                        messages.push(TextMessage { name: name.clone(), message, ..template.clone() });
                        if messages.len() >= MAX_MESSAGES {
                            log::error!("tmessage::TextMessageParse : messageCount>=MAX_MESSAGES");
                            return Self::default();
                        }
                        body = None;
                    } else {
                        lines.push(raw);
                    }
                }
            }
        }
        log::debug!("Parsed {} text messages", messages.len());
        Self { messages }
    }

    /// `pfnTextMessageGet`: case-insensitive name lookup.
    pub fn get(&self, name: &str) -> Option<&TextMessage> {
        self.messages.iter().find(|m| m.name.eq_ignore_ascii_case(name))
    }

    /// The engine's own entries (`__DEMOMESSAGE__`), found by the same lookup.
    pub fn set(&mut self, msg: TextMessage) {
        match self.messages.iter_mut().find(|m| m.name.eq_ignore_ascii_case(&msg.name)) {
            Some(m) => *m = msg,
            None => self.messages.push(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directives_are_sticky_and_bodies_are_raw() {
        let t = Titles::parse(
            "// comment\n$position -1 0.65\n$effect 2\n$color 100 100 100\n$fadeout 0.5 9 9\n\nHINT\n{\n  Line one\nLine two\n}\n$color2 1 2 3\nOTHER\n{\nX\n}\n",
        );
        let hint = t.get("hint");
        assert_eq!(hint.map(|m| (m.x, m.y, m.effect, m.fadeout)), Some((-1.0, 0.65, 2, 0.5)));
        assert_eq!(hint.map(|m| m.message.as_str()), Some("  Line one\nLine two"));
        assert_eq!(hint.map(|m| m.color1), Some([100, 100, 100, 0]), "alpha stays 0");
        let other = t.get("OTHER");
        assert_eq!(other.map(|m| (m.color1, m.color2)), Some(([100, 100, 100, 0], [1, 2, 3, 0])));
    }

    #[test]
    fn a_stray_brace_discards_the_table() {
        let t = Titles::parse("A\n{\nx\n}\n}\nB\n{\ny\n}\n");
        assert!(t.get("A").is_none());
    }
}
