//! Sentences (docs/re/audio.md §9.1–§9.4): `sound/sentences.txt` and the word lists with their
//! `(v p s e t)` parameters.

/// `CVOXFILESENTENCEMAX`.
pub const MAX_SENTENCES: usize = 2048;
/// Words per sentence.
pub const MAX_WORDS: usize = 32;

/// `sentences.txt`: names and texts in file order (the wire sentence index).
#[derive(Debug, Clone, Default)]
pub struct Sentences {
    entries: Vec<(String, String)>,
}

impl Sentences {
    /// `VOX_ReadSentenceFile`: one sentence per line, `/` comments, name then text.
    pub fn parse(text: &str) -> Self {
        let mut entries = Vec::new();
        for line in text.lines() {
            let line = line.trim_start_matches(['\n', '\r', '\t', ' ']);
            if line.is_empty() || line.starts_with('/') {
                continue;
            }
            let (name, rest) = line.split_once([' ', '\t']).unwrap_or((line, ""));
            entries.push((name.to_owned(), rest.trim_end_matches(['\r', '\n']).to_owned()));
            if entries.len() == MAX_SENTENCES {
                break;
            }
        }
        Self { entries }
    }

    /// `rgpszrawsentence[index]`'s name.
    pub fn name(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(|(n, _)| n.as_str())
    }

    /// The text of sentence `name` (case-insensitive), leading whitespace skipped.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.entries.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, t)| t.trim_start())
    }
}

/// A word's parameters (`voxword_t` without the playback state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WordParams {
    pub volume: i32,
    /// -1 = the channel's pitch.
    pub pitch: i32,
    pub start: i32,
    pub end: i32,
    pub timecompress: i32,
}

impl Default for WordParams {
    fn default() -> Self {
        Self { volume: 100, pitch: -1, start: 0, end: 100, timecompress: 0 }
    }
}

/// `VOX_ParseString` + `VOX_ParseWordParams`: the sound directory and the words with their
/// parameters. An isolated `(…)` block sets the defaults of the words after it.
pub fn parse_sentence(text: &str) -> (String, Vec<(String, WordParams)>) {
    let (dir, list) = match text.rfind('/') {
        Some(i) if i + 1 < 32 => (text[..=i].to_owned(), &text[i + 1..]),
        _ => ("vox/".to_owned(), text),
    };
    let bytes = list.as_bytes();
    let mut tokens: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() && tokens.len() < MAX_WORDS {
        while bytes.get(i) == Some(&b' ') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        while i < bytes.len() && !matches!(bytes[i], b' ' | b'.' | b',') {
            if bytes[i] == b'(' {
                while i < bytes.len() && bytes[i] != b')' {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        tokens.push(list[start..i].to_owned());
        if let Some(&c) = bytes.get(i) {
            if matches!(c, b'.' | b',') && !matches!(bytes.get(i + 1), None | Some(b'\n' | b'\r')) && tokens.len() < MAX_WORDS {
                tokens.push(if c == b'.' { "_period" } else { "_comma" }.to_owned());
            }
            i += 1;
        }
    }
    let mut defaults = WordParams::default();
    let mut words = Vec::new();
    for tok in tokens {
        let mut params = defaults;
        let name = match (tok.find('('), tok.ends_with(')')) {
            (Some(open), true) => {
                parse_params(&tok[open + 1..tok.len() - 1], &mut params);
                tok[..open].to_owned()
            }
            _ => tok,
        };
        if name.is_empty() {
            defaults = params;
        } else {
            words.push((name, params));
        }
    }
    (dir, words)
}

/// `v`, `p`, `s`, `e`, `t` each followed by up to 7 digits.
fn parse_params(s: &str, p: &mut WordParams) {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        i += 1;
        if !matches!(c, b'v' | b'p' | b's' | b'e' | b't') {
            continue;
        }
        let digits: String = s[i..].chars().take_while(char::is_ascii_digit).take(7).collect();
        if digits.is_empty() {
            break;
        }
        i += digits.len();
        let v = digits.parse().unwrap_or(0);
        match c {
            b'v' => p.volume = v,
            b'p' => p.pitch = v,
            b's' => p.start = v,
            b'e' => p.end = v,
            _ => p.timecompress = v,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences_index_in_file_order_and_split_into_words() {
        let s = Sentences::parse("// comment\n  HG_ALERT0 hgrunt/(p100) clik alert. go\nC1A0_SCI_GREET scientist/greet\n");
        assert_eq!(s.name(0), Some("HG_ALERT0"));
        assert_eq!(s.text("hg_alert0"), Some("hgrunt/(p100) clik alert. go"));
        let (dir, words) = parse_sentence(s.text("HG_ALERT0").unwrap());
        assert_eq!(dir, "hgrunt/");
        let names: Vec<&str> = words.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["clik", "alert", "_period", "go"]);
        assert_eq!(words[0].1.pitch, 100, "an isolated block sets the defaults");
        let (dir, words) = parse_sentence("deeoo(e50 t10) doop(v70),");
        assert_eq!(dir, "vox/");
        assert_eq!(words[0].1, WordParams { volume: 100, pitch: -1, start: 0, end: 50, timecompress: 10 });
        assert_eq!(words[1].1.volume, 70);
        assert_eq!(words.len(), 2, "a trailing comma adds no _comma word");
    }
}
