//! The engine's 2D API for the client DLL (docs/re/client-hud.md §2.3–2.4): the HUD sprite
//! table and `SPR_*` draws, `FillRGBA`, VGUI2 scheme fonts with the console/credits text calls,
//! scissoring, and localization. Calls append [`DrawCmd`]s (screen pixels, origin top-left) to
//! [`Screen::list`]; glyphs live in a [`GlyphAtlas`] the renderer uploads.

// The calls mirror the engine's `cl_enginefunc_t` signatures (x, y, w, h, r, g, b, a).
#![allow(clippy::too_many_arguments)]

pub mod font;
pub mod localize;
pub mod scheme;
pub mod titles;

use std::collections::HashMap;
use std::sync::Arc;

use gs_formats::sprite::Sprite;

use crate::font::{Abc, Font, FontIndex};
use crate::localize::Localize;
use crate::scheme::Scheme;

/// HUD sprite slots (`cannot allocate more than %d HUD sprites`).
pub const MAX_HUD_SPRITES: usize = 256;

/// Font-table key prefix of client-scheme fonts.
const CLIENT_FONT: &str = "ClientScheme/";
/// Font-table key suffix of proportional fonts.
const PROPORTIONAL: &str = "#proportional";
/// Font-table key suffix of HD-proportional fonts.
const HD_PROPORTIONAL: &str = "#hdproportional";

/// Blend state of a quad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Blend {
    /// `SPR_Draw`: no blending.
    Opaque,
    /// `SPR_DrawHoles`: alpha test, then `(SRC_ALPHA, ONE_MINUS_SRC_ALPHA)`.
    Holes,
    /// `SPR_DrawAdditive`: `(ONE, ONE)`.
    Additive,
    /// `FillRGBA` and additive glyphs: `(SRC_ALPHA, ONE)`.
    AlphaAdditive,
    /// `FillRGBABlend` and normal glyphs: `(SRC_ALPHA, ONE_MINUS_SRC_ALPHA)`.
    Alpha,
    /// `R_DrawScreenFade`'s FFADE_MODULATE: `(ZERO, SRC_COLOR)`.
    Modulate,
    /// No blending with the alpha test (`NOTEQUAL 0`) on: VGUI textured rects after a glyph
    /// flush or a line disabled blending (docs/re/vgui-surface.md §4.4).
    OpaqueTested,
}

/// What a quad samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Texture {
    /// Flat colour.
    None,
    /// A frame of a sprite uploaded by the [`SpriteLoader`] as `texture`.
    Sprite { texture: u32, frame: u32 },
    /// The glyph atlas.
    Glyphs,
}

/// One screen quad.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawCmd {
    /// `x0, y0, x1, y1` in pixels.
    pub rect: [f32; 4],
    /// `u0, v0, u1, v1`.
    pub uv: [f32; 4],
    /// Modulating colour, 0..1.
    pub color: [f32; 4],
    pub texture: Texture,
    pub blend: Blend,
    /// `x, y, w, h` scissor in pixels (top-left origin).
    pub scissor: Option<[u32; 4]>,
}

/// Uploads HUD sprites and VGUI images for the renderer.
pub trait SpriteLoader {
    /// `(parsed sprite, texture id)`.
    fn load_sprite(&mut self, name: &str) -> Option<(Arc<Sprite>, u32)>;
    /// A VGUI2 image (`name` without extension): `.tga`, else `.bmp`.
    fn load_image(&mut self, name: &str) -> Option<Image>;
    /// `DrawSetTextureFile(…, forceReload)`: the file again, replacing `prev`'s texture.
    fn reload_image(&mut self, name: &str, prev: Option<Image>) -> Option<Image> {
        let _ = prev;
        self.load_image(name)
    }
}

/// An uploaded VGUI image: frame 0 of overlay texture `texture`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Image {
    pub texture: u32,
    pub width: u32,
    pub height: u32,
}

/// `wrect_t`: `left, right, top, bottom` in frame pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub right: i32,
    pub top: i32,
    pub bottom: i32,
}

/// A HUD sprite handle (1-based; 0 = invalid).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SprHandle(pub u32);

struct HudSprite {
    name: String,
    sprite: Option<(Arc<Sprite>, u32)>,
}

