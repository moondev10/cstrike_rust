//! The engine side of GameUI: the vgui2 surface over the 2D screen (docs/re/vgui-surface.md
//! §4, §8.8), the SDL → vgui2 input translation (docs/re/gameui-engine.md §9.2, §9.3) and the
//! file loading of `CBaseUI::Start`.

use std::collections::HashMap;
use std::time::Instant;

use gs_draw::{Blend, FontId, Image, Screen, SpriteLoader};
use gs_fs::FileSystem;
use gs_vgui::keys::*;
use gs_vgui::surface::{Abc, Texture};
use gs_vgui::{Color, Cursor, FontHandle, Surface, Ui};
use winit::keyboard::{Key, KeyCode, KeyLocation, NamedKey};

/// The surface's persistent side: textures by name, font handles, the clipboard, the cursor
/// and UI sounds to play.
#[derive(Default)]
pub struct SurfaceState {
    textures: HashMap<String, Option<Texture>>,
    images: Vec<Image>,
    fonts: Vec<FontId>,
    font_keys: HashMap<(String, bool, bool), FontHandle>,
    clipboard: Option<arboard::Clipboard>,
    pub cursor: Cursor,
    pub sounds: Vec<String>,
}

/// One frame's `ISurface` over the 2D screen.
pub struct VguiSurface<'a> {
    pub st: &'a mut SurfaceState,
    pub screen: &'a mut Screen,
    pub loader: &'a mut dyn SpriteLoader,
    origin: (i32, i32),
    /// The software clip (absolute); `None` outside a Push/Pop pair.
    clip: Option<[i32; 4]>,
    /// `GL_BLEND` as the primitives leave it.
    blend: bool,
    draw_color: Color,
    text_color: Color,
    text_pos: (i32, i32),
    text_font: FontHandle,
    /// The deferred glyph batch (absolute pen positions).
    pending: Vec<(FontId, i32, i32, char)>,
}

impl<'a> VguiSurface<'a> {
    pub fn new(st: &'a mut SurfaceState, screen: &'a mut Screen, loader: &'a mut dyn SpriteLoader) -> Self {
        Self { st, screen, loader, origin: (0, 0), clip: None, blend: false, draw_color: Color::TRANSPARENT, text_color: Color::TRANSPARENT, text_pos: (0, 0), text_font: FontHandle(0), pending: Vec::new() }
    }

    fn font_id(&self, f: FontHandle) -> Option<FontId> {
        usize::try_from(f.0).ok().and_then(|i| i.checked_sub(1)).and_then(|i| self.st.fonts.get(i).copied())
    }

    /// `DrawFlushText`: the batch in the current text colour, alpha-blended; blending is off
    /// afterwards.
    fn flush_text(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let color = self.text_color.0;
        for (font, x, y, ch) in std::mem::take(&mut self.pending) {
            self.screen.vgui_glyph(font, x, y, ch, color, Blend::Holes, self.clip);
        }
        self.blend = false;
    }

