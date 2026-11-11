//! Key numbers, their names and the 256 binding slots (docs/re/engine-core.md §3 "Key
//! bindings"): `bind`/`unbind` parsing, `Key_Event` command text and `Key_WriteBindings`.

pub const K_TAB: u8 = 9;
pub const K_ENTER: u8 = 13;
pub const K_ESCAPE: u8 = 27;
pub const K_SPACE: u8 = 32;
pub const K_BACKSPACE: u8 = 127;
pub const K_UPARROW: u8 = 128;
pub const K_DOWNARROW: u8 = 129;
pub const K_LEFTARROW: u8 = 130;
pub const K_RIGHTARROW: u8 = 131;
pub const K_ALT: u8 = 132;
pub const K_CTRL: u8 = 133;
pub const K_SHIFT: u8 = 134;
pub const K_F1: u8 = 135;
pub const K_INS: u8 = 147;
pub const K_DEL: u8 = 148;
pub const K_PGDN: u8 = 149;
pub const K_PGUP: u8 = 150;
pub const K_HOME: u8 = 151;
pub const K_END: u8 = 152;
pub const K_KP_HOME: u8 = 160;
pub const K_KP_UPARROW: u8 = 161;
pub const K_KP_PGUP: u8 = 162;
pub const K_KP_LEFTARROW: u8 = 163;
pub const K_KP_5: u8 = 164;
pub const K_KP_RIGHTARROW: u8 = 165;
pub const K_KP_END: u8 = 166;
pub const K_KP_DOWNARROW: u8 = 167;
pub const K_KP_PGDN: u8 = 168;
pub const K_KP_ENTER: u8 = 169;
pub const K_KP_INS: u8 = 170;
pub const K_KP_DEL: u8 = 171;
pub const K_KP_SLASH: u8 = 172;
pub const K_KP_MINUS: u8 = 173;
pub const K_KP_PLUS: u8 = 174;
pub const K_CAPSLOCK: u8 = 175;
pub const K_KP_MUL: u8 = 176;
pub const K_WIN: u8 = 177;
pub const K_MWHEELDOWN: u8 = 239;
pub const K_MWHEELUP: u8 = 240;
pub const K_MOUSE1: u8 = 241;
pub const K_PAUSE: u8 = 255;

/// The key-name table (`off_1031B690`).
const NAMES: &[(&str, u8)] = &[
    ("TAB", 9),
    ("ENTER", 13),
    ("ESCAPE", 27),
    ("SPACE", 32),
    ("BACKSPACE", 127),
    ("UPARROW", 128),
    ("DOWNARROW", 129),
    ("LEFTARROW", 130),
    ("RIGHTARROW", 131),
    ("ALT", 132),
    ("CTRL", 133),
    ("SHIFT", 134),
    ("F1", 135),
    ("F2", 136),
    ("F3", 137),
    ("F4", 138),
    ("F5", 139),
    ("F6", 140),
    ("F7", 141),
    ("F8", 142),
    ("F9", 143),
    ("F10", 144),
    ("F11", 145),
    ("F12", 146),
    ("INS", 147),
    ("DEL", 148),
    ("PGDN", 149),
    ("PGUP", 150),
    ("HOME", 151),
    ("END", 152),
    ("KP_HOME", 160),
    ("KP_UPARROW", 161),
    ("KP_PGUP", 162),
    ("KP_LEFTARROW", 163),
    ("KP_5", 164),
    ("KP_RIGHTARROW", 165),
    ("KP_END", 166),
    ("KP_DOWNARROW", 167),
    ("KP_PGDN", 168),
    ("KP_ENTER", 169),
    ("KP_INS", 170),
    ("KP_DEL", 171),
    ("KP_SLASH", 172),
    ("KP_MINUS", 173),
    ("KP_PLUS", 174),
    ("CAPSLOCK", 175),
    ("KP_MUL", 176),
    ("WIN", 177),
    ("JOY1", 203),
    ("JOY2", 204),
    ("JOY3", 205),
    ("JOY4", 206),
    ("AUX1", 207),
    ("AUX2", 208),
    ("AUX3", 209),
    ("AUX4", 210),
    ("AUX5", 211),
    ("AUX6", 212),
    ("AUX7", 213),
    ("AUX8", 214),
    ("AUX9", 215),
    ("AUX10", 216),
    ("AUX11", 217),
    ("AUX12", 218),
    ("AUX13", 219),
    ("AUX14", 220),
    ("AUX15", 221),
    ("AUX16", 222),
    ("AUX17", 223),
    ("AUX18", 224),
    ("AUX19", 225),
    ("AUX20", 226),
    ("AUX21", 227),
    ("AUX22", 228),
    ("AUX23", 229),
    ("AUX24", 230),
    ("AUX25", 231),
    ("AUX26", 232),
    ("AUX27", 233),
    ("AUX28", 234),
    ("AUX29", 235),
    ("AUX30", 236),
    ("AUX31", 237),
    ("AUX32", 238),
    ("MWHEELDOWN", 239),
    ("MWHEELUP", 240),
    ("MOUSE1", 241),
    ("MOUSE2", 242),
    ("MOUSE3", 243),
    ("MOUSE4", 244),
    ("MOUSE5", 245),
    ("PAUSE", 255),
    ("SEMICOLON", 59),
];

