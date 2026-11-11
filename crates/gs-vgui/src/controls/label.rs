//! `TextImage` and `Label` (docs/re/vgui-controls.md §3).

use std::any::Any;

use crate::msg::Msg;
use crate::scheme::Scheme;
use crate::surface::{Color, FontHandle, Surface, Texture};
use crate::ui::{Control, PanelId, Ui};

/// `vgui2::TextImage`: single-line text with an ellipsis, or wrapped lines.
#[derive(Debug, Clone, Default)]
pub struct TextImage {
    pub text: Vec<char>,
    pub font: FontHandle,
    pub color: Color,
    pub pos: (i32, i32),
    pub size: (i32, i32),
    draw_width: i32,
    wrap: bool,
    recalc: bool,
    ellipsis: Option<usize>,
    line_breaks: Vec<usize>,
}

/// `TextImage::SetText(const char*)`: `#token` localized; an unknown token stays literal.
pub fn localized(ui: &Ui, text: &str) -> String {
    if let Some(token) = text.strip_prefix('#')
        && let Some(v) = ui.localize.find(token)
    {
        return v.to_owned();
    }
    text.to_owned()
}

impl TextImage {
    /// `new TextImage("")` with a font and colour.
    pub fn with_font(font: FontHandle, color: Color) -> Self {
        Self { font, color, ..Self::default() }
    }

    pub fn set_text(&mut self, ui: &Ui, text: &str) {
        self.set_text_raw(&localized(ui, text));
    }

    pub fn set_text_raw(&mut self, text: &str) {
        self.text = text.chars().collect();
        self.line_breaks.clear();
        self.recalc = true;
    }

    pub fn text_string(&self) -> String {
        self.text.iter().collect()
    }

    pub fn set_size(&mut self, w: i32, h: i32) {
        self.size = (w, h);
        self.recalc = true;
        self.draw_width = w;
    }

    pub fn set_draw_width(&mut self, w: i32) {
        self.recalc = true;
        self.draw_width = w;
    }

    pub fn set_wrap(&mut self, wrap: bool) {
        self.wrap = wrap;
    }

    /// `GetContentSize`: ABC widths; `&` skipped; `\n` and recorded breaks start lines.
    pub fn content_size(&mut self, s: &dyn Surface) -> (i32, i32) {
        if self.font == FontHandle(0) {
            return (0, 0);
        }
        if self.wrap {
            self.recalculate_line_breaks(s);
        }
        let line_h = s.font_tall(self.font);
        let (mut w, mut h, mut max_w) = (0, line_h, 0);
        let mut i = 0;
        while i < self.text.len() {
            let mut c = self.text[i];
            if c == '&' && i + 1 < self.text.len() {
                i += 1;
                c = self.text[i];
            }
            if self.wrap && self.line_breaks.contains(&i) {
                h += line_h;
                max_w = max_w.max(w);
                w = 0;
            }
            w += s.char_abc(self.font, c).advance();
            if c == '\n' {
                h += line_h;
                max_w = max_w.max(w);
                w = 0;
            }
            i += 1;
        }
        (w.max(max_w), h)
    }

    /// `RecalculateEllipsesPosition` (the first character is never counted: a quirk).
    fn recalculate_ellipsis(&mut self, s: &dyn Surface) {
        self.ellipsis = None;
        let ell = 3 * s.char_width(self.font, '.');
        let mut x = 0;
        let chars: Vec<(usize, char)> = self.text.iter().copied().enumerate().collect();
        let counted = |idx: usize, c: char| -> bool { c != '\r' && !(c == '&' && chars.get(idx + 1).is_some_and(|&(_, n)| n != '&')) };
        for (k, &(i, c)) in chars.iter().enumerate().skip(1) {
            if !counted(i, c) {
                continue;
            }
            let cw = s.char_width(self.font, c);
            if x + cw + ell <= self.draw_width {
                x += cw;
                continue;
            }
            let rest: i32 = cw + chars[k + 1..].iter().filter(|&&(j, d)| counted(j, d)).map(|&(_, d)| s.char_width(self.font, d)).sum::<i32>();
            if x + rest <= self.draw_width {
                x += cw;
                continue;
            }
            self.ellipsis = Some(i);
            return;
        }
    }