    /// Panel coordinates to absolute ones (C `int` arithmetic: an `INT_MIN` slider nob wraps).
    fn abs(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> [i32; 4] {
        let (ox, oy) = self.origin;
        [x0.wrapping_add(ox), y0.wrapping_add(oy), x1.wrapping_add(ox), y1.wrapping_add(oy)]
    }
}

impl Surface for VguiSurface<'_> {
    fn screen_size(&self) -> (i32, i32) {
        (self.screen.width as i32, self.screen.height as i32)
    }
    fn push_make_current(&mut self, origin: (i32, i32), clip: [i32; 4]) {
        self.origin = origin;
        self.clip = Some(clip);
        self.blend = true;
    }
    fn pop_make_current(&mut self) {
        self.flush_text();
        self.clip = None;
        self.origin = (0, 0);
    }
    fn draw_set_color(&mut self, c: Color) {
        self.draw_color = c;
    }
    fn draw_filled_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        if self.draw_color.a() == 0 {
            return;
        }
        let blend = if self.blend { Blend::Alpha } else { Blend::Opaque };
        let r = self.abs(x0, y0, x1, y1);
        self.screen.vgui_fill(r, self.draw_color.0, blend, self.clip);
    }
    fn draw_outlined_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        if self.draw_color.a() == 0 {
            return;
        }
        self.draw_filled_rect(x0, y0, x1, y0 + 1);
        self.draw_filled_rect(x0, y1 - 1, x1, y1);
        self.draw_filled_rect(x0, y0 + 1, x0 + 1, y1 - 1);
        self.draw_filled_rect(x1 - 1, y0 + 1, x1, y1 - 1);
    }
    /// `DrawLine`: text colour, additive, unclipped; blending is off afterwards.
    fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let [ax, ay, bx, by] = self.abs(x0, y0, x1, y1);
        let steps = (bx - ax).abs().max((by - ay).abs()).max(1);
        for i in 0..steps {
            let x = ax + (bx - ax) * i / steps;
            let y = ay + (by - ay) * i / steps;
            self.screen.vgui_fill([x, y, x + 1, y + 1], self.text_color.0, Blend::AlphaAdditive, None);
        }
        self.blend = false;
    }
    /// `pfnFillRGBA` / `pfnFillRGBABlend` from a vgui paint: translated like vgui drawing,
    /// blending off afterwards.
    fn engine_fill_rgba(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color, additive: bool) {
        let r = self.abs(x, y, x + w, y + h);
        self.screen.vgui_fill(r, c.0, if additive { Blend::AlphaAdditive } else { Blend::Alpha }, self.clip);
        self.blend = false;
    }
    fn draw_set_text_font(&mut self, font: FontHandle) {
        self.text_font = font;
    }
    /// `DrawSetTextColor`: a change flushes the batch first.
    fn draw_set_text_color(&mut self, c: Color) {
        if c != self.text_color {
            self.flush_text();
        }
        self.text_color = c;
    }
    fn draw_set_text_pos(&mut self, x: i32, y: i32) {
        self.text_pos = (x, y);
    }
    fn draw_get_text_pos(&self) -> (i32, i32) {
        self.text_pos
    }
    /// `DrawUnicodeChar`: a batched glyph at `x + a`; the pen moves `a + b + c` (not at all when
    /// the font has no glyph).
    fn draw_unicode_char(&mut self, ch: char) {
        let Some(font) = self.font_id(self.text_font) else { return };
        let (a, b, c) = self.screen.char_abc(font, ch);
        let (x, y) = self.text_pos;
        if self.text_color.a() != 0 {
            if self.pending.len() >= 64 {
                self.flush_text();
            }
            let (ox, oy) = self.origin;
            self.pending.push((font, x + ox, y + oy, ch));
        }
        self.text_pos = (x + a + b + c, y);
    }
    fn texture(&mut self, name: &str, _hardware_filtered: bool) -> Option<Texture> {
        let key = name.to_ascii_lowercase();
        if let Some(t) = self.st.textures.get(&key) {
            return *t;
        }
        let tex = self.screen.load_image(name, self.loader).map(|img| {
            self.st.images.push(img);
            Texture { id: self.st.images.len() as u32 - 1, width: img.width as i32, height: img.height as i32 }
        });
        self.st.textures.insert(key, tex);
        tex
    }
    fn reload_texture(&mut self, name: &str, hardware_filtered: bool) -> Option<Texture> {
        let key = name.to_ascii_lowercase();
        let Some(Some(old)) = self.st.textures.get(&key).copied() else { return self.texture(name, hardware_filtered) };
        let img = self.screen.reload_image(name, self.loader)?;
        if let Some(slot) = self.st.images.get_mut(old.id as usize) {
            *slot = img;
        }
        let tex = Texture { id: old.id, width: img.width as i32, height: img.height as i32 };
        self.st.textures.insert(key, Some(tex));
        Some(tex)
    }
    /// `DrawSetTexture` + `DrawTexturedRect`: a texture change flushes the glyph batch.
    fn draw_textured_rect(&mut self, tex: Texture, x0: i32, y0: i32, x1: i32, y1: i32) {
        if self.draw_color.a() == 0 {
            return;
        }
        let Some(&img) = self.st.images.get(tex.id as usize) else { return };
        self.flush_text();
        let blend = if self.blend { Blend::Holes } else { Blend::OpaqueTested };
        let r = self.abs(x0, y0, x1, y1);
        self.screen.vgui_image(img, r, [0.0, 0.0, 1.0, 1.0], self.draw_color.0, blend, self.clip);
    }
    fn font(&mut self, _scheme: &str, name: &str, proportional: bool, hd: bool) -> FontHandle {
        let key = (name.to_ascii_lowercase(), proportional, hd);
        if let Some(&h) = self.st.font_keys.get(&key) {
            return h;
        }
        // IScheme::GetFont: 0 for a font the scheme does not define (remembered too).
        let h = match self.screen.vgui_font(name, proportional, hd) {
            Some(id) => {
                self.st.fonts.push(id);
                FontHandle(self.st.fonts.len() as u32)
            }
            None => FontHandle(0),
        };
        self.st.font_keys.insert(key, h);
        h
    }
    fn font_tall(&self, font: FontHandle) -> i32 {
        self.font_id(font).map_or(0, |f| self.screen.font_tall(f))
    }
    fn char_abc(&self, font: FontHandle, ch: char) -> Abc {
        self.font_id(font).map_or(Abc::default(), |f| {
            let (a, b, c) = self.screen.char_abc(f, ch);
            Abc { a, b, c }
        })
    }
    fn play_sound(&mut self, name: &str) {
        self.st.sounds.push(name.to_owned());
    }
    fn set_cursor(&mut self, cursor: Cursor) {
        self.st.cursor = cursor;
    }
    fn clipboard_text(&mut self) -> String {
        if self.st.clipboard.is_none() {
            self.st.clipboard = arboard::Clipboard::new().map_err(|e| log::warn!("clipboard: {e}")).ok();
        }
        self.st.clipboard.as_mut().and_then(|c| c.get_text().ok()).unwrap_or_default()
    }
    fn set_clipboard_text(&mut self, text: &str) {
        if self.st.clipboard.is_none() {
            self.st.clipboard = arboard::Clipboard::new().map_err(|e| log::warn!("clipboard: {e}")).ok();
        }
        if let Some(c) = &mut self.st.clipboard
            && let Err(e) = c.set_text(text.to_owned())
        {
            log::warn!("clipboard: {e}");
        }
    }
}

