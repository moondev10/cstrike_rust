//! Console variables (docs/re/engine-core.md §2.6): the case-insensitively sorted list,
//! `Cvar_DirectSet`'s value filters and `Q_atof`.

/// `FCVAR_*`.
pub mod flags {
    pub const ARCHIVE: u32 = 0x1;
    pub const USERINFO: u32 = 0x2;
    pub const SERVER: u32 = 0x4;
    pub const EXTDLL: u32 = 0x8;
    pub const CLIENTDLL: u32 = 0x10;
    pub const PROTECTED: u32 = 0x20;
    pub const SPONLY: u32 = 0x40;
    pub const PRINTABLEONLY: u32 = 0x80;
    pub const UNLOGGED: u32 = 0x100;
    pub const NOEXTRAWHITESPACE: u32 = 0x200;
    pub const PRIVILEGED: u32 = 0x400;
    pub const FILTERSTUFFTEXT: u32 = 0x800;
    /// Value cut at the first `;` or newline.
    pub const FILTERCHARS: u32 = 0x1000;
    /// Values that look like paths outside the game are refused.
    pub const NOBADPATHS: u32 = 0x2000;
    /// Not in the original: a cvar the original client lacks (`cl_predlog`), which the server
    /// neither sees nor sets. Its query answers as for no such cvar, and server text passes over
    /// it as an unknown name.
    pub const HIDDEN: u32 = 0x4000_0000;
}

/// `cvar_t`.
#[derive(Debug, Clone, PartialEq)]
pub struct Cvar {
    pub name: String,
    pub string: String,
    pub flags: u32,
    pub value: f32,
}

/// Result of [`Cvars::direct_set`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetResult {
    /// No such variable.
    Missing,
    /// NOBADPATHS refused the value (`Refusing to set %s to %s (invalid path)`).
    Refused,
    /// Stored; `changed` compares the old and new string case-sensitively.
    Set { changed: bool, value: String, flags: u32 },
}

/// The cvar list, kept sorted by `stricmp` like `Cvar_RegisterVariable`.
#[derive(Debug, Clone, Default)]
pub struct Cvars {
    list: Vec<Cvar>,
}

fn stricmp(a: &str, b: &str) -> std::cmp::Ordering {
    for (x, y) in a.bytes().zip(b.bytes()) {
        let (x, y) = (x.to_ascii_lowercase(), y.to_ascii_lowercase());
        if x != y {
            return x.cmp(&y);
        }
    }
    a.len().cmp(&b.len())
}

impl Cvars {
    /// `Cvar_RegisterVariable`: `false` when the name already exists (`Can't register
    /// variable %s, already defined`). Registration has no flag side effects.
    pub fn register(&mut self, name: &str, default: &str, flags: u32) -> bool {
        if self.find(name).is_some() {
            log::warn!("Can't register variable {name}, already defined");
            return false;
        }
        let at = self.list.partition_point(|c| stricmp(&c.name, name).is_lt());
        self.list.insert(at, Cvar { name: name.to_owned(), string: default.to_owned(), flags, value: q_atof(default) });
        true
    }

    /// `Cvar_FindVar`: case-insensitive.
    pub fn find(&self, name: &str) -> Option<&Cvar> {
        self.index(name).and_then(|i| self.list.get(i))
    }

    /// `name`'s position: the list is sorted and unique under `stricmp`, so a binary search
    /// finds what the original's linear `Q_stricmp` walk does.
    fn index(&self, name: &str) -> Option<usize> {
        self.list.binary_search_by(|c| stricmp(&c.name, name)).ok()
    }

    fn find_mut(&mut self, name: &str) -> Option<&mut Cvar> {
        self.index(name).and_then(|i| self.list.get_mut(i))
    }

    /// `Cvar_VariableString`: `""` when missing.
    pub fn string(&self, name: &str) -> &str {
        self.find(name).map_or("", |c| c.string.as_str())
    }

