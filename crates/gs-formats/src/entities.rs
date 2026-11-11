//! Entity lump text: `{ "key" "value" ... }` blocks.

use crate::FormatError;

const WHAT: &str = "entities";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entity {
    /// Key/value pairs in file order; later duplicates win on lookup like the engine's last-write.
    pub pairs: Vec<(String, String)>,
}

impl Entity {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn classname(&self) -> &str {
        self.get("classname").unwrap_or("")
    }

    /// Parses a `"x y z"` vector value; missing components are zero.
    pub fn vec3(&self, key: &str) -> Option<[f32; 3]> {
        let mut out = [0.0; 3];
        let mut parts = self.get(key)?.split_ascii_whitespace();
        for slot in &mut out {
            *slot = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0.0);
        }
        Some(out)
    }

    /// Brush model index for `"model" "*N"`.
    pub fn brush_model(&self) -> Option<usize> {
        self.get("model")?.strip_prefix('*')?.parse().ok()
    }
}

pub fn parse(text: &str) -> Result<Vec<Entity>, FormatError> {
    let mut tokens = Tokens { rest: text };
    let mut out = Vec::new();
    while let Some(tok) = tokens.next() {
        if tok != "{" {
            return Err(FormatError::Invalid { what: WHAT, detail: format!("expected '{{', found {tok:?}") });
        }
        let mut entity = Entity::default();
        loop {
            let Some(key) = tokens.next() else {
                return Err(FormatError::Invalid { what: WHAT, detail: "EOF inside entity".to_owned() });
            };
            if key == "}" {
                break;
            }
            let Some(value) = tokens.next().filter(|v| *v != "}") else {
                return Err(FormatError::Invalid { what: WHAT, detail: format!("key {key:?} without value") });
            };
            entity.pairs.push((key.to_owned(), value.to_owned()));
        }
        out.push(entity);
    }
    Ok(out)
}

struct Tokens<'a> {
    rest: &'a str,
}

impl<'a> Tokens<'a> {
    fn next(&mut self) -> Option<&'a str> {
        loop {
            self.rest = self.rest.trim_start();
            if let Some(after) = self.rest.strip_prefix("//") {
                self.rest = after.find('\n').map_or("", |i| &after[i..]);
                continue;
            }
            break;
        }
        let s = self.rest;
        let first = s.chars().next()?;
        if first == '"' {
            let body = &s[1..];
            let end = body.find('"').unwrap_or(body.len());
            self.rest = body.get(end + 1..).unwrap_or("");
            return Some(&body[..end]);
        }
        if first == '{' || first == '}' {
            self.rest = &s[1..];
            return Some(&s[..1]);
        }
        let end = s.find(|c: char| c.is_whitespace() || c == '"' || c == '{' || c == '}').unwrap_or(s.len());
        self.rest = &s[end..];
        Some(&s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks_comments_and_empty_values() {
        let text = "// header\n{\n\"classname\" \"worldspawn\"\n\"wad\" \"\"\n}\n{ \"model\" \"*3\" \"origin\" \"1 2\" }";
        let ents = parse(text).unwrap_or_default();
        assert_eq!(ents.len(), 2);
        assert_eq!(ents[0].classname(), "worldspawn");
        assert_eq!(ents[0].get("wad"), Some(""));
        assert_eq!(ents[1].brush_model(), Some(3));
        assert_eq!(ents[1].vec3("origin"), Some([1.0, 2.0, 0.0]));
    }

    #[test]
    fn unterminated_entity_is_an_error() {
        assert!(parse("{ \"a\" \"b\"").is_err());
        assert!(parse("{ \"a\" }").is_err());
    }
}