/// Fonts by scheme name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FontId(usize);

/// Glyph cells packed into one RGBA texture; cleared and refilled when full.
pub struct GlyphAtlas {
    pub size: u32,
    pub pixels: Vec<u8>,
    cursor: (u32, u32),
    row: u32,
    /// `x, y, w, h` changed since the renderer last uploaded.
    dirty: Option<[u32; 4]>,
    cells: HashMap<(FontId, char), Option<[u32; 4]>>,
}

impl GlyphAtlas {
    fn new(size: u32) -> Self {
        Self {
            size,
            pixels: vec![0; (size * size * 4) as usize],
            cursor: (0, 0),
            row: 0,
            dirty: None,
            cells: HashMap::new(),
        }
    }

    /// The changed region since the last call.
    pub fn take_dirty(&mut self) -> Option<[u32; 4]> {
        self.dirty.take()
    }

    fn mark(&mut self, r: [u32; 4]) {
        self.dirty = Some(match self.dirty {
            None => r,
            Some(d) => {
                let (x0, y0) = (d[0].min(r[0]), d[1].min(r[1]));
                let (x1, y1) = ((d[0] + d[2]).max(r[0] + r[2]), (d[1] + d[3]).max(r[1] + r[3]));
                [x0, y0, x1 - x0, y1 - y0]
            }
        });
    }

    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w > self.size || h > self.size {
            return None;
        }
        if self.cursor.0 + w > self.size {
            self.cursor = (0, self.cursor.1 + self.row + 1);
            self.row = 0;
        }
        if self.cursor.1 + h > self.size {
            return None;
        }
        let at = self.cursor;
        self.cursor.0 += w + 1;
        self.row = self.row.max(h);
        Some(at)
    }

    /// The atlas rect of `ch` in `font`, rasterizing it on first use.
    fn cell(&mut self, id: FontId, font: &Font, ch: char) -> Option<[u32; 4]> {
        if let Some(c) = self.cells.get(&(id, ch)) {
            return *c;
        }
        let rect = font.rasterize(ch).and_then(|cell| {
            let at = self.alloc(cell.width, cell.height).or_else(|| {
                log::debug!("glyph atlas full; clearing");
                self.cells.clear();
                self.cursor = (0, 0);
                self.row = 0;
                self.alloc(cell.width, cell.height)
            })?;
            for row in 0..cell.height {
                let src = (row * cell.width * 4) as usize..((row + 1) * cell.width * 4) as usize;
                let dst = (((at.1 + row) * self.size + at.0) * 4) as usize;
                self.pixels[dst..dst + src.len()].copy_from_slice(&cell.rgba[src]);
            }
            let r = [at.0, at.1, cell.width, cell.height];
            self.mark(r);
            Some(r)
        });
        self.cells.insert((id, ch), rect);
        rect
    }
}

/// `SCREENINFO` from `pfnGetScreenInfo`.
#[derive(Debug, Clone)]
pub struct ScreenInfo {
    pub width: u32,
    pub height: u32,
    /// CreditsFont tall.
    pub char_height: i32,
    /// CreditsFont ABC advances for 0..256.
    pub char_widths: [i32; 256],
}

/// The engine 2D state and this frame's draw list.
pub struct Screen {
    pub width: u32,
    pub height: u32,
    sprites: Vec<HudSprite>,
    /// VGUI images by lower-case name (`None` = failed to load).
    images: HashMap<String, Option<Image>>,
    /// `SPR_Set`: current sprite slot and colour.
    current: Option<usize>,
    color: [f32; 4],
    scissor: Option<[u32; 4]>,
    /// The global text colour (`pfnDrawSetTextColor`).
    text_color: [u8; 3],
    /// `con_color`, restored after every console string.
    pub con_color: [u8; 3],
    pub localize: Localize,
    scheme: Scheme,
    /// The client DLL's `resource/ClientScheme.res` (VGUI2 viewport panels).
    client_scheme: Scheme,
    /// Installed faces, scanned on the first font load.
    index: Option<FontIndex>,
    fonts: Vec<(String, Option<Font>)>,
    /// The surface language (`ISurface::GetLanguage`), which sizes the scheme fonts.
    language: String,
    pub atlas: GlyphAtlas,
    pub list: Vec<DrawCmd>,
}