    /// `Cvar_VariableValue`: 0 when missing.
    pub fn value(&self, name: &str) -> f32 {
        self.find(name).map_or(0.0, |c| c.value)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Cvar> {
        self.list.iter()
    }

    /// A cvar's string and value overwritten without any checks (`cmd_serverinfo`'s direct
    /// `cvar_t` write); nothing when the cvar does not exist.
    pub fn set_raw(&mut self, name: &str, value: &str) {
        if let Some(var) = self.find_mut(name) {
            var.string = value.to_owned();
            var.value = q_atof(value);
        }
    }

    /// `Cvar_DirectSet` without its userinfo/server side effects (the console applies those):
    /// NOBADPATHS, PRINTABLEONLY, NOEXTRAWHITESPACE and FILTERCHARS in that order, then the
    /// store and `value = Q_atof(string)`.
    pub fn direct_set(&mut self, name: &str, value: &str) -> SetResult {
        let Some(var) = self.find_mut(name) else {
            return SetResult::Missing;
        };
        if var.flags & flags::NOBADPATHS != 0
            && (value.starts_with('/') || value.contains(':') || value.contains("..") || value.contains('\\'))
        {
            return SetResult::Refused;
        }
        let mut v = gs_common::bytestr::truncate_encoded(value, 1023).into_owned();
        if var.flags & flags::PRINTABLEONLY != 0 && v.is_empty() {
            v = "empty".to_owned();
        }
        if var.flags & flags::NOEXTRAWHITESPACE != 0 {
            v = strip_unprintable_and_space(&v);
        }
        if var.flags & flags::FILTERCHARS != 0
            && let Some(end) = v.find([';', '\n'])
        {
            v.truncate(end);
        }
        let changed = var.string != v;
        var.value = q_atof(&v);
        var.string.clone_from(&v);
        SetResult::Set { changed, value: v, flags: var.flags }
    }

    /// `var.value = v` written straight into the `cvar_t` (`svc_timescale`, `net_scale`'s
    /// clamp): the string keeps its old text.
    pub fn set_value_only(&mut self, name: &str, v: f32) {
        if let Some(var) = self.find_mut(name) {
            var.value = v;
        }
    }
}

/// `Q_StripUnprintableAndSpace`: drops control characters, U+0085 and the bidi controls
/// U+202A–202E / U+206A–206F, then trims whitespace at both ends.
fn strip_unprintable_and_space(s: &str) -> String {
    let kept: String = s
        .chars()
        .filter(|&c| !(c < ' ' || c == '\u{85}' || ('\u{202A}'..='\u{202E}').contains(&c) || ('\u{206A}'..='\u{206F}').contains(&c)))
        .collect();
    kept.trim_matches(|c: char| c.is_whitespace() || c == '\u{200B}' || c == '\u{FEFF}').to_owned()
}

/// `Q_atof`: optional `-`, then `0x` hex, `'c` character code, or decimal digits where both
/// `.` and `,` mark the decimal point; parsing stops at the first other character. No
/// whitespace skip and no exponent.
pub fn q_atof(s: &str) -> f32 {
    let mut b = s.as_bytes();
    let sign = if b.first() == Some(&b'-') {
        b = &b[1..];
        -1.0
    } else {
        1.0
    };
    let mut val = 0.0f64;
    if b.len() >= 2 && b[0] == b'0' && (b[1] == b'x' || b[1] == b'X') {
        for &c in &b[2..] {
            let digit = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => break,
            };
            val = val * 16.0 + f64::from(digit);
        }
        return (val * sign) as f32;
    }
    if b.first() == Some(&b'\'') {
        return (sign * f64::from(b.get(1).copied().map_or(0, |c| c as i8))) as f32;
    }
    let mut decimal: Option<u32> = None;
    let mut total = 0u32;
    for &c in b {
        if c == b'.' || c == b',' {
            decimal = Some(total);
            continue;
        }
        if !c.is_ascii_digit() {
            break;
        }
        val = val * 10.0 + f64::from(c - b'0');
        total += 1;
    }
    if let Some(decimal) = decimal {
        while total > decimal {
            val /= 10.0;
            total -= 1;
        }
    }
    (val * sign) as f32
}