/// `KeyCode_SDLKeyToVGUI`: `key` is the layout's unmodified key (an SDL keycode), `physical`
/// picks the keypad codes; 0 for keys without a mapping.
pub fn vgui_key(key: &Key, physical: KeyCode, location: KeyLocation) -> i32 {
    let pad = match physical {
        KeyCode::Numpad0 => Some(37),
        KeyCode::Numpad1 => Some(38),
        KeyCode::Numpad2 => Some(39),
        KeyCode::Numpad3 => Some(40),
        KeyCode::Numpad4 => Some(41),
        KeyCode::Numpad5 => Some(42),
        KeyCode::Numpad6 => Some(43),
        KeyCode::Numpad7 => Some(44),
        KeyCode::Numpad8 => Some(45),
        KeyCode::Numpad9 => Some(46),
        KeyCode::NumpadDivide => Some(47),
        KeyCode::NumpadMultiply => Some(48),
        KeyCode::NumpadSubtract => Some(49),
        KeyCode::NumpadAdd => Some(50),
        KeyCode::NumpadEnter => Some(51),
        KeyCode::NumpadDecimal => Some(52),
        _ => None,
    };
    if let Some(p) = pad {
        return p;
    }
    match key {
        Key::Character(s) => {
            let mut chars = s.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { return 0 };
            // SDL2's Windows keymap takes a letter key's code from its virtual key, which
            // non-Latin layouts (Cyrillic, Greek, ...) keep at the US positions: under the
            // Russian layout the C key is still `SDLK_c`, so Ctrl+C/V work. Their other keys
            // keep the layout's character (the Cyrillic letters on `[`, `;`, ... → no code).
            if !c.is_ascii()
                && let Some(i) = LETTER_KEYS.iter().position(|&k| k == physical)
                && let Ok(i) = u8::try_from(i)
            {
                return char_code(char::from(b'a' + i));
            }
            char_code(c)
        }
        Key::Named(n) => match n {
            NamedKey::Backspace => KEY_BACKSPACE,
            NamedKey::Tab => KEY_TAB,
            NamedKey::Enter => KEY_ENTER,
            NamedKey::Escape => KEY_ESCAPE,
            NamedKey::Space => KEY_SPACE,
            NamedKey::Delete => KEY_DELETE,
            NamedKey::CapsLock => KEY_CAPSLOCK,
            NamedKey::ScrollLock => KEY_SCROLLLOCK,
            NamedKey::Pause => KEY_BREAK,
            NamedKey::Insert => KEY_INSERT,
            NamedKey::Home => KEY_HOME,
            NamedKey::PageUp => KEY_PAGEUP,
            NamedKey::End => KEY_END,
            NamedKey::PageDown => KEY_PAGEDOWN,
            NamedKey::ArrowRight => KEY_RIGHT,
            NamedKey::ArrowLeft => KEY_LEFT,
            NamedKey::ArrowDown => KEY_DOWN,
            NamedKey::ArrowUp => KEY_UP,
            NamedKey::NumLock => KEY_NUMLOCK,
            NamedKey::Control => if location == KeyLocation::Right { KEY_RCONTROL } else { KEY_LCONTROL },
            NamedKey::Shift => if location == KeyLocation::Right { KEY_RSHIFT } else { KEY_LSHIFT },
            NamedKey::Alt | NamedKey::AltGraph => if location == KeyLocation::Right { KEY_RALT } else { KEY_LALT },
            NamedKey::Super | NamedKey::Meta => if location == KeyLocation::Right { 86 } else { 85 },
            NamedKey::F1 => KEY_F1,
            NamedKey::F2 => KEY_F1 + 1,
            NamedKey::F3 => KEY_F1 + 2,
            NamedKey::F4 => KEY_F1 + 3,
            NamedKey::F5 => KEY_F1 + 4,
            NamedKey::F6 => KEY_F1 + 5,
            NamedKey::F7 => KEY_F1 + 6,
            NamedKey::F8 => KEY_F1 + 7,
            NamedKey::F9 => KEY_F1 + 8,
            NamedKey::F10 => KEY_F1 + 9,
            NamedKey::F11 => KEY_F1 + 10,
            NamedKey::F12 => KEY_F1 + 11,
            _ => 0,
        },
        _ => 0,
    }
}

