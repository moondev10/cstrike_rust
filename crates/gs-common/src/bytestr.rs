//! The engine's strings are bytes. Network text is kept in Rust `String`s losslessly: valid
//! UTF-8 stays as it is, every other byte (cp1251 text from old plugins, stray bytes) becomes
//! one character of a private-use block, and [`encode`] gives the exact bytes back. Lengths the
//! engine limits are counted in those original bytes ([`encoded_len`]).

/// Byte `b` that is not valid UTF-8 is stored as `ESCAPE + b`.
const ESCAPE: u32 = 0x10_FF00;

/// A character standing for a byte that was not valid UTF-8. VGUI2's `ConvertANSIToUnicode`
/// (`MultiByteToWideChar(CP_UTF8)`) shows such a byte as U+FFFD.
pub fn is_escape(c: char) -> bool {
    (ESCAPE..=ESCAPE + 0xFF).contains(&u32::from(c))
}

fn escape(out: &mut String, bytes: &[u8]) {
    out.extend(bytes.iter().filter_map(|&b| char::from_u32(ESCAPE + u32::from(b))));
}

/// Bytes → `String` keeping every byte. A valid sequence that itself encodes an escape
/// character is escaped byte by byte, so [`encode`] always returns the input.
pub fn decode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            if is_escape(c) {
                let mut buf = [0u8; 4];
                escape(&mut out, c.encode_utf8(&mut buf).as_bytes());
            } else {
                out.push(c);
            }
        }
        escape(&mut out, chunk.invalid());
    }
    out
}

/// The bytes a [`decode`]d string came from (plain UTF-8 for any other string).
pub fn encode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        if is_escape(c) {
            out.push((u32::from(c) - ESCAPE) as u8);
        } else {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    out
}

/// `strlen` of the encoded bytes.
pub fn encoded_len(s: &str) -> usize {
    s.chars().map(|c| if is_escape(c) { 1 } else { c.len_utf8() }).sum()
}

/// Where the first `max` encoded bytes of `s` end: the index of the first character that does
/// not fit whole, the character, and how many of its bytes fit.
fn cut(s: &str, max: usize) -> Option<(usize, char, usize)> {
    let mut used = 0;
    for (i, c) in s.char_indices() {
        let n = if is_escape(c) { 1 } else { c.len_utf8() };
        if used + n > max {
            return Some((i, c, max - used));
        }
        used += n;
    }
    None
}

fn push_partial(out: &mut String, c: char, keep: usize) {
    let mut buf = [0u8; 4];
    escape(out, &c.encode_utf8(&mut buf).as_bytes()[..keep]);
}

/// The first `max` encoded bytes of `s`, as `Q_strncpy` into a `max + 1` byte buffer leaves
/// them: a UTF-8 character the cut splits keeps its leading bytes (as escapes).
pub fn truncate_encoded(s: &str, max: usize) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    match cut(s, max) {
        None => Cow::Borrowed(s),
        Some((i, _, 0)) => Cow::Borrowed(&s[..i]),
        Some((i, c, keep)) => {
            let mut out = s[..i].to_owned();
            push_partial(&mut out, c, keep);
            Cow::Owned(out)
        }
    }
}

/// [`truncate_encoded`] in place.
pub fn truncate_encoded_in_place(s: &mut String, max: usize) {
    if let Some((i, c, keep)) = cut(s, max) {
        s.truncate(i);
        push_partial(s, c, keep);
    }
}

/// What vgui2's `GetText(buf, n)` leaves of `text` (`ConvertUnicodeToANSI` into an `n`-byte
/// buffer, then `buf[n-1] = 0`; vgui2-core.md §5.1a): all of it when it fits with its NUL,
/// else the whole characters that fit in `n` bytes, the last byte dropped when they fill all
/// `n` (a character cut that way keeps its leading bytes, as escapes). An escaped byte is the
/// U+FFFD the wide text held in its place.
pub fn ansi_cap(text: &str, n: usize) -> String {
    let mut bytes = Vec::with_capacity(n.min(text.len() + 1));
    let mut buf = [0u8; 4];
    let mut all = true;
    for c in text.chars() {
        let c = if is_escape(c) { char::REPLACEMENT_CHARACTER } else { c };
        let enc = c.encode_utf8(&mut buf).as_bytes();
        if bytes.len() + enc.len() > n {
            all = false;
            break;
        }
        bytes.extend_from_slice(enc);
    }
    if !(all && bytes.len() < n) && bytes.len() == n {
        bytes.pop();
    }
    decode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A four-letter Cyrillic word ("test"); [`WORD_CP1251`] is the same word in cp1251.
    const WORD: &str = "\u{422}\u{435}\u{441}\u{442}";
    const WORD_CP1251: [u8; 4] = [0xD2, 0xE5, 0xF1, 0xF2];

    #[test]
    fn cp1251_and_utf8_round_trip_byte_for_byte() {
        // The word in cp1251, then in UTF-8, then a stray 0xFF and an escape-range character.
        let bytes: Vec<u8> = [&WORD_CP1251[..], WORD.as_bytes(), &[0xFF], "\u{10FF41}".as_bytes(), b"x"].concat();
        let s = decode(&bytes);
        assert!(s.contains(WORD), "valid UTF-8 stays readable");
        assert_eq!(encode(&s), bytes);
        assert_eq!(encoded_len(&s), bytes.len());
        assert_eq!(encode(&truncate_encoded(&s, 5)), &bytes[..5], "a cut keeps a split character's leading byte");
        let mut t = s.clone();
        truncate_encoded_in_place(&mut t, 5);
        assert_eq!(encode(&t), &bytes[..5]);
        assert_eq!(truncate_encoded(&s, 6), &s[..s.char_indices().nth(5).map_or(0, |(i, _)| i)], "whole characters stay as they are");
    }

    #[test]
    fn ansi_cap_keeps_whole_characters_and_room_for_the_nul() {
        assert_eq!(ansi_cap("abcde", 4), "abc", "ASCII: n-1 characters");
        assert_eq!(ansi_cap("ab", 3), "ab", "fits with its NUL");
        assert_eq!(encode(&ansi_cap("ab€c", 5)), [0x61, 0x62, 0xE2, 0x82], "a filled buffer loses its last byte");
        assert_eq!(ansi_cap("ab€c", 4), "ab", "a character that does not fit is left out");
        assert_eq!(encode(&ansi_cap("aéb", 3)), [0x61, 0xC3]);
        let twice = WORD.repeat(2);
        assert_eq!(ansi_cap(WORD, 64), WORD);
        assert_eq!(ansi_cap(&twice, 7), &WORD[..6], "Cyrillic takes two bytes a character");
        assert_eq!(encode(&ansi_cap(&twice, 8)), &WORD.as_bytes()[..7], "four whole characters fill the buffer");
    }
}
