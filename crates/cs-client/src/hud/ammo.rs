//! `CHudAmmo` with `WeaponsResource` (docs/re/client-hud.md §5.1, §5.2): the weapon registry
//! from `WeaponList`/`CurWeapon`/`AmmoX`, weapon selection (`slotN`, `invnext`/`invprev`,
//! `cancelselect`), the weapon list, the ammo counters and the CS crosshair.

use gs_draw::{Rect, Screen, SprHandle};

use super::{EMPTY, Hud, HudEngine, MsgReader, YELLOWISH, dhn, hide, scale, spr_get_list};

const SLOTS: usize = 6;
const POSITIONS: usize = 22;
/// `ITEM_FLAG_SELECTONEMPTY`.
const SELECT_ON_EMPTY: i32 = 1;
/// `MAX_HISTORY`.
const HISTORY: usize = 12;

/// A `HIST_ITEM`: what was picked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pickup {
    /// Ammo type, shown with the icon of a weapon using it.
    Ammo(usize),
    Weapon(usize),
    /// `hud.txt` sprite index.
    Item(usize),
}

#[derive(Debug, Clone, Copy)]
struct HistoryItem {
    kind: Pickup,
    count: i32,
    display_time: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct WeaponSprite {
    handle: SprHandle,
    rect: Rect,
}

impl WeaponSprite {
    fn width(&self) -> i32 {
        self.rect.right - self.rect.left
    }

    fn height(&self) -> i32 {
        self.rect.bottom - self.rect.top
    }

    fn draw(&self, screen: &mut Screen, x: i32, y: i32, c: [i32; 3]) {
        screen.spr_set(self.handle, c[0], c[1], c[2]);
        screen.spr_draw_additive(0, x, y, Some(self.rect));
    }
}

/// `WEAPON`.
#[derive(Debug, Clone, Default)]
struct Weapon {
    name: String,
    ammo_type: i32,
    max1: i32,
    ammo2_type: i32,
    max2: i32,
    slot: usize,
    slot_pos: usize,
    flags: i32,
    clip: i32,
    inactive: WeaponSprite,
    active: WeaponSprite,
    ammo: WeaponSprite,
    ammo2: WeaponSprite,
}

/// `gpActiveSel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Selection {
    #[default]
    Closed,
    /// `(WEAPON *)1`: the list is open with nothing selected.
    Open,
    Weapon(usize),
}

/// Effects of `CHudAmmo::Think` on the next command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionOutcome {
    /// `g_weaponselect`.
    pub weaponselect: Option<i32>,
    /// The confirming click is eaten (`CL_ResetButtonBits`).
    pub release_attack: bool,
}

#[derive(Default)]
pub(super) struct Ammo {
    weapons: Vec<Option<Weapon>>,
    slots: [[Option<usize>; POSITIONS]; SLOTS],
    /// `riAmmo`.
    ammo: [i32; 32],
    current: Option<usize>,
    selection: Selection,
    weapon_bits: i32,
    fade: f32,
    player_dead: bool,
    /// `m_bObserverCrosshair`: set by the `Crosshair` message while observing.
    pub(super) observer_crosshair: bool,
    /// `bucket1..bucket5` sprites.
    buckets: Vec<(SprHandle, Rect)>,
    selection_rect: Option<(SprHandle, Rect)>,
    bucket_size: (i32, i32),
    /// `giABWidth`, `giABHeight`.
    ammo_bar: (i32, i32),
    /// Crosshair spread state.
    cross_dist: f32,
    cross_alpha: i32,
    shots_last_check: i32,
    /// A weapon chosen this frame (`g_weaponselect`) by fast switch.
    pending_select: Option<i32>,
    /// `HistoryResource`: pickups, the next slot, and `iHistoryGap`.
    history: [Option<HistoryItem>; HISTORY],
    history_slot: usize,
    history_gap: i32,
}

