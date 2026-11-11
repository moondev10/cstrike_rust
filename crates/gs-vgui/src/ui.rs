//! The panel tree, message queues, input focus, popups, scheme/think/solve traversal and paint
//! traversal: vgui2's `CVGui`/`VPanel`/`CInputWin32` (docs/re/vgui2-core.md §1–§3), the surface's
//! popup list and traversals (docs/re/vgui-surface.md §6) and vgui_controls `Panel`'s generic
//! behaviour (docs/re/vgui-controls.md §1).

use std::any::Any;
use std::collections::VecDeque;
use std::sync::Arc;

use crate::border::Border;
use crate::keys::{KEY_LAST, MOUSE_LAST};
use crate::msg::{Msg, Value};
use crate::scheme::Scheme;
use crate::surface::{Color, Cursor, Surface};

/// `VPANEL`/`HPanel`: an arena index plus a serial, so handles to deleted panels go dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelId {
    index: u32,
    serial: u32,
}

/// `Panel::m_eIsHDProportional`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HdMode {
    #[default]
    UseParent,
    ForceDisabled,
    Enabled,
}

/// A panel's class behaviour (the vgui_controls subclass). Default methods are `Panel`'s.
pub trait Control: Any {
    /// The class name (`GetClassName`).
    fn class(&self) -> &'static str;
    /// A message the class handles; `false` lets `Panel`'s default run.
    fn on_message(&mut self, _ui: &mut Ui, _me: PanelId, _msg: &Msg, _from: Option<PanelId>, _s: &mut dyn Surface) -> bool {
        false
    }
    /// `ApplySchemeSettings`.
    fn apply_scheme(&mut self, ui: &mut Ui, me: PanelId, scheme: &Scheme, _s: &mut dyn Surface) {
        ui.panel_apply_scheme(me, scheme);
    }
    /// `OnThink` (every frame, before layout).
    fn think(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {}
    /// `PerformLayout`.
    fn perform_layout(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {}
    /// `PaintBackground`.
    fn paint_background(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        ui.panel_paint_background(me, s);
    }
    /// `Paint`.
    fn paint(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {}
    /// `PaintBorder`.
    fn paint_border(&mut self, ui: &mut Ui, me: PanelId, s: &mut dyn Surface) {
        if let Some(b) = ui.node(me).and_then(|n| n.border.clone()) {
            let (w, h) = ui.size(me);
            b.paint_panel(s, w, h);
        }
    }
    /// `PostChildPaint`.
    fn post_child_paint(&mut self, _ui: &mut Ui, _me: PanelId, _s: &mut dyn Surface) {}
    /// `OnSizeChanged` (after the VPanel size changed).
    fn on_size_changed(&mut self, ui: &mut Ui, me: PanelId, _w: i32, _h: i32) {
        ui.invalidate_layout(me);
    }
    /// `RequestInfo("CanBeDefaultButton")`.
    fn can_be_default_button(&self) -> bool {
        false
    }
    /// `HasHotkey(key)` (key already lower-cased): `true` when this panel answers to it.
    fn has_hotkey(&self, _key: char) -> bool {
        false
    }
    /// `ApplySettings(KeyValues*)`: the panel's `.res` block.
    fn apply_settings(&mut self, ui: &mut Ui, me: PanelId, kv: &gs_formats::keyvalues::Block, s: &mut dyn Surface) {
        ui.panel_apply_settings(me, kv, s);
    }
    /// The `Frame` part of a frame-derived class (grips and buttons reach it through this).
    fn frame(&mut self) -> Option<&mut crate::controls::frame::FrameState> {
        None
    }
    /// The `SectionedListPanel` part of a list class (its rows and headers reach it through this).
    fn sectioned(&mut self) -> Option<&mut crate::controls::sectioned::SectionedListPanel> {
        None
    }
    /// The `ListPanel` part of a list class.
    fn list_panel(&mut self) -> Option<&mut crate::controls::list_panel::ListPanel> {
        None
    }
    /// `RequestFocus(direction)` override: `true` when handled (otherwise Panel's).
    fn request_focus_override(&mut self, _ui: &mut Ui, _me: PanelId, _dir: i32) -> bool {
        false
    }
    /// `RequestFocusNext(from)` override; `None` runs Panel's.
    fn request_focus_next(&mut self, _ui: &mut Ui, _me: PanelId, _from: Option<PanelId>) -> Option<bool> {
        None
    }
    /// `RequestFocusPrev(from)` override; `None` runs Panel's.
    fn request_focus_prev(&mut self, _ui: &mut Ui, _me: PanelId, _from: Option<PanelId>) -> Option<bool> {
        None
    }
    /// `Frame::GetDefaultScreenPosition(x, y, w, t)`: where the frame goes when the user
    /// config has no position; `None` = `false`.
    fn default_screen_position(&self, _ui: &Ui) -> Option<(i32, i32, i32, i32)> {
        None
    }
    /// `TabCatchingTextEntry`: every key code but Enter goes to the parent before the control
    /// handles it. `Ui::send` does that while the control is still in its node, because the
    /// parent (the console) edits this very control in its handler.
    fn key_codes_to_parent_first(&self) -> bool {
        false
    }
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// A plain `vgui2::Panel`.
#[derive(Debug, Default)]
pub struct PlainPanel;

impl Control for PlainPanel {
    fn class(&self) -> &'static str {
        "Panel"
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// One panel: the VPanel state and vgui_controls `Panel`'s common fields.
pub struct Node {
    serial: u32,
    parent: Option<PanelId>,
    children: Vec<PanelId>,
    pub pos: (i32, i32),
    size: (i32, i32),
    min_size: (i32, i32),
    zpos: i32,
    pub inset: [i32; 4],
    abs: (i32, i32),
    clip: [i32; 4],
    visible: bool,
    pub enabled: bool,
    popup: bool,
    kb_input: bool,
    mouse_input: bool,
    render_popup_visible: bool,
    // Panel
    pub name: String,
    pub cursor: Cursor,
    pub border: Option<Arc<Border>>,
    pub fg: Color,
    pub bg: Color,
    pub paint_border_enabled: bool,
    pub paint_background_enabled: bool,
    pub paint_enabled: bool,
    pub post_child_paint_enabled: bool,
    pub tab_position: i32,
    needs_layout: bool,
    needs_scheme: bool,
    marked_for_deletion: bool,
    action_targets: Vec<PanelId>,
    proportional: bool,
    pub hd: HdMode,
    /// `m_iScheme`: a scheme index; `None` inherits the parent's.
    pub scheme: Option<usize>,
    /// The popup's `GetCurrentKeyFocus` (its focus-nav group's current panel).
    current_focus: Option<PanelId>,
    /// An `EditablePanel`: children added to it send it their action signals and join its
    /// build group (`EditablePanel::OnChildAdded`).
    pub editable: bool,
    /// The EditablePanel's `FocusNavGroup`.
    pub nav: crate::editable::FocusNav,
    /// `BuildGroup::_panelDar`: panels `.res` sub-keys are matched against.
    pub build_group: Vec<PanelId>,
    /// `_baseWide/_baseTall` (EditablePanel auto-resize reference).
    pub base_size: (i32, i32),
    /// `_pinCorner` (0 top-left, 1 top-right, 2 bottom-left, 3 bottom-right).
    pub pin_corner: i32,
    /// `_autoResizeDirection` (0 none, 1 right, 2 down, 3 down and right).
    pub auto_resize: i32,
    /// `Button::IsDepressed`, mirrored so a parent can read it while the button's control is
    /// busy (a ScrollBar polling its arrows from the button's own forwarded tick).
    pub depressed: bool,
    ctrl: Option<Box<dyn Control>>,
    /// `size_change_pending`: `SetSize` while the control was out of the node: its
    /// `OnSizeChanged` is still owed.
    size_change_pending: bool,
    /// `m_pTooltips` (created on demand).
    pub tooltip: Option<crate::tooltip::Tooltip>,
    /// `m_pszConfigName` / `m_iConfigID` of `LoadUserConfig`.
    pub(crate) config: Option<(String, i32)>,
}

impl Node {
    /// `GetMinimumSize`.
    pub fn min_size(&self) -> (i32, i32) {
        self.min_size
    }
}

struct Queued {
    to: PanelId,
    from: Option<PanelId>,
    msg: Msg,
    arrival: f32,
    id: u32,
}

struct Tick {
    panel: PanelId,
    interval: u32,
    next: u32,
}

/// `CInputWin32`'s default context.
struct Input {
    cursor: (i32, i32),
    mouse_over: Option<PanelId>,
    old_mouse_focus: Option<PanelId>,
    mouse_focus: Option<PanelId>,
    capture: Option<PanelId>,
    key_focus: Option<PanelId>,
    app_modal: Option<PanelId>,
    mouse_down: [bool; MOUSE_LAST as usize],
    key_down: [bool; KEY_LAST as usize],
}

impl Default for Input {
    fn default() -> Self {
        Self {
            cursor: (0, 0),
            mouse_over: None,
            old_mouse_focus: None,
            mouse_focus: None,
            capture: None,
            key_focus: None,
            app_modal: None,
            mouse_down: [false; MOUSE_LAST as usize],
            key_down: [false; KEY_LAST as usize],
        }
    }
}

/// vgui2 + the surface's popup list.
pub struct Ui {
    nodes: Vec<Option<Node>>,
    next_serial: u32,
    embedded: PanelId,
    popups: Vec<PanelId>,
    /// `RestrictPaintToSinglePanel` target.
    restrict_paint: Option<PanelId>,
    queue: VecDeque<Queued>,
    secondary: VecDeque<Queued>,
    delayed: Vec<Queued>,
    in_dispatch: bool,
    /// `FocusNavGroup::RequestFocusNext`'s recursion depth (parent hops stop past 14).
    pub(crate) nav_depth: i32,
    next_msg_id: u32,
    ticks: Vec<Tick>,
    input: Input,
    schemes: Vec<Scheme>,
    /// `ISystem::GetTimeMillis`, set by the host each frame.
    pub time_ms: u32,
    /// `GetScreenSize`, set by the host.
    pub screen: (i32, i32),
    /// `GetProportionalBase` / HD base.
    pub proportional_base: (i32, i32),
    pub hd_base: (i32, i32),
    /// Whether the cursor is visible (`IsCursorVisible`): mouse focus is only computed then.
    pub cursor_visible: bool,
    /// The game window has focus (`HasFocus`).
    pub window_focus: bool,
    /// `ILocalize`: the token table (`#token` text).
    pub localize: gs_draw::localize::Localize,
    /// `ISystem`'s user config (`SetUserConfigFile`): `None` until the host sets one.
    pub user_config: Option<crate::keyvalues::KeyValues>,
    /// `s_TooltipWindow` (a PHandle).
    pub(crate) tooltip_window: Option<PanelId>,
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

impl Ui {
    pub fn new() -> Self {
        let mut ui = Self {
            nodes: Vec::new(),
            next_serial: 1,
            embedded: PanelId { index: 0, serial: 0 },
            popups: Vec::new(),
            restrict_paint: None,
            queue: VecDeque::new(),
            secondary: VecDeque::new(),
            delayed: Vec::new(),
            in_dispatch: false,
            nav_depth: 0,
            next_msg_id: 0,
            ticks: Vec::new(),
            input: Input::default(),
            schemes: Vec::new(),
            time_ms: 0,
            screen: (640, 480),
            proportional_base: (640, 480),
            hd_base: (1280, 720),
            cursor_visible: true,
            window_focus: true,
            localize: gs_draw::localize::Localize::default(),
            user_config: None,
            tooltip_window: None,
        };
        ui.embedded = ui.create(None, "", Box::new(PlainPanel));
        ui
    }

    // --- arena ---------------------------------------------------------------------------

    /// The surface's embedded (root) panel.
    pub fn embedded(&self) -> PanelId {
        self.embedded
    }

    /// `new Panel(parent, name)`: position (0, 0), size 64×24.
    pub fn create(&mut self, parent: Option<PanelId>, name: &str, ctrl: Box<dyn Control>) -> PanelId {
        let serial = self.next_serial;
        self.next_serial += 1;
        let node = Node {
            serial,
            parent: None,
            children: Vec::new(),
            pos: (0, 0),
            size: (64, 24),
            min_size: (0, 0),
            zpos: 0,
            inset: [0; 4],
            abs: (0, 0),
            clip: [0; 4],
            visible: true,
            enabled: true,
            popup: false,
            kb_input: true,
            mouse_input: true,
            render_popup_visible: false,
            name: name.to_owned(),
            cursor: Cursor::Arrow,
            border: None,
            fg: Color::TRANSPARENT,
            bg: Color::TRANSPARENT,
            paint_border_enabled: true,
            paint_background_enabled: true,
            paint_enabled: true,
            post_child_paint_enabled: false,
            tab_position: 0,
            needs_layout: true,
            needs_scheme: true,
            marked_for_deletion: false,
            action_targets: Vec::new(),
            proportional: false,
            hd: HdMode::UseParent,
            scheme: None,
            current_focus: None,
            editable: false,
            nav: crate::editable::FocusNav::default(),
            build_group: Vec::new(),
            base_size: (0, 0),
            pin_corner: 0,
            auto_resize: 0,
            depressed: false,
            ctrl: Some(ctrl),
            size_change_pending: false,
            tooltip: None,
            config: None,
        };
        let index = match self.nodes.iter().position(Option::is_none) {
            Some(i) => {
                self.nodes[i] = Some(node);
                i
            }
            None => {
                self.nodes.push(Some(node));
                self.nodes.len() - 1
            }
        };
        let id = PanelId { index: index as u32, serial };
        if let Some(p) = parent {
            self.set_parent(id, Some(p));
        }
        id
    }

    pub fn node(&self, id: PanelId) -> Option<&Node> {
        self.nodes.get(id.index as usize)?.as_ref().filter(|n| n.serial == id.serial)
    }

    pub fn node_mut(&mut self, id: PanelId) -> Option<&mut Node> {
        self.nodes.get_mut(id.index as usize)?.as_mut().filter(|n| n.serial == id.serial)
    }

    pub fn exists(&self, id: PanelId) -> bool {
        self.node(id).is_some()
    }

    /// The control of `id` as `T`, when it is one.
    pub fn ctrl<T: Control>(&mut self, id: PanelId) -> Option<&mut T> {
        self.node_mut(id)?.ctrl.as_mut()?.as_any_mut().downcast_mut::<T>()
    }

    /// Replaces the control of `id` by `f(old)`: a subclass wrapping a created base control.
    pub fn replace_ctrl(&mut self, id: PanelId, f: impl FnOnce(Box<dyn Control>) -> Box<dyn Control>) {
        if let Some(n) = self.node_mut(id)
            && let Some(old) = n.ctrl.take()
        {
            n.ctrl = Some(f(old));
        }
    }

    /// Runs `f` with the control taken out of its node (so it can use the whole `Ui`).
    pub fn with_ctrl<R>(&mut self, id: PanelId, f: impl FnOnce(&mut dyn Control, &mut Ui) -> R) -> Option<R> {
        let mut ctrl = self.node_mut(id)?.ctrl.take()?;
        let r = f(ctrl.as_mut(), self);
        if let Some(n) = self.node_mut(id)
            && n.ctrl.is_none()
        {
            n.ctrl = Some(ctrl);
            self.flush_size_change(id);
        }
        Some(r)
    }

    /// A `SetSize` on a panel from inside its own method: its `OnSizeChanged` now.
    fn flush_size_change(&mut self, id: PanelId) {
        loop {
            let Some(n) = self.node_mut(id) else { return };
            if !std::mem::take(&mut n.size_change_pending) {
                return;
            }
            let (w, h) = n.size;
            let Some(mut c) = n.ctrl.take() else {
                n.size_change_pending = true;
                return;
            };
            c.on_size_changed(self, id, w, h);
            if let Some(n) = self.node_mut(id)
                && n.ctrl.is_none()
            {
                n.ctrl = Some(c);
            }
        }
    }

    /// `MarkForDeletion` + the deletion: the subtree goes, focus/capture pointers are dropped.
    pub fn delete(&mut self, id: PanelId) {
        let Some(n) = self.node(id) else { return };
        let children = n.children.clone();
        for c in children {
            self.delete(c);
        }
        if let Some(parent) = self.node(id).and_then(|n| n.parent)
            && let Some(p) = self.node_mut(parent)
        {
            p.children.retain(|&c| c != id);
        }
        self.popups.retain(|&p| p != id);
        self.ticks.retain(|t| t.panel != id);
        // PanelDeleted.
        let i = &mut self.input;
        for slot in [&mut i.key_focus, &mut i.mouse_over, &mut i.old_mouse_focus, &mut i.mouse_focus, &mut i.capture, &mut i.app_modal] {
            if *slot == Some(id) {
                *slot = None;
            }
        }
        if self.restrict_paint == Some(id) {
            self.restrict_paint = None;
        }
        self.nodes[id.index as usize] = None;
    }

    // --- VPanel state ---------------------------------------------------------------------

    pub fn parent(&self, id: PanelId) -> Option<PanelId> {
        self.node(id).and_then(|n| n.parent)
    }

    pub fn children(&self, id: PanelId) -> Vec<PanelId> {
        self.node(id).map(|n| n.children.clone()).unwrap_or_default()
    }

    /// `FindChildByName(name)`: the first direct child whose name matches (case-insensitive).
    pub fn find_child_by_name(&self, id: PanelId, name: &str) -> Option<PanelId> {
        let n = self.node(id)?;
        n.children.iter().copied().find(|&c| self.node(c).is_some_and(|cn| cn.name.eq_ignore_ascii_case(name)))
    }

    pub fn size(&self, id: PanelId) -> (i32, i32) {
        self.node(id).map_or((0, 0), |n| n.size)
    }

    pub fn pos(&self, id: PanelId) -> (i32, i32) {
        self.node(id).map_or((0, 0), |n| n.pos)
    }

    /// `GetAbsPos` from the last solve.
    pub fn abs_pos(&self, id: PanelId) -> (i32, i32) {
        self.node(id).map_or((0, 0), |n| n.abs)
    }

    pub fn is_visible(&self, id: PanelId) -> bool {
        self.node(id).is_some_and(|n| n.visible)
    }

    pub fn is_enabled(&self, id: PanelId) -> bool {
        self.node(id).is_some_and(|n| n.enabled)
    }

    pub fn is_popup(&self, id: PanelId) -> bool {
        self.node(id).is_some_and(|n| n.popup)
    }

    pub fn is_proportional(&self, id: PanelId) -> bool {
        self.node(id).is_some_and(|n| n.proportional)
    }

    pub fn is_mouse_input_enabled(&self, id: PanelId) -> bool {
        self.node(id).is_some_and(|n| n.mouse_input)
    }

    pub fn is_keyboard_input_enabled(&self, id: PanelId) -> bool {
        self.node(id).is_some_and(|n| n.kb_input)
    }

    pub fn set_pos(&mut self, id: PanelId, x: i32, y: i32) {
        if let Some(n) = self.node_mut(id) {
            n.pos = (x, y);
        }
    }

    /// `VPanel::SetSize` (clamped to the minimum size; `OnSizeChanged` when it changed).
    pub fn set_size(&mut self, id: PanelId, w: i32, h: i32) {
        let Some(n) = self.node_mut(id) else { return };
        let (w, h) = (w.max(n.min_size.0), h.max(n.min_size.1));
        if (w, h) == n.size {
            return;
        }
        n.size = (w, h);
        if self.with_ctrl(id, |c, ui| c.on_size_changed(ui, id, w, h)).is_none()
            && let Some(n) = self.node_mut(id)
        {
            n.size_change_pending = true;
        }
        if self.node(id).is_some_and(|n| n.editable) {
            self.editable_size_changed(id, w, h);
        }
    }

    pub fn set_bounds(&mut self, id: PanelId, x: i32, y: i32, w: i32, h: i32) {
        self.set_pos(id, x, y);
        self.set_size(id, w, h);
    }

    pub fn set_min_size(&mut self, id: PanelId, w: i32, h: i32) {
        let Some(n) = self.node_mut(id) else { return };
        n.min_size = (w, h);
        let (nw, nh) = (n.size.0.max(w), n.size.1.max(h));
        if (nw, nh) != n.size {
            self.set_size(id, nw, nh);
        }
    }

    /// `VPanel::SetVisible`.
    pub fn set_visible(&mut self, id: PanelId, state: bool) {
        if let Some(n) = self.node_mut(id) {
            n.visible = state;
        }
    }

    pub fn set_enabled(&mut self, id: PanelId, state: bool) {
        if let Some(n) = self.node_mut(id)
            && n.enabled != state
        {
            n.enabled = state;
            self.invalidate_layout(id);
        }
    }

    pub fn set_keyboard_input_enabled(&mut self, id: PanelId, state: bool) {
        if let Some(n) = self.node_mut(id) {
            n.kb_input = state;
        }
    }

    pub fn set_mouse_input_enabled(&mut self, id: PanelId, state: bool) {
        if let Some(n) = self.node_mut(id) {
            n.mouse_input = state;
        }
    }

    pub fn set_proportional(&mut self, id: PanelId, state: bool) {
        let Some(n) = self.node_mut(id) else { return };
        n.proportional = state;
        n.needs_layout = true;
        for c in self.children(id) {
            self.set_proportional(c, state);
        }
    }

    /// `SetHDProportional`.
    pub fn set_hd_proportional(&mut self, id: PanelId, state: bool) {
        let Some(n) = self.node_mut(id) else { return };
        n.hd = if state { HdMode::Enabled } else { HdMode::UseParent };
        if state && !n.proportional {
            self.set_proportional(id, true);
        }
    }

    /// `Panel::SetParent` → `VPanel::SetParent`: re-sorted by z-pos; proportional and input
    /// flags inherited by non-popups.
    pub fn set_parent(&mut self, id: PanelId, parent: Option<PanelId>) {
        let old = self.parent(id);
        if parent == Some(id) || parent == old {
            return;
        }
        if let Some(o) = old {
            if let Some(n) = self.node_mut(o) {
                n.children.retain(|&c| c != id);
            }
            if let Some(n) = self.node_mut(id) {
                n.parent = None;
            }
        }
        let Some(p) = parent else { return };
        if !self.exists(p) {
            return;
        }
        if let Some(n) = self.node_mut(id) {
            n.parent = Some(p);
        }
        if let Some(pn) = self.node_mut(p)
            && !pn.children.contains(&id)
        {
            pn.children.push(id);
        }
        let z = self.node(id).map_or(0, |n| n.zpos);
        self.set_zpos(id, z);
        if self.node(p).is_some_and(|n| n.editable) {
            if let Some(pn) = self.node_mut(p) {
                pn.build_group.push(id);
            }
            self.add_action_signal_target(id, p);
        }
        if !self.is_popup(id) {
            let (prop, kb, mouse) = self.node(p).map_or((false, true, true), |n| (n.proportional, n.kb_input, n.mouse_input));
            self.set_proportional(id, prop);
            self.set_keyboard_input_enabled(id, kb);
            self.set_mouse_input_enabled(id, mouse);
        }
    }

    /// `VPanel::SetZPos`: one bubble pass among the siblings.
    pub fn set_zpos(&mut self, id: PanelId, z: i32) {
        let Some(n) = self.node_mut(id) else { return };
        n.zpos = z;
        let Some(p) = n.parent else { return };
        let zs: Vec<i32> = self.children(p).iter().map(|&c| self.node(c).map_or(0, |n| n.zpos)).collect();
        let Some(pn) = self.node_mut(p) else { return };
        let Some(mut i) = pn.children.iter().position(|&c| c == id) else { return };
        let mut zs = zs;
        loop {
            if i > 0 && zs[i - 1] > z {
                pn.children.swap(i - 1, i);
                zs.swap(i - 1, i);
                i -= 1;
            } else if i + 1 < pn.children.len() && zs[i + 1] < z {
                pn.children.swap(i + 1, i);
                zs.swap(i + 1, i);
                i += 1;
            } else {
                break;
            }
        }
    }

    /// `VPanel::MoveToFront`: the surface moves a popup to the front of its list; among the
    /// siblings it goes last before any of higher z.
    pub fn move_to_front(&mut self, id: PanelId) {
        self.move_popup_to_front(id);
        let Some(p) = self.parent(id) else { return };
        let zs: Vec<(PanelId, i32)> = self.children(p).iter().map(|&c| (c, self.node(c).map_or(0, |n| n.zpos))).collect();
        let Some(pn) = self.node_mut(p) else { return };
        pn.children.retain(|&c| c != id);
        pn.children.push(id);
        let z = zs.iter().find(|(c, _)| *c == id).map_or(0, |(_, z)| *z);
        let zof = |c: PanelId| zs.iter().find(|(x, _)| *x == c).map_or(0, |(_, z)| *z);
        let mut i = pn.children.len() - 1;
        while i > 0 && zof(pn.children[i - 1]) > z {
            pn.children.swap(i - 1, i);
            i -= 1;
        }
    }

    /// `VPanel::MoveToBack` (the surface is not involved).
    pub fn move_to_back(&mut self, id: PanelId) {
        let Some(p) = self.parent(id) else { return };
        let zs: Vec<(PanelId, i32)> = self.children(p).iter().map(|&c| (c, self.node(c).map_or(0, |n| n.zpos))).collect();
        let zof = |c: PanelId| zs.iter().find(|(x, _)| *x == c).map_or(0, |(_, z)| *z);
        let z = zof(id);
        let Some(pn) = self.node_mut(p) else { return };
        pn.children.retain(|&c| c != id);
        pn.children.insert(0, id);
        let mut i = 0;
        while i + 1 < pn.children.len() && zof(pn.children[i + 1]) < z {
            pn.children.swap(i, i + 1);
            i += 1;
        }
    }

    // --- surface popups -------------------------------------------------------------------

    /// `Panel::MakePopup` → `CreatePopup`.
    pub fn make_popup(&mut self, id: PanelId) {
        if self.parent(id).is_none() {
            let root = self.embedded;
            self.set_parent(id, Some(root));
        }
        if let Some(n) = self.node_mut(id) {
            n.popup = true;
            n.kb_input = true;
            n.mouse_input = true;
        }
        if !self.popups.contains(&id) {
            self.popups.push(id);
        }
    }

    /// `MovePopupToFront`.
    pub fn move_popup_to_front(&mut self, id: PanelId) {
        if self.popups.last() == Some(&id) {
            return;
        }
        if let Some(i) = self.popups.iter().rposition(|&p| p == id) {
            self.popups.remove(i);
            self.popups.push(id);
        }
    }

    /// `MovePopupToBack`.
    pub fn move_popup_to_back(&mut self, id: PanelId) {
        if let Some(i) = self.popups.iter().position(|&p| p == id) {
            self.popups.remove(i);
            self.popups.insert(0, id);
        }
    }

    /// `SetTopLevelFocus`: the popup containing `id` comes to the front.
    pub fn set_top_level_focus(&mut self, id: PanelId) {
        let mut p = Some(id);
        while let Some(q) = p {
            if self.is_popup(q) {
                self.move_popup_to_front(q);
                return;
            }
            p = self.parent(q);
        }
    }

    /// `RestrictPaintToSinglePanel`.
    pub fn restrict_paint_to(&mut self, id: Option<PanelId>) {
        self.restrict_paint = id;
    }

    /// `HasParent`: `id` or one of its ancestors is `ancestor`.
    pub fn has_parent(&self, id: PanelId, ancestor: PanelId) -> bool {
        let mut p = Some(id);
        while let Some(q) = p {
            if q == ancestor {
                return true;
            }
            p = self.parent(q);
        }
        false
    }

    // --- panel generics (vgui_controls Panel) -------------------------------------------

    pub fn add_scheme(&mut self, scheme: Scheme) -> usize {
        self.schemes.push(scheme);
        self.schemes.len() - 1
    }

    /// The scheme a panel uses: its own, else its parent's, else the default (index 0).
    pub fn scheme_index(&self, id: PanelId) -> usize {
        let mut p = Some(id);
        while let Some(q) = p {
            if let Some(s) = self.node(q).and_then(|n| n.scheme) {
                return s;
            }
            p = self.parent(q);
        }
        0
    }

    pub fn scheme_of(&self, id: PanelId) -> Option<&Scheme> {
        self.schemes.get(self.scheme_index(id))
    }

    /// `Panel::ApplySchemeSettings`: fg `FgColor`, bg `BgColor`.
    pub fn panel_apply_scheme(&mut self, id: PanelId, scheme: &Scheme) {
        let fg = scheme.color("FgColor", Color::WHITE);
        let bg = scheme.color("BgColor", Color::WHITE);
        if let Some(n) = self.node_mut(id) {
            n.fg = fg;
            n.bg = bg;
            n.needs_scheme = false;
        }
    }

    /// `Panel::PaintBackground`: the full rect in the background colour.
    pub fn panel_paint_background(&mut self, id: PanelId, s: &mut dyn Surface) {
        let (w, h) = self.size(id);
        let bg = self.node(id).map_or(Color::TRANSPARENT, |n| n.bg);
        s.draw_set_color(bg);
        s.draw_filled_rect(0, 0, w, h);
    }

    /// `InvalidateLayout()`.
    pub fn invalidate_layout(&mut self, id: PanelId) {
        if let Some(n) = self.node_mut(id) {
            n.needs_layout = true;
            if let Some(t) = n.tooltip.as_mut() {
                t.invalidate_layout();
            }
        }
    }

    /// Marks the panel for `ApplySchemeSettings` at the next scheme traversal.
    pub fn reapply_scheme(&mut self, id: PanelId) {
        if let Some(n) = self.node_mut(id) {
            n.needs_scheme = true;
        }
    }

    /// `InvalidateLayout(layoutNow, reloadScheme)`.
    pub fn invalidate_layout_ex(&mut self, id: PanelId, layout_now: bool, reload_scheme: bool, s: &mut dyn Surface) {
        self.invalidate_layout(id);
        if reload_scheme {
            if let Some(n) = self.node_mut(id) {
                n.needs_scheme = true;
            }
            for c in self.children(id) {
                self.invalidate_layout_ex(c, layout_now, true, s);
            }
            self.perform_apply_scheme(id, s);
        }
        if layout_now {
            self.perform_layout(id, s);
        }
    }

    /// `PerformLayout` now (the `_needsLayout` flag cleared first).
    pub fn perform_layout(&mut self, id: PanelId, s: &mut dyn Surface) {
        if let Some(n) = self.node_mut(id) {
            n.needs_layout = false;
        }
        self.with_ctrl(id, |c, ui| c.perform_layout(ui, id, s));
    }

    pub(crate) fn perform_apply_scheme(&mut self, id: PanelId, s: &mut dyn Surface) {
        if !self.node(id).is_some_and(|n| n.needs_scheme) {
            return;
        }
        let Some(scheme) = self.scheme_of(id).cloned() else { return };
        self.with_ctrl(id, |c, ui| c.apply_scheme(ui, id, &scheme, s));
        if let Some(n) = self.node_mut(id) {
            n.needs_scheme = false;
        }
    }

    /// `surface()->GetProportionalBase` (slot 82) / `GetHDProportionalBase` (slot 104): the base
    /// resolution, capped at the screen.
    pub fn base_size(&self, hd: bool) -> (i32, i32) {
        let (w, h) = if hd { self.hd_base } else { self.proportional_base };
        (w.min(self.screen.0), h.min(self.screen.1))
    }

    /// `Panel::GetProportionalBase` (gu@0x10048340): the HD base when `id`, or the first parent
    /// with a mode of its own, is HD-proportional; the normal base otherwise.
    pub fn proportional_base_of(&self, id: PanelId) -> (i32, i32) {
        self.base_size(self.is_hd_proportional(id))
    }

    /// `GetProportionalScaledValue` with the HD search up the parents.
    pub fn psv(&self, id: PanelId, v: i32) -> i32 {
        let sh = f64::from(self.screen.1);
        if self.is_hd_proportional(id) {
            return (sh / f64::from(self.base_size(true).1) * f64::from(v)) as i32;
        }
        if self.is_proportional(id) { (sh / f64::from(self.base_size(false).1) * f64::from(v)) as i32 } else { v }
    }

    /// Whether `id` (or a parent before a FORCE_DISABLED one) is HD-proportional.
    pub fn is_hd_proportional(&self, id: PanelId) -> bool {
        let mut p = Some(id);
        while let Some(q) = p {
            match self.node(q).map(|n| n.hd) {
                Some(HdMode::Enabled) => return true,
                Some(HdMode::ForceDisabled) => return false,
                _ => p = self.parent(q),
            }
        }
        false
    }

    /// `AddActionSignalTarget` (duplicates ignored).
    pub fn add_action_signal_target(&mut self, id: PanelId, target: PanelId) {
        if let Some(n) = self.node_mut(id)
            && !n.action_targets.contains(&target)
        {
            n.action_targets.push(target);
        }
    }

    /// `RemoveActionSignalTarget`.
    pub fn remove_action_signal_target(&mut self, id: PanelId, target: PanelId) {
        if let Some(n) = self.node_mut(id) {
            n.action_targets.retain(|&t| t != target);
        }
    }

    /// `PostActionSignal`: a copy with `panel = id` to every target, last added first.
    pub fn post_action_signal(&mut self, id: PanelId, mut msg: Msg) {
        msg.set("panel", Value::Panel(id));
        let targets = self.node(id).map(|n| n.action_targets.clone()).unwrap_or_default();
        for &t in targets.iter().rev() {
            self.post(t, msg.clone(), Some(id), 0.0);
        }
    }

    /// `Panel::RequestFocus` → the `OnRequestFocus` chain: every EditablePanel on the way records
    /// the sub-focus in its nav group and passes itself up; the popup at the top keeps the panel
    /// as its current focus (applied by the input system's next `RunFrame`).
    pub fn request_focus(&mut self, id: PanelId) {
        if self.with_ctrl(id, |c, ui| c.request_focus_override(ui, id, 0)).unwrap_or(false) {
            return;
        }
        self.request_focus_base(id);
    }

    /// `Panel::RequestFocus` (no class override).
    pub fn request_focus_base(&mut self, id: PanelId) {
        let mut sub = id;
        let mut default: Option<PanelId> = None;
        let mut p = Some(id);
        while let Some(q) = p {
            if self.node(q).is_some_and(|n| n.editable) {
                if !self.is_popup(sub) {
                    default = self.nav_set_current_focus(q, sub, default);
                }
                sub = q;
            }
            if self.is_popup(q) || self.parent(q).is_none() {
                if let Some(n) = self.node_mut(q) {
                    n.current_focus = Some(id);
                }
                return;
            }
            p = self.parent(q);
        }
    }

    /// The panel with keyboard focus (`input()->GetFocus`).
    pub fn focus(&self) -> Option<PanelId> {
        self.input.key_focus
    }

    pub fn mouse_over(&self) -> Option<PanelId> {
        self.input.mouse_over
    }

    pub fn cursor_pos(&self) -> (i32, i32) {
        self.input.cursor
    }

    pub fn is_key_down(&self, code: i32) -> bool {
        usize::try_from(code).ok().and_then(|c| self.input.key_down.get(c).copied()).unwrap_or(false)
    }

    pub fn is_mouse_down(&self, code: i32) -> bool {
        usize::try_from(code).ok().and_then(|c| self.input.mouse_down.get(c).copied()).unwrap_or(false)
    }

    /// `SetMouseCapture`.
    pub fn set_mouse_capture(&mut self, panel: Option<PanelId>) {
        if !self.is_child_of_modal(panel) {
            return;
        }
        if let Some(c) = self.input.capture
            && panel != Some(c)
        {
            self.post(c, Msg::new("MouseCaptureLost"), None, 0.0);
        }
        self.input.capture = panel;
    }

    pub fn mouse_capture(&self) -> Option<PanelId> {
        self.input.capture
    }

    pub fn set_app_modal(&mut self, panel: Option<PanelId>) {
        self.input.app_modal = panel;
    }

    pub fn app_modal(&self) -> Option<PanelId> {
        self.input.app_modal
    }

    fn is_child_of_modal(&self, p: Option<PanelId>) -> bool {
        match (p, self.input.app_modal) {
            (None, _) | (_, None) => true,
            (Some(p), Some(m)) => self.has_parent(p, m),
        }
    }

    // --- messages ---------------------------------------------------------------------------

    /// `IVGui::PostMessage`.
    pub fn post(&mut self, to: PanelId, msg: Msg, from: Option<PanelId>, delay: f32) {
        let id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.wrapping_add(1);
        let mut q = Queued { to, from, msg, arrival: 0.0, id };
        if delay > 0.0 {
            q.arrival = self.time_ms as f32 + delay * 1000.0;
            self.delayed.push(q);
        } else if self.in_dispatch {
            self.secondary.push_back(q);
        } else {
            self.queue.push_back(q);
        }
    }

    /// `ipanel()->SendMessage`: synchronous delivery.
    pub fn send(&mut self, to: PanelId, msg: &Msg, from: Option<PanelId>, s: &mut dyn Surface) {
        if !self.exists(to) {
            return;
        }
        // TabCatchingTextEntry::OnKeyCodeTyped: PAD_ENTER counts as ENTER, and any other code is
        // the parent's first (a direct call in the original).
        if msg.is("KeyCodeTyped") && self.node(to).and_then(|n| n.ctrl.as_deref()).is_some_and(|c| c.key_codes_to_parent_first()) {
            use crate::keys::{KEY_ENTER, KEY_PAD_ENTER};
            let mut code = msg.get_int("code");
            if code == KEY_PAD_ENTER {
                code = KEY_ENTER;
            }
            if code != KEY_ENTER
                && let Some(p) = self.parent(to)
            {
                self.send(p, &Msg::new("KeyCodeTyped").int("code", code), Some(to), s);
            }
            if !self.exists(to) {
                return;
            }
            let own = Msg::new("KeyCodeTyped").int("code", code);
            let handled = self.with_ctrl(to, |c, ui| c.on_message(ui, to, &own, from, s)).unwrap_or(false);
            if !handled {
                self.panel_default_message(to, &own, s);
            }
            return;
        }
        let handled = self.with_ctrl(to, |c, ui| c.on_message(ui, to, msg, from, s)).unwrap_or(false);
        if !handled {
            self.panel_default_message(to, msg, s);
        }
    }

    /// `CallParentFunction`.
    pub fn call_parent(&mut self, id: PanelId, msg: &Msg, s: &mut dyn Surface) {
        if let Some(p) = self.parent(id) {
            self.send(p, msg, Some(id), s);
        }
    }

    /// `Panel::OnMessage` defaults (`EditablePanel`'s first for editable panels): key codes
    /// navigate or go to the parent, the other key/tick messages go to the parent.
    fn panel_default_message(&mut self, id: PanelId, msg: &Msg, s: &mut dyn Surface) {
        if self.node(id).is_some_and(|n| n.editable) && self.editable_default_message(id, msg, s) {
            return;
        }
        const TO_PARENT: [&str; 5] = ["KeyCodePressed", "KeyTyped", "KeyCodeReleased", "MouseWheeled", "KeyFocusTicked"];
        if msg.is("KeyCodeTyped") {
            self.panel_on_key_code_typed(id, msg.get_int("code"), s);
        } else if TO_PARENT.iter().any(|n| msg.is(n)) {
            self.call_parent(id, msg, s);
        } else if msg.is("MouseFocusTicked") || msg.is("OnMouseFocusTicked") {
            self.call_parent(id, &Msg::new("OnMouseFocusTicked"), s);
        } else if msg.is("Delete") {
            self.delete(id);
        }
    }

    /// `Panel::OnKeyCodeTyped(code)`: Tab/arrows move the focus (Shift or left Ctrl = back),
    /// other codes go to the parent.
    pub fn panel_on_key_code_typed(&mut self, id: PanelId, code: i32, s: &mut dyn Surface) {
        use crate::keys::{KEY_DOWN, KEY_LCONTROL, KEY_LEFT, KEY_LSHIFT, KEY_RIGHT, KEY_RSHIFT, KEY_TAB, KEY_UP};
        let back = self.is_key_down(KEY_LSHIFT) || self.is_key_down(KEY_RSHIFT) || self.is_key_down(KEY_LCONTROL);
        match code {
            KEY_TAB if back => {
                self.panel_request_focus_prev(id, None);
            }
            KEY_TAB | KEY_DOWN | KEY_RIGHT => {
                self.panel_request_focus_next(id, None);
            }
            KEY_UP | KEY_LEFT => {
                self.panel_request_focus_prev(id, None);
            }
            _ => self.call_parent(id, &Msg::new("KeyCodeTyped").int("code", code), s),
        }
    }

    fn dispatch(&mut self, s: &mut dyn Surface) {
        self.in_dispatch = true;
        let now = self.time_ms as f32;
        let mut use_delayed = !self.delayed.is_empty();
        loop {
            let item = if let Some(q) = self.secondary.pop_front() {
                q
            } else if use_delayed {
                let best = self
                    .delayed
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.arrival.total_cmp(&b.arrival).then(a.id.cmp(&b.id)))
                    .map(|(i, q)| (i, q.arrival));
                match best {
                    Some((i, arrival)) if arrival <= now => self.delayed.remove(i),
                    _ => {
                        use_delayed = false;
                        continue;
                    }
                }
            } else if let Some(q) = self.queue.pop_front() {
                q
            } else {
                break;
            };
            if self.exists(item.to) {
                self.deliver(item.to, &item.msg, item.from, s);
            }
        }
        self.in_dispatch = false;
    }

    /// A queued message: the input gates of `Panel::Internal*`, then the class handler.
    fn deliver(&mut self, to: PanelId, msg: &Msg, from: Option<PanelId>, s: &mut dyn Surface) {
        const MOUSE: [&str; 7] = ["MousePressed", "MouseDoublePressed", "MouseReleased", "CursorEntered", "CursorExited", "MouseWheeled", "CursorMoved"];
        if MOUSE.iter().any(|m| msg.is(m)) {
            let Some(n) = self.node(to) else { return };
            if matches!(n.cursor, Cursor::User) || !n.mouse_input {
                return;
            }
            if n.tooltip.is_some() {
                if msg.is("CursorEntered") {
                    self.tooltip_reset_delay(to);
                    self.tooltip_show(to, s);
                } else if msg.is("CursorExited") {
                    self.tooltip_hide(to);
                } else if msg.is("CursorMoved") {
                    self.tooltip_show(to, s);
                }
            }
        }
        self.send(to, msg, from, s);
    }

    /// `AddTickSignal`.
    pub fn add_tick_signal(&mut self, panel: PanelId, interval_ms: u32) {
        let next = self.time_ms.wrapping_add(interval_ms);
        match self.ticks.iter_mut().find(|t| t.panel == panel) {
            Some(t) => {
                t.interval = interval_ms;
                t.next = next;
            }
            None => self.ticks.push(Tick { panel, interval: interval_ms, next }),
        }
    }

    pub fn remove_tick_signal(&mut self, panel: PanelId) {
        if let Some(i) = self.ticks.iter().position(|t| t.panel == panel) {
            self.ticks.remove(i);
        }
    }

    // --- input entry points (IInputInternal) ---------------------------------------------

    fn post_to_capture_or_focus(&mut self, msg: Msg, modal_check: bool) {
        let i = &self.input;
        if let Some(cap) = i.capture.filter(|&c| self.is_child_of_modal(Some(c))) {
            if let Some(over) = i.mouse_over
                && over != cap
                && self.has_parent(over, cap)
            {
                self.post(over, msg.clone(), None, 0.0);
            }
            self.post(cap, msg, None, 0.0);
        } else if let Some(f) = i.mouse_focus
            && (!modal_check || self.is_child_of_modal(Some(f)))
        {
            self.post(f, msg, None, 0.0);
        }
    }

    pub fn cursor_moved(&mut self, x: i32, y: i32) {
        if (x, y) == self.input.cursor {
            return;
        }
        self.input.cursor = (x, y);
        self.update_mouse_focus(x, y);
        self.post_to_capture_or_focus(Msg::new("CursorMoved").int("xpos", x).int("ypos", y), false);
    }

    pub fn mouse_pressed(&mut self, code: i32, double: bool) {
        if let Some(d) = usize::try_from(code).ok().and_then(|c| self.input.mouse_down.get_mut(c)) {
            *d = true;
        }
        let name = if double { "MouseDoublePressed" } else { "MousePressed" };
        self.post_to_capture_or_focus(Msg::new(name).int("code", code), true);
        if !double
            && let Some(cap) = self.input.capture
        {
            let over = self.input.mouse_over;
            if over.is_none_or(|o| o == cap || !self.has_parent(o, cap)) {
                self.set_mouse_capture(None);
            }
        }
        if let Some(over) = self.input.mouse_over
            && self.is_child_of_modal(Some(over))
        {
            self.set_top_level_focus(over);
        }
    }

    pub fn mouse_released(&mut self, code: i32) {
        if let Some(d) = usize::try_from(code).ok().and_then(|c| self.input.mouse_down.get_mut(c)) {
            *d = false;
        }
        self.post_to_capture_or_focus(Msg::new("MouseReleased").int("code", code), true);
    }

    pub fn mouse_wheeled(&mut self, delta: i32) {
        if let Some(f) = self.input.mouse_focus.filter(|&f| self.is_child_of_modal(Some(f))) {
            self.post(f, Msg::new("MouseWheeled").int("delta", delta), None, 0.0);
        }
    }

    fn post_key(&mut self, msg: Msg) {
        if let Some(f) = self.input.key_focus.filter(|&f| self.is_child_of_modal(Some(f))) {
            self.post(f, msg, None, 0.0);
        }
    }

    pub fn key_code_pressed(&mut self, code: i32) {
        let Some(d) = usize::try_from(code).ok().filter(|&c| c < KEY_LAST as usize) else { return };
        self.input.key_down[d] = true;
        self.post_key(Msg::new("KeyCodePressed").int("code", code));
    }

    pub fn key_code_typed(&mut self, code: i32) {
        if !(0..KEY_LAST).contains(&code) {
            return;
        }
        self.post_key(Msg::new("KeyCodeTyped").int("code", code));
    }

    pub fn key_typed(&mut self, ch: char) {
        self.post_key(Msg::new("KeyTyped").int("unichar", i32::from(ch as u32 as u16)));
    }

    pub fn key_code_released(&mut self, code: i32) {
        let Some(d) = usize::try_from(code).ok().filter(|&c| c < KEY_LAST as usize) else { return };
        self.input.key_down[d] = false;
        self.post_key(Msg::new("KeyCodeReleased").int("code", code));
    }

    /// `UpdateMouseFocus`: popups top-most first, then the embedded panel.
    fn update_mouse_focus(&mut self, x: i32, y: i32) {
        let mut focus = None;
        if self.cursor_visible && x >= 0 && y >= 0 && x < self.screen.0 && y < self.screen.1 {
            for &p in self.popups.iter().rev() {
                let mut visible = true;
                let mut q = Some(p);
                while let Some(qq) = q {
                    if self.parent(qq).is_none() {
                        break;
                    }
                    visible = self.is_visible(qq);
                    q = self.parent(qq);
                    if !visible {
                        break;
                    }
                }
                if self.is_mouse_input_enabled(p) && visible {
                    focus = self.within_traverse(p, x, y);
                    if focus.is_some() {
                        break;
                    }
                }
            }
            if focus.is_none() {
                focus = self.within_traverse(self.embedded, x, y);
            }
        }
        if !self.is_child_of_modal(focus) {
            focus = None;
        }
        self.set_mouse_focus(focus);
    }

    /// `Panel::IsWithinTraverse`: the deepest visible, mouse-enabled, non-popup child under the
    /// point (children from the end), else the panel itself when it contains the point.
    fn within_traverse(&self, id: PanelId, x: i32, y: i32) -> Option<PanelId> {
        let n = self.node(id)?;
        if !n.visible || !self.contains(id, x, y) {
            return None;
        }
        for &c in n.children.iter().rev() {
            if self.is_popup(c) || !self.is_mouse_input_enabled(c) {
                continue;
            }
            if let Some(hit) = self.within_traverse(c, x, y) {
                return Some(hit);
            }
        }
        Some(id)
    }

    /// `IsWithin`: inside the clip rect from the last solve.
    pub fn contains(&self, id: PanelId, x: i32, y: i32) -> bool {
        self.node(id).is_some_and(|n| x >= n.clip[0] && x < n.clip[2] && y >= n.clip[1] && y < n.clip[3])
    }

    fn set_mouse_focus(&mut self, new: Option<PanelId>) {
        if !self.is_child_of_modal(new) {
            return;
        }
        let mut p = new;
        while let Some(q) = p {
            let ok = self.is_mouse_input_enabled(q);
            let popup = self.is_popup(q);
            p = self.parent(q);
            if !ok {
                return;
            }
            if popup || p.is_none_or(|pp| self.parent(pp).is_none()) {
                break;
            }
        }
        let i = &self.input;
        if i.mouse_over == new && (i.capture.is_some() || i.mouse_focus == new) {
            return;
        }
        let capture = i.capture;
        let old = i.mouse_over;
        self.input.old_mouse_focus = old;
        self.input.mouse_over = new;
        if let Some(o) = old
            && capture.is_none_or(|c| self.has_parent(o, c))
        {
            self.post(o, Msg::new("CursorExited"), None, 0.0);
        }
        if let Some(n) = new
            && capture.is_none_or(|c| self.has_parent(n, c))
        {
            self.post(n, Msg::new("CursorEntered"), None, 0.0);
        }
        self.input.mouse_focus = capture.or(new);
    }

    /// `CalculateNewKeyFocus`: the top-most visible, keyboard-enabled popup's current focus.
    fn calculate_key_focus(&self) -> Option<PanelId> {
        let mut top = None;
        for &p in self.popups.iter().rev() {
            if !(self.is_popup(p) && self.is_visible(p) && self.is_keyboard_input_enabled(p)) {
                continue;
            }
            let mut vis = true;
            let mut q = self.parent(p);
            while let Some(qq) = q {
                vis = self.is_visible(qq);
                if !vis {
                    break;
                }
                q = self.parent(qq);
            }
            if vis {
                top = Some(p);
                break;
            }
        }
        let focus = top.map(|t| self.node(t).and_then(|n| n.current_focus).filter(|&f| self.exists(f)).unwrap_or(t));
        if !self.window_focus || !self.is_child_of_modal(focus) {
            return None;
        }
        focus
    }

    fn input_run_frame(&mut self) {
        if let Some(f) = self.input.key_focus.filter(|&f| self.is_child_of_modal(Some(f))) {
            self.post(f, Msg::new("KeyFocusTicked"), None, 0.0);
        }
        if let Some(f) = self.input.mouse_focus.filter(|&f| self.is_child_of_modal(Some(f))) {
            self.post(f, Msg::new("MouseFocusTicked"), None, 0.0);
        }
        let new = self.calculate_key_focus();
        if new != self.input.key_focus {
            if let Some(old) = self.input.key_focus {
                self.post(old, Msg::new("KillFocus"), None, 0.0);
            }
            match new {
                Some(n) => {
                    self.post(n, Msg::new("SetFocus"), None, 0.0);
                    self.input.key_focus = Some(n);
                    self.move_to_front(n);
                }
                None => self.input.key_focus = None,
            }
        }
    }

    // --- frame ------------------------------------------------------------------------------

    /// `CVGui::RunFrame`: mouse focus, key focus, messages, ticks, then the surface's
    /// scheme/think/solve traversal.
    pub fn run_frame(&mut self, s: &mut dyn Surface) {
        self.screen = s.screen_size();
        let (x, y) = self.input.cursor;
        self.update_mouse_focus(x, y);
        self.input_run_frame();
        self.dispatch(s);
        let now = self.time_ms;
        let mut i = self.ticks.len();
        while i > 0 {
            i -= 1;
            let t = &mut self.ticks[i];
            if t.interval == 0 || now >= t.next {
                if t.interval != 0 {
                    t.next = now.wrapping_add(t.interval);
                }
                let p = t.panel;
                self.post(p, Msg::new("Tick"), None, 0.0);
            }
        }
        let root = self.embedded;
        let screen = self.screen;
        if let Some(n) = self.node_mut(root) {
            n.size = screen;
        }
        self.scheme_traverse(root, s);
        self.think_traverse(root, s);
        self.solve_traverse(root);
    }

    fn scheme_traverse(&mut self, id: PanelId, s: &mut dyn Surface) {
        for c in self.children(id) {
            if self.is_visible(c) {
                self.scheme_traverse(c, s);
            }
        }
        self.perform_apply_scheme(id, s);
    }

    /// `Panel::Think`: `OnThink`, then the layout if needed (visible panels only).
    fn think_traverse(&mut self, id: PanelId, s: &mut dyn Surface) {
        self.with_ctrl(id, |c, ui| c.think(ui, id, s));
        if self.is_visible(id) && self.node(id).is_some_and(|n| n.tooltip.is_some()) {
            self.tooltip_perform_layout(id, s);
        }
        if self.is_visible(id) && self.node(id).is_some_and(|n| n.needs_layout) {
            self.perform_layout(id, s);
        }
        for c in self.children(id) {
            if self.is_visible(c) {
                self.think_traverse(c, s);
            }
        }
    }

    /// `VPanel::Solve` over the tree: absolute positions and clip rects.
    fn solve_traverse(&mut self, id: PanelId) {
        let Some(n) = self.node(id) else { return };
        let popup = n.popup;
        let (pos, size) = (n.pos, n.size);
        let p = if popup { Some(self.embedded).filter(|&e| e != id) } else { n.parent };
        let mut abs = pos;
        let mut clip;
        match p.and_then(|p| self.node(p)) {
            Some(pn) => {
                abs.0 += pn.abs.0 + pn.inset[0];
                abs.1 += pn.abs.1 + pn.inset[1];
                clip = [abs.0, abs.1, abs.0 + size.0, abs.1 + size.1];
                if !popup {
                    let pc = pn.clip;
                    clip[0] = clip[0].max(pc[0]);
                    clip[1] = clip[1].max(pc[1]);
                    if clip[2] > pc[2] {
                        clip[2] = pc[2] - pn.inset[2];
                    }
                    if clip[3] > pc[3] {
                        clip[3] = pc[3] - pn.inset[3];
                    }
                }
            }
            None => clip = [abs.0, abs.1, abs.0 + size.0, abs.1 + size.1],
        }
        if let Some(n) = self.node_mut(id) {
            n.abs = abs;
            n.clip = clip;
        }
        for c in self.children(id) {
            if self.is_visible(c) {
                self.solve_traverse(c);
            }
        }
    }

    /// The surface's `PaintTraverse(embedded)`: the tree, then every popup reached in it, in
    /// popup-list order.
    pub fn paint(&mut self, s: &mut dyn Surface) {
        let root = self.embedded;
        if !self.is_visible(root) {
            return;
        }
        for &p in &self.popups.clone() {
            if let Some(n) = self.node_mut(p) {
                n.render_popup_visible = false;
            }
        }
        let start = self.restrict_paint.unwrap_or(root);
        self.paint_traverse(start, s, true);
        for p in self.popups.clone() {
            if self.node(p).is_some_and(|n| n.render_popup_visible) {
                self.paint_traverse(p, s, true);
            }
        }
    }

    fn should_paint_child(&mut self, child: PanelId) -> bool {
        if let Some(r) = self.restrict_paint
            && !self.has_parent(child, r)
        {
            return false;
        }
        if self.is_popup(child) {
            if self.popups.contains(&child)
                && let Some(n) = self.node_mut(child)
            {
                n.render_popup_visible = true;
            }
            return false;
        }
        true
    }

    /// `Panel::PaintTraverse(repaint, allowForce)` (gu@0x1004b610): a child the surface does
    /// not paint (a popup, or outside the restricted panel) is still walked with `repaint`
    /// off, so the popups nested in it are flagged for this frame's popup pass.
    fn paint_traverse(&mut self, id: PanelId, s: &mut dyn Surface, repaint: bool) {
        let Some(n) = self.node(id) else { return };
        if !n.visible {
            return;
        }
        let (clip, abs, inset) = (n.clip, n.abs, n.inset);
        let (bg, paint_en, border_en, post_en) = (n.paint_background_enabled, n.paint_enabled, n.paint_border_enabled, n.post_child_paint_enabled);
        let has_border = n.border.is_some();
        let repaint = repaint && clip[2] > clip[0] && clip[3] > clip[1];
        if repaint && (border_en || bg || paint_en) {
            s.push_make_current(abs, clip);
            if bg {
                self.with_ctrl(id, |c, ui| c.paint_background(ui, id, s));
            }
            s.pop_make_current();
            s.push_make_current((abs.0 + inset[0], abs.1 + inset[1]), clip);
            if paint_en {
                self.with_ctrl(id, |c, ui| c.paint(ui, id, s));
            }
            s.pop_make_current();
        }
        for c in self.children(id) {
            let paint = self.should_paint_child(c);
            self.paint_traverse(c, s, paint && repaint);
        }
        if repaint {
            if border_en && has_border {
                s.push_make_current(abs, clip);
                self.with_ctrl(id, |c, ui| c.paint_border(ui, id, s));
                s.pop_make_current();
            }
            if post_en {
                s.push_make_current(abs, clip);
                self.with_ctrl(id, |c, ui| c.post_child_paint(ui, id, s));
                s.pop_make_current();
            }
        }
    }

    /// Whether a visible popup wants keyboard input (`NeedKBInput`).
    pub fn need_kb_input(&self) -> bool {
        self.calculate_key_focus().is_some()
    }

    /// Whether the panel and every ancestor are visible.
    pub fn is_fully_visible(&self, id: PanelId) -> bool {
        let mut p = Some(id);
        while let Some(q) = p {
            if !self.is_visible(q) {
                return false;
            }
            p = self.parent(q);
        }
        true
    }

    pub fn mark_for_deletion(&mut self, id: PanelId) {
        if let Some(n) = self.node_mut(id) {
            n.marked_for_deletion = true;
        }
        self.post(id, Msg::new("Delete"), None, 0.0);
    }
}