const LETTER_KEYS: [KeyCode; 26] = [
    KeyCode::KeyA,
    KeyCode::KeyB,
    KeyCode::KeyC,
    KeyCode::KeyD,
    KeyCode::KeyE,
    KeyCode::KeyF,
    KeyCode::KeyG,
    KeyCode::KeyH,
    KeyCode::KeyI,
    KeyCode::KeyJ,
    KeyCode::KeyK,
    KeyCode::KeyL,
    KeyCode::KeyM,
    KeyCode::KeyN,
    KeyCode::KeyO,
    KeyCode::KeyP,
    KeyCode::KeyQ,
    KeyCode::KeyR,
    KeyCode::KeyS,
    KeyCode::KeyT,
    KeyCode::KeyU,
    KeyCode::KeyV,
    KeyCode::KeyW,
    KeyCode::KeyX,
    KeyCode::KeyY,
    KeyCode::KeyZ,
];

/// `SDL_GetModState` bits as `SyncModifiers` checks them, with their vgui codes (RSHIFT is
/// reported as LSHIFT).
const MOD_KEYS: [(u32, i32); 9] = [(0x1, KEY_LSHIFT), (0x100, KEY_LALT), (0x40, KEY_LCONTROL), (0x2, KEY_LSHIFT), (0x200, KEY_RALT), (0x80, KEY_RCONTROL), (0x2000, KEY_CAPSLOCK), (0x400, 85), (0x800, 86)];

/// `AppHandler`'s per-event state: the modifier mask and the pending double click.
#[derive(Default)]
pub struct InputState {
    mods: u32,
    pending_click: Option<(Instant, i32, i32)>,
}