impl Screen {
    pub fn new(scheme: Scheme, localize: Localize) -> Self {
        Self {
            width: 640,
            height: 480,
            sprites: Vec::new(),
            images: HashMap::new(),
            current: None,
            color: [1.0; 4],
            scissor: None,
            text_color: [255, 180, 30],
            con_color: [255, 180, 30],
            localize,
            scheme,
            client_scheme: Scheme::default(),
            index: None,
            fonts: Vec::new(),
            language: "english".to_owned(),
            atlas: GlyphAtlas::new(1024),
            list: Vec::new(),
        }
    }

    /// `VID` size change: scheme fonts are re-created for the new height.
    pub fn set_size(&mut self, width: u32, height: u32) {
        if (width, height) == (self.width, self.height) && !self.fonts.is_empty() {
            return;
        }
        self.width = width;
        self.height = height;
        self.reload_fonts();
    }

    /// `ISurface::SetLanguage`: the scheme fonts are re-created at the language's sizes.
    pub fn set_language(&mut self, language: &str) {
        if self.language != language {
            language.clone_into(&mut self.language);
            self.reload_fonts();
        }
    }

    /// Every font made so far again, under the same ids, with a fresh glyph atlas.
    fn reload_fonts(&mut self) {
        let names: Vec<String> = self.fonts.iter().map(|(n, _)| n.clone()).collect();
        self.fonts.clear();
        self.atlas = GlyphAtlas::new(self.atlas.size);
        for name in names {
            self.font(&name);
        }
    }

    /// Starts a frame: clears the draw list.
    pub fn begin_frame(&mut self) {
        self.list.clear();
    }

    /// The client DLL's scheme (`TeamFortressViewport::Initialize`).
    pub fn set_client_scheme(&mut self, scheme: Scheme) {
        self.client_scheme = scheme;
    }

    /// A font of the client scheme.
    pub fn client_font(&mut self, name: &str) -> FontId {
        self.font(&format!("{CLIENT_FONT}{name}"))
    }

    /// A client-scheme font as a proportional panel gets it (`GetFont(name, true)`).
    pub fn client_font_proportional(&mut self, name: &str) -> FontId {
        self.font(&format!("{CLIENT_FONT}{name}{PROPORTIONAL}"))
    }

    /// A colour of the client scheme (`IScheme::GetColor`).
    pub fn client_color(&self, name: &str) -> Option<[u8; 4]> {
        self.client_scheme.color(name)
    }

    /// A TrackerScheme font as `IScheme::GetFont(name, proportional, hd)` returns it: `None`
    /// (handle 0) for a name the scheme does not define.
    pub fn vgui_font(&mut self, name: &str, proportional: bool, hd: bool) -> Option<FontId> {
        let suffix = if hd {
            HD_PROPORTIONAL
        } else if proportional {
            PROPORTIONAL
        } else {
            ""
        };
        let full = format!("{name}{suffix}");
        self.font_spec(&full)?;
        Some(self.font(&full))
    }

    /// The scheme's definition of a font name (with the client and proportional prefixes), at
    /// the language's size.
    fn font_spec(&self, name: &str) -> Option<scheme::FontSpec> {
        let (scheme, rest) = match name.strip_prefix(CLIENT_FONT) {
            Some(client) => (&self.client_scheme, client),
            None => (&self.scheme, name),
        };
        let mut spec = if let Some(base) = rest.strip_suffix(HD_PROPORTIONAL) {
            scheme.font_hd_proportional(base, self.height)
        } else if let Some(base) = rest.strip_suffix(PROPORTIONAL) {
            scheme.font_proportional(base, self.height)
        } else {
            scheme.font(rest, self.height)
        }?;
        spec.tall = scheme::language_tall(spec.tall, spec.symbol, &self.language);
        Some(spec)
    }

