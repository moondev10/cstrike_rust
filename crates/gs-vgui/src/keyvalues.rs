//! vgui2 `KeyValues` as a mutable tree (docs/re/vgui2-core.md §7): `FindKey` with `/` paths
//! and creation, the typed getters/setters, `LoadFromFile`'s text reader (duplicate keys merge)
//! and `SaveToFile`'s writer. Used for the user config (`InGameDialogConfig.vdf`) and the
//! platform modules' own files (`ServerBrowser.vdf`).

use crate::msg::atoi;

/// The value of one key (`dataType`).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum KvData {
    #[default]
    None,
    Str(String),
    Int(i32),
    Float(f32),
    U64(u64),
}

/// One `KeyValues` node: a name, its own value and its sub-keys in insertion order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeyValues {
    pub name: String,
    pub data: KvData,
    pub children: Vec<KeyValues>,
}

impl KeyValues {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_owned(), data: KvData::None, children: Vec::new() }
    }

    /// `FindKey(path, false)`: `a/b` addresses sub-key `b` of `a`; names compare
    /// case-insensitively (the symbol table); `""` is the key itself.
    pub fn find_key(&self, path: &str) -> Option<&KeyValues> {
        if path.is_empty() {
            return Some(self);
        }
        let (head, rest) = path.split_once('/').map_or((path, None), |(h, r)| (h, Some(r)));
        let found = self.children.iter().find(|c| c.name.eq_ignore_ascii_case(head))?;
        match rest {
            Some(r) => found.find_key(r),
            None => Some(found),
        }
    }

    /// `FindKey(path, true)`: missing keys are appended as the last child; a key that gets a
    /// child loses its own value.
    pub fn find_or_create(&mut self, path: &str) -> &mut KeyValues {
        if path.is_empty() {
            return self;
        }
        let (head, rest) = path.split_once('/').map_or((path, None), |(h, r)| (h, Some(r)));
        let i = match self.children.iter().position(|c| c.name.eq_ignore_ascii_case(head)) {
            Some(i) => i,
            None => {
                self.children.push(KeyValues::new(head));
                self.data = KvData::None;
                self.children.len() - 1
            }
        };
        let found = &mut self.children[i];
        match rest {
            Some(r) => found.find_or_create(r),
            None => found,
        }
    }

    /// `GetString(key, def)`: ints and floats are formatted (`%d`, `%f`).
    pub fn get_string(&self, key: &str, def: &str) -> String {
        let Some(k) = self.find_key(key) else { return def.to_owned() };
        match &k.data {
            KvData::Str(s) => s.clone(),
            KvData::Int(i) => i.to_string(),
            KvData::Float(f) => format!("{f:.6}"),
            KvData::U64(u) => (*u as i64).to_string(),
            KvData::None => def.to_owned(),
        }
    }

    /// `GetInt(key, def)`: strings by `atoi`, floats truncated; a key without a value is 0.
    pub fn get_int(&self, key: &str, def: i32) -> i32 {
        let Some(k) = self.find_key(key) else { return def };
        match &k.data {
            KvData::Str(s) => atoi(s),
            KvData::Int(i) => *i,
            KvData::Float(f) => *f as i32,
            KvData::U64(_) | KvData::None => 0,
        }
    }

    /// `GetUint64(key, def)`: strings by `_atoi64`.
    pub fn get_u64(&self, key: &str, def: u64) -> u64 {
        let Some(k) = self.find_key(key) else { return def };
        match &k.data {
            KvData::Str(s) => atoi64(s) as u64,
            KvData::Int(i) => *i as u64,
            KvData::Float(f) => *f as i32 as u64,
            KvData::U64(u) => *u,
            KvData::None => 0,
        }
    }

    pub fn set_string(&mut self, key: &str, v: &str) {
        self.find_or_create(key).data = KvData::Str(v.to_owned());
    }

    pub fn set_int(&mut self, key: &str, v: i32) {
        self.find_or_create(key).data = KvData::Int(v);
    }

    pub fn set_u64(&mut self, key: &str, v: u64) {
        self.find_or_create(key).data = KvData::U64(v);
    }

    /// `RemoveSubKey(FindKey(name, false))`: the first direct child of that name.
    pub fn remove_sub_key(&mut self, name: &str) -> Option<KeyValues> {
        let i = self.children.iter().position(|c| c.name.eq_ignore_ascii_case(name))?;
        Some(self.children.remove(i))
    }

    /// `AddSubKey`: appended as the last child.
    pub fn add_sub_key(&mut self, kv: KeyValues) {
        self.children.push(kv);
    }

    /// `Clear`: the sub-keys go.
    pub fn clear(&mut self) {
        self.children.clear();
    }

    /// `LoadFromFile`'s parse: the root takes the file's first name, then the first block's
    /// keys merge into this tree. `false` only for an empty file (the original's open failure
    /// is the caller's).
    pub fn load_from_buffer(&mut self, buf: &[u8]) -> bool {
        let mut p = Reader { buf, pos: 0 };
        let Some(name) = p.token() else { return true };
        self.name = String::from_utf8_lossy(&name).into_owned();
        if p.token().is_some_and(|t| t.first() == Some(&b'{')) {
            self.load_block(&mut p);
        }
        true
    }

    fn load_block(&mut self, p: &mut Reader<'_>) {
        loop {
            let Some(name) = p.token() else { return };
            if name.is_empty() || name == b"}" {
                return;
            }
            let name = String::from_utf8_lossy(&name).into_owned();
            let dat = self.find_or_create(&name);
            let Some(value) = p.token() else { return };
            if value == b"}" {
                return;
            }
            if value == b"{" {
                dat.load_block(p);
            } else {
                dat.data = KvData::Str(String::from_utf8_lossy(&value).into_owned());
            }
        }
    }

    /// `SaveToFile`'s text, with the trailing NUL byte.
    pub fn save_to_buffer(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out, 0);
        out.push(0);
        out
    }

    fn write(&self, out: &mut Vec<u8>, depth: usize) {
        let tabs = |out: &mut Vec<u8>, n: usize| out.extend(std::iter::repeat_n(b'\t', n));
        tabs(out, depth);
        out.push(b'"');
        out.extend_from_slice(self.name.as_bytes());
        out.extend_from_slice(b"\"\n");
        tabs(out, depth);
        out.extend_from_slice(b"{\n");
        for c in &self.children {
            if !c.children.is_empty() {
                c.write(out, depth + 1);
                continue;
            }
            let value = match &c.data {
                KvData::Str(s) if !s.is_empty() => s.clone(),
                KvData::Int(i) => i.to_string(),
                KvData::Float(f) => format!("{f:.6}"),
                _ => continue,
            };
            tabs(out, depth + 1);
            out.push(b'"');
            escape_into(out, &c.name);
            out.extend_from_slice(b"\"\t\t\"");
            escape_into(out, &value);
            out.extend_from_slice(b"\"\n");
        }
        tabs(out, depth);
        out.extend_from_slice(b"}\n");
    }
}