impl InputState {
    /// The modifier bit a physical key holds.
    fn mod_bit(physical: KeyCode) -> u32 {
        match physical {
            KeyCode::ShiftLeft => 0x1,
            KeyCode::ShiftRight => 0x2,
            KeyCode::ControlLeft => 0x40,
            KeyCode::ControlRight => 0x80,
            KeyCode::AltLeft => 0x100,
            KeyCode::AltRight => 0x200,
            KeyCode::SuperLeft => 0x400,
            KeyCode::SuperRight => 0x800,
            _ => 0,
        }
    }

    /// KEYDOWN / KEYUP: press+typed or release, then `SyncModifiers`.
    pub fn key(&mut self, ui: &mut Ui, code: i32, physical: KeyCode, down: bool) {
        if code != 0 {
            if down {
                ui.key_code_pressed(code);
                ui.key_code_typed(code);
            } else {
                ui.key_code_released(code);
            }
        }
        let mut mods = self.mods;
        let bit = Self::mod_bit(physical);
        if down {
            mods |= bit;
        } else {
            mods &= !bit;
        }
        if physical == KeyCode::CapsLock && down {
            mods ^= 0x2000;
        }
        let diff = self.mods ^ mods;
        for (b, c) in MOD_KEYS {
            if diff & b != 0 {
                if self.mods & b != 0 {
                    ui.key_code_released(c);
                } else {
                    ui.key_code_pressed(c);
                    ui.key_code_typed(c);
                }
            }
        }
        self.mods = mods;
    }

    /// MOUSEBUTTONDOWN: a second press within `sdl_double_click_time` ms and
    /// `sdl_double_click_size` px (any button) is a double press.
    pub fn mouse_down(&mut self, ui: &mut Ui, code: i32, (x, y): (i32, i32), time_ms: f32, size: f32) {
        let now = Instant::now();
        if let Some((t, lx, ly)) = self.pending_click
            && time_ms >= now.duration_since(t).as_millis() as f32
            && size >= (x - lx).abs() as f32
            && size >= (y - ly).abs() as f32
        {
            self.pending_click = None;
            ui.mouse_pressed(code, true);
            return;
        }
        self.pending_click = Some((now, x, y));
        ui.mouse_pressed(code, false);
    }
}

/// The keycode table's character entries (SDL keycodes are unshifted, lower-case).
fn char_code(c: char) -> i32 {
    match c.to_ascii_lowercase() {
        c @ '1'..='9' => c as i32 - '1' as i32 + 2,
        '0' => 1,
        c @ 'a'..='z' => c as i32 - 'a' as i32 + 11,
        '\'' => 56,
        ',' => 58,
        '-' => 62,
        '.' => 59,
        '/' => 60,
        ';' => 55,
        '=' => 63,
        '[' => 53,
        '\\' => 61,
        ']' => 54,
        '`' => 57,
        _ => 0,
    }
}

/// `GetVGUI2KeyCodeForBind`'s conversion of an engine key number to a vgui2 key code (ASCII
/// keys through the keycode table; the engine's special keys by their SDL equivalents).
pub fn engine_key_to_vgui(key: i32) -> i32 {
    use gs_console::keys as k;
    let Ok(b) = u8::try_from(key) else { return 0 };
    match b {
        k::K_TAB => KEY_TAB,
        k::K_ENTER => KEY_ENTER,
        k::K_ESCAPE => KEY_ESCAPE,
        b' ' => KEY_SPACE,
        k::K_BACKSPACE => KEY_BACKSPACE,
        k::K_UPARROW => KEY_UP,
        k::K_DOWNARROW => KEY_DOWN,
        k::K_LEFTARROW => KEY_LEFT,
        k::K_RIGHTARROW => KEY_RIGHT,
        k::K_INS => KEY_INSERT,
        k::K_DEL => KEY_DELETE,
        k::K_HOME => KEY_HOME,
        k::K_END => KEY_END,
        k::K_PGUP => KEY_PAGEUP,
        k::K_PGDN => KEY_PAGEDOWN,
        f if (k::K_F1..k::K_F1 + 12).contains(&f) => KEY_F1 + i32::from(f - k::K_F1),
        c if c.is_ascii_graphic() => char_code(char::from(c)),
        _ => 0,
    }
}