    /// `RecalculateNewLinePositions`: break before a word that would cross the draw width; an
    /// over-long word breaks where it overflows; `\r`/`\n` reset the line.
    fn recalculate_line_breaks(&mut self, s: &dyn Surface) {
        self.line_breaks.clear();
        let mut x = 0;
        let mut word_start: Option<usize> = None;
        let mut word_x = 0;
        for (i, &c) in self.text.iter().enumerate() {
            if c == '\r' || c == '\n' {
                x = 0;
                word_start = None;
                continue;
            }
            let cw = s.char_width(self.font, c);
            if c == ' ' {
                word_start = None;
            } else if word_start.is_none() {
                word_start = Some(i);
                word_x = x;
            }
            if x + cw > self.draw_width && x > 0 {
                match word_start.filter(|&ws| ws != i && word_x > 0) {
                    Some(ws) => {
                        self.line_breaks.push(ws);
                        x -= word_x;
                    }
                    None => {
                        self.line_breaks.push(i);
                        x = 0;
                    }
                }
                word_x = 0;
            }
            x += cw;
        }
    }

    /// `TextImage::Paint`.
    pub fn paint(&mut self, s: &mut dyn Surface) {
        if self.text.is_empty() || self.font == FontHandle(0) {
            return;
        }
        if self.recalc {
            self.recalc = false;
            self.ellipsis = None;
            if !self.wrap && !self.text.contains(&'\n') {
                self.recalculate_ellipsis(s);
            } else if self.wrap {
                self.recalculate_line_breaks(s);
            }
        }
        s.draw_set_text_color(self.color);
        s.draw_set_text_font(self.font);
        let line_h = s.font_tall(self.font);
        let (px, py) = self.pos;
        let (mut x, mut y, mut lb) = (0, 0, 0);
        let mut i = 0;
        while i < self.text.len() {
            let c = self.text[i];
            if c == '\r' {
                i += 1;
                continue;
            }
            if c == '\n' {
                x = 0;
                y += line_h;
                i += 1;
                continue;
            }
            if c == '&' {
                if self.text.get(i + 1) != Some(&'&') {
                    i += 1;
                    continue;
                }
                i += 1;
            }
            if self.ellipsis == Some(i) {
                for _ in 0..3 {
                    s.draw_set_text_pos(px + x, py + y);
                    s.draw_unicode_char('.');
                    x += s.char_width(self.font, '.');
                }
                return;
            }
            if lb < self.line_breaks.len() && i == self.line_breaks[lb] {
                x = 0;
                y += line_h;
                lb += 1;
            }
            s.draw_set_text_pos(px + x, py + y);
            s.draw_unicode_char(self.text[i]);
            x += s.char_width(self.font, self.text[i]);
            i += 1;
        }
    }
}

/// `Label::_contentAlignment`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    NorthWest,
    North,
    NorthEast,
    #[default]
    West,
    Center,
    East,
    SouthWest,
    South,
    SouthEast,
}

impl Align {
    /// The `.res` `textAlignment` names.
    pub fn parse(s: &str) -> Option<Self> {
        const NAMES: [&str; 9] = ["north-west", "north", "north-east", "west", "center", "east", "south-west", "south", "south-east"];
        const ALL: [Align; 9] =
            [Align::NorthWest, Align::North, Align::NorthEast, Align::West, Align::Center, Align::East, Align::SouthWest, Align::South, Align::SouthEast];
        NAMES.iter().position(|n| n.eq_ignore_ascii_case(s)).map(|i| ALL[i])
    }
}

/// `Label::_textColorState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextColorState {
    #[default]
    Normal,
    Dull,
    Bright,
    Light,
}

