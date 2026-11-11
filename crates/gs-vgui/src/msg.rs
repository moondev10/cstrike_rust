//! vgui2 messages: a name and named parameters (the `KeyValues` a panel posts), matched
//! case-insensitively like the message maps do (docs/re/vgui-controls.md §1.3).

use crate::ui::PanelId;

/// A message parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i32),
    Float(f32),
    Str(String),
    Panel(PanelId),
}

/// One message.
#[derive(Debug, Clone, PartialEq)]
pub struct Msg {
    pub name: String,
    pub params: Vec<(String, Value)>,
}

impl Msg {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_owned(), params: Vec::new() }
    }

    pub fn with(mut self, key: &str, value: Value) -> Self {
        self.set(key, value);
        self
    }

    pub fn int(self, key: &str, v: i32) -> Self {
        self.with(key, Value::Int(v))
    }

    pub fn str(self, key: &str, v: &str) -> Self {
        self.with(key, Value::Str(v.to_owned()))
    }

    /// `KeyValues::SetX`: replaces an existing key (case-insensitive).
    pub fn set(&mut self, key: &str, value: Value) {
        match self.params.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            Some((_, v)) => *v = value,
            None => self.params.push((key.to_owned(), value)),
        }
    }

    /// `KeyValues::GetName` compared case-insensitively.
    pub fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.params.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
    }

    /// `GetInt(key, 0)` (floats truncate, strings parse like `atoi`).
    pub fn get_int(&self, key: &str) -> i32 {
        match self.get(key) {
            Some(Value::Int(i)) => *i,
            Some(Value::Float(f)) => *f as i32,
            Some(Value::Str(s)) => atoi(s),
            _ => 0,
        }
    }

    /// `GetString(key, "")`.
    pub fn get_str(&self, key: &str) -> String {
        match self.get(key) {
            Some(Value::Str(s)) => s.clone(),
            Some(Value::Int(i)) => i.to_string(),
            Some(Value::Float(f)) => format!("{f:.6}"),
            _ => String::new(),
        }
    }

    pub fn get_panel(&self, key: &str) -> Option<PanelId> {
        match self.get(key) {
            Some(Value::Panel(p)) => Some(*p),
            _ => None,
        }
    }
}

/// C `atoi`: leading whitespace, optional sign, digits.
pub fn atoi(s: &str) -> i32 {
    let s = s.trim_start();
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut v: i64 = 0;
    for b in rest.bytes().take_while(u8::is_ascii_digit) {
        v = (v * 10 + i64::from(b - b'0')).min(i64::from(i32::MAX) + 1);
    }
    let v = if neg { -v } else { v };
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}