fn escape_into(out: &mut Vec<u8>, s: &str) {
    for b in s.bytes() {
        if b == b'\\' || b == b'"' {
            out.push(b'\\');
        }
        out.push(b);
    }
}

/// `_atoi64`.
fn atoi64(s: &str) -> i64 {
    let t = s.trim_start();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let mut v: i64 = 0;
    for b in digits.bytes().take_while(u8::is_ascii_digit) {
        v = v.wrapping_mul(10).wrapping_add(i64::from(b - b'0'));
    }
    if neg { v.wrapping_neg() } else { v }
}

/// `ReadToken` over the file buffer (the CRT `isspace`; bytes ≥ 0x80 end unquoted tokens and
/// comments).
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn peek(&self, off: usize) -> u8 {
        self.buf.get(self.pos + off).copied().unwrap_or(0)
    }

    fn token(&mut self) -> Option<Vec<u8>> {
        const MAX: usize = 511;
        loop {
            while matches!(self.peek(0), b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r') {
                self.pos += 1;
            }
            if self.peek(0) == 0 {
                return None;
            }
            if self.peek(0) == b'/' && self.peek(1) == b'/' {
                while !matches!(self.peek(0), 0 | b'\n') && self.peek(0) < 0x80 {
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
        let mut tok = Vec::new();
        if self.peek(0) == b'"' {
            self.pos += 1;
            loop {
                let c = self.peek(0);
                if c == 0 {
                    break;
                }
                self.pos += 1;
                if c == b'"' {
                    break;
                }
                let ch = if c == b'\\' {
                    let e = self.peek(0);
                    if e != 0 {
                        self.pos += 1;
                    }
                    match e {
                        b'n' => b'\n',
                        b't' => b'\t',
                        other => other,
                    }
                } else {
                    c
                };
                if tok.len() < MAX {
                    tok.push(ch);
                }
            }
        } else {
            loop {
                let c = self.peek(0);
                if c == 0 {
                    // The last character of an unquoted token ending at the end of the file is lost.
                    tok.pop();
                    break;
                }
                if matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r') || c >= 0x80 {
                    break;
                }
                self.pos += 1;
                if tok.len() < MAX {
                    tok.push(c);
                }
            }
        }
        Some(tok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_merges_duplicates_and_paths_address_sub_keys() {
        let mut kv = KeyValues::new("x");
        kv.load_from_buffer(b"\"Root\"\n{\n \"A\" { \"b\" \"1\" }\n \"a\" { \"C\" \"2\" \"b\" \"3\" }\n // note\n \"d\" \"q\\\"z\"\n}\n");
        assert_eq!(kv.name, "Root");
        assert_eq!(kv.children.len(), 2);
        assert_eq!(kv.get_int("a/B", -1), 3);
        assert_eq!(kv.get_string("A/c", ""), "2");
        assert_eq!(kv.get_string("d", ""), "q\"z");
        assert_eq!(kv.get_int("missing", -1), -1);
    }

    #[test]
    fn save_skips_empty_values_and_round_trips() {
        let mut kv = KeyValues::new("UserConfigData");
        kv.set_int("Servers/DialogServerBrowser.res/xpos", 480);
        kv.set_string("Servers/empty", "");
        kv.find_or_create("Servers/Pages/LanGames/gamelist");
        kv.set_string("s", "a\\b\"c");
        let text = kv.save_to_buffer();
        assert_eq!(text.last(), Some(&0));
        let s = String::from_utf8_lossy(&text[..text.len() - 1]).into_owned();
        assert_eq!(
            s,
            "\"UserConfigData\"\n{\n\t\"Servers\"\n\t{\n\t\t\"DialogServerBrowser.res\"\n\t\t{\n\t\t\t\"xpos\"\t\t\"480\"\n\t\t}\n\t\t\"Pages\"\n\t\t{\n\t\t\t\"LanGames\"\n\t\t\t{\n\t\t\t}\n\t\t}\n\t}\n\t\"s\"\t\t\"a\\\\b\\\"c\"\n}\n"
        );
        let mut back = KeyValues::new("");
        back.load_from_buffer(&text);
        assert_eq!(back.get_int("Servers/DialogServerBrowser.res/xpos", 0), 480);
        assert_eq!(back.get_string("s", ""), "a\\b\"c");
    }
}
