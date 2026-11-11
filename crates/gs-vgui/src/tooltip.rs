//! `vgui2::Tooltip` (docs/re/vgui-controls.md §9; GameUI.dll `gu@0x1007f6f0…0x100800f5`): not a
//! panel but a per-panel record, shown in one shared popup `TextEntry` named `"tooltip"`.

use crate::controls::text_entry::{TextEntry, with_te};
use crate::surface::{Color, Surface};
use crate::ui::{PanelId, Ui};

/// One panel's tooltip (`Tooltip`, 0x24 bytes).
#[derive(Debug, Clone)]
pub struct Tooltip {
    /// `m_Text`.
    pub text: String,
    /// `+0x18 _delay`: the time (ms) the window may show.
    delay: u32,
    /// `+0x1c _tooltipDelay` (500 ms by default).
    pub tooltip_delay: u32,
    /// `+0x20 _makeVisible`.
    make_visible: bool,
    /// `+0x21 _displayOnOneLine`.
    pub one_line: bool,
    /// `+0x22 m_bInvalidLayout`: set by the ctor and the owner's `InvalidateLayout`, cleared by
    /// the layout that shows the window (`HideTooltip` leaves it alone, so the window comes
    /// back only after the owner's next `InvalidateLayout`).
    invalid_layout: bool,
}

impl Tooltip {
    pub(crate) fn invalidate_layout(&mut self) {
        self.invalid_layout = true;
    }
}

impl Ui {
    /// `s_TooltipWindow` (re)created: `new TextEntry(NULL, "tooltip")`, `MakePopup(false, true)`,
    /// not editable, multiline, hidden.
    fn tooltip_window_get(&mut self) -> PanelId {
        if let Some(w) = self.tooltip_window.filter(|&w| self.exists(w)) {
            return w;
        }
        let w = TextEntry::create(self, None, "tooltip");
        self.make_popup(w);
        with_te(self, w, |te, _| {
            te.set_editable(false);
            te.set_multiline(true);
        });
        self.set_visible(w, false);
        self.tooltip_window = Some(w);
        w
    }

    /// `Panel::GetTooltip()` (created on demand) and `Tooltip::SetText(text)`.
    pub fn set_tooltip_text(&mut self, panel: PanelId, text: &str) {
        self.tooltip_window_get();
        let Some(n) = self.node_mut(panel) else { return };
        let t = n.tooltip.get_or_insert_with(|| Tooltip { text: String::new(), delay: 0, tooltip_delay: 500, make_visible: false, one_line: false, invalid_layout: true });
        t.text = text.to_owned();
    }

    /// `Tooltip::SetTooltipDelay(ms)`.
    pub fn set_tooltip_delay(&mut self, panel: PanelId, ms: u32) {
        if let Some(t) = self.node_mut(panel).and_then(|n| n.tooltip.as_mut()) {
            t.tooltip_delay = ms;
        }
    }

    /// `Tooltip::SetTooltipFormatToSingleLine`.
    pub fn set_tooltip_one_line(&mut self, panel: PanelId) {
        if let Some(t) = self.node_mut(panel).and_then(|n| n.tooltip.as_mut()) {
            t.one_line = true;
        }
    }

    /// `ResetDelay`: `_delay = GetTimeMillis() + _tooltipDelay`.
    pub(crate) fn tooltip_reset_delay(&mut self, panel: PanelId) {
        let now = self.time_ms;
        if let Some(t) = self.node_mut(panel).and_then(|n| n.tooltip.as_mut()) {
            t.delay = now.wrapping_add(t.tooltip_delay);
        }
    }

    /// `ShowTooltip(panel)`: the window takes the text and the panel as parent, then the layout.
    pub(crate) fn tooltip_show(&mut self, panel: PanelId, s: &mut dyn Surface) {
        let Some(text) = self.node(panel).and_then(|n| n.tooltip.as_ref()).map(|t| t.text.clone()) else { return };
        let w = self.tooltip_window_get();
        with_te(self, w, |te, ui| te.set_text(ui, w, &text, s));
        self.set_parent(w, Some(panel));
        if let Some(t) = self.node_mut(panel).and_then(|n| n.tooltip.as_mut()) {
            t.make_visible = true;
        }
        self.tooltip_perform_layout(panel, s);
    }

    /// `HideTooltip`.
    pub(crate) fn tooltip_hide(&mut self, panel: PanelId) {
        if let Some(w) = self.tooltip_window.filter(|&w| self.exists(w)) {
            self.set_visible(w, false);
        }
        if let Some(t) = self.node_mut(panel).and_then(|n| n.tooltip.as_mut()) {
            t.make_visible = false;
        }
    }

    /// `Tooltip::PerformLayout` (from the owner's `Think` and `ShowTooltip`).
    pub(crate) fn tooltip_perform_layout(&mut self, panel: PanelId, s: &mut dyn Surface) {
        let now = self.time_ms;
        let Some(t) = self.node_mut(panel).and_then(|n| n.tooltip.as_mut()) else { return };
        if !t.invalid_layout || !t.make_visible || t.delay > now {
            return;
        }
        t.invalid_layout = false;
        let one_line = t.one_line;
        let w = self.tooltip_window_get();
        self.set_visible(w, true);
        self.perform_apply_scheme(w, s);
        if let Some(sc) = self.scheme_of(w).cloned() {
            let (bg, fg) = self.node(w).map_or((Color::TRANSPARENT, Color::TRANSPARENT), |n| (n.bg, n.fg));
            let (bg, fg) = (sc.color("SelectionBG", bg), sc.color("BorderSelection", fg));
            let border = sc.border("ToolTipBorder");
            let font = s.font(&sc.file, "DefaultSmall", false, false);
            if let Some(n) = self.node_mut(w) {
                n.bg = bg;
                n.fg = fg;
                n.border = border;
            }
            with_te(self, w, |te, ui| te.set_font(ui, w, font));
        }
        let (cx, cy) = self.cursor_pos();
        with_te(self, w, |te, ui| {
            te.set_multiline(false);
            te.set_to_full_width(ui, w, s);
            if one_line {
                return;
            }
            let (tw, th) = ui.size(w);
            let nw = (2.0 * f64::from(tw) * f64::from(th)).sqrt();
            te.set_multiline(true);
            ui.set_size(w, nw as i32, 0);
            te.set_to_full_height(ui, w, s);
            let (mut tw, mut th) = ui.size(w);
            if tw < 100 && te.num_lines() == 2 {
                te.set_multiline(false);
                te.set_to_full_width(ui, w, s);
            } else {
                while (tw as f32) / (th as f32) < 2.0 {
                    ui.set_size(w, tw + 1, th);
                    te.set_to_full_height(ui, w, s);
                    (tw, th) = ui.size(w);
                }
            }
        });
        let (tw, th) = self.size(w);
        let (sw, sh) = self.screen;
        let (x, y) = if cx >= sw - tw {
            (cx - tw, if cy >= sh - th { cy - th - 20 } else { cy })
        } else {
            let y = cy + 20;
            (cx, if y >= sh - th { y - th - 20 } else { y })
        };
        self.set_pos(w, x, y);
    }
}
