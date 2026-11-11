//! `CHudMenu` (docs/re/client-hud.md §6.6): text menus from the `ShowMenu` user message and
//! `SelectMenuItem`, reached through the `slot1..slot10` commands. Drawing belongs to the HUD.

/// The text menu state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextMenu {
    /// Bit n-1 set ⇒ key n is valid.
    valid_slots: u16,
    /// `m_fMenuDisplayed`.
    displayed: bool,
    /// HUD time the menu closes at (`None` = never).
    shutoff: Option<f32>,
    /// `g_szPrelocalisedMenuString`: the `needMore` parts' bytes, joined before decoding (a UTF-8
    /// character may span two parts).
    pending: Vec<u8>,
    /// The displayed text: localised, `+command`s replaced by their keys.
    pub text: String,
}

/// `ShowMenu` payload: `short validSlots, char seconds, byte needMore, string text`.
fn parse(data: &[u8]) -> Option<(u16, i8, bool, &[u8])> {
    let (head, rest) = data.split_first_chunk::<4>()?;
    let text = rest.split(|&b| b == 0).next().unwrap_or_default();
    Some((u16::from_le_bytes([head[0], head[1]]), head[2] as i8, head[3] != 0, text))
}

impl TextMenu {
    pub fn is_displayed(&self) -> bool {
        self.displayed
    }

    /// `MsgFunc_ShowMenu` at HUD time `now`; `convert` turns the whole prelocalised text into
    /// the displayed one (`BufferedLocaliseTextString`, then `KB_ConvertString`).
    pub fn show(&mut self, data: &[u8], now: f32, convert: impl FnOnce(String) -> String) {
        let Some((slots, seconds, need_more, text)) = parse(data) else {
            log::warn!("ShowMenu: short message");
            return;
        };
        self.valid_slots = slots;
        self.shutoff = (seconds > 0).then(|| now + f32::from(seconds));
        if slots == 0 {
            self.close();
            return;
        }
        // strncpy/strncat into char[512].
        self.pending.extend_from_slice(text);
        self.pending.truncate(511);
        if !need_more {
            let text = convert(gs_common::bytestr::decode(&std::mem::take(&mut self.pending)));
            self.text = gs_common::bytestr::truncate_encoded(&text, 511).into_owned();
            self.displayed = true;
        }
    }

    /// Closes the menu once its display time ran out.
    pub fn think(&mut self, now: f32) {
        if self.displayed && self.shutoff.is_some_and(|t| now >= t) {
            self.close();
        }
    }

    fn close(&mut self) {
        self.displayed = false;
        self.pending.clear();
    }

    /// `SelectMenuItem(n)`: the `menuselect` command to execute when `n` is a valid key.
    pub fn select(&mut self, n: i32) -> Option<String> {
        if !self.displayed || !(1..=16).contains(&n) || self.valid_slots & (1 << (n - 1)) == 0 {
            return None;
        }
        self.close();
        Some(format!("menuselect {n}\n"))
    }
}

/// `KB_ConvertString`: each `+word` (alphanumerics, at most 63 bytes with the `+`) whose word
/// `key_for` names a key becomes `[KEY]`; any other text is copied.
pub fn kb_convert_string(text: &str, key_for: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('+') {
        out.push_str(&rest[..i]);
        let word_len = rest[i + 1..].find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len() - i - 1).min(62);
        let binding = &rest[i..i + 1 + word_len];
        match (word_len > 0).then(|| key_for(&binding[1..])).flatten() {
            Some(key) => {
                out.push('[');
                out.push_str(&key);
                out.push(']');
            }
            None => out.push_str(binding),
        }
        rest = &rest[i + 1 + word_len..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(slots: u16, seconds: i8, more: bool, text: impl AsRef<[u8]>) -> Vec<u8> {
        let mut m = slots.to_le_bytes().to_vec();
        m.push(seconds as u8);
        m.push(u8::from(more));
        m.extend_from_slice(text.as_ref());
        m.push(0);
        m
    }

    #[test]
    fn only_valid_slots_select_and_selection_closes() {
        let mut menu = TextMenu::default();
        menu.show(&msg(0b10_0000_0011, -1, false, "Team"), 0.0, |t| t);
        assert!(menu.is_displayed());
        assert_eq!(menu.select(3), None);
        assert_eq!(menu.select(10).as_deref(), Some("menuselect 10\n"));
        assert!(!menu.is_displayed());
        assert_eq!(menu.select(1), None);
    }

    #[test]
    fn multi_part_text_is_joined_and_timed_menus_expire() {
        let mut menu = TextMenu::default();
        menu.show(&msg(1, 5, true, "first "), 10.0, |t| t);
        assert!(!menu.is_displayed());
        menu.show(&msg(1, 5, false, "second"), 10.0, |t| t);
        assert_eq!(menu.text, "first second");
        menu.think(14.9);
        assert!(menu.is_displayed());
        menu.think(15.0);
        assert!(!menu.is_displayed());
        menu.show(&msg(1, -1, false, "x"), 0.0, |t| t);
        menu.show(&msg(0, -1, false, ""), 0.0, |t| t);
        assert!(!menu.is_displayed(), "no valid slots closes the menu");
    }

    #[test]
    fn menu_text_is_cut_at_511_bytes_even_inside_a_character() {
        let text = "é".repeat(300);
        let mut menu = TextMenu::default();
        menu.show(&msg(1, -1, true, &text[..350]), 0.0, |t| t);
        menu.show(&msg(1, -1, false, &text[350..]), 0.0, |t| t);
        assert_eq!(gs_common::bytestr::encode(&menu.text), &text.as_bytes()[..511], "the split character keeps its lead byte");
    }

    /// The parts join as bytes: a UTF-8 character the server split between two parts is whole.
    #[test]
    fn a_character_split_between_parts_stays_whole() {
        let text = "Teleport café".as_bytes();
        let (head, tail) = text.split_at(text.len() - 1);
        let mut menu = TextMenu::default();
        menu.show(&msg(1, -1, true, head), 0.0, |t| t);
        menu.show(&msg(1, -1, false, tail), 0.0, |t| t);
        assert_eq!(menu.text, "Teleport café");
    }

    #[test]
    fn kb_convert_string_puts_bound_keys_in_brackets() {
        let key_for = |b: &str| (b == "attack").then(|| "MOUSE1".to_owned());
        assert_eq!(kb_convert_string("Press +attack to fire", key_for), "Press [MOUSE1] to fire");
        assert_eq!(kb_convert_string("+reload or +", key_for), "+reload or +", "unbound words and a lone + are kept");
    }
}