    /// A scheme font by name (`ISchemeManager::GetFont`); loaded on first use.
    pub fn font(&mut self, name: &str) -> FontId {
        if let Some(i) = self.fonts.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)) {
            return FontId(i);
        }
        let font = self.load_font(name);
        self.fonts.push((name.to_owned(), font));
        FontId(self.fonts.len() - 1)
    }

    /// Creates every font a client panel can ask for by name, as the scheme manager does when
    /// it loads a scheme: the engine scheme's fonts and the client scheme's plain and
    /// proportional ones. Fonts made earlier stay, so a level finds them ready instead of a
    /// panel building one in the middle of a frame.
    pub fn preload_fonts(&mut self) {
        let mut names: Vec<String> = self.scheme.font_names().map(str::to_owned).collect();
        for name in self.client_scheme.font_names() {
            names.push(format!("{CLIENT_FONT}{name}"));
            names.push(format!("{CLIENT_FONT}{name}{PROPORTIONAL}"));
        }
        for name in &names {
            self.font(name);
        }
    }

    /// Scheme font `name` at the current height, on the face file its other sizes share.
    /// `SetFontGlyphSet`: an installed face draws U+0000..U+00FF and Tahoma at the same size,
    /// weight and flags the rest; Tahoma and Marlett draw everything themselves.
    fn load_font(&mut self, name: &str) -> Option<Font> {
        let spec = self.font_spec(name)?;
        let index = self.index.get_or_insert_with(FontIndex::scan);
        let face = index.face(&spec.family, spec.weight, spec.italic);
        if face.is_none() {
            log::warn!("font {name}: no installed face for {}", spec.family);
        }
        let own = ["Tahoma", "Marlett"].iter().any(|f| spec.family.eq_ignore_ascii_case(f));
        let wide = if own { None } else { index.face("Tahoma", spec.weight, spec.italic) };
        Some(Font::new(face?, wide, &spec))
    }

    fn font_ref(&self, id: FontId) -> Option<&Font> {
        self.fonts.get(id.0).and_then(|(_, f)| f.as_ref())
    }

    /// `pfnGetScreenInfo`.
    pub fn screen_info(&mut self) -> ScreenInfo {
        let credits = self.font("CreditsFont");
        let mut widths = [0; 256];
        let mut char_height = 0;
        if let Some(f) = self.font_ref(credits) {
            char_height = f.tall;
            for (i, w) in widths.iter_mut().enumerate() {
                *w = f.abc(char::from(i as u8)).advance();
            }
        }
        ScreenInfo { width: self.width, height: self.height, char_height, char_widths: widths }
    }

    // ------------------------------------------------------------------ sprites

    /// `SPR_Load`: the slot named `name` (case-insensitive) or the first free one; loaded via
    /// `loader`. Handle 0 when the sprite failed to load.
    pub fn spr_load(&mut self, name: &str, loader: &mut dyn SpriteLoader) -> SprHandle {
        let slot = match self.sprites.iter().position(|s| s.name.eq_ignore_ascii_case(name)) {
            Some(i) => i,
            None => {
                if self.sprites.len() >= MAX_HUD_SPRITES {
                    log::error!("cannot allocate more than {MAX_HUD_SPRITES} HUD sprites");
                    return SprHandle(0);
                }
                self.sprites.push(HudSprite { name: name.to_owned(), sprite: None });
                self.sprites.len() - 1
            }
        };
        if self.sprites[slot].sprite.is_none() {
            self.sprites[slot].sprite = loader.load_sprite(name);
        }
        if self.sprites[slot].sprite.is_some() { SprHandle(slot as u32 + 1) } else { SprHandle(0) }
    }

    fn sprite(&self, h: SprHandle) -> Option<&(Arc<Sprite>, u32)> {
        self.sprites.get((h.0 as usize).checked_sub(1)?)?.sprite.as_ref()
    }

    /// `SPR_Frames`.
    pub fn spr_frames(&self, h: SprHandle) -> i32 {
        self.sprite(h).map_or(0, |(s, _)| s.frames.len() as i32)
    }

    /// `SPR_Width` / `SPR_Height` of a frame (frame 0 when out of range; 0 for group frames).
    pub fn spr_size(&self, h: SprHandle, frame: i32) -> (i32, i32) {
        self.sprite(h).and_then(|(s, _)| s.frame(frame as f32)).map_or((0, 0), |f| (f.width as i32, f.height as i32))
    }

    /// `SPR_Set`: a bad handle leaves the current sprite and colour unchanged.
    pub fn spr_set(&mut self, h: SprHandle, r: i32, g: i32, b: i32) {
        if self.sprite(h).is_none() {
            return;
        }
        self.current = Some(h.0 as usize - 1);
        self.color = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0];
    }

    /// `SPR_Draw`.
    pub fn spr_draw(&mut self, frame: i32, x: i32, y: i32, rect: Option<Rect>) {
        self.sprite_frame(frame, x, y, rect, Blend::Opaque);
    }

    /// `SPR_DrawHoles`.
    pub fn spr_draw_holes(&mut self, frame: i32, x: i32, y: i32, rect: Option<Rect>) {
        self.sprite_frame(frame, x, y, rect, Blend::Holes);
    }

    /// `SPR_DrawAdditive`.
    pub fn spr_draw_additive(&mut self, frame: i32, x: i32, y: i32, rect: Option<Rect>) {
        self.sprite_frame(frame, x, y, rect, Blend::Additive);
    }

    /// `Draw_SpriteFrame`: the frame (or `rect` of it, half-texel inset) at `(x+0.5, y+0.5)`.
    fn sprite_frame(&mut self, frame: i32, x: i32, y: i32, rect: Option<Rect>, blend: Blend) {
        let Some(slot) = self.current else { return };
        if x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let Some((sprite, texture)) = self.sprites.get(slot).and_then(|s| s.sprite.as_ref()) else { return };
        let frame = if frame < 0 || frame as usize >= sprite.frames.len() {
            log::debug!("SPR_Draw: invalid frame");
            0
        } else {
            frame
        };
        let Some(f) = sprite.frame(frame as f32) else { return };
        let (w, h) = (f.width as f32, f.height as f32);
        let (mut size, mut uv) = ((w, h), [0.0, 0.0, 1.0, 1.0]);
        if let Some(r) = rect.filter(|r| r.left < r.right && r.top < r.bottom) {
            let l = r.left.clamp(0, f.width as i32) as f32;
            let rt = r.right.clamp(0, f.width as i32) as f32;
            let t = r.top.clamp(0, f.height as i32) as f32;
            let b = r.bottom.clamp(0, f.height as i32) as f32;
            if l < rt && t < b {
                size = (rt - l, b - t);
                uv = [(l + 0.5) / w, (t + 0.5) / h, (rt - 0.5) / w, (b - 0.5) / h];
            }
        }
        let (x0, y0) = (x as f32 + 0.5, y as f32 + 0.5);
        self.list.push(DrawCmd {
            rect: [x0, y0, x0 + size.0, y0 + size.1],
            uv,
            color: self.color,
            texture: Texture::Sprite { texture: *texture, frame: frame as u32 },
            blend,
            scissor: self.scissor,
        });
    }

    /// `SPR_EnableScissor`.
    pub fn enable_scissor(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let (sw, sh) = (self.width as i32, self.height as i32);
        let x = x.clamp(0, sw);
        let y = y.clamp(0, sh);
        let w = w.clamp(0, sw - x);
        let h = h.clamp(0, sh - y);
        self.scissor = Some([x as u32, y as u32, w as u32, h as u32]);
    }

    /// `SPR_DisableScissor`.
    pub fn disable_scissor(&mut self) {
        self.scissor = None;
    }

    // ------------------------------------------------------------------ fills

    fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, rgba: [i32; 4], blend: Blend) {
        let c = rgba.map(|v| v as f32 / 255.0);
        self.list.push(DrawCmd {
            rect: [x as f32, y as f32, (x + w) as f32, (y + h) as f32],
            uv: [0.0; 4],
            color: c,
            texture: Texture::None,
            blend,
            scissor: None,
        });
    }

    /// `pfnFillRGBA`: additive scaled by alpha.
    pub fn fill_rgba(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, g: i32, b: i32, a: i32) {
        self.fill(x, y, w, h, [r, g, b, a], Blend::AlphaAdditive);
    }

    /// `pfnFillRGBABlend`: normal alpha blend.
    pub fn fill_rgba_blend(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, g: i32, b: i32, a: i32) {
        self.fill(x, y, w, h, [r, g, b, a], Blend::Alpha);
    }

    /// `R_DrawScreenFade`: a full-window quad, alpha-blended or modulating (`(ZERO, SRC_COLOR)`).
    pub fn screen_fade(&mut self, rgba: [u8; 4], modulate: bool) {
        let (w, h) = (self.width as i32, self.height as i32);
        let blend = if modulate { Blend::Modulate } else { Blend::Alpha };
        self.fill(0, 0, w, h, rgba.map(i32::from), blend);
    }

    // ------------------------------------------------------------------ images

    /// `ISurface::DrawSetTextureFile`: a VGUI image, loaded once and cached (also misses).
    pub fn load_image(&mut self, name: &str, loader: &mut dyn SpriteLoader) -> Option<Image> {
        let key = name.to_ascii_lowercase();
        if let Some(image) = self.images.get(&key) {
            return *image;
        }
        let image = loader.load_image(name);
        self.images.insert(key, image);
        image
    }

    /// `DrawSetTextureFile` with `forceReload`: the file is read and uploaded again.
    pub fn reload_image(&mut self, name: &str, loader: &mut dyn SpriteLoader) -> Option<Image> {
        let key = name.to_ascii_lowercase();
        let prev = self.images.get(&key).copied().flatten();
        let image = loader.reload_image(name, prev);
        self.images.insert(key, image);
        image
    }

    /// `ISurface::DrawTexturedRect`: the whole image stretched over the rect, modulated by
    /// `color`.
    pub fn draw_image(&mut self, image: Image, x: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
        self.draw_image_uv(image, x, y, w, h, [0.0, 0.0, 1.0, 1.0], color);
    }

    /// `ISurface::DrawTexturedSubRect`: texture coordinates `u0, v0, u1, v1` over the rect (a
    /// reversed pair mirrors the image).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_image_uv(&mut self, image: Image, x: i32, y: i32, w: i32, h: i32, uv: [f32; 4], color: [u8; 4]) {
        self.list.push(DrawCmd {
            rect: [x as f32, y as f32, (x + w) as f32, (y + h) as f32],
            uv,
            color: color.map(|c| f32::from(c) / 255.0),
            texture: Texture::Sprite { texture: image.texture, frame: 0 },
            blend: Blend::Alpha,
            scissor: self.scissor,
        });
    }

    /// `Draw_Pic`: a wad pic at integer coordinates, alpha-tested and unblended (`GL_REPLACE`).
    pub fn draw_pic(&mut self, image: Image, x: i32, y: i32, w: i32, h: i32) {
        self.list.push(DrawCmd {
            rect: [x as f32, y as f32, (x + w) as f32, (y + h) as f32],
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0; 4],
            texture: Texture::Sprite { texture: image.texture, frame: 0 },
            blend: Blend::Holes,
            scissor: None,
        });
    }

    // ------------------------------------------------------------------ text

    /// Text after `#` localization; non-printable characters become spaces.
    fn text<'a>(&'a self, s: &'a str) -> String {
        self.localize.text(s).chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
    }

    /// `GetCharABCwide` a + b + c of `ch` in `font` (0 for an unloaded font).
    pub fn char_width(&self, font: FontId, ch: char) -> i32 {
        self.font_ref(font).map_or(0, |f| f.abc(ch).advance())
    }

    /// One glyph of `font` at `(x, y)` (cell top-left); returns its ABC advance.
    fn glyph(&mut self, id: FontId, x: i32, y: i32, ch: char, color: [u8; 3], blend: Blend) -> i32 {
        self.glyph_alpha(id, x, y, ch, [color[0], color[1], color[2], 255], blend)
    }

    fn glyph_alpha(&mut self, id: FontId, x: i32, y: i32, ch: char, color: [u8; 4], blend: Blend) -> i32 {
        let Some(font) = self.fonts.get(id.0).and_then(|(_, f)| f.as_ref()) else { return 0 };
        let abc: Abc = font.abc(ch);
        if let Some(cell) = self.atlas.cell(id, font, ch) {
            let s = self.atlas.size as f32;
            let (x0, y0) = ((x + abc.a) as f32, y as f32);
            self.list.push(DrawCmd {
                rect: [x0, y0, x0 + cell[2] as f32, y0 + cell[3] as f32],
                uv: [cell[0] as f32 / s, cell[1] as f32 / s, (cell[0] + cell[2]) as f32 / s, (cell[1] + cell[3]) as f32 / s],
                color: color.map(|c| f32::from(c) / 255.0),
                texture: Texture::Glyphs,
                blend,
                scissor: None,
            });
        }
        abc.advance()
    }

    /// ABC widths of `ch` (`GetCharABCwide`).
    pub fn char_abc(&self, font: FontId, ch: char) -> (i32, i32, i32) {
        self.font_ref(font).map_or((0, 0, 0), |f| {
            let abc = f.abc(ch);
            (abc.a, abc.b, abc.c)
        })
    }

    /// A VGUI flat rect `[x0,x1)×[y0,y1)` (absolute), clipped like `ClipRect`.
    pub fn vgui_fill(&mut self, rect: [i32; 4], color: [u8; 4], blend: Blend, clip: Option<[i32; 4]>) {
        let Some((r, _)) = clip_rect(rect, [0.0; 4], clip) else { return };
        self.list.push(DrawCmd {
            rect: r.map(|v| v as f32),
            uv: [0.0; 4],
            color: color.map(|c| f32::from(c) / 255.0),
            texture: Texture::None,
            blend,
            scissor: None,
        });
    }

    /// A VGUI textured rect (absolute), clipped with re-interpolated texture coordinates.
    pub fn vgui_image(&mut self, image: Image, rect: [i32; 4], uv: [f32; 4], color: [u8; 4], blend: Blend, clip: Option<[i32; 4]>) {
        let Some((r, uv)) = clip_rect(rect, uv, clip) else { return };
        self.list.push(DrawCmd {
            rect: r.map(|v| v as f32),
            uv,
            color: color.map(|c| f32::from(c) / 255.0),
            texture: Texture::Sprite { texture: image.texture, frame: 0 },
            blend,
            scissor: None,
        });
    }

    /// A VGUI glyph quad at pen `(x, y)` (absolute): the cell at `x + a`, clipped; `None`
    /// when the font has no glyph for `ch` (the pen does not advance).
    #[allow(clippy::too_many_arguments)]
    pub fn vgui_glyph(&mut self, id: FontId, x: i32, y: i32, ch: char, color: [u8; 4], blend: Blend, clip: Option<[i32; 4]>) -> Option<()> {
        let font = self.fonts.get(id.0).and_then(|(_, f)| f.as_ref())?;
        let abc: Abc = font.abc(ch);
        let cell = self.atlas.cell(id, font, ch)?;
        let s = self.atlas.size as f32;
        let x0 = x + abc.a;
        let rect = [x0, y, x0 + cell[2] as i32, y + cell[3] as i32];
        let uv = [cell[0] as f32 / s, cell[1] as f32 / s, (cell[0] + cell[2]) as f32 / s, (cell[1] + cell[3]) as f32 / s];
        let (r, uv) = clip_rect(rect, uv, clip)?;
        self.list.push(DrawCmd {
            rect: r.map(|v| v as f32),
            uv,
            color: color.map(|c| f32::from(c) / 255.0),
            texture: Texture::Glyphs,
            blend,
            scissor: None,
        });
        Some(())
    }

    /// Width of `text` in `font`.
    pub fn text_width(&mut self, font: FontId, text: &str) -> i32 {
        let text = self.text(text);
        self.font_ref(font).map_or(0, |f| f.text_width(&text))
    }

    pub fn font_tall(&self, font: FontId) -> i32 {
        self.font_ref(font).map_or(0, |f| f.tall)
    }

    /// `pfnDrawSetTextColor` (0..1 floats).
    pub fn set_text_color(&mut self, r: f32, g: f32, b: f32) {
        self.text_color = [r, g, b].map(|c| (c * 255.0) as u8);
    }

    /// `pfnDrawConsoleString`: EngineFont in the text colour; returns `x + width`, then the
    /// colour reverts to `con_color`.
    pub fn draw_console_string(&mut self, x: i32, y: i32, s: &str) -> i32 {
        let font = self.font("EngineFont");
        let mut cx = x;
        let color = self.text_color;
        for ch in self.text(s).chars() {
            cx += self.glyph(font, cx, y, ch, color, Blend::Alpha);
        }
        self.text_color = self.con_color;
        cx
    }

    /// `pfnDrawConsoleStringLen`: `(width, EngineFont tall)`.
    pub fn draw_console_string_len(&mut self, s: &str) -> (i32, i32) {
        let font = self.font("EngineFont");
        (self.text_width(font, s), self.font_tall(font))
    }

    /// `pfnDrawString`: additive CreditsFont glyphs; returns the total advance.
    pub fn draw_string(&mut self, x: i32, y: i32, s: &str, r: i32, g: i32, b: i32) -> i32 {
        let font = self.credits_font();
        let color = [r, g, b].map(|c| c.clamp(0, 255) as u8);
        let mut cx = x;
        for ch in self.text(s).chars() {
            cx += self.glyph(font, cx, y, ch, color, Blend::AlphaAdditive);
        }
        cx - x
    }

    /// `pfnDrawStringReverse`: right-aligned so the text ends at `x`.
    pub fn draw_string_reverse(&mut self, x: i32, y: i32, s: &str, r: i32, g: i32, b: i32) -> i32 {
        let font = self.credits_font();
        let width = self.text_width(font, s);
        self.draw_string(x - width, y, s, r, g, b)
    }

    /// `pfnDrawCharacter`: 0 for black; else an additive CreditsFont glyph.
    pub fn draw_character(&mut self, x: i32, y: i32, ch: char, r: i32, g: i32, b: i32) -> i32 {
        if r == 0 && g == 0 && b == 0 {
            return 0;
        }
        let font = self.credits_font();
        self.vgui2_draw_character_add(x, y, ch, r, g, b, font)
    }

    /// `pfnVGUI2DrawCharacter`: a glyph of `font` in the text colour.
    pub fn vgui2_draw_character(&mut self, x: i32, y: i32, ch: char, font: FontId) -> i32 {
        let color = self.text_color;
        self.glyph(font, x, y, ch, color, Blend::Alpha)
    }

    /// `pfnVGUI2DrawCharacterAdd`: sets the text colour and draws an additive glyph.
    pub fn vgui2_draw_character_add(&mut self, x: i32, y: i32, ch: char, r: i32, g: i32, b: i32, font: FontId) -> i32 {
        self.text_color = [r, g, b].map(|c| c.clamp(0, 255) as u8);
        let color = self.text_color;
        self.glyph(font, x, y, ch, color, Blend::AlphaAdditive)
    }

    /// `ISurface::DrawSetTextColor(r, g, b, a)` + `DrawUnicodeChar`: an alpha-blended glyph.
    pub fn draw_char_rgba(&mut self, x: i32, y: i32, ch: char, font: FontId, color: [u8; 4]) -> i32 {
        self.glyph_alpha(font, x, y, ch, color, Blend::Alpha)
    }

    /// The engine's CreditsFont (`Legacy_CreditsFont` when the scheme disables scaling).
    pub fn credits_font(&mut self) -> FontId {
        let scaled = self.scheme.base_setting("UseScaledCreditsFont").is_none_or(|v| v.trim() != "0");
        self.font(if scaled { "CreditsFont" } else { "Legacy_CreditsFont" })
    }
}

