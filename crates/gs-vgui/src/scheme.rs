//! A loaded scheme `.res` (`CScheme`, docs/re/vgui2-core.md §4): colours, resource strings and
//! borders. Fonts are created by the engine surface ([`crate::Surface::font`]) from the same file.

use std::sync::Arc;

use gs_formats::keyvalues::{self, Block, Value};

use crate::border::{Border, Line};
use crate::surface::Color;

/// One scheme.
#[derive(Debug, Clone, Default)]
pub struct Scheme {
    /// The file name it was loaded from (the font namespace for [`crate::Surface::font`]).
    pub file: String,
    pub tag: String,
    base_settings: Block,
    colors: Block,
    borders: Vec<(String, Option<Arc<Border>>)>,
    base_border: Option<Arc<Border>>,
}

/// `KeyValues::FindKey` with `/` walking into sub-keys; duplicate keys merge (the later wins).
fn find<'a>(block: &'a Block, path: &str) -> Option<&'a Value> {
    let mut cur = block;
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        let v = cur.entries.iter().rev().find(|(k, _)| k.eq_ignore_ascii_case(part)).map(|(_, v)| v)?;
        if parts.peek().is_none() {
            return Some(v);
        }
        match v {
            Value::Block(b) => cur = b,
            Value::Str(_) => return None,
        }
    }
    None
}

fn find_string<'a>(block: &'a Block, path: &str) -> Option<&'a str> {
    match find(block, path)? {
        Value::Str(s) => Some(s),
        Value::Block(_) => None,
    }
}

/// `sscanf("%d %d %d %d")`: the parsed values and how many there were.
fn scan_ints<const N: usize>(s: &str) -> ([i32; N], usize) {
    let mut out = [0; N];
    let mut n = 0;
    for tok in s.split_ascii_whitespace().take(N) {
        let digits = tok.trim_start_matches(['-', '+']);
        if digits.is_empty() || !digits.starts_with(|c: char| c.is_ascii_digit()) {
            break;
        }
        out[n] = crate::msg::atoi(tok);
        n += 1;
    }
    (out, n)
}

fn color_of(v: [i32; 4]) -> Color {
    Color(v.map(|c| c as u8))
}

impl Scheme {
    /// `CScheme::LoadFromFile` over the file text.
    pub fn parse(file: &str, tag: &str, text: &str) -> Self {
        let root = keyvalues::parse(text);
        let data = root.block("Scheme").cloned().unwrap_or_default();
        let base_settings = data.block("BaseSettings").cloned().unwrap_or_default();
        let colors = data.block("Colors").cloned().unwrap_or_default();
        let mut s = Self { file: file.to_owned(), tag: tag.to_owned(), base_settings, colors, borders: Vec::new(), base_border: None };
        s.load_borders(data.block("Borders"));
        s
    }

    /// `GetColor(name, def)`.
    pub fn color(&self, name: &str, def: Color) -> Color {
        self.color_depth(name, def, 0)
    }

    fn color_depth(&self, name: &str, def: Color, depth: u32) -> Color {
        if let Some(s) = find_string(&self.colors, name) {
            return color_of(scan_ints::<4>(s).0);
        }
        if let Some(s) = find_string(&self.base_settings, name) {
            let (v, n) = scan_ints::<4>(s);
            if n < 3 {
                // The value names another colour (bounded against self-reference).
                return if depth < 16 { self.color_depth(s, def, depth + 1) } else { def };
            }
            return color_of(v);
        }
        let (v, n) = scan_ints::<4>(name);
        if n >= 3 { color_of(v) } else { def }
    }

    /// `GetResourceString(name)`: a BaseSettings string, `""` if absent.
    pub fn resource_string(&self, name: &str) -> &str {
        find_string(&self.base_settings, name).unwrap_or("")
    }

    /// A BaseSettings value as `GetFloat`.
    pub fn base_float(&self, name: &str) -> Option<f32> {
        find_string(&self.base_settings, name).map(|s| s.trim().parse().unwrap_or(0.0))
    }

    /// `GetBorder(name)`: the first entry with that (case-insensitive) name, else `BaseBorder`.
    pub fn border(&self, name: &str) -> Option<Arc<Border>> {
        match self.borders.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            Some((_, b)) => b.clone(),
            None => self.base_border.clone(),
        }
    }

    fn load_borders(&mut self, borders: Option<&Block>) {
        let Some(borders) = borders else { return };
        for (name, v) in &borders.entries {
            let Value::Block(b) = v else { continue };
            let mut border = Border { name: name.clone(), ..Border::default() };
            let (inset, n) = scan_ints::<4>(find_string(b, "inset").unwrap_or("0 0 0 0"));
            border.inset[..n].copy_from_slice(&inset[..n]);
            for (i, side) in ["Left", "Top", "Right", "Bottom"].iter().enumerate() {
                let Some(side_kv) = b.block(side) else { continue };
                border.sides[i] = side_kv
                    .blocks()
                    .map(|(_, line)| {
                        let color = self.color(line.string("color").unwrap_or(""), Color::TRANSPARENT);
                        let (off, _) = scan_ints::<2>(line.string("offset").unwrap_or(""));
                        Line { color, start: off[0], end: off[1] }
                    })
                    .collect();
            }
            self.borders.push((name.clone(), Some(Arc::new(border))));
        }
        // Aliases ("Name" "OtherBorder"), resolved through what is loaded so far.
        for (name, v) in &borders.entries {
            let Value::Str(target) = v else { continue };
            let resolved = self.borders.iter().find(|(n, _)| n.eq_ignore_ascii_case(target)).and_then(|(_, b)| b.clone());
            self.borders.push((name.clone(), resolved));
        }
        self.base_border = self.border("BaseBorder");
    }
}
