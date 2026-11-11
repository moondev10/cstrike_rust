//! VGUI2 `ILocalize`: `#token` → text from `resource/*_<language>.txt` files (UTF-16 or UTF-8
//! KeyValues `"lang" { "Tokens" { ... } }`); later files override earlier tokens.

use std::collections::HashMap;

use gs_formats::keyvalues;

#[derive(Debug, Clone, Default)]
pub struct Localize {
    /// Lowercased token → text.
    tokens: HashMap<String, String>,
}

/// File bytes as text: UTF-16LE with BOM, else UTF-8 (lossy).
fn decode(data: &[u8]) -> String {
    match data {
        [0xFF, 0xFE, rest @ ..] => {
            let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&units)
        }
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(data).into_owned(),
    }
}

impl Localize {
    /// `AddFile`: merges the file's tokens.
    pub fn add_file(&mut self, data: &[u8]) {
        let kv = keyvalues::parse_escaped(&decode(data));
        let Some(tokens) = kv.block("lang").and_then(|l| l.block("Tokens")) else {
            log::warn!("localization file without lang/Tokens");
            return;
        };
        for (key, value) in &tokens.entries {
            if let keyvalues::Value::Str(text) = value {
                self.tokens.insert(key.to_ascii_lowercase(), text.clone());
            }
        }
    }

    /// `Find(token)`: `token` may carry its leading `#`.
    pub fn find(&self, token: &str) -> Option<&str> {
        self.tokens.get(&token.trim_start_matches('#').to_ascii_lowercase()).map(String::as_str)
    }

    /// The text a `#`-prefixed string stands for, else the string itself.
    pub fn text<'a>(&'a self, s: &'a str) -> &'a str {
        if s.starts_with('#') { self.find(s).unwrap_or(s) } else { s }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_files_with_escapes_and_overrides() {
        let file = "\"lang\" { \"Tokens\" { \"Cstrike_Hi\" \"Say \\\"hi\\\"\\nnow\" } }";
        let mut data = vec![0xFF, 0xFE];
        data.extend(file.encode_utf16().flat_map(u16::to_le_bytes));
        let mut loc = Localize::default();
        loc.add_file(&data);
        assert_eq!(loc.find("#cstrike_hi"), Some("Say \"hi\"\nnow"));
        loc.add_file(b"\"lang\" { \"Tokens\" { \"CSTRIKE_HI\" \"bye\" } }");
        assert_eq!(loc.text("#Cstrike_Hi"), "bye");
        assert_eq!(loc.text("plain"), "plain");
        assert_eq!(loc.text("#missing"), "#missing");
    }
}
