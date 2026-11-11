//! `sizebuf_t` command buffers (docs/re/engine-core.md §2.1): appending, inserting and the
//! line splitter of `Cbuf_ExecuteFromBuffer`.

use gs_common::bytestr::{encoded_len, truncate_encoded};

/// Both buffers are 0x4000 bytes.
pub const CBUF_SIZE: usize = 16384;
/// Longest executed line; the rest of a longer line is discarded with it.
const MAX_LINE: usize = 1023;

#[derive(Debug, Clone, Default)]
pub struct Cbuf {
    text: String,
    /// `cursize`: the text's length in original bytes.
    size: usize,
}

impl Cbuf {
    pub fn len(&self) -> usize {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// `Cbuf_AddText`: appends without a terminator; text that does not fit is dropped whole
    /// (`false`, "Cbuf_AddTextToBuffer: overflow").
    pub fn add(&mut self, text: &str) -> bool {
        let n = encoded_len(text);
        if self.size + n >= CBUF_SIZE {
            return false;
        }
        self.text.push_str(text);
        self.size += n;
        true
    }

    /// `Cbuf_InsertText`: puts `text` in front of the current contents (`false` on overflow,
    /// "Cbuf_InsertText: overflow").
    pub fn insert(&mut self, text: &str) -> bool {
        let n = encoded_len(text);
        if self.size + n >= CBUF_SIZE {
            return false;
        }
        self.text.insert_str(0, text);
        self.size += n;
        true
    }

    /// Removes and returns the next line: up to the first `;` outside quotes or any newline
    /// (`\r` is not a separator), capped at 1023 bytes.
    pub fn next_line(&mut self) -> Option<String> {
        if self.text.is_empty() {
            return None;
        }
        let mut quotes = 0;
        let mut end = self.text.len();
        for (i, b) in self.text.bytes().enumerate() {
            if b == b'"' {
                quotes += 1;
            }
            if (b == b';' && quotes % 2 == 0) || b == b'\n' {
                end = i;
                break;
            }
        }
        let line = truncate_encoded(&self.text[..end], MAX_LINE).into_owned();
        let consumed = (end + 1).min(self.text.len());
        self.size -= encoded_len(&self.text[..consumed]);
        self.text.replace_range(..consumed, "");
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        let mut b = Cbuf::default();
        assert!(b.add(text));
        std::iter::from_fn(|| b.next_line()).collect()
    }

    #[test]
    fn lines_split_on_unquoted_semicolons_and_newlines() {
        assert_eq!(lines("a 1;b \"x;y\"\nc\r;d"), ["a 1", "b \"x;y\"", "c\r", "d"]);
        assert_eq!(lines("// foo; quit"), ["// foo", " quit"], "comments do not hide separators");
    }

    #[test]
    fn long_lines_are_cut_and_their_rest_dropped() {
        let long = format!("{};next", "x".repeat(1500));
        let got = lines(&long);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].len(), 1023);
        assert_eq!(got[1], "next");
    }

    #[test]
    fn overflowing_text_is_dropped_whole() {
        let mut b = Cbuf::default();
        assert!(b.add(&"a".repeat(CBUF_SIZE - 10)));
        assert!(!b.add(&"b".repeat(10)));
        assert!(!b.insert(&"c".repeat(10)));
        assert_eq!(b.len(), CBUF_SIZE - 10);
    }
}