/// Minimum crosshair gap and spread step per weapon id.
fn crosshair_params(id: usize) -> (f32, f32) {
    const GAP: [f32; 31] =
        [4.0, 8.0, 4.0, 5.0, 8.0, 9.0, 6.0, 9.0, 3.0, 8.0, 4.0, 8.0, 6.0, 5.0, 4.0, 4.0, 8.0, 8.0, 8.0, 6.0, 6.0, 8.0, 4.0, 7.0, 6.0, 8.0, 8.0, 5.0, 4.0, 7.0, 7.0];
    const DELTA: [f32; 31] =
        [3.0, 3.0, 3.0, 3.0, 3.0, 4.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 2.0, 3.0, 6.0, 3.0, 3.0, 4.0, 3.0, 3.0, 3.0, 4.0, 3.0, 3.0];
    if (1..=30).contains(&id) { (GAP[id], DELTA[id]) } else { (4.0, 3.0) }
}

/// `GetWeaponAccuracyFlags`: 1 air, 2 speed, 4 duck, 8/16 extra ×1.4.
fn accuracy_flags(id: usize, weapon_flags: i32) -> i32 {
    match id {
        1 | 11 | 26 => 7,
        7 | 12 | 19 | 23 => 1,
        8 | 14 | 20 | 27 | 28 | 30 => 3,
        15 => if weapon_flags & 0x10 != 0 { 0x13 } else { 3 },
        16 => if weapon_flags & 1 != 0 { 15 } else { 7 },
        17 => if weapon_flags & 2 != 0 { 0x17 } else { 7 },
        22 => if weapon_flags & 4 != 0 { 11 } else { 3 },
        _ => 0,
    }
}

/// Speed above which SPEED-flagged weapons widen.
fn speed_threshold(id: usize) -> i32 {
    match id {
        8 | 14 | 15 | 20 | 22 | 27 | 28 => 140,
        30 => 170,
        _ => 0,
    }
}

impl Ammo {
    pub(super) fn vid_init(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        if self.weapons.is_empty() {
            self.weapons = vec![None; 32];
            self.cross_alpha = 255;
        }
        let bucket0 = hud.sprite_index("bucket1");
        self.buckets = bucket0.map_or_else(Vec::new, |b| (0..5).map(|i| hud.sprite(b + i)).collect());
        self.selection_rect = hud.sprite_index("selection").map(|s| hud.sprite(s));
        self.bucket_size = bucket0.map_or((0, 0), |b| hud.sprite_size(b));
        self.ammo_bar = if eng.screen().width >= 640 { (20, 4) } else { (10, 2) };
    }

    /// `CHudAmmo::Reset`: keeps only the crosshair-hide bit.
    pub(super) fn reset(&mut self, hide_flags: &mut i32) {
        self.fade = 0.0;
        *hide_flags &= hide::CROSSHAIR;
        self.selection = Selection::Closed;
        self.current = None;
    }

    /// Rebuilds the owned-weapon slots when `m_iWeaponBits` changes.
    pub(super) fn think(&mut self, bits: i32) {
        if bits == self.weapon_bits {
            return;
        }
        self.weapon_bits = bits;
        for id in (0..32).rev() {
            let Some(w) = self.weapons.get(id).and_then(Option::as_ref) else { continue };
            let owned = bits & (1 << id) != 0;
            self.slots[w.slot][w.slot_pos] = owned.then_some(id);
        }
    }

    fn weapon(&self, id: usize) -> Option<&Weapon> {
        self.weapons.get(id).and_then(Option::as_ref)
    }

    fn count(&self, ammo_type: i32) -> i32 {
        usize::try_from(ammo_type).ok().and_then(|t| self.ammo.get(t)).copied().unwrap_or(0)
    }

    /// `HasAmmo`.
    fn has_ammo(&self, w: &Weapon) -> bool {
        w.max1 == -1 || w.ammo_type == -1 || w.clip > 0 || self.count(w.ammo_type) > 0 || self.count(w.ammo2_type) > 0 || w.flags & SELECT_ON_EMPTY != 0
    }

    /// Whether weapon `id` is registered.
    #[cfg(test)]
    pub(super) fn known(&self, id: usize) -> bool {
        self.weapon(id).is_some()
    }

    /// `m_pWeapon`'s id.
    #[cfg(test)]
    pub(super) fn current(&self) -> Option<usize> {
        self.current
    }

