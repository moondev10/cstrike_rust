//! VGUI2 Win32 fonts: system TrueType faces chosen by family and weight like the GDI font
//! mapper, sized by cell height (`CreateFont` with a positive height = ascent + descent), with
//! ABC character widths and glyph cells (white glyph, optional 1-px black drop shadow). As in
//! `SetFontGlyphSet` (vgui-surface.md §8.2) a scheme face draws U+0000..U+00FF and Tahoma the
//! rest, and what Tahoma lacks GDI draws from Tahoma's font links.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, OnceLock};

use crate::scheme::FontSpec;

/// Pixels per em `fontdue` flattens a face's outlines for. Every size of the face shares that
/// geometry, and one at or below it rasterizes from curves at least as fine as its own
/// (scheme fonts are at most 128 tall).
const OUTLINE_SCALE: f32 = 128.0;

/// One installed face.
#[derive(Debug, Clone)]
struct FaceInfo {
    path: PathBuf,
    /// Family names in every language the face lists.
    families: Vec<String>,
    /// Full names (`Verdana Bold`), which GDI also accepts as face names.
    full: Vec<String>,
    weight: i32,
    italic: bool,
}

/// The installed TrueType faces (scanned once) and the face files parsed so far.
#[derive(Default)]
pub struct FontIndex {
    faces: Vec<FaceInfo>,
    parsed: HashMap<PathBuf, Option<Arc<Face>>>,
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(win) = std::env::var_os("WINDIR") {
        dirs.push(Path::new(&win).join("Fonts"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(Path::new(&local).join("Microsoft").join("Windows").join("Fonts"));
    }
    dirs.push(PathBuf::from("/usr/share/fonts/truetype/msttcorefonts"));
    dirs.push(PathBuf::from("/usr/share/fonts/TTF"));
    dirs
}

impl FontIndex {
    /// Scans the system font directories for `.ttf`/`.otf` faces.
    pub fn scan() -> Self {
        let mut faces = Vec::new();
        for dir in font_dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for path in entries.flatten().map(|e| e.path()) {
                let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
                if !matches!(ext.as_deref(), Some("ttf" | "otf")) {
                    continue;
                }
                let Ok(data) = std::fs::read(&path) else { continue };
                let Ok(face) = ttf_parser::Face::parse(&data, 0) else { continue };
                let names = |id: u16| -> Vec<String> { face.names().into_iter().filter(|n| n.name_id == id).filter_map(|n| n.to_string()).collect() };
                let families = names(ttf_parser::name_id::FAMILY);
                if !families.is_empty() {
                    let full = names(ttf_parser::name_id::FULL_NAME);
                    faces.push(FaceInfo { path, families, full, weight: i32::from(face.weight().to_number()), italic: face.is_italic() });
                }
            }
        }
        Self { faces, parsed: HashMap::new() }
    }

    /// The face for `family` closest to `weight` (GDI picks the nearest weight; italic must
    /// match when available; a full face name like `Verdana Bold` picks that face), parsed on
    /// its first use and shared by every size.
    pub fn face(&mut self, family: &str, weight: i32, italic: bool) -> Option<Arc<Face>> {
        let weight = if weight == 0 { 400 } else { weight };
        let named = |names: &[String]| names.iter().any(|n| n.eq_ignore_ascii_case(family));
        let by_family = self.faces.iter().filter(|f| named(&f.families));
        let best = by_family
            .min_by_key(|f| ((f.italic != italic) as i32 * 10_000) + (f.weight - weight).abs() * 2 + i32::from(f.weight < weight))
            .or_else(|| self.faces.iter().filter(|f| named(&f.full)).min_by_key(|f| f.italic != italic))?;
        self.parsed
            .entry(best.path.clone())
            .or_insert_with(|| std::fs::read(&best.path).ok().and_then(|data| Face::parse(&data, 0)).map(Arc::new))
            .clone()
    }
}

/// One face file: its outlines and the metrics every size is derived from.
pub struct Face {
    outlines: fontdue::Font,
    /// `usWinAscent` and `usWinDescent` (else the `hhea` values), font units.
    ascent: f32,
    descent: f32,
    units_per_em: f32,
    /// A symbol font (Marlett): GDI reads character codes through the Windows Symbol cmap
    /// (U+F000 + code); the glyph index per code 0..256.
    symbol: Option<Vec<Option<u16>>>,
}

impl Face {
    /// Face `index` of a file (a collection's member, else 0).
    fn parse(data: &[u8], index: u32) -> Option<Self> {
        let parsed = ttf_parser::Face::parse(data, index).ok()?;
        let (ascent, descent) = parsed
            .tables()
            .os2
            .map(|os2| (f32::from(os2.windows_ascender()), f32::from(os2.windows_descender()).abs()))
            .unwrap_or((f32::from(parsed.ascender()), f32::from(parsed.descender()).abs()));
        let symbol = parsed.tables().cmap.and_then(|cmap| {
            let table = cmap.subtables.into_iter().find(|s| s.platform_id == ttf_parser::PlatformId::Windows && s.encoding_id == 0)?;
            Some((0u32..256).map(|c| table.glyph_index(0xF000 + c).map(|g| g.0)).collect())
        });
        let settings = fontdue::FontSettings { scale: OUTLINE_SCALE, collection_index: index, ..Default::default() };
        let outlines = fontdue::Font::from_bytes(data, settings).ok()?;
        Some(Self { outlines, ascent, descent, units_per_em: f32::from(parsed.units_per_em()), symbol })
    }

