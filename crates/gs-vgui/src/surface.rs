//! The engine's `ISurface` as vgui2 and the controls call it (docs/re/vgui-surface.md §2). Drawing
//! happens between [`Surface::push_make_current`] and [`Surface::pop_make_current`], in the
//! panel's own coordinates; the engine applies the translation, the software clip and its GL
//! state rules (text batching, blending turned off by text/line draws).

/// RGBA, 0..255.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Color(pub [u8; 4]);

impl Color {
    pub const WHITE: Color = Color([255, 255, 255, 255]);
    pub const TRANSPARENT: Color = Color([0, 0, 0, 0]);

    pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self([r, g, b, a])
    }

    pub fn a(self) -> u8 {
        self.0[3]
    }
}

/// `HFont`: an index into the surface's font table; 0 = no font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FontHandle(pub u32);

/// `vgui2::CursorCode` (dc_*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cursor {
    /// dc_user (0): such a panel gets no mouse input (docs/re/vgui-controls.md §1.8).
    User,
    None,
    #[default]
    Arrow,
    IBeam,
    Hourglass,
    Crosshair,
    Up,
    SizeNwSe,
    SizeNeSw,
    SizeWe,
    SizeNs,
    SizeAll,
    No,
    Hand,
}

/// A texture the surface loaded (`DrawSetTextureFile`): its id and size in texels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Texture {
    pub id: u32,
    pub width: i32,
    pub height: i32,
}

/// ABC widths of a glyph (`GetCharABCwide`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Abc {
    pub a: i32,
    pub b: i32,
    pub c: i32,
}

impl Abc {
    pub fn advance(self) -> i32 {
        self.a + self.b + self.c
    }
}

/// The engine surface.
pub trait Surface {
    /// `GetScreenSize`.
    fn screen_size(&self) -> (i32, i32);
    /// `GetProportionalBase`: (640, 480) unless a scheme overrides it.
    fn proportional_base(&self) -> (i32, i32) {
        (640, 480)
    }

    /// `PushMakeCurrent`: drawing origin (absolute) and clip rect (absolute `x0 y0 x1 y1`).
    fn push_make_current(&mut self, origin: (i32, i32), clip: [i32; 4]);
    /// `PopMakeCurrent`: ends a panel's paint step (flushes batched glyphs).
    fn pop_make_current(&mut self);

    fn draw_set_color(&mut self, c: Color);
    fn draw_filled_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32);
    fn draw_outlined_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32);
    /// `DrawLine`: drawn with the text colour, additive, unclipped (vgui-surface.md).
    fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32);

    fn draw_set_text_font(&mut self, font: FontHandle);
    fn draw_set_text_color(&mut self, c: Color);
    fn draw_set_text_pos(&mut self, x: i32, y: i32);
    fn draw_get_text_pos(&self) -> (i32, i32);
    /// `DrawUnicodeChar`: draws at the text position and advances it.
    fn draw_unicode_char(&mut self, ch: char);
    /// `DrawPrintText`.
    fn draw_print_text(&mut self, text: &str) {
        for ch in text.chars() {
            self.draw_unicode_char(ch);
        }
    }

    /// `DrawSetTextureFile` + size, cached by name; `None` when the file does not load.
    fn texture(&mut self, name: &str, hardware_filtered: bool) -> Option<Texture>;
    /// `DrawSetTextureFile(…, forceReload = 1)`: the file is loaded again (same handle).
    fn reload_texture(&mut self, name: &str, hardware_filtered: bool) -> Option<Texture> {
        self.texture(name, hardware_filtered)
    }
    /// `DrawSetTexture` + `DrawTexturedRect` with the current draw colour.
    fn draw_textured_rect(&mut self, tex: Texture, x0: i32, y0: i32, x1: i32, y1: i32);

    /// A scheme font (`IScheme::GetFont(name, proportional, hd)`) of `scheme`.
    fn font(&mut self, scheme: &str, name: &str, proportional: bool, hd: bool) -> FontHandle;
    fn font_tall(&self, font: FontHandle) -> i32;
    fn char_abc(&self, font: FontHandle, ch: char) -> Abc;
    /// `GetCharacterWidth`.
    fn char_width(&self, font: FontHandle, ch: char) -> i32 {
        self.char_abc(font, ch).advance()
    }

    /// `PlaySound` (a path under `sound/`).
    fn play_sound(&mut self, _name: &str) {}
    /// The engine's `pfnFillRGBA` (`additive`) / `pfnFillRGBABlend` called inside a vgui paint:
    /// the panel translation applies; GL blending is off afterwards.
    fn engine_fill_rgba(&mut self, _x: i32, _y: i32, _w: i32, _h: i32, _c: Color, _additive: bool) {}
    /// `SetCursor` for the panel under the mouse.
    fn set_cursor(&mut self, _cursor: Cursor) {}
    /// `ISystem::GetClipboardText` (the whole clipboard text).
    fn clipboard_text(&mut self) -> String;
    /// `ISystem::SetClipboardText`.
    fn set_clipboard_text(&mut self, text: &str);
    /// `HasFocus`: the game window has focus.
    fn has_focus(&self) -> bool {
        true
    }
}