/// The engine surface's `ClipRect`: `IntersectRect` with the clip (empty when either rect is
/// empty), texture coordinates re-interpolated in float32; no clip passes the rect through.
pub fn clip_rect(rect: [i32; 4], uv: [f32; 4], clip: Option<[i32; 4]>) -> Option<([i32; 4], [f32; 4])> {
    let Some(c) = clip else { return Some((rect, uv)) };
    let [x0, y0, x1, y1] = rect;
    if x1 <= x0 || y1 <= y0 || c[2] <= c[0] || c[3] <= c[1] {
        return None;
    }
    let out = [x0.max(c[0]), y0.max(c[1]), x1.min(c[2]), y1.min(c[3])];
    if out[2] <= out[0] || out[3] <= out[1] {
        return None;
    }
    let d = |a: i32, b: i32| (f64::from(a) - f64::from(b)) as f32;
    let (w, h) = (d(x1, x0), d(y1, y0));
    let [s0, t0, s1, t1] = uv;
    let st = [
        d(out[0], x0) / w * (s1 - s0) + s0,
        d(out[1], y0) / h * (t1 - t0) + t0,
        d(out[2], x0) / w * (s1 - s0) + s0,
        d(out[3], y0) / h * (t1 - t0) + t0,
    ];
    Some((out, st))
}

#[cfg(test)]
mod tests;