    /// The glyph of `ch` (0: the missing-glyph box).
    fn glyph(&self, ch: char) -> u16 {
        self.outlines.lookup_glyph_index(ch)
    }

    /// Pixels per em and the baseline from the cell top for a cell `tall` high.
    fn size(&self, tall: i32) -> (f32, i32) {
        let scale = tall as f32 / (self.ascent + self.descent).max(1.0);
        (self.units_per_em * scale, (self.ascent * scale).round() as i32)
    }
}

/// One of Tahoma's font links (`HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\FontLink\
/// SystemLink`, value `Tahoma`: `file,family` entries): when Tahoma has no glyph for a
/// character, GDI's `ExtTextOutW` draws it from the first linked face that has one. Each face
/// is read when a character first gets that far: its coverage, then its outlines if it is the
/// one drawing.
struct Link {
    file: PathBuf,
    family: String,
    /// The member of the file named `family` and the BMP characters it maps, one bit each.
    coverage: OnceLock<Option<(u32, Box<[u64]>)>>,
    face: OnceLock<Option<Face>>,
}

impl Link {
    fn covers(&self, code: u32) -> bool {
        let coverage = self.coverage.get_or_init(|| {
            let data = std::fs::read(&self.file).ok()?;
            let count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
            (0..count).find_map(|index| {
                let face = ttf_parser::Face::parse(&data, index).ok()?;
                let named = face.names().into_iter().any(|n| n.name_id == ttf_parser::name_id::FAMILY && n.to_string().is_some_and(|s| s.eq_ignore_ascii_case(&self.family)));
                if !named {
                    return None;
                }
                let mut bits = vec![0u64; 0x10000 / 64].into_boxed_slice();
                for table in face.tables().cmap?.subtables.into_iter().filter(|t| t.is_unicode()) {
                    table.codepoints(|c| {
                        if c < 0x10000 && table.glyph_index(c).is_some_and(|g| g.0 != 0) {
                            bits[c as usize / 64] |= 1 << (c % 64);
                        }
                    });
                }
                Some((index, bits))
            })
        });
        coverage.as_ref().is_some_and(|(_, bits)| code < 0x10000 && bits[code as usize / 64] >> (code % 64) & 1 == 1)
    }

    fn face(&self) -> Option<&Face> {
        let index = self.coverage.get()?.as_ref()?.0;
        self.face.get_or_init(|| std::fs::read(&self.file).ok().and_then(|data| Face::parse(&data, index))).as_ref()
    }
}

static TAHOMA_LINKS: LazyLock<Vec<Link>> = LazyLock::new(|| {
    system_links("Tahoma")
        .iter()
        .filter_map(|entry| {
            let mut fields = entry.split(',');
            let (file, family) = (fields.next()?.trim(), fields.next()?.trim());
            let file = font_dirs().into_iter().map(|d| d.join(file)).find(|p| p.is_file())?;
            Some(Link { file, family: family.to_owned(), coverage: OnceLock::new(), face: OnceLock::new() })
        })
        .collect()
});

/// The `SystemLink` entries of `family`.
#[cfg(windows)]
fn system_links(family: &str) -> Vec<String> {
    winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\FontLink\SystemLink")
        .and_then(|key| key.get_value(family))
        .unwrap_or_default()
}

#[cfg(not(windows))]
fn system_links(_family: &str) -> Vec<String> {
    Vec::new()
}

/// The first of Tahoma's links that has `ch`.
fn linked_face(ch: char) -> Option<&'static Face> {
    TAHOMA_LINKS.iter().find(|l| l.covers(u32::from(ch))).and_then(Link::face)
}

/// A font at one size (`CWin32Font`, with the amalgam's Tahoma for U+0100 and up).
pub struct Font {
    face: Arc<Face>,
    /// Tahoma at the same size, weight and flags: draws U+0100 and up. `None` when the scheme
    /// face is Tahoma or Marlett, which draw everything.
    wide: Option<Arc<Face>>,
    /// The scheme face is Tahoma: its missing characters go through the font links too.
    tahoma: bool,
    /// Cell height (`tall`).
    pub tall: i32,
    antialias: bool,
    /// Drop-shadow offset (0 or 1).
    pub shadow: i32,
}

