//! `\key\value\key\value` info strings (userinfo, serverinfo, protinfo) with the engine's
//! `Info_SetValueForStarKey` rules (docs/re/engine-core.md §2.7).

/// Max size of `cls.userinfo`.
pub const MAX_USERINFO: usize = 256;
/// Max size of the connect `protinfo`.
pub const MAX_PROTINFO: usize = 1024;
const MAX_KV_LEN: usize = 127;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InfoError {
    #[error("Can't use keys or values with a \\")]
    Backslash,
    #[error("Can't use keys or values with a \"")]
    Quote,
    #[error("Keys and values must be < 128 characters and > 0.")]
    Length,
    #[error("Can't set * keys")]
    StarKey,
    #[error("Info string length exceeded")]
    Exceeded,
}

/// Keys that may evict other pairs when the string is full (`Info_IsKeyImportant`).
fn is_important(key: &str) -> bool {
    key.starts_with('*')
        || matches!(key, "name" | "model" | "rate" | "topcolor" | "bottomcolor" | "cl_updaterate" | "cl_lw" | "cl_lc")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoString {
    text: String,
    max: usize,
}

impl InfoString {
    pub fn new(max: usize) -> Self {
        Self { text: String::new(), max }
    }

    /// Wraps existing text (e.g. received from the server) without validation.
    pub fn from_raw(text: &str, max: usize) -> Self {
        Self { text: text.to_owned(), max }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        let body = self.text.strip_prefix('\\').unwrap_or(&self.text);
        let mut parts = (!body.is_empty()).then(|| body.split('\\')).into_iter().flatten();
        std::iter::from_fn(move || {
            let key = parts.next()?;
            Some((key, parts.next().unwrap_or("")))
        })
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.pairs().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    pub fn remove(&mut self, key: &str) {
        let kept: Vec<(String, String)> =
            self.pairs().filter(|(k, _)| *k != key).map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        self.text = kept.iter().map(|(k, v)| format!("\\{k}\\{v}")).collect();
    }

    /// `Info_SetValueForKey`: like [`Self::set_star`] but refuses `*` keys.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), InfoError> {
        if key.starts_with('*') {
            return Err(InfoError::StarKey);
        }
        self.set_star(key, value)
    }

    /// `Info_SetValueForStarKey`. Keys/values containing `..` are silently ignored; an empty
    /// value only removes the key.
    pub fn set_star(&mut self, key: &str, value: &str) -> Result<(), InfoError> {
        if key.contains('\\') || value.contains('\\') {
            return Err(InfoError::Backslash);
        }
        if key.contains("..") || value.contains("..") {
            return Ok(());
        }
        if key.contains('"') || value.contains('"') {
            return Err(InfoError::Quote);
        }
        // The limits count the original bytes (`strlen`), not this representation's UTF-8.
        let len = crate::bytestr::encoded_len;
        if key.is_empty() || len(key) > MAX_KV_LEN || len(value) > MAX_KV_LEN {
            return Err(InfoError::Length);
        }
        self.remove(key);
        if value.is_empty() {
            return Ok(());
        }
        let mut pair = format!("\\{key}\\{value}");
        if key.eq_ignore_ascii_case("team") {
            pair.make_ascii_lowercase();
        }
        while len(&self.text) + len(&pair) >= self.max {
            if !is_important(key) {
                return Err(InfoError::Exceeded);
            }
            let victim = self
                .pairs()
                .filter(|(k, _)| !is_important(k))
                .max_by_key(|(k, v)| len(k) + len(v))
                .map(|(k, _)| k.to_owned());
            let Some(victim) = victim else { return Err(InfoError::Exceeded) };
            self.remove(&victim);
        }
        self.text.push_str(&pair);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_replaces_and_empty_removes() {
        let mut info = InfoString::new(MAX_USERINFO);
        assert_eq!(info.set("name", "a"), Ok(()));
        assert_eq!(info.set("rate", "25000"), Ok(()));
        assert_eq!(info.set("name", "b"), Ok(()));
        assert_eq!(info.as_str(), "\\rate\\25000\\name\\b");
        assert_eq!(info.set("rate", ""), Ok(()));
        assert_eq!(info.as_str(), "\\name\\b");
    }

    #[test]
    fn rejects_like_the_engine() {
        let mut info = InfoString::new(MAX_USERINFO);
        assert_eq!(info.set("a\\b", "x"), Err(InfoError::Backslash));
        assert_eq!(info.set("a", "x\"y"), Err(InfoError::Quote));
        assert_eq!(info.set("", "x"), Err(InfoError::Length));
        assert_eq!(info.set("a", &"x".repeat(128)), Err(InfoError::Length));
        assert_eq!(info.set("*sid", "1"), Err(InfoError::StarKey));
        assert_eq!(info.set("a", "../x"), Ok(()), "'..' is silently ignored");
        assert_eq!(info.get("a"), None);
        assert_eq!(info.set("Team", "CT"), Ok(()));
        assert_eq!(info.as_str(), "\\team\\ct", "team pairs are lowercased");
    }

    #[test]
    fn important_keys_evict_largest_unimportant_pair() {
        let mut info = InfoString::new(40);
        assert_eq!(info.set("aa", "1"), Ok(()));
        assert_eq!(info.set("bbbbbbbb", "22222222"), Ok(()));
        assert_eq!(info.set("zz", "3333333333333"), Err(InfoError::Exceeded), "5 + 18 + 17 >= 40");
        assert_eq!(info.set("name", "player1234567"), Ok(()));
        assert_eq!(info.as_str(), "\\aa\\1\\name\\player1234567");
    }
}