/// `Key_StringToKeynum`: one character is its own code (case kept), otherwise the name table
/// (case-insensitive).
pub fn key_number(name: &str) -> Option<u8> {
    let b = name.as_bytes();
    if b.len() == 1 {
        return Some(b[0]);
    }
    NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|&(_, k)| k)
}

/// `Key_KeynumToString` as used by `Key_WriteBindings`: printable ASCII 33..=126 is the
/// character, otherwise the table name.
pub fn key_name(key: u8) -> String {
    if (33..=126).contains(&key) {
        return char::from(key).to_string();
    }
    NAMES.iter().find(|&&(_, k)| k == key).map_or_else(|| "<UNKNOWN KEYNUM>".to_owned(), |(n, _)| (*n).to_owned())
}

/// Keys that repeat while held (`cle_Key_Event` ignores autorepeat for the rest).
pub(crate) fn repeats(key: u8) -> bool {
    matches!(key, K_BACKSPACE | K_PAUSE | K_PGUP | K_KP_PGUP | K_PGDN | K_KP_PGDN)
}

/// `keybindings[256]`.
#[derive(Debug, Clone)]
pub struct Bindings {
    slots: Vec<String>,
}

impl Default for Bindings {
    fn default() -> Self {
        Self { slots: vec![String::new(); 256] }
    }
}

impl Bindings {
    pub fn get(&self, key: u8) -> &str {
        &self.slots[usize::from(key)]
    }

    /// `Key_SetBinding`; ESC always gets `cancelselect`.
    pub fn set(&mut self, key: u8, command: &str) {
        self.slots[usize::from(key)] = if key == K_ESCAPE { "cancelselect".to_owned() } else { command.to_owned() };
    }

    pub fn clear_all(&mut self) {
        self.slots.iter_mut().for_each(String::clear);
    }

    /// `Key_CountBindings`.
    pub fn count(&self) -> usize {
        self.slots.iter().filter(|s| !s.is_empty()).count()
    }

    /// `cle_Key_LookupBinding` (behind `GetVGUI2KeyCodeForBind`): the lowest key whose binding,
    /// a leading `+` stripped, starts with `binding` (case-insensitive).
    pub fn lookup(&self, binding: &str) -> Option<u8> {
        let n = binding.len();
        (0..=255u8).find(|&k| {
            let b = self.get(k);
            let b = b.strip_prefix('+').unwrap_or(b);
            !b.is_empty() && b.get(..n).is_some_and(|p| p.eq_ignore_ascii_case(binding))
        })
    }

    /// The command text a key transition queues into the privileged buffer
    /// (`cle_Key_Event`): `+cmd <key>` / `-cmd <key>` for `+` bindings, the binding itself on
    /// key-down otherwise. `repeat` marks an autorepeat key-down.
    pub fn event_text(&self, key: u8, down: bool, repeat: bool) -> Option<String> {
        if repeat && !repeats(key) {
            return None;
        }
        let binding = self.get(key);
        if binding.is_empty() {
            return None;
        }
        match binding.strip_prefix('+') {
            Some(rest) if down => Some(format!("+{rest} {key}\n")),
            Some(rest) => Some(format!("-{rest} {key}\n")),
            None if down => Some(format!("{binding}\n")),
            None => None,
        }
    }

    /// `Key_WriteBindings`: `bind "<key>" "<command>"` for every bound slot.
    pub fn write(&self, out: &mut String) {
        for (key, binding) in self.slots.iter().enumerate().filter(|(_, b)| !b.is_empty()) {
            out.push_str(&format!("bind \"{}\" \"{binding}\"\n", key_name(key as u8)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_single_characters_keep_case() {
        assert_eq!(key_number("mouse1"), Some(K_MOUSE1));
        assert_eq!(key_number("A"), Some(65));
        assert_eq!(key_number("a"), Some(97));
        assert_eq!(key_number("nosuchkey"), None);
        assert_eq!(key_name(32), "SPACE");
        assert_eq!(key_name(b'w'), "w");
        assert_eq!(key_name(200), "<UNKNOWN KEYNUM>");
    }

    #[test]
    fn plus_bindings_send_key_numbers_on_press_and_release() {
        let mut b = Bindings::default();
        b.set(b'w', "+forward");
        b.set(K_F1, "autobuy");
        assert_eq!(b.event_text(b'w', true, false).as_deref(), Some("+forward 119\n"));
        assert_eq!(b.event_text(b'w', false, false).as_deref(), Some("-forward 119\n"));
        assert_eq!(b.event_text(b'w', true, true), None, "autorepeat is ignored");
        assert_eq!(b.event_text(K_F1, true, false).as_deref(), Some("autobuy\n"));
        assert_eq!(b.event_text(K_F1, false, false), None);
    }

    #[test]
    fn escape_is_always_cancelselect_and_writing_uses_key_names() {
        let mut b = Bindings::default();
        b.set(K_ESCAPE, "quit");
        b.set(K_SPACE, "+jump");
        let mut out = String::new();
        b.write(&mut out);
        assert_eq!(out, "bind \"ESCAPE\" \"cancelselect\"\nbind \"SPACE\" \"+jump\"\n");
    }
}
