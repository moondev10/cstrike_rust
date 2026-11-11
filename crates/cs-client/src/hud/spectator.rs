//! `CHudSpectator`'s overview data (docs/re/client-hud.md §8.4): the map's
//! `overviews/<map>.txt` and the PIP inset rectangle computed with it.

use gs_common::token::com_parse;

/// `CHudSpectator`'s overview data (`overviews/<map>.txt`).
#[derive(Debug, Clone, PartialEq)]
pub struct Overview {
    pub zoom: f32,
    pub origin: [f32; 3],
    pub rotated: bool,
    /// `insetWindowX/Y/Width/Height`: `(4, 4, 240r, 180r)` or the file's `inset` times `r`,
    /// `r = width/640 + 0.5` at parse time.
    pub window: [i32; 4],
    /// The first layer's image and height.
    pub layer: Option<(String, f32)>,
}

impl Default for Overview {
    fn default() -> Self {
        Self::new(640)
    }
}

impl Overview {
    /// The defaults `ParseOverviewFile` resets to for a `width`-pixel screen.
    pub fn new(width: u32) -> Self {
        let r = width as f32 / 640.0 + 0.5;
        Self { zoom: 1.0, origin: [0.0; 3], rotated: false, window: [4, 4, (240.0 * r) as i32, (180.0 * r) as i32], layer: None }
    }

    /// `ParseOverviewFile`: `global { zoom origin rotated inset }` and `layer { image height }`;
    /// an unknown key stops the parse with what was read so far.
    pub fn parse(text: &[u8], width: u32) -> Self {
        let mut out = Self::new(width);
        let r = width as f32 / 640.0 + 0.5;
        let mut rest = text;
        let mut next = || -> Option<String> {
            let (tok, r) = com_parse(rest, false)?;
            rest = r;
            Some(String::from_utf8_lossy(&tok).into_owned())
        };
        let num = |t: Option<String>| t.and_then(|t| t.trim().parse::<f32>().ok()).unwrap_or(0.0);
        while let Some(block) = next() {
            let global = block.eq_ignore_ascii_case("global");
            if !global && !block.eq_ignore_ascii_case("layer") {
                continue;
            }
            if next().as_deref() != Some("{") {
                log::warn!("Error parsing overview file (expected {{ )");
                return out;
            }
            let mut image = String::new();
            let mut height = 0.0;
            loop {
                let Some(key) = next() else { return out };
                if key == "}" {
                    break;
                }
                match (global, key.to_ascii_lowercase().as_str()) {
                    (true, "zoom") => out.zoom = num(next()),
                    (true, "origin") => out.origin = [num(next()), num(next()), num(next())],
                    (true, "rotated") => out.rotated = num(next()) != 0.0,
                    (true, "inset") => out.window = [num(next()), num(next()), num(next()), num(next())].map(|v| (v * r) as i32),
                    (false, "image") => image = next().unwrap_or_default(),
                    (false, "height") => height = num(next()),
                    _ => {
                        log::warn!("Error parsing overview file ({key} unknown)");
                        return out;
                    }
                }
            }
            if !global && out.layer.is_none() {
                out.layer = Some((image, height));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_files_set_zoom_origin_the_first_layer_and_the_scaled_inset() {
        let o = Overview::parse(b"// de_x\nglobal\n{\n\tZOOM\t1.10\n\tORIGIN\t-131.00 1146.00 -3000.00\n\tROTATED\t1\n}\nlayer\n{\n\tIMAGE\t\"overviews/de_x.bmp\"\n\tHEIGHT\t-3000.00\n}\n", 1280);
        assert_eq!((o.zoom, o.origin, o.rotated), (1.1, [-131.0, 1146.0, -3000.0], true));
        assert_eq!(o.layer, Some(("overviews/de_x.bmp".to_owned(), -3000.0)));
        assert_eq!(o.window, [4, 4, 600, 450], "the default inset at 1280 wide");
        assert_eq!(Overview::new(800).window, [4, 4, 420, 315]);
        assert_eq!(Overview::parse(b"global { inset 10 20 100 50 }", 640).window, [15, 30, 150, 75], "file values times 1.5");
        assert_eq!(Overview::parse(b"global { zoom 2 bogus 1 origin 1 2 3 }", 640).zoom, 2.0, "an unknown key stops the parse");
    }
}
