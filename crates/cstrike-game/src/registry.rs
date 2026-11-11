//! The engine registry `HKCU\Software\Valve\Half-Life\Settings` (docs/re/engine-video.md §1):
//! `CRegistry` reads (DWORD/REG_SZ only, otherwise the default) and writes, and vgui2's
//! `GetRegistryString`/`SetRegistryString` for GameUI's `User Token 2`.

#[cfg(windows)]
mod imp {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_DWORD, REG_SZ};
    use winreg::{RegKey, RegValue};

    const STEAM_KEY: &str = "Software\\Valve\\Steam";

    const KEY: &str = "Software\\Valve\\Half-Life\\Settings\\";

    fn key() -> Option<RegKey> {
        RegKey::predef(HKEY_CURRENT_USER).create_subkey_with_flags(KEY, KEY_ALL_ACCESS).ok().map(|(k, _)| k)
    }

    /// `ReadInt(name, def)`: a REG_DWORD, else `def`.
    pub fn read_int(name: &str, def: i32) -> i32 {
        key()
            .and_then(|k| k.get_raw_value(name).ok())
            .filter(|v| v.vtype == REG_DWORD && v.bytes.len() == 4)
            .map_or(def, |v| i32::from_le_bytes([v.bytes[0], v.bytes[1], v.bytes[2], v.bytes[3]]))
    }

    /// A REG_SZ value's text (the raw value is UTF-16LE up to its NUL) when it is shorter than
    /// `max` characters, the size of the original's ANSI buffer.
    fn sz(v: &RegValue, max: usize) -> Option<String> {
        if v.vtype != REG_SZ {
            return None;
        }
        let units: Vec<u16> = v.bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        (end < max).then(|| String::from_utf16_lossy(&units[..end]))
    }

    /// `ReadString(name, def)`: a REG_SZ shorter than 512 bytes, else `def`.
    pub fn read_string(name: &str, def: &str) -> String {
        key().and_then(|k| k.get_raw_value(name).ok()).and_then(|v| sz(&v, 512)).unwrap_or_else(|| def.to_owned())
    }

    /// A REG_SZ of `HKCU\Software\Valve\Steam` read into a `char[32]` (`None` when missing or
    /// longer).
    pub fn steam_string(name: &str) -> Option<String> {
        let k = RegKey::predef(HKEY_CURRENT_USER).open_subkey(STEAM_KEY).ok()?;
        sz(&k.get_raw_value(name).ok()?, 32)
    }

    /// Writes a REG_SZ under `HKCU\Software\Valve\Steam` (the key is created when absent).
    pub fn set_steam_string(name: &str, value: &str) {
        match RegKey::predef(HKEY_CURRENT_USER).create_subkey(STEAM_KEY) {
            Ok((k, _)) => {
                if let Err(e) = k.set_value(name, &value) {
                    log::debug!("registry Steam\\{name}: {e}");
                }
            }
            Err(e) => log::debug!("registry Steam: {e}"),
        }
    }

    /// `WriteString(name, value)` (`SetRegistryString`).
    pub fn write_string(name: &str, value: &str) {
        if let Some(k) = key()
            && let Err(e) = k.set_value(name, &value)
        {
            log::debug!("registry {name}: {e}");
        }
    }

    /// `WriteInt(name, value)`: a REG_DWORD.
    pub fn write_int(name: &str, value: i32) {
        if let Some(k) = key() {
            let v = RegValue { bytes: value.to_le_bytes().to_vec(), vtype: REG_DWORD };
            if let Err(e) = k.set_raw_value(name, &v) {
                log::debug!("registry {name}: {e}");
            }
        }
    }

    /// vgui2 `GetRegistryInteger("HKEY_CURRENT_USER\Software\Valve\Steam\<name>")`: a REG_DWORD of
    /// the Steam client's key, opened read-only.
    pub fn steam_int(name: &str) -> Option<i32> {
        let k = RegKey::predef(HKEY_CURRENT_USER).open_subkey(STEAM_KEY).ok()?;
        let v = k.get_raw_value(name).ok().filter(|v| v.vtype == REG_DWORD && v.bytes.len() == 4)?;
        Some(i32::from_le_bytes([v.bytes[0], v.bytes[1], v.bytes[2], v.bytes[3]]))
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn read_int(_name: &str, def: i32) -> i32 {
        def
    }
    pub fn read_string(_name: &str, def: &str) -> String {
        def.to_owned()
    }
    pub fn write_string(_name: &str, _value: &str) {}
    pub fn write_int(_name: &str, _value: i32) {}
    pub fn steam_int(_name: &str) -> Option<i32> {
        None
    }
    pub fn steam_string(_name: &str) -> Option<String> {
        None
    }
    pub fn set_steam_string(_name: &str, _value: &str) {}
}

pub use imp::{read_int, read_string, set_steam_string, steam_int, steam_string, write_int, write_string};
