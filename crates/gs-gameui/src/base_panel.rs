//! `CBasePanel`: GameUI's full-screen background (docs/re/gameui.md §2).

use std::any::Any;

use gs_vgui::scheme::Scheme;
use gs_vgui::surface::Texture;
use gs_vgui::{Color, Control, Msg, PanelId, Surface, Ui};

use crate::Shared;

/// `bimage_t`.
#[derive(Debug, Clone)]
struct BImage {
    name: String,
    tex: Option<Texture>,
    x: i32,
    y: i32,
    wide: i32,
    tall: i32,
    scaled: bool,
    fit: bool,
}

/// `CBasePanel`.
#[derive(Debug)]
pub struct BasePanel {
    host: Shared,
    state: i32,
    images: Vec<BImage>,
    base_res: (i32, i32),
    layout: Option<String>,
}

/// `filesystem()->ParseFile`: the next token (quoted strings, `//` comments, `{}()':` single).
pub fn parse_token(p: &str) -> Option<(String, &str)> {
    let mut s = p;
    loop {
        s = s.trim_start_matches(|c: char| c <= ' ');
        if s.is_empty() {
            return None;
        }
        if let Some(rest) = s.strip_prefix("//") {
            s = rest.find('\n').map_or("", |i| &rest[i..]);
            continue;
        }
        break;
    }
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"').unwrap_or(rest.len());
        let tok = rest[..end].to_owned();
        let after = rest.get(end + 1..).unwrap_or("");
        return Some((tok, after));
    }
    let first = s.chars().next()?;
    if matches!(first, '{' | '}' | '(' | ')' | '\'' | ':') {
        return Some((first.to_string(), &s[first.len_utf8()..]));
    }
    let end = s.find(|c: char| c <= ' ' || matches!(c, '{' | '}' | '(' | ')' | '\'' | ':')).unwrap_or(s.len());
    Some((s[..end].to_owned(), &s[end..]))
}

impl BasePanel {
    /// `new CBasePanel()` under the GameUI panel: background painting on, no input.
    pub fn create(ui: &mut Ui, host: Shared, parent: PanelId, layout: Option<&str>) -> PanelId {
        let me = ui.create(None, "BasePanel", Box::new(BasePanel { host, state: 0, images: Vec::new(), base_res: (0, 0), layout: layout.map(str::to_owned) }));
        ui.set_bounds(me, 0, 0, 400, 300);
        if let Some(n) = ui.node_mut(me) {
            n.paint_border_enabled = false;
            n.paint_background_enabled = true;
            n.paint_enabled = false;
        }
        ui.set_visible(me, true);
        ui.set_mouse_input_enabled(me, false);
        ui.set_keyboard_input_enabled(me, false);
        ui.set_parent(me, Some(parent));
        me
    }

    /// `SetBackgroundRenderState`.
    pub fn set_state(ui: &mut Ui, me: PanelId, state: i32) {
        if let Some(b) = ui.ctrl::<BasePanel>(me) {
            b.state = state;
        }
    }

    /// `DrawBackgroundImage` (float products, `ceil` through double).
    fn draw_background_image(&mut self, s: &mut dyn Surface) {
        let (sw, sh) = s.screen_size();
        let (bx, by) = self.base_res;
        let xs = sw as f32 / bx as f32;
        let ys = sh as f32 / by as f32;
        let sc = if sw as f32 > bx as f32 * ys { xs } else { ys };
        let off_x = sw / 2 - (bx as f32 * sc * 0.5) as i32;
        let off_y = sh / 2 - (by as f32 * sc * 0.5) as i32;
        let ceil = |v: f32| f64::from(v).ceil() as i32;
        for b in &mut self.images {
            let (x0, y0, x1, y1) = if b.scaled {
                let x0 = if b.x != 0 { ceil(b.x as f32 * xs) } else { 0 };
                let y0 = if b.y != 0 { ceil(b.y as f32 * ys) } else { 0 };
                (x0, y0, ceil((b.wide + b.x) as f32 * xs), ceil((b.tall + b.y) as f32 * ys))
            } else if b.fit {
                let x0 = off_x + if b.x != 0 { ceil(b.x as f32 * sc) } else { 0 };
                let y0 = off_y + if b.y != 0 { ceil(b.y as f32 * sc) } else { 0 };
                (x0, y0, x0 + ceil(b.wide as f32 * sc), y0 + ceil(b.tall as f32 * sc))
            } else {
                (b.x, b.y, b.x + b.wide, b.y + b.tall)
            };
            s.draw_set_color(Color::WHITE);
            if b.tex.is_none() {
                b.tex = s.texture(&b.name, true);
            }
            if let Some(t) = b.tex {
                s.draw_textured_rect(t, x0, y0, x1, y1);
            }
        }
    }
}

impl Control for BasePanel {
    fn class(&self) -> &'static str {
        "CBasePanel"
    }
    fn on_message(&mut self, _ui: &mut Ui, _me: PanelId, _msg: &Msg, _from: Option<PanelId>, _s: &mut dyn Surface) -> bool {
        false
    }
    /// Parses the layout on every scheme application (each one appends another copy).
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let Some(text) = self.layout.clone() else { return };
        let mut p = text.as_str();
        while let Some((tok, rest)) = parse_token(p) {
            p = rest;
            if tok.is_empty() {
                break;
            }
            if tok.eq_ignore_ascii_case("resolution") {
                let Some((x, rest)) = parse_token(p) else { break };
                let Some((y, rest)) = parse_token(rest) else { break };
                p = rest;
                self.base_res = (gs_vgui::msg::atoi(&x), gs_vgui::msg::atoi(&y));
                continue;
            }
            let name = match tok.find(".tga") {
                Some(i) => tok[..i].to_owned(),
                None => tok.clone(),
            };
            let tex = s.texture(&name, true);
            let (wide, tall) = tex.map_or((0, 0), |t| (t.width, t.height));
            let (mode, rest) = parse_token(p).unwrap_or_default();
            let (x, rest) = parse_token(rest).unwrap_or_default();
            let (y, rest) = parse_token(rest).unwrap_or_default();
            p = rest;
            self.images.push(BImage {
                name,
                tex,
                x: gs_vgui::msg::atoi(&x),
                y: gs_vgui::msg::atoi(&y),
                wide,
                tall,
                scaled: mode.eq_ignore_ascii_case("scaled"),
                fit: mode.eq_ignore_ascii_case("fit"),
            });
        }
    }
    /// `PaintBackground`: dimmed in a level, else by render state.
    fn paint_background(&mut self, _ui: &mut Ui, _me: PanelId, s: &mut dyn Surface) {
        let in_level = !self.host.borrow().level_name.is_empty();
        let (sw, sh) = s.screen_size();
        if in_level && self.state != 5 {
            s.draw_set_color(Color::rgba(0, 0, 0, 128));
            s.draw_filled_rect(0, 0, sw, sh);
            return;
        }
        match self.state {
            1 | 5 => {
                s.draw_set_color(Color::rgba(0, 0, 0, 255));
                s.draw_filled_rect(0, 0, sw, sh);
            }
            2 | 3 => self.draw_background_image(s),
            _ => {}
        }
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