    /// `MsgFunc_WeaponList`.
    pub(super) fn msg_weapon_list(&mut self, r: &mut MsgReader, res: i32, eng: &mut dyn HudEngine) {
        let name = r.string();
        let (ammo_type, max1, ammo2_type, max2) = (r.char(), r.byte(), r.char(), r.byte());
        let (slot, slot_pos, id, flags) = (r.char(), r.char(), r.char(), r.byte());
        let type_ok = |t: i32| (-1..=31).contains(&t);
        if !(0..=31).contains(&id) || !(0..=5).contains(&slot) || !(0..=21).contains(&slot_pos) || !type_ok(ammo_type) || !type_ok(ammo2_type) {
            return;
        }
        let max = |m: i32| if m == 255 { -1 } else { m };
        let (max1, max2) = (max(max1), max(max2));
        if !((ammo_type == -1 || max1 != 0) && (ammo2_type == -1 || max2 != 0)) {
            return;
        }
        let mut w = Weapon {
            name: name.clone(),
            ammo_type,
            max1,
            ammo2_type,
            max2,
            slot: slot as usize,
            slot_pos: slot_pos as usize,
            flags,
            clip: 0,
            ..Weapon::default()
        };
        let list = eng.read_file(&format!("sprites/{name}.txt")).map(|d| spr_get_list(&d)).unwrap_or_default();
        let mut find = |entry: &str| -> Option<WeaponSprite> {
            let s = list.iter().find(|s| s.name == entry && s.res == res)?;
            Some(WeaponSprite { handle: eng.spr_load(&format!("sprites/{}.spr", s.sprite)), rect: s.rect })
        };
        w.inactive = find("weapon").unwrap_or_default();
        w.active = find("weapon_s").unwrap_or_default();
        w.ammo = find("ammo").unwrap_or_default();
        w.ammo2 = find("ammo2").unwrap_or_default();
        if res == 640 {
            self.history_gap = self.history_gap.max(w.active.height());
        }
        if self.weapons.is_empty() {
            self.weapons = vec![None; 32];
        }
        self.weapons[id as usize] = Some(w);
        self.weapon_bits = 0;
    }

    /// `MsgFunc_CurWeapon` (§3.1); `user1` is the observer mode.
    pub(super) fn msg_cur_weapon(&mut self, r: &mut MsgReader, user1: i32) {
        let state = r.byte();
        let id = r.char();
        let mut clip = r.char();
        if id <= 0 && !self.observer_crosshair {
            return;
        }
        if user1 != 4 {
            if id == -1 && clip == -1 {
                self.player_dead = true;
                self.selection = Selection::Closed;
                return;
            }
            self.player_dead = false;
        }
        if clip <= -2 {
            clip = -clip;
        }
        let Some(w) = usize::try_from(id).ok().and_then(|id| self.weapons.get_mut(id)).and_then(Option::as_mut) else { return };
        w.clip = clip;
        if state != 0 {
            self.current = usize::try_from(id).ok();
            self.observer_crosshair = false;
            self.fade = 200.0;
        }
    }

    /// `MsgFunc_AmmoX`.
    pub(super) fn msg_ammo_x(&mut self, r: &mut MsgReader) {
        let index = r.byte();
        let amount = r.byte();
        if let Some(a) = usize::try_from(index).ok().and_then(|i| self.ammo.get_mut(i)) {
            *a = amount;
        }
    }

    /// `AmmoPickup`, `WeapPickup`, `ItemPickup` → `HistoryResource::AddToHistory`.
    pub(super) fn msg_pickup(&mut self, hud: &Hud, name: &str, r: &mut MsgReader, eng: &mut dyn HudEngine) {
        let (kind, count) = match name {
            "AmmoPickup" => {
                let index = r.byte();
                let count = r.byte();
                let Ok(index) = usize::try_from(index) else { return };
                (Pickup::Ammo(index), count)
            }
            "WeapPickup" => match usize::try_from(r.byte()) {
                Ok(id) => (Pickup::Weapon(id), 0),
                Err(_) => return,
            },
            _ => match hud.sprite_index(&r.string()) {
                Some(sprite) => (Pickup::Item(sprite), 0),
                None => return,
            },
        };
        if matches!(kind, Pickup::Ammo(_)) && count == 0 {
            return;
        }
        let gap = self.history_gap;
        let height = eng.screen().height as i32;
        // The next entry would be drawn too high (or past the last slot): start at the bottom.
        if self.history_slot as i32 * (gap + 5) + 2 * gap + 72 > height - 100 || self.history_slot >= HISTORY {
            self.history_slot = 0;
        }
        let display_time = hud.time + eng.cvar("hud_drawhistory_time") as i32 as f32;
        self.history[self.history_slot] = Some(HistoryItem { kind, count, display_time });
        self.history_slot += 1;
    }