/// `Q_atoi`: optional `-`, then `0x` hex, `'c` character code, or decimal digits (stopping at the
/// first other character); no whitespace skip; 32-bit wrapping.
pub fn q_atoi(s: &str) -> i32 {
    let mut b = s.as_bytes();
    let neg = b.first() == Some(&b'-');
    if neg {
        b = &b[1..];
    }
    let mut val: i32 = 0;
    if b.len() >= 2 && b[0] == b'0' && (b[1] == b'x' || b[1] == b'X') {
        for &c in &b[2..] {
            let digit = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => break,
            };
            val = val.wrapping_mul(16).wrapping_add(i32::from(digit));
        }
    } else if b.first() == Some(&b'\'') {
        val = i32::from(b.get(1).copied().map_or(0, |c| c as i8));
    } else {
        for &c in b.iter().take_while(|c| c.is_ascii_digit()) {
            val = val.wrapping_mul(10).wrapping_add(i32::from(c - b'0'));
        }
    }
    if neg { val.wrapping_neg() } else { val }
}

/// `Cvar_SetValue`'s formatting: `%d` when integral (within 1e-6), else `%f`.
pub fn format_value(v: f32) -> String {
    if (v - v.trunc()).abs() < 1e-6 { format!("{}", v as i32) } else { format!("{v:.6}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q_atof_reads_like_the_engine() {
        assert_eq!(q_atof("1.5"), 1.5);
        assert_eq!(q_atof("1,5"), 1.5, "comma is a decimal point");
        assert_eq!(q_atof("-0x1F"), -31.0);
        assert_eq!(q_atof("'A"), 65.0);
        assert_eq!(q_atof(" 3"), 0.0, "no whitespace skip");
        assert_eq!(q_atof("2e3"), 2.0, "no exponent");
        assert_eq!(q_atof("1.2.5"), 12.5, "a second point moves the decimal position");
    }

    #[test]
    fn list_is_sorted_and_lookup_is_case_insensitive() {
        let mut c = Cvars::default();
        assert!(c.register("mp_decals", "300", 0));
        assert!(c.register("MP3FadeTime", "2", 0));
        assert!(c.register("model", "", 0));
        assert!(!c.register("MODEL", "x", 0));
        let names: Vec<&str> = c.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["model", "MP3FadeTime", "mp_decals"]);
        assert_eq!(c.value("Mp_Decals"), 300.0);
    }

    #[test]
    fn direct_set_filters_values_by_flags() {
        let mut c = Cvars::default();
        c.register("name", "unnamed", flags::PRINTABLEONLY | flags::NOEXTRAWHITESPACE);
        c.register("servercfgfile", "server.cfg", flags::FILTERCHARS);
        c.register("mapcyclefile", "mapcycle.txt", flags::NOBADPATHS);
        assert_eq!(c.direct_set("name", "  a\u{202E}b\t "), SetResult::Set { changed: true, value: "ab".into(), flags: 0x280 });
        assert_eq!(c.direct_set("name", ""), SetResult::Set { changed: true, value: "empty".into(), flags: 0x280 });
        assert_eq!(c.direct_set("servercfgfile", "x.cfg;quit"), SetResult::Set { changed: true, value: "x.cfg".into(), flags: 0x1000 });
        assert_eq!(c.direct_set("mapcyclefile", "../evil.txt"), SetResult::Refused);
        assert_eq!(c.string("mapcyclefile"), "mapcycle.txt");
        assert_eq!(c.direct_set("nope", "1"), SetResult::Missing);
    }

    #[test]
    fn set_value_formats_integers_without_decimals() {
        assert_eq!(format_value(3.0), "3");
        assert_eq!(format_value(0.25), "0.250000");
    }
}