/// The text of a game file, if it loads.
pub fn read_text(fs: &FileSystem, name: &str) -> Option<String> {
    fs.read(name).map(|d| String::from_utf8_lossy(&d).into_owned()).map_err(|e| log::debug!("{name}: {e}")).ok()
}

/// GameUI's filesystem: the engine's search paths.
pub struct HostFs(pub FileSystem);

impl gs_gameui::GameFs for HostFs {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.read(path).map_err(|e| log::debug!("{path}: {e}")).ok()
    }
    fn language_files(&self, pattern: &str) -> Vec<String> {
        self.0.language_files(pattern)
    }
    fn read_in(&self, path: &str, id: &str) -> Option<Vec<u8>> {
        let p = self.0.resolve_in(path, Some(path_id(id)?)).ok()?;
        std::fs::read(p).ok()
    }
    fn exists(&self, path: &str) -> bool {
        self.0.exists(path)
    }
    fn find(&self, pattern: &str, id: Option<&str>) -> Vec<(String, bool)> {
        match id {
            Some(i) => path_id(i).map_or_else(Vec::new, |pid| self.0.find(pattern, Some(pid))),
            None => self.0.find(pattern, None),
        }
    }
    fn write_in(&self, path: &str, id: &str, data: &[u8]) -> bool {
        let Some(pid) = path_id(id) else { return false };
        self.0.write(path, pid, data).map_err(|e| log::debug!("{path}: {e}")).is_ok()
    }
    fn add_search_path(&self, path: &str, id: &str) {
        if let Some(pid) = path_id(id) {
            self.0.add_search_path(path, pid);
        }
    }
    fn remove(&self, path: &str, id: Option<&str>) {
        self.0.remove(path, id.and_then(path_id));
    }
    fn game_dir(&self) -> String {
        self.0.game_dir().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default()
    }
}

/// A FileSystem_Stdio path ID by name.
fn path_id(id: &str) -> Option<gs_fs::PathId> {
    Some(match id.to_ascii_uppercase().as_str() {
        "GAMECONFIG" => gs_fs::PathId::GameConfig,
        "GAME" => gs_fs::PathId::Game,
        "GAMEDOWNLOAD" => gs_fs::PathId::GameDownload,
        "GAME_FALLBACK" => gs_fs::PathId::GameFallback,
        "BASE" => gs_fs::PathId::Base,
        "DEFAULTGAME" => gs_fs::PathId::DefaultGame,
        "PLATFORM" => gs_fs::PathId::Platform,
        "CONFIG" => gs_fs::PathId::Config,
        _ => return None,
    })
}

/// A vgui2 `Ui` with the localization files in the engine's and GameUI's order.
pub fn new_ui(fs: &FileSystem) -> Ui {
    let mut ui = Ui::new();
    for pattern in [
        "resource/valve_%language%.txt",
        "resource/cstrike_%language%.txt",
        "resource/gameui_%language%.txt",
        "resource/platform_%language%.txt",
        "resource/vgui_%language%.txt",
    ] {
        for file in fs.language_files(pattern) {
            match fs.read(&file) {
                Ok(data) => ui.localize.add_file(&data),
                Err(e) => log::debug!("{file}: {e}"),
            }
        }
    }
    ui
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(ch: &str, physical: KeyCode) -> i32 {
        vgui_key(&Key::Character(ch.into()), physical, KeyLocation::Standard)
    }

    /// HL25's SDL2 under the Russian layout (probed): letter keys are `a`..`z` by position, the
    /// other keys keep the layout's character. Latin layouts keep their letters (AZERTY Q = A).
    #[test]
    fn letter_keys_of_non_latin_layouts_are_latin() {
        // Cyrillic es on C, em on V, ha on `[`.
        assert_eq!(code("\u{441}", KeyCode::KeyC), KEY_C);
        assert_eq!(code("\u{43C}", KeyCode::KeyV), KEY_V);
        assert_eq!(code("\u{445}", KeyCode::BracketLeft), 0);
        assert_eq!(code(".", KeyCode::Slash), KEY_PERIOD);
        assert_eq!(code("a", KeyCode::KeyQ), KEY_A);
    }
}
