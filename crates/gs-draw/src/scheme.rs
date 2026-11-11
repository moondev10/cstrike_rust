//! VGUI2 schemes (`resource/TrackerScheme.res`, `ClientScheme.res`): font definitions picked
//! by screen height (`yres "min max"`), and base settings.

use gs_formats::keyvalues::{self, Block};

/// One scheme font at the current resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontSpec {
    pub family: String,
    /// Cell height in pixels (GDI `CreateFont` height).
    pub tall: i32,
    pub weight: i32,
    pub antialias: bool,
    pub dropshadow: bool,
    pub italic: bool,
    /// `"symbol"`: drawn through the Windows Symbol cmap, exempt from the language sizes.
    pub symbol: bool,
}

/// `CScheme::LoadFonts`' last size steps (vgui2-core.md §4.4): at most 128, then by the
/// surface language for every non-symbol font: Korean, Chinese and Thai at least 13, Russian
/// above 12 cut to 90 %.
pub fn language_tall(tall: i32, symbol: bool, language: &str) -> i32 {
    let tall = tall.min(128);
    if symbol {
        tall
    } else if ["korean", "tchinese", "schinese", "thai"].iter().any(|l| language.eq_ignore_ascii_case(l)) {
        tall.max(13)
    } else if language.eq_ignore_ascii_case("russian") && tall > 12 {
        ((f64::from(tall) * 0.9) as i32).max(12)
    } else {
        tall
    }
}

#[derive(Debug, Clone, Default)]
pub struct Scheme {
    root: Block,
}

/// `"min max"` contains `h`.
fn yres_matches(range: &str, h: u32) -> bool {
    let mut it = range.split_whitespace().filter_map(|v| v.parse::<u32>().ok());
    match (it.next(), it.next()) {
        (Some(lo), Some(hi)) => (lo..=hi).contains(&h),
        _ => true,
    }
}

/// `"r g b"` or `"r g b a"`.
fn parse_color(s: &str) -> Option<[u8; 4]> {
    let v: Vec<u8> = s.split_whitespace().map(|c| c.parse::<u8>().ok()).collect::<Option<_>>()?;
    match v.as_slice() {
        [r, g, b] => Some([*r, *g, *b, 255]),
        [r, g, b, a] => Some([*r, *g, *b, *a]),
        _ => None,
    }
}

impl Scheme {
    pub fn parse(text: &str) -> Self {
        let kv = keyvalues::parse(text);
        Self { root: kv.block("Scheme").cloned().unwrap_or_default() }
    }

    /// `BaseSettings` value.
    pub fn base_setting(&self, key: &str) -> Option<&str> {
        self.root.block("BaseSettings")?.string(key)
    }

    /// `IScheme::GetColor`: a `BaseSettings` entry (a `Colors` name or `"r g b a"`), else a
    /// `Colors` entry.
    pub fn color(&self, name: &str) -> Option<[u8; 4]> {
        let colors = self.root.block("Colors");
        let value = self.base_setting(name).or_else(|| colors?.string(name))?;
        parse_color(value).or_else(|| colors?.string(value).and_then(parse_color))
    }

    /// The fonts the scheme defines, in file order.
    pub fn font_names(&self) -> impl Iterator<Item = &str> {
        self.root.block("Fonts").into_iter().flat_map(|fonts| fonts.blocks().map(|(name, _)| name))
    }

    /// The first variant of font `name` whose `yres` range contains `screen_height` (variants
    /// without `yres` always match).
    pub fn font(&self, name: &str, screen_height: u32) -> Option<FontSpec> {
        let variants = self.root.block("Fonts")?.block(name)?;
        let (_, v) = variants.blocks().find(|(_, v)| v.string("yres").is_none_or(|r| yres_matches(r, screen_height)))?;
        let int = |key: &str| v.string(key).and_then(|s| s.trim().parse::<i32>().ok()).unwrap_or(0);
        Some(FontSpec {
            family: v.string("name")?.to_owned(),
            tall: int("tall"),
            weight: int("weight"),
            antialias: int("antialias") != 0,
            dropshadow: int("dropshadow") != 0,
            italic: int("italic") != 0,
            symbol: int("symbol") != 0,
        })
    }

    /// The font a proportional panel gets: a variant without `yres` has its `tall` scaled by
    /// `SH / min(SH, 480)`, capped at 128; one picked by `yres` is the plain font.
    pub fn font_proportional(&self, name: &str, screen_height: u32) -> Option<FontSpec> {
        self.font_scaled(name, screen_height, screen_height.min(480))
    }

    /// The HD-proportional variant (`GetFont(name, true, true)`): base `min(SH, 720)`.
    pub fn font_hd_proportional(&self, name: &str, screen_height: u32) -> Option<FontSpec> {
        self.font_scaled(name, screen_height, screen_height.min(720))
    }

    fn font_scaled(&self, name: &str, screen_height: u32, base: u32) -> Option<FontSpec> {
        let variants = self.root.block("Fonts")?.block(name)?;
        let (_, v) = variants.blocks().find(|(_, v)| v.string("yres").is_none_or(|r| yres_matches(r, screen_height)))?;
        let mut spec = self.font(name, screen_height)?;
        if v.string("yres").is_none() && base > 0 {
            spec.tall = ((f64::from(screen_height) / f64::from(base) * f64::from(spec.tall)) as i32).min(128);
        }
        Some(spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_are_picked_by_screen_height() {
        let s = Scheme::parse(
            "Scheme { BaseSettings { \"UseScaledCreditsFont\" \"1\" } Fonts { \"EngineFont\" {\n\
             \"1\" { \"name\" \"Verdana\" \"tall\" \"13\" \"weight\" \"600\" \"yres\" \"480 599\" \"dropshadow\" \"1\" }\n\
             \"2\" { \"name\" \"Verdana\" \"tall\" \"15\" \"weight\" \"600\" \"yres\" \"600 767\" } } } }",
        );
        assert_eq!(s.base_setting("UseScaledCreditsFont"), Some("1"));
        let c = Scheme::parse("Scheme { Colors { \"BaseText\" \"255 176 0 255\" } BaseSettings { \"FgColor\" \"BaseText\" \"Bg\" \"0 0 0\" } }");
        assert_eq!(c.color("FgColor"), Some([255, 176, 0, 255]));
        assert_eq!(c.color("Bg"), Some([0, 0, 0, 255]));
        assert_eq!(c.color("BaseText"), Some([255, 176, 0, 255]));
        assert_eq!(c.color("Missing"), None);
        let f = s.font("EngineFont", 540);
        assert_eq!(f.as_ref().map(|f| (f.tall, f.dropshadow)), Some((13, true)));
        assert_eq!(s.font("enginefont", 720).map(|f| f.tall), Some(15));
        assert_eq!(s.font("EngineFont", 2000), None);
    }

    /// vgui2's language sizes: Russian cuts fonts above 12 to 90 % but never below 12, the
    /// CJK languages and Thai raise them to 13, symbol fonts keep theirs, nothing exceeds 128.
    #[test]
    fn the_language_resizes_scheme_fonts() {
        assert_eq!(language_tall(28, false, "russian"), 25);
        assert_eq!(language_tall(13, false, "Russian"), 12);
        assert_eq!(language_tall(12, false, "russian"), 12);
        assert_eq!(language_tall(14, true, "russian"), 14);
        assert_eq!(language_tall(10, false, "schinese"), 13);
        assert_eq!(language_tall(28, false, "english"), 28);
        assert_eq!(language_tall(200, false, "english"), 128);
    }
}