/// `GetCharABCWidths`: leading space, black-box width, trailing space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Abc {
    pub a: i32,
    pub b: i32,
    pub c: i32,
}

impl Abc {
    pub fn advance(&self) -> i32 {
        self.a + self.b + self.c
    }
}

/// A rasterized character cell: `width × height` RGBA, drawn at `x + a`, `y`.
pub struct GlyphCell {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Font {
    /// `face` at the scheme spec's size, `wide` (Tahoma) drawing U+0100 and up.
    pub fn new(face: Arc<Face>, wide: Option<Arc<Face>>, spec: &FontSpec) -> Self {
        Self {
            face,
            wide,
            tahoma: spec.family.eq_ignore_ascii_case("Tahoma"),
            tall: spec.tall.max(1),
            antialias: spec.antialias,
            shadow: i32::from(spec.dropshadow),
        }
    }

    /// The glyph of `ch` through the symbol cmap, for symbol fonts.
    fn symbol_glyph(&self, ch: char) -> Option<u16> {
        self.face.symbol.as_ref()?.get(u32::from(ch) as usize).copied().flatten()
    }

    /// What GDI draws `ch` with: the face whose cell it is (scheme face below U+0100, Tahoma
    /// above), the face the glyph comes from (that one, or for a character Tahoma lacks the
    /// first font link that has it) and the glyph (0: the cell face's missing-glyph box).
    fn source(&self, ch: char) -> (&Face, &Face, u16) {
        if let Some(g) = self.symbol_glyph(ch) {
            return (&self.face, &self.face, g);
        }
        let (cell, links) = match &self.wide {
            Some(tahoma) if u32::from(ch) >= 0x100 => (tahoma.as_ref(), true),
            _ => (self.face.as_ref(), self.tahoma),
        };
        let g = cell.glyph(ch);
        if g == 0
            && links
            && let Some(linked) = linked_face(ch)
        {
            return (cell, linked, linked.glyph(ch));
        }
        (cell, cell, g)
    }

    pub fn abc(&self, ch: char) -> Abc {
        let (_, face, g) = self.source(shown(ch));
        let m = face.outlines.metrics_indexed(g, face.size(self.tall).0);
        let advance = m.advance_width.round() as i32;
        if m.width == 0 {
            return Abc { a: 0, b: 0, c: advance };
        }
        let (a, b) = (m.xmin, m.width as i32);
        Abc { a, b: b + self.shadow, c: advance - a - b - self.shadow }
    }

    /// Text width in pixels (sum of ABC advances).
    pub fn text_width(&self, text: &str) -> i32 {
        text.chars().map(|c| self.abc(c).advance()).sum()
    }

    /// The character cell (`GetCharRGBA` + `ApplyDropShadowToTexture`), `None` for blank
    /// characters. A linked glyph sits on the cell face's baseline.
    pub fn rasterize(&self, ch: char) -> Option<GlyphCell> {
        let (cell, face, g) = self.source(shown(ch));
        let (m, coverage) = face.outlines.rasterize_indexed(g, face.size(self.tall).0);
        if m.width == 0 || m.height == 0 {
            return None;
        }
        let shadow = self.shadow as usize;
        let (w, h) = (m.width + shadow, self.tall as usize + shadow);
        let mut alpha = vec![0u8; w * h];
        let top = cell.size(self.tall).1 - (m.ymin + m.height as i32);
        for row in 0..m.height {
            let y = top + row as i32;
            if y < 0 || y as usize >= h {
                continue;
            }
            for col in 0..m.width {
                let c = coverage[row * m.width + col];
                alpha[y as usize * w + col] = if self.antialias { c } else if c >= 128 { 255 } else { 0 };
            }
        }
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let a = alpha[y * w + x];
                if a > 0 {
                    rgba.extend_from_slice(&[255, 255, 255, a]);
                } else if shadow > 0 && x >= shadow && y >= shadow && alpha[(y - shadow) * w + x - shadow] > 0 {
                    rgba.extend_from_slice(&[0, 0, 0, alpha[(y - shadow) * w + x - shadow]]);
                } else {
                    rgba.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        Some(GlyphCell { width: w as u32, height: h as u32, rgba })
    }
}

/// The character a VGUI2 font draws for `ch`: a byte that was not UTF-8 shows as U+FFFD.
fn shown(ch: char) -> char {
    if gs_common::bytestr::is_escape(ch) { '\u{FFFD}' } else { ch }
}