    /// The ammo icon of the first weapon using ammo type `index` (`GetAmmoPicFromWeapon`).
    fn ammo_pic(&self, index: usize) -> Option<WeaponSprite> {
        let index = i32::try_from(index).ok()?;
        self.weapons.iter().flatten().find_map(|w| {
            if w.ammo_type == index {
                Some(w.ammo)
            } else if w.ammo2_type == index {
                Some(w.ammo2)
            } else {
                None
            }
        })
    }

    /// `DrawAmmoHistory`: newest at the bottom, each fading over its last 3.2 s.
    fn draw_history(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        let draw_time = eng.cvar("hud_drawhistory_time") as i32 as f32;
        let gap = self.history_gap;
        let screen = eng.screen();
        let (w, h) = (screen.width as i32, screen.height as i32);
        for i in 0..HISTORY {
            let Some(item) = &mut self.history[i] else { continue };
            item.display_time = item.display_time.min(hud.time + draw_time);
            if item.display_time <= hud.time {
                self.history[i] = None;
                // CheckClearHistory.
                if self.history.iter().all(Option::is_none) {
                    self.history_slot = 0;
                }
                continue;
            }
            let item = *item;
            let a = ((item.display_time - hud.time) * 80.0).min(255.0) as i32;
            let y = h - (2 * gap + 72 + (gap + 5) * i as i32);
            match item.kind {
                Pickup::Ammo(index) => {
                    let c = scale(YELLOWISH, a);
                    let x = w - 24;
                    if let Some(pic) = self.ammo_pic(index).filter(|p| p.handle.0 != 0) {
                        pic.draw(screen, x, y, c);
                    }
                    screen.draw_string_reverse(x - 10, y, &item.count.to_string(), c[0], c[1], c[2]);
                }
                Pickup::Weapon(id) => {
                    // An unknown weapon ends the history drawing for this frame.
                    let Some(weapon) = self.weapon(id) else { return };
                    let c = scale(if self.has_ammo(weapon) { YELLOWISH } else { EMPTY }, a);
                    weapon.inactive.draw(screen, w - weapon.inactive.width(), y, c);
                }
                Pickup::Item(sprite) => {
                    let c = scale(YELLOWISH, a);
                    hud.draw_sprite(screen, sprite, w - hud.sprite_size(sprite).0 - 10, y, c);
                }
            }
        }
    }

    fn first_pos(&self, slot: usize) -> Option<usize> {
        self.slots.get(slot)?.iter().flatten().next().copied()
    }

    fn next_active_pos(&self, slot: usize, pos: usize) -> Option<usize> {
        self.slots.get(slot)?.iter().skip(pos + 1).flatten().next().copied()
    }

    /// `SlotInput(slot)` for `slot1..slot6` (0-based) when no text menu took the key.
    pub(super) fn select_slot(&mut self, slot: usize, hide_flags: i32, eng: &mut dyn HudEngine) {
        if slot >= SLOTS || self.player_dead || hide_flags & (hide::WEAPONS | hide::ALL) != 0 {
            return;
        }
        let suit = self.weapon_bits < 0;
        if !suit || self.weapon_bits & 0x7FFF_FFFF == 0 {
            return;
        }
        let fast = eng.cvar("hud_fastswitch") != 0.0;
        let open_slot = match self.selection {
            Selection::Weapon(id) => self.weapon(id).map(|w| w.slot),
            _ => None,
        };
        let pick = if open_slot != Some(slot) {
            eng.play_sound("common/wpn_hudon.wav", 1.0);
            let p = self.first_pos(slot);
            if let Some(first) = p
                && fast
                && let Some(w) = self.weapon(first)
                && self.next_active_pos(w.slot, w.slot_pos).is_none()
            {
                let name = w.name.clone();
                eng.server_cmd(&name);
                self.pending_select = Some(first as i32);
                return;
            }
            p
        } else {
            eng.play_sound("common/wpn_moveselect.wav", 1.0);
            let next = match self.selection {
                Selection::Weapon(id) => self.weapon(id).and_then(|w| self.next_active_pos(w.slot, w.slot_pos)),
                _ => None,
            };
            next.or_else(|| self.first_pos(slot))
        };
        self.selection = match pick {
            Some(id) => Selection::Weapon(id),
            None if !fast => Selection::Open,
            None => Selection::Closed,
        };
    }

