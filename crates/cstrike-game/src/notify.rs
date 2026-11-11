//! The engine console's screen side (docs/re/engine-misc.md §7, §8.4): `Con_Printf` with the
//! developer notify line, the `Con_NPrintf` slots, the `messagemode` chat line, and
//! `Con_DrawNotify`, which draws all three.

use std::sync::{Mutex, MutexGuard, PoisonError};

use gs_console::keys::{K_BACKSPACE, K_ENTER, K_ESCAPE, K_KP_ENTER};
use gs_draw::Screen;

/// The chat buffer holds 120 bytes with its terminator.
const CHAT_MAX: usize = 119;
/// `con_nprint` slots and their text size (with the terminator).
const NPRINT_SLOTS: usize = 32;
const NPRINT_TEXT: usize = 80;

/// The notify line (`con_times[0]` and its 256-byte text) and what recording needs.
struct Notify {
    text: Vec<u8>,
    time: f32,
    realtime: f64,
    record: bool,
}

static NOTIFY: Mutex<Notify> = Mutex::new(Notify { text: Vec::new(), time: 0.0, realtime: 0.0, record: false });

fn notify() -> MutexGuard<'static, Notify> {
    NOTIFY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Text for the GameUI console (`VGuiWrap2_ConPrintf` / `VGuiWrap2_ConDPrintf`): `(developer
/// colour, text)`. Text printed before GameUI exists waits here (`g_PreGameUIConsole`).
static CONSOLE: Mutex<Vec<(bool, String)>> = Mutex::new(Vec::new());