/// A non-text image of a label.
#[derive(Debug, Clone)]
pub enum LabelImage {
    /// A texture by name (`scheme()->GetImage`), drawn at its own size.
    Texture { name: String, tex: Option<Texture>, size: (i32, i32), color: Color },
    /// Glyphs in another font (FrameButton's Marlett symbol).
    Glyph(TextImage),
    /// RadioButton's `RadioImage`: reserves `reserve` in the layout, paints the texture at the
    /// panel's (0,0) with `size`.
    Pinned { tex: Option<Texture>, size: (i32, i32), reserve: (i32, i32) },
}

/// One entry of `_imageDar`.
#[derive(Debug, Clone)]
enum Slot {
    /// The label's own text image.
    Text,
    /// A cleared entry (`image = NULL`): skipped.
    Empty,
    Image(LabelImage),
}

#[derive(Debug, Clone)]
struct ImageInfo {
    image: Slot,
    offset: i32,
    xpos: i32,
    width: i32,
}

/// `vgui2::Label`'s state, shared by Button and its subclasses.
#[derive(Debug, Clone)]
pub struct LabelState {
    pub text: TextImage,
    pub align: Align,
    pub text_inset: (i32, i32),
    disabled_fg1: Color,
    disabled_fg2: Color,
    pub color_state: TextColorState,
    images: Vec<ImageInfo>,
    pub font_override: Option<String>,
    pub wrap: bool,
    pub proportional_scale_images: bool,
    pub hotkey: Option<char>,
    /// `_associateName`: the panel a hotkey on this label focuses.
    pub associate: Option<String>,
}

