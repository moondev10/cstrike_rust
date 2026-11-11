//! Valve KeyValues text (`.res` schemes and layouts): `"key" "value"` pairs and
//! `"key" { ... }` blocks, with `//` comments and `[$PLATFORM]` conditionals (ignored).

/// One value: a string or a nested block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Block(Block),
}

/// The entries of a block in file order; keys may repeat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Block {
    pub entries: Vec<(String, Value)>,
}

impl Block {
    /// The first entry named `key` (case-insensitive).
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
    }

    pub fn block(&self, key: &str) -> Option<&Block> {
        match self.get(key)? {
            Value::Block(b) => Some(b),
            Value::Str(_) => None,
        }
    }

    pub fn string(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::Str(s) => Some(s),
            Value::Block(_) => None,
        }
    }

    /// The last string entry named `key`: VGUI2 layouts repeat keys and the later one is what
    /// the shipped menus show (e.g. `labelText` in `Classmenu_CT.res`).
    pub fn last_string(&self, key: &str) -> Option<&str> {
        self.entries.iter().rev().find_map(|(k, v)| match v {
            Value::Str(s) if k.eq_ignore_ascii_case(key) => Some(s.as_str()),
            _ => None,
        })
    }

    /// Nested blocks in file order.
    pub fn blocks(&self) -> impl Iterator<Item = (&str, &Block)> {
        self.entries.iter().filter_map(|(k, v)| match v {
            Value::Block(b) => Some((k.as_str(), b)),
            Value::Str(_) => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Str(String),
    Open,
    Close,
    Conditional,
}

fn tokens(text: &str, escapes: bool) -> Vec<Token> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            c if c.is_ascii_whitespace() => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'{' => {
                out.push(Token::Open);
                i += 1;
            }
            b'}' => {
                out.push(Token::Close);
                i += 1;
            }
            b'[' => {
                while i < b.len() && b[i] != b']' {
                    i += 1;
                }
                i += 1;
                out.push(Token::Conditional);
            }
            b'"' => {
                let mut s = Vec::new();
                let mut end = i + 1;
                while end < b.len() && b[end] != b'"' {
                    if escapes && b[end] == b'\\' && end + 1 < b.len() {
                        end += 1;
                        s.push(match b[end] {
                            b'n' => b'\n',
                            b't' => b'\t',
                            other => other,
                        });
                    } else {
                        s.push(b[end]);
                    }
                    end += 1;
                }
                out.push(Token::Str(String::from_utf8_lossy(&s).into_owned()));
                i = end + 1;
            }
            _ => {
                let start = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'{' | b'}' | b'"') {
                    i += 1;
                }
                out.push(Token::Str(String::from_utf8_lossy(&b[start..i]).into_owned()));
            }
        }
    }
    out
}

/// Parses a whole file into its top-level block. Malformed input yields what was readable.
pub fn parse(text: &str) -> Block {
    parse_tokens(&tokens(text, false))
}

/// Like [`parse`], with `\n`, `\t`, `\"` and `\\` escapes in quoted strings (localization
/// files).
pub fn parse_escaped(text: &str) -> Block {
    parse_tokens(&tokens(text, true))
}

fn parse_tokens(toks: &[Token]) -> Block {
    let toks: Vec<Token> = toks.iter().filter(|t| **t != Token::Conditional).cloned().collect();
    let mut pos = 0;
    parse_block(&toks, &mut pos)
}

fn parse_block(toks: &[Token], pos: &mut usize) -> Block {
    let mut block = Block::default();
    while let Some(tok) = toks.get(*pos) {
        *pos += 1;
        let key = match tok {
            Token::Str(k) => k.clone(),
            Token::Close => return block,
            Token::Open | Token::Conditional => continue,
        };
        match toks.get(*pos) {
            Some(Token::Open) => {
                *pos += 1;
                block.entries.push((key, Value::Block(parse_block(toks, pos))));
            }
            Some(Token::Str(v)) => {
                *pos += 1;
                block.entries.push((key, Value::Str(v.clone())));
            }
            _ => return block,
        }
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_blocks_comments_and_conditionals() {
        let kv = parse(
            "Scheme\n{\n  // fonts\n  Fonts {\n    \"EngineFont\" { \"1\" { \"name\" \"Verdana\" \"tall\" \"13\" [$WIN32] } }\n  }\n  \"Color\"  \"255 0 0 255\"\n}\n",
        );
        let scheme = kv.block("scheme");
        assert_eq!(scheme.and_then(|s| s.string("color")), Some("255 0 0 255"));
        let font = scheme.and_then(|s| s.block("Fonts")).and_then(|f| f.block("EngineFont")).and_then(|f| f.block("1"));
        assert_eq!(font.and_then(|f| f.string("tall")), Some("13"));
        assert_eq!(font.map(|f| f.entries.len()), Some(2));
    }
}
