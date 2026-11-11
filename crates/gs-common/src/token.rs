//! `COM_Parse` and `Cmd_TokenizeString` (docs/re/engine-core.md §2.2).

/// Longest token `COM_Parse` produces (its buffer is 2048 bytes).
pub const MAX_TOKEN: usize = 2047;
/// `Cmd_TokenizeString` stops entirely at a token of this many bytes or more.
pub const MAX_ARG_LEN: usize = 515;
/// Tokens beyond this count are parsed but dropped.
pub const MAX_ARGS: usize = 80;

/// One `COM_Parse` step: the next token and the remaining input, or `None` at end of data.
///
/// `{ } ( ) ' ,` are always single-character tokens; `:` is too unless `ignore_colons`.
pub fn com_parse(input: &[u8], ignore_colons: bool) -> Option<(Vec<u8>, &[u8])> {
    let mut s = skip_whitespace_and_comments(input)?;
    if s[0] == b'"' {
        s = &s[1..];
        let end = s.iter().position(|&b| b == b'"' || b == 0).unwrap_or(s.len());
        let token = s[..end.min(MAX_TOKEN)].to_vec();
        let rest = if s.get(end) == Some(&b'"') { &s[end + 1..] } else { &s[end..] };
        return Some((token, rest));
    }
    if is_break(s[0], ignore_colons) {
        return Some((vec![s[0]], &s[1..]));
    }
    let end = s
        .iter()
        .position(|&b| b <= b' ' || is_break(b, ignore_colons))
        .unwrap_or(s.len())
        .min(MAX_TOKEN);
    Some((s[..end].to_vec(), &s[end..]))
}

fn is_break(b: u8, ignore_colons: bool) -> bool {
    matches!(b, b'{' | b'}' | b'(' | b')' | b'\'' | b',') || (b == b':' && !ignore_colons)
}

/// Skips UTF-8 codepoints `<= 0x20` and `//` comments. `None` at end of data (or NUL).
fn skip_whitespace_and_comments(mut s: &[u8]) -> Option<&[u8]> {
    loop {
        let &first = s.first()?;
        if first == 0 {
            return None;
        }
        if first <= b' ' {
            s = &s[1..];
            continue;
        }
        if s.starts_with(b"//") {
            let nl = s.iter().position(|&b| b == b'\n').unwrap_or(s.len());
            s = &s[nl..];
            continue;
        }
        return Some(s);
    }
}

/// A tokenized command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    pub argv: Vec<String>,
    /// Everything from the first non-blank byte of the second token to the end of the line,
    /// verbatim (quotes included). `None` when there is no second token.
    pub args: Option<String>,
}

impl Args {
    pub fn argc(&self) -> usize {
        self.argv.len()
    }

    /// `Cmd_Argv`: empty string when out of range.
    pub fn arg(&self, i: usize) -> &str {
        self.argv.get(i).map_or("", String::as_str)
    }
}

/// `Cmd_TokenizeString`: tokenizes one line; a newline outside of a token ends it. Lengths count
/// the line's original bytes.
pub fn tokenize(line: &str) -> Args {
    if line.chars().any(crate::bytestr::is_escape) {
        tokenize_bytes(&crate::bytestr::encode(line), false)
    } else {
        tokenize_bytes(line.as_bytes(), false)
    }
}

pub fn tokenize_bytes(line: &[u8], ignore_colons: bool) -> Args {
    let mut out = Args::default();
    let mut s = line;
    loop {
        while let Some(&b) = s.first() {
            if b == b'\n' {
                return out;
            }
            if b == 0 || b > b' ' {
                break;
            }
            s = &s[1..];
        }
        let rest_of_line = (out.argv.len() == 1).then(|| &s[..s.iter().position(|&b| b == 0).unwrap_or(s.len())]);
        let Some((token, rest)) = com_parse(s, ignore_colons) else { return out };
        if let Some(line_rest) = rest_of_line {
            out.args = Some(crate::bytestr::decode(line_rest));
        }
        if token.len() >= MAX_ARG_LEN {
            return out;
        }
        if out.argv.len() < MAX_ARGS {
            out.argv.push(crate::bytestr::decode(&token));
        }
        s = rest;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(line: &str) -> Vec<String> {
        tokenize(line).argv
    }

    #[test]
    fn colon_and_punctuation_are_separate_tokens() {
        assert_eq!(argv("connect 1.2.3.4:27015"), ["connect", "1.2.3.4", ":", "27015"]);
        assert_eq!(argv("say don't"), ["say", "don", "'", "t"]);
        assert_eq!(tokenize_bytes(b"a:b", true).argv, ["a:b"]);
    }

    #[test]
    fn quotes_comments_and_empty_tokens() {
        assert_eq!(argv("bind \"k\" \"+attack; say hi\""), ["bind", "k", "+attack; say hi"]);
        assert_eq!(argv("echo \"\" x"), ["echo", "", "x"]);
        assert_eq!(argv("echo a // rest"), ["echo", "a"]);
        assert_eq!(argv("echo \"unterminated"), ["echo", "unterminated"]);
    }

    #[test]
    fn newline_ends_line_but_comment_skip_crosses_it() {
        assert_eq!(argv("a b\nc"), ["a", "b"]);
        // COM_Parse skips the comment up to '\n' and keeps skipping whitespace, like the original.
        assert_eq!(argv("a //x\nb"), ["a", "b"]);
    }

    #[test]
    fn args_keeps_the_rest_verbatim() {
        let t = tokenize("say  \"hello\"  world ");
        assert_eq!(t.args.as_deref(), Some("\"hello\"  world "));
        assert_eq!(tokenize("quit").args, None);
    }

    #[test]
    fn limits() {
        let many = (0..100).map(|i| i.to_string()).collect::<Vec<_>>().join(" ");
        assert_eq!(argv(&many).len(), MAX_ARGS);
        let long = format!("a {} b", "x".repeat(MAX_ARG_LEN));
        assert_eq!(argv(&long), ["a"], "an oversized token stops tokenizing");
    }
}