/// `con_debuglog` (`-condebug`, `condebug`).
static CONDEBUG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn condebug() -> bool {
    CONDEBUG.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_condebug(on: bool) {
    CONDEBUG.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// `Con_DebugLog("qconsole.log", "%s", text)`: appended to the working directory's file in text
/// mode (CRLF), opened per message, at most 1023 bytes.
fn debug_log(text: &str) {
    use std::io::Write;
    let b = gs_common::bytestr::encode(text);
    let mut out = Vec::with_capacity(b.len().min(1023) + 8);
    for &c in &b[..b.len().min(1023)] {
        if c == b'\n' {
            out.push(b'\r');
        }
        out.push(c);
    }
    let res = std::fs::OpenOptions::new().create(true).append(true).open("qconsole.log").and_then(|mut f| f.write_all(&out));
    if let Err(e) = res {
        log::debug!("qconsole.log: {e}");
    }
}

/// `Con_Printf`: to the terminal and the GameUI console, and with `developer` on and a client
/// state other than 0 into the notify line (the last print only, at most 255 bytes).
pub fn con_printf(text: &str) {
    print!("{text}");
    if condebug() {
        debug_log(text);
    }
    CONSOLE.lock().unwrap_or_else(PoisonError::into_inner).push((false, text.to_owned()));
    let mut n = notify();
    if n.record {
        let mut b = gs_common::bytestr::encode(text);
        b.truncate(255);
        n.text = b;
        n.time = n.realtime as f32;
    }
}

/// `Con_DPrintf` (the caller checks `developer`): the GameUI console's developer colour.
pub fn con_dprintf(text: &str) {
    print!("{text}");
    if condebug() {
        debug_log(text);
    }
    CONSOLE.lock().unwrap_or_else(PoisonError::into_inner).push((true, text.to_owned()));
    let mut n = notify();
    if n.record {
        let mut b = gs_common::bytestr::encode(text);
        b.truncate(255);
        n.text = b;
        n.time = n.realtime as f32;
    }
}

/// The console text printed since the last call.
pub fn take_console_text() -> Vec<(bool, String)> {
    std::mem::take(&mut *CONSOLE.lock().unwrap_or_else(PoisonError::into_inner))
}

/// This frame's `realtime`, and whether prints are recorded (`developer != 0`, `cls.state != 0`).
pub fn set_frame(realtime: f64, record: bool) {
    let mut n = notify();
    n.realtime = realtime;
    n.record = record;
}

/// `Con_ClearNotify` (loading plaque) and `Con_CheckResize`: the notify time is cleared.
pub fn clear_notify() {
    notify().time = 0.0;
}

/// `con_linewidth` of the last `Con_CheckResize`.
static LINE_WIDTH: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// `Con_CheckResize`: a new `W/8 - 2` line width clears the notify time.
pub fn check_resize(width: i32) {
    let lw = width / 8 - 2;
    if LINE_WIDTH.swap(lw, std::sync::atomic::Ordering::Relaxed) != lw {
        clear_notify();
    }
}

/// `con_num_times` (`contimes`; nothing reads it but the command, since only one line shows).
static CON_NUM_TIMES: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(4);

/// `contimes [n]`: the overlay line count, clamped to 4..64; the console output.
pub fn contimes(argv: &[&str]) -> String {
    use std::sync::atomic::Ordering;
    if argv.len() == 2 {
        let s = argv[1].trim_start();
        let (neg, digits) = s.strip_prefix('-').map_or((false, s), |d| (true, d));
        let v = digits.bytes().take_while(u8::is_ascii_digit).fold(0i32, |a, d| a.wrapping_mul(10).wrapping_add(i32::from(d - b'0')));
        let n = if neg { v.wrapping_neg() } else { v }.clamp(4, 64);
        CON_NUM_TIMES.store(n, Ordering::Relaxed);
        format!("{n} lines will overlay.\n")
    } else {
        format!("contimes <n>\nShow <n> overlay lines [4-64].\n{} current overlay lines.\n", CON_NUM_TIMES.load(Ordering::Relaxed))
    }
}

/// `con_nprint` slots (`Con_NPrintf` / `Con_NXPrintf`): text, expiry, colour.
#[derive(Debug, Default)]
pub struct NPrint {
    slots: Vec<Option<(String, f32, [f32; 3])>>,
}

impl NPrint {
    /// `Con_NXPrintf`: slot `index` (0..32) for `ttl` seconds of `realtime` in `color`.
    pub fn nxprintf(&mut self, index: i32, ttl: f32, color: [f32; 3], text: &str, realtime: f64) {
        let Ok(i) = usize::try_from(index) else { return };
        if i >= NPRINT_SLOTS {
            return;
        }
        if self.slots.len() < NPRINT_SLOTS {
            self.slots.resize(NPRINT_SLOTS, None);
        }
        let text = gs_common::bytestr::truncate_encoded(text, NPRINT_TEXT - 1).into_owned();
        self.slots[i] = Some((text, (f64::from(ttl) + realtime) as f32, color));
    }
}

/// `messagemode` state: the chat line is open while `key_dest == key_message`.
#[derive(Debug)]
pub struct Chat {
    open: bool,
    target: String,
    buffer: Vec<u8>,
    privileged: bool,
}

impl Default for Chat {
    fn default() -> Self {
        Self { open: false, target: "say".to_owned(), buffer: Vec::new(), privileged: true }
    }
}

impl Chat {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// `messagemode [cmd]` (target `say` unless exactly one argument, cut to 31 bytes) and
    /// `messagemode2` (`say_team`); `privileged` is `Cmd_IsPrivileged`. The buffer is kept.
    pub fn open(&mut self, target: Option<&str>, privileged: bool) {
        self.open = true;
        self.privileged = privileged;
        self.target = match target {
            Some(t) => gs_common::bytestr::truncate_encoded(t, 31).into_owned(),
            None => "say".to_owned(),
        };
    }

    /// Typed text (`SDL_TEXTINPUT`): appended up to 119 bytes, cut byte-wise.
    pub fn text(&mut self, s: &str) {
        let n = s.len().min(CHAT_MAX - self.buffer.len());
        self.buffer.extend_from_slice(&s.as_bytes()[..n]);
    }

    /// `Key_Message` for a key press: on Enter the command text to add (`say "…"`, `;` turned
    /// into `,`, sent even when empty) and whether it goes to the privileged buffer.
    pub fn key(&mut self, key: u8) -> Option<(String, bool)> {
        match key {
            K_ENTER | K_KP_ENTER => {
                let text: Vec<u8> = self.buffer.iter().map(|&b| if b == b';' { b',' } else { b }).collect();
                let line = format!("{} \"{}\"\n", self.target, gs_common::bytestr::decode(&text));
                let privileged = self.privileged;
                self.reset();
                Some((line, privileged))
            }
            K_ESCAPE => {
                self.reset();
                None
            }
            K_BACKSPACE if !self.buffer.is_empty() => {
                // Drop the last code point; an invalid buffer is cleared.
                match std::str::from_utf8(&self.buffer) {
                    Ok(s) => {
                        let cut = s.char_indices().last().map_or(0, |(i, _)| i);
                        self.buffer.truncate(cut);
                    }
                    Err(e) if e.error_len().is_none() => self.buffer.truncate(e.valid_up_to()),
                    Err(_) => self.buffer.clear(),
                }
                None
            }
            _ => None,
        }
    }

    /// Reset after send or cancel.
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// `say : text` at (8, y): the last `W/10` code points' worth of the buffer; the cursor
    /// (code point 10/11) is not printable, so nothing more is drawn.
    fn draw(&self, screen: &mut Screen, x: i32, y: i32) {
        let len = self.buffer.len();
        let keep = screen.width as usize / 10;
        let shown: &[u8] = if len <= keep {
            &self.buffer
        } else {
            // Q_UnicodeAdvance by the byte difference, counted in code points.
            let skip = len - keep;
            let s = String::from_utf8_lossy(&self.buffer);
            let start = s.char_indices().nth(skip).map_or(s.len(), |(i, _)| i);
            return self.draw_text(screen, x, y, &s[start..]);
        };
        self.draw_text(screen, x, y, &String::from_utf8_lossy(shown));
    }

    fn draw_text(&self, screen: &mut Screen, x: i32, y: i32, text: &str) {
        let x1 = screen.draw_console_string(x, y, &self.target);
        let x2 = screen.draw_console_string(x1 + 3, y, ":");
        screen.draw_console_string(x2 + 3, y, text);
    }
}

/// `Con_DrawNotify`: the `Con_NPrintf` slots right-aligned from y 20, the developer notify line
/// at (8, 0) for `con_notifytime`, then the chat line (moved by `HUD_ChatInputPosition`).
pub fn draw_notify(screen: &mut Screen, nprint: &NPrint, chat: &Chat, realtime: f64, notifytime: f32, chat_y: Option<i32>) {
    let font = screen.font("EngineFont");
    let tall = screen.font_tall(font);
    let (w, h) = (screen.width as i32, screen.height as i32);
    for (i, slot) in nprint.slots.iter().enumerate() {
        let Some((text, expire, color)) = slot else { continue };
        if f64::from(*expire) <= realtime {
            continue;
        }
        let (tw, _) = screen.draw_console_string_len(text);
        let y = 20 + i as i32 * tall;
        if y > h - 20 {
            continue;
        }
        screen.set_text_color(color[0], color[1], color[2]);
        screen.draw_console_string(w - tw - 10, y, text);
    }
    let [r, g, b] = screen.con_color;
    screen.set_text_color(f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0);
    let mut y = 0;
    let (text, time) = {
        let n = notify();
        (n.text.clone(), n.time)
    };
    if time != 0.0 && ((realtime - f64::from(time)) as f32) <= notifytime {
        // Exactly W/8 - 2 bytes of the zero-padded copy; the bytes after the text draw nothing.
        let mut x = 8;
        for &byte in text.iter().take(usize::try_from(w / 8 - 2).unwrap_or(0)) {
            let ch = char::from_u32(u32::from(byte as i8 as i16 as u16)).unwrap_or(' ');
            x += screen.vgui2_draw_character(x, y, ch, font);
        }
        y += tall;
    }
    if chat.is_open() {
        chat.draw(screen, 8, chat_y.unwrap_or(y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_sends_the_line_with_semicolons_replaced_even_when_empty() {
        let mut c = Chat::default();
        c.open(None, true);
        c.text("hi; all");
        assert_eq!(c.key(K_ENTER), Some(("say \"hi, all\"\n".to_owned(), true)));
        assert!(!c.is_open());
        c.open(Some("say_team"), false);
        assert_eq!(c.key(K_KP_ENTER), Some(("say_team \"\"\n".to_owned(), false)));
    }

    #[test]
    fn the_buffer_caps_at_119_bytes_and_backspace_drops_a_code_point() {
        let mut c = Chat::default();
        c.open(None, true);
        c.text(&"a".repeat(118));
        c.text("é");
        assert_eq!(c.buffer.len(), 119, "a 2-byte character cut after its first byte");
        c.key(K_BACKSPACE);
        assert_eq!(c.buffer.len(), 118, "the partial sequence goes");
        c.text("é");
        c.key(K_BACKSPACE);
        assert_eq!(c.buffer.len(), 118);
        c.key(K_ESCAPE);
        assert!(!c.is_open() && c.buffer.is_empty());
    }
}