/// `CalculateHotkey`: the letter after an unescaped `&`.
pub fn hotkey_of(text: &str) -> Option<char> {
    let c: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '&' {
            match c.get(i + 1) {
                Some('&') => i += 1,
                None => return None,
                Some(&n) if n.is_alphanumeric() => return n.to_lowercase().next(),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

impl LabelState {
    pub fn new(ui: &Ui, text: &str) -> Self {
        let mut st = Self {
            text: TextImage::default(),
            align: Align::West,
            text_inset: (0, 0),
            disabled_fg1: Color::TRANSPARENT,
            disabled_fg2: Color::TRANSPARENT,
            color_state: TextColorState::Normal,
            images: vec![ImageInfo { image: Slot::Text, offset: 0, xpos: -1, width: -1 }],
            font_override: None,
            wrap: false,
            proportional_scale_images: true,
            hotkey: None,
            associate: None,
        };
        st.set_text(ui, text);
        st
    }

    /// `Label::SetText`.
    pub fn set_text(&mut self, ui: &Ui, text: &str) {
        self.text.set_text(ui, text);
        self.hotkey = hotkey_of(&localized(ui, text));
    }

    /// `SetTextInset`.
    pub fn set_text_inset(&mut self, ui: &Ui, me: PanelId, x: i32, y: i32) {
        self.text_inset = (x, y);
        self.text.set_draw_width(ui.size(me).0 - x);
    }

    /// `SetImageAtIndex(index, image, offset)`.
    pub fn set_image_at(&mut self, index: usize, image: LabelImage, offset: i32) {
        while self.images.len() <= index {
            self.images.push(ImageInfo { image: Slot::Empty, offset: 0, xpos: -1, width: -1 });
        }
        self.images[index] = ImageInfo { image: Slot::Image(image), offset, xpos: -1, width: -1 };
    }

    /// `AddImage(image, offset)`.
    pub fn add_image(&mut self, image: LabelImage, offset: i32) {
        self.images.push(ImageInfo { image: Slot::Image(image), offset, xpos: -1, width: -1 });
    }

    /// The most recently added non-text image.
    pub fn last_image_mut(&mut self) -> Option<&mut LabelImage> {
        self.images.iter_mut().rev().find_map(|i| match &mut i.image {
            Slot::Image(img) => Some(img),
            _ => None,
        })
    }

    /// `SetTextImageIndex(i)`: the text image moves to entry `i` (its old entry is cleared).
    pub fn set_text_image_index(&mut self, index: usize) {
        for info in &mut self.images {
            if matches!(info.image, Slot::Text) {
                info.image = Slot::Empty;
            }
        }
        while self.images.len() <= index {
            self.images.push(ImageInfo { image: Slot::Empty, offset: 0, xpos: -1, width: -1 });
        }
        self.images[index].image = Slot::Text;
    }

    /// `SetTextImageIndex(-1)`: the text image is no longer drawn.
    pub fn clear_text_image(&mut self) {
        for info in &mut self.images {
            if matches!(info.image, Slot::Text) {
                info.image = Slot::Empty;
            }
        }
    }

    pub fn font(&self) -> FontHandle {
        self.text.font
    }

    /// `Label::ApplySettings`: `labelText` (a `%name%` dialog variable shows as written),
    /// `textAlignment`, `associate`, `dulltext`/`brighttext`, `font`, `wrap`, then an immediate
    /// layout.
    pub fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &gs_formats::keyvalues::Block, s: &mut dyn Surface) {
        use crate::editable::{kv_int, kv_str};
        ui.panel_apply_settings(me, kv, s);
        if let Some(t) = kv_str(kv, "labelText") {
            if t.len() > 1 && t.starts_with('%') && t.ends_with('%') {
                self.text.set_text_raw(t);
                self.hotkey = hotkey_of(t);
            } else {
                self.set_text(ui, t);
            }
        }
        const ALIGN: [(&str, Align); 9] = [
            ("north-west", Align::NorthWest),
            ("north", Align::North),
            ("north-east", Align::NorthEast),
            ("west", Align::West),
            ("center", Align::Center),
            ("east", Align::East),
            ("south-west", Align::SouthWest),
            ("south", Align::South),
            ("south-east", Align::SouthEast),
        ];
        let a = kv_str(kv, "textAlignment").unwrap_or("");
        if let Some(&(_, al)) = ALIGN.iter().find(|(n, _)| n.eq_ignore_ascii_case(a)) {
            self.align = al;
        }
        if let Some(assoc) = kv_str(kv, "associate").filter(|s| !s.is_empty()) {
            self.associate = Some(assoc.to_owned());
        }
        self.color_state = if kv_int(kv, "dulltext", 0) == 1 {
            TextColorState::Dull
        } else if kv_int(kv, "brighttext", 0) == 1 {
            TextColorState::Bright
        } else {
            TextColorState::Normal
        };
        let (prop, hd) = (ui.is_proportional(me), ui.is_hd_proportional(me));
        let file = ui.scheme_of(me).map(|sc| sc.file.clone()).unwrap_or_default();
        match kv_str(kv, "font").filter(|f| !f.is_empty()) {
            Some(f) => {
                self.font_override = Some(f.to_owned());
                self.text.font = s.font(&file, f, prop, hd);
            }
            None => {
                if self.font_override.take().is_some() {
                    self.text.font = s.font(&file, "Default", prop, hd);
                }
            }
        }
        self.wrap = kv_int(kv, "wrap", 0) > 0;
        self.text.set_wrap(self.wrap);
        ui.invalidate_layout_ex(me, true, false, s);
    }

    /// `Label::ApplySchemeSettings`.
    pub fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
        let (prop, hd) = (ui.is_proportional(me), ui.is_hd_proportional(me));
        if let Some(name) = self.font_override.clone() {
            self.text.font = s.font(&scheme.file, &name, prop, hd);
        }
        if self.text.font == FontHandle(0) {
            self.text.font = s.font(&scheme.file, "Default", prop, hd);
        }
        if self.wrap {
            let (w, h) = ui.size(me);
            self.text.set_size(w - self.text_inset.0, h);
        } else {
            let (cw, ch) = self.text.content_size(s);
            self.text.set_size(cw, ch);
        }
        for info in &mut self.images {
            if matches!(info.image, Slot::Image(_)) {
                info.image = Slot::Empty;
            }
        }
        self.disabled_fg1 = scheme.color("DisabledFgColor1", Color::WHITE);
        self.disabled_fg2 = scheme.color("DisabledFgColor2", Color::WHITE);
        let key = match self.color_state {
            TextColorState::Normal => "BaseText",
            TextColorState::Dull => "LabelDimText",
            TextColorState::Bright => "BrightControlText",
            TextColorState::Light => "BrightBaseText",
        };
        let fg = scheme.color(key, Color::WHITE);
        let bg = scheme.color("LabelBgColor", Color::WHITE);
        if let Some(n) = ui.node_mut(me) {
            n.bg = bg;
            n.fg = fg;
        }
        self.text.color = fg;
        ui.invalidate_layout(me);
    }

    fn image_size(&self, ui: &Ui, me: PanelId, img: &LabelImage, s: &mut dyn Surface) -> (i32, i32) {
        let (w, h) = match img {
            LabelImage::Texture { size, .. } => *size,
            LabelImage::Pinned { reserve, .. } => return *reserve,
            LabelImage::Glyph(t) => {
                let mut t = t.clone();
                t.content_size(s)
            }
        };
        if self.proportional_scale_images { (ui.psv(me, w), ui.psv(me, h)) } else { (w, h) }
    }

    /// `Label::PerformLayout`.
    pub fn perform_layout(&mut self, ui: &Ui, me: PanelId, s: &mut dyn Surface) {
        let (w, _) = ui.size(me);
        let w = w - self.text_inset.0;
        if self.images.len() == 1 {
            let (cw, ch) = self.text.content_size(s);
            if self.wrap {
                self.text.set_size(w, ch);
            } else {
                self.text.set_size(cw.min(w), ch);
            }
        } else {
            let sum: i32 = self
                .images
                .iter()
                .filter_map(|i| match &i.image {
                    Slot::Image(img) => Some(img),
                    _ => None,
                })
                .map(|img| self.image_size(ui, me, img, s).0)
                .sum();
            if w - sum >= 0 {
                let ih = self.text.size.1;
                self.text.set_size(w - sum, ih);
            }
        }
    }

    /// `ComputeAlignment`.
    fn compute_alignment(&self, ui: &Ui, me: PanelId, s: &mut dyn Surface) -> (i32, i32, i32, i32) {
        let (pw, ph) = ui.size(me);
        let (mut max_x, mut max_y) = (0, 0);
        let mut xa = self.align;
        for info in &self.images {
            let (iw, ih) = match &info.image {
                Slot::Text => self.text.size,
                Slot::Image(img) => self.image_size(ui, me, img, s),
                Slot::Empty => continue,
            };
            if iw > pw {
                xa = Align::West;
            }
            max_y = max_y.max(ih);
            max_x += iw + info.offset;
        }
        let tx0 = match xa {
            Align::NorthEast | Align::East | Align::SouthEast => pw - max_x,
            Align::North | Align::Center | Align::South => (pw - max_x) / 2,
            _ => 0,
        };
        let ty0 = match self.align {
            Align::SouthWest | Align::South | Align::SouthEast => ph - max_y,
            Align::West | Align::Center | Align::East => (ph - max_y) / 2,
            _ => 0,
        };
        (tx0, ty0, tx0 + max_x, ty0 + max_y)
    }

    /// `GetContentSize`.
    pub fn content_size(&mut self, ui: &Ui, me: PanelId, s: &mut dyn Surface) -> (i32, i32) {
        let (tx0, ty0, tx1, ty1) = self.compute_alignment(ui, me, s);
        let offsets: i32 = self.images.iter().map(|i| i.offset).sum();
        let (cw, _) = self.text.content_size(s);
        (self.text_inset.0 + (tx1 - tx0) - self.text.size.0 + cw + offsets, self.text_inset.1 + (ty1 - ty0))
    }

    /// `Label::Paint`.
    pub fn paint(&mut self, ui: &Ui, me: PanelId, s: &mut dyn Surface) {
        let (tx0, ty0, _tx1, ty1) = self.compute_alignment(ui, me, s);
        let enabled = ui.is_enabled(me);
        let fg = ui.node(me).map_or(Color::WHITE, |n| n.fg);
        let mut x = tx0;
        let y = ty0 + self.text_inset.1;
        let one = ui.psv(me, 1);
        for i in 0..self.images.len() {
            let info = self.images[i].clone();
            let (iw, ih) = match &info.image {
                Slot::Text => self.text.size,
                Slot::Image(img) => self.image_size(ui, me, img, s),
                Slot::Empty => continue,
            };
            let mut xpos = x + info.offset;
            if matches!(info.image, Slot::Text) {
                xpos += self.text_inset.0;
            }
            if info.xpos >= 0 {
                xpos = info.xpos;
            }
            let centred = matches!(self.align, Align::West | Align::Center | Align::East) && ih < ty1 - ty0;
            let ypos = if centred { y + ((ty1 - ty0 - ih) >> 1) } else { y };
            match info.image {
                Slot::Empty => {}
                Slot::Text => {
                    self.text.pos = (xpos, ypos);
                    if enabled {
                        self.text.color = fg;
                        self.text.paint(s);
                    } else {
                        self.text.pos = (xpos + one, ypos + one);
                        self.text.color = self.disabled_fg1;
                        self.text.paint(s);
                        self.text.pos = (xpos, ypos);
                        self.text.color = self.disabled_fg2;
                        self.text.paint(s);
                    }
                }
                Slot::Image(LabelImage::Texture { name, mut tex, color, .. }) => {
                    if tex.is_none() {
                        tex = s.texture(&name, true);
                    }
                    if let Some(t) = tex {
                        // An image wider than its slot width is drawn at that width.
                        let dw = if info.width >= 0 && iw > info.width { info.width } else { iw };
                        s.draw_set_color(color);
                        s.draw_textured_rect(t, xpos, ypos, xpos + dw, ypos + ih);
                    }
                }
                Slot::Image(LabelImage::Glyph(mut t)) => {
                    t.pos = (xpos, ypos);
                    t.paint(s);
                }
                // RadioImage: the bitmap at the button's (0,0), whatever the layout reserved.
                Slot::Image(LabelImage::Pinned { tex: Some(t), size, .. }) => {
                    s.draw_set_color(Color::WHITE);
                    s.draw_textured_rect(t, 0, 0, size.0, size.1);
                }
                Slot::Image(LabelImage::Pinned { tex: None, .. }) => {}
            }
            x = xpos + iw;
        }
    }
}

/// Runs `f` on the `LabelState` of the Label `id`.
pub fn with_label<R>(ui: &mut Ui, id: PanelId, f: impl FnOnce(&mut LabelState, &mut Ui) -> R) -> Option<R> {
    ui.with_ctrl(id, |c, ui| c.as_any_mut().downcast_mut::<Label>().map(|l| f(&mut l.st, ui))).flatten()
}

/// A plain Label panel.
#[derive(Debug)]
pub struct Label {
    pub st: LabelState,
}

impl Label {
    pub fn create(ui: &mut Ui, parent: Option<PanelId>, name: &str, text: &str) -> PanelId {
        let st = LabelState::new(ui, text);
        let id = ui.create(parent, name, Box::new(Label { st }));
        ui.invalidate_layout(id);
        id
    }
}

impl Control for Label {
    fn class(&self) -> &'static str {
        "Label"
    }
    fn has_hotkey(&self, key: char) -> bool {
        self.st.hotkey == Some(key)
    }
    fn on_message(&mut self, ui: &mut Ui, me: PanelId, msg: &Msg, _from: Option<PanelId>, _s: &mut dyn Surface) -> bool {
        if msg.is("SetText") {
            self.st.set_text(ui, &msg.get_str("text"));
            ui.invalidate_layout(me);
            return true;
        }
        false
    }
    fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &gs_formats::keyvalues::Block, s: &mut dyn Surface) {
        self.st.apply_settings(ui, me, kv, s);
    }
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, s: &mut dyn Surface) {
        self.st.apply_scheme(ui, me, scheme, s);
    }
    fn perform_layout(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.st.perform_layout(ui, me, s);
    }
    fn paint(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        self.st.paint(ui, me, s);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