    /// `UserCmd_NextWeapon` / `UserCmd_PrevWeapon`.
    pub(super) fn cycle(&mut self, forward: bool, hide_flags: i32, eng: &mut dyn HudEngine) {
        if self.player_dead || hide_flags & (hide::WEAPONS | hide::ALL) != 0 {
            return;
        }
        let from = match self.selection {
            Selection::Weapon(id) => Some(id),
            _ => self.current,
        };
        let (mut slot, mut pos) = from.and_then(|id| self.weapon(id)).map_or((0, 0), |w| (w.slot, w.slot_pos));
        let order: Vec<(usize, usize)> = (0..5).flat_map(|s| (0..POSITIONS).map(move |p| (s, p))).collect();
        let start = order.iter().position(|&o| o == (slot, pos)).unwrap_or(0);
        let n = order.len();
        for step in 1..=n {
            let i = if forward { (start + step) % n } else { (start + n - step) % n };
            (slot, pos) = order[i];
            if let Some(id) = self.slots[slot][pos] {
                if eng.cvar("hud_fastswitch") == 1.0 {
                    eng.play_sound("common/wpn_hudon.wav", 1.0);
                    if let Some(w) = self.weapon(id) {
                        let name = w.name.clone();
                        eng.server_cmd(&name);
                    }
                    self.pending_select = Some(id as i32);
                    self.selection = Selection::Closed;
                } else {
                    self.selection = Selection::Weapon(id);
                }
                return;
            }
        }
    }

    /// `UserCmd_Close`: closes an open selection, else `escape`.
    pub(super) fn cancel(&mut self, eng: &mut dyn HudEngine) {
        if self.selection == Selection::Closed {
            eng.client_cmd("escape\n");
        } else {
            self.selection = Selection::Closed;
            eng.play_sound("common/wpn_hudoff.wav", 1.0);
        }
    }

    /// `CHudAmmo::Think`'s confirmation: an open selection plus `+attack` picks the weapon.
    pub(super) fn confirm(&mut self, attack: bool, eng: &mut dyn HudEngine) -> SelectionOutcome {
        let mut out = SelectionOutcome { weaponselect: self.pending_select.take(), release_attack: false };
        if self.selection == Selection::Closed || !attack {
            return out;
        }
        if let Selection::Weapon(id) = self.selection
            && let Some(w) = self.weapon(id)
        {
            let name = w.name.clone();
            eng.server_cmd(&name);
            out.weaponselect = Some(id as i32);
        }
        self.selection = Selection::Closed;
        eng.play_sound("common/wpn_select.wav", 1.0);
        out.release_attack = true;
        out
    }

    pub(super) fn draw(&mut self, hud: &Hud, eng: &mut dyn HudEngine) {
        if !hud.has_suit() || hud.hide & (hide::WEAPONS | hide::ALL) != 0 {
            return;
        }
        // HLTV: no weapon HUD; only the in-eye crosshair.
        if eng.spectate_only() {
            if hud.state.user1 == 4
                && hud.hide & hide::CROSSHAIR == 0
                && let Some(id) = self.current
            {
                self.draw_crosshair(hud, id, eng);
            }
            return;
        }
        if hud.hide & hide::CROSSHAIR == 0
            && let Some(id) = self.current
        {
            self.draw_crosshair(hud, id, eng);
        }
        self.draw_list(eng);
        self.draw_history(hud, eng);
        let Some(w) = self.current.and_then(|id| self.weapon(id)).cloned() else { return };
        if w.ammo_type < 0 && w.ammo2_type < 0 {
            return;
        }
        let a = (self.fade as i32).max(100);
        if self.fade > 0.0 {
            self.fade -= hud.delta * 20.0;
        }
        let c = scale(YELLOWISH, a);
        let screen = eng.screen();
        let (width, height) = (screen.width as i32, screen.height as i32);
        let (fh, dw) = (hud.font_height, hud.digit_width());
        let mut y = height - fh - fh / 2;
        let flags = dhn::DRAWZERO | dhn::THREE_DIGITS;
        if w.ammo_type > 0 {
            let icon = w.ammo.width();
            let mut x;
            if w.clip >= 0 {
                x = width - 8 * dw - icon;
                x = hud.draw_number(screen, x, y, flags, w.clip, c);
                x += dw / 2;
                screen.fill_rgba(x, y, dw / 10, fh, YELLOWISH[0], YELLOWISH[1], YELLOWISH[2], a);
                x += dw / 10 + dw / 2;
                x = hud.draw_number(screen, x, y, flags, self.count(w.ammo_type), c);
            } else {
                x = width - 4 * dw - icon;
                x = hud.draw_number(screen, x, y, flags, self.count(w.ammo_type), c);
            }
            w.ammo.draw(screen, x, y - w.ammo.height() / 8, c);
        }
        if w.ammo2_type > 0 && self.count(w.ammo2_type) > 0 {
            y -= fh + fh / 4;
            let x = width - 4 * dw - w.ammo2.width();
            let x = hud.draw_number(screen, x, y, flags, self.count(w.ammo2_type), c);
            w.ammo2.draw(screen, x, y - w.ammo2.height() / 8, c);
        }
    }

    /// `DrawAmmoBar`.
    fn draw_ammo_bar(&self, screen: &mut Screen, w: &Weapon, mut x: i32, y: i32) {
        if w.ammo_type == -1 || self.count(w.ammo_type) == 0 {
            return;
        }
        let (bw, bh) = self.ammo_bar;
        let bar = |screen: &mut Screen, x: i32, f: f32| -> i32 {
            let f = f.clamp(0.0, 1.0);
            let (mut x, mut width) = (x, bw);
            if f != 0.0 {
                let filled = ((f * bw as f32) as i32).max(1);
                screen.fill_rgba(x, y, filled, bh, 0, 160, 0, 255);
                x += filled;
                width -= filled;
            }
            screen.fill_rgba(x, y, width, bh, YELLOWISH[0], YELLOWISH[1], YELLOWISH[2], 128);
            x + width
        };
        x = bar(screen, x, self.count(w.ammo_type) as f32 / w.max1 as f32);
        if w.ammo2_type != -1 {
            bar(screen, x + 5, self.count(w.ammo2_type) as f32 / w.max2 as f32);
        }
    }

    /// `DrawWList`: buckets across the top, the open slot's weapons below it.
    fn draw_list(&mut self, eng: &mut dyn HudEngine) {
        if self.selection == Selection::Closed {
            return;
        }
        let mut active_slot = match self.selection {
            Selection::Weapon(id) => self.weapon(id).map(|w| w.slot),
            _ => None,
        };
        if let Some(s) = active_slot
            && self.first_pos(s).is_none()
        {
            self.selection = Selection::Open;
            active_slot = None;
        }
        if self.buckets.is_empty() {
            return;
        }
        let (bucket_w, bucket_h) = self.bucket_size;
        let screen = eng.screen();
        let x0 = if screen.width >= 640 { 138 } else { 70 };
        let first_width = |slot: usize| self.first_pos(slot).and_then(|id| self.weapon(id)).map_or(bucket_w, |w| w.active.width());
        let mut x = x0;
        for slot in 0..5 {
            let (h, r) = self.buckets.get(slot).copied().unwrap_or_default();
            screen.spr_set(h, YELLOWISH[0], YELLOWISH[1], YELLOWISH[2]);
            screen.spr_draw_additive(0, x, 10, Some(r));
            x += if active_slot == Some(slot) { first_width(slot) } else { bucket_w } + 5;
        }
        x = x0;
        for slot in 0..5 {
            let mut y = bucket_h + 10;
            if active_slot == Some(slot) {
                let width = first_width(slot);
                for id in self.slots[slot].iter().flatten() {
                    let Some(w) = self.weapon(*id) else { continue };
                    let c = if self.has_ammo(w) { scale(YELLOWISH, 192) } else { scale(EMPTY, 128) };
                    if self.selection == Selection::Weapon(*id) {
                        w.active.draw(screen, x, y, c);
                        if let Some((h, r)) = self.selection_rect {
                            screen.spr_set(h, c[0], c[1], c[2]);
                            screen.spr_draw_additive(0, x, y, Some(r));
                        }
                    } else {
                        w.inactive.draw(screen, x, y, c);
                    }
                    self.draw_ammo_bar(screen, w, x + self.ammo_bar.0 / 2, y + 3);
                    y += w.active.height() + 5;
                }
                x += width + 5;
            } else {
                for id in self.slots[slot].iter().flatten() {
                    let Some(w) = self.weapon(*id) else { continue };
                    let (c, a) = if self.has_ammo(w) { (YELLOWISH, 128) } else { (EMPTY, 96) };
                    screen.fill_rgba(x, y, bucket_w, bucket_h, c[0], c[1], c[2], a);
                    y += bucket_h + 5;
                }
                x += bucket_w + 5;
            }
        }
    }

    /// `DrawCrosshair` (§5.2): four bars around the screen centre, widened by movement.
    fn draw_crosshair(&mut self, hud: &Hud, id: usize, eng: &mut dyn HudEngine) {
        if hud.fov <= 54 || matches!(id, 3 | 13 | 18 | 24) {
            return;
        }
        let (min_gap, delta) = crosshair_params(id);
        let mut gap = min_gap;
        let flags = accuracy_flags(id, hud.state.weapon_flags);
        if flags != 0 && eng.cvar("cl_dynamiccrosshair") != 0.0 && hud.hide & hide::WEAPONS == 0 {
            if hud.state.flags & 0x200 == 0 && flags & 1 != 0 {
                gap *= 2.0;
            } else if hud.state.flags & 0x4000 != 0 && flags & 4 != 0 {
                gap = (gap * 0.5).trunc();
            } else if (hud.state.speed as i32) > speed_threshold(id) && flags & 2 != 0 {
                gap = (gap * 1.5).trunc();
            }
            if flags & 8 != 0 {
                gap = (gap * 1.4).trunc();
            }
            if flags & 16 != 0 {
                gap = (gap * 1.4).trunc();
            }
        }
        // The original resets g_iShotsFired to 1 above 600; the counter here only grows, which
        // compares the same way.
        let shots = hud.state.shots_fired;
        if shots > self.shots_last_check {
            self.cross_dist = (self.cross_dist + delta).min(15.0);
            self.cross_alpha = (self.cross_alpha - 40).max(120);
        } else {
            self.cross_dist -= 0.013 * self.cross_dist + 0.1;
            self.cross_alpha += 2;
        }
        self.shots_last_check = shots;
        self.cross_dist = self.cross_dist.max(gap);
        self.cross_alpha = self.cross_alpha.min(255);
        let mut len = ((self.cross_dist - gap) * 0.5) as i32 + 5;
        let mut dist = self.cross_dist;
        let color_text = eng.cvar_string("cl_crosshair_color");
        let mut rgb = [50, 250, 50];
        for (o, v) in rgb.iter_mut().zip(color_text.split_whitespace()) {
            *o = v.parse::<i32>().unwrap_or(0).clamp(0, 255);
        }
        let translucent = eng.cvar("cl_crosshair_translucent") != 0.0;
        let size = eng.cvar_string("cl_crosshair_size");
        let screen = eng.screen();
        let (w, h) = (screen.width as i32, screen.height as i32);
        let base = match size.trim().to_ascii_lowercase().as_str() {
            "small" | "1" => 1024,
            "medium" | "2" => 800,
            "large" | "3" => 640,
            _ if w <= 640 => 1024,
            _ if w <= 1023 => 800,
            _ => 640,
        };
        if w != base {
            dist = dist * w as f32 / base as f32;
            len = w * len / base;
        }
        let (cx, cy, d) = (w / 2, h / 2, dist as i32);
        let a = self.cross_alpha;
        let nvg = hud.nightvision.on;
        let c = if nvg { [250, 50, 50] } else { rgb };
        let mut bar = |x, y, bw, bh| {
            if translucent && !nvg {
                screen.fill_rgba(x, y, bw, bh, c[0], c[1], c[2], a);
            } else {
                screen.fill_rgba_blend(x, y, bw, bh, c[0], c[1], c[2], a);
            }
        };
        bar(cx - d - len + 1, cy, len, 1);
        bar(cx + d, cy, len, 1);
        bar(cx, cy - d - len + 1, 1, len);
        bar(cx, cy + d, 1, len);
    }
}
