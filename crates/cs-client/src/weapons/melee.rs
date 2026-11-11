//! Knife, grenades and C4 (docs/re/client-sim.md Part C §5–§7).

use gs_pmove::math::{add, angle_vectors, scale};

use super::{Ctx, FL_ONGROUND, SHIELD_DRAWN, Weapon, id, view_model};

fn grenade_model(id: i32) -> &'static str {
    match id {
        id::HEGRENADE => "hegrenade",
        id::FLASHBANG => "flashbang",
        _ => "smokegrenade",
    }
}

fn is_grenade(id: i32) -> bool {
    matches!(id, id::HEGRENADE | id::FLASHBANG | id::SMOKEGRENADE)
}

pub fn deploy(w: &mut Weapon, c: &mut Ctx) -> bool {
    match w.id {
        id::KNIFE => {
            w.swing = 0;
            w.weapon_state &= !SHIELD_DRAWN;
            c.p.shield_drawn = false;
            let model = view_model("knife", c.g.holding_shield);
            c.default_deploy(w, &model, 3)
        }
        id::C4 => {
            w.started_arming = false;
            w.armed_time = 0.0;
            c.default_deploy(w, &view_model("c4", false), 1)
        }
        _ => {
            if c.ammo(w) == 0 {
                return false;
            }
            w.weapon_state &= !SHIELD_DRAWN;
            w.release_throw = -1.0;
            c.p.shield_drawn = false;
            let model = view_model(grenade_model(w.id), c.g.holding_shield);
            c.default_deploy(w, &model, 3)
        }
    }
}

pub fn holster(w: &mut Weapon, c: &mut Ctx) {
    c.p.next_attack = 0.5;
    if is_grenade(w.id) {
        w.start_throw = 0.0;
        w.release_throw = -1.0;
    } else if w.id == id::C4 {
        w.started_arming = false;
    }
}

pub fn primary(w: &mut Weapon, c: &mut Ctx) {
    match w.id {
        id::KNIFE => swing(w, c),
        id::C4 => c4_primary(w, c),
        _ => {
            // Pin pull.
            if w.weapon_state & SHIELD_DRAWN == 0 && w.start_throw == 0.0 && c.ammo(w) > 0 {
                w.release_throw = 0.0;
                w.start_throw = c.time;
                c.send_anim(w.id, 1);
                w.twi = 0.5;
            }
        }
    }
}

/// `TraceLine` on the client: the standing hull, world and entities, only on the first
/// prediction (re-predictions reuse the last fraction).
fn knife_trace(c: &mut Ctx, range: f32) -> bool {
    let (fwd, _, _) = angle_vectors(c.p.v_angle);
    let src = c.gun_position();
    let end = add(src, scale(fwd, range));
    if c.runfuncs {
        *c.last_fraction = c.env.trace(src, end, 0, 0).fraction;
    }
    *c.last_fraction < 1.0
}

/// `CKnife::Swing(1)`.
fn swing(w: &mut Weapon, c: &mut Ctx) {
    let hit = knife_trace(c, 48.0);
    let shield = c.g.holding_shield;
    let anim = if shield {
        2
    } else {
        let a = if w.swing % 2 == 0 { 6 } else { 7 };
        w.swing += 1;
        a
    };
    c.send_anim(w.id, anim);
    match (hit, shield) {
        (_, true) => {
            w.npa = 1.0;
            w.nsa = 1.2;
        }
        (false, false) => {
            w.npa = 0.35;
            w.nsa = 0.5;
        }
        (true, false) => {
            w.npa = 0.4;
            w.nsa = 0.5;
        }
    }
    w.twi = 2.0;
}

pub fn secondary(w: &mut Weapon, c: &mut Ctx) {
    match w.id {
        id::KNIFE => {
            if c.g.holding_shield {
                shield_toggle(w, c);
                return;
            }
            // Stab.
            let hit = knife_trace(c, 32.0);
            if hit {
                c.send_anim(w.id, 4);
                w.nsa = 1.1;
                w.npa = 1.1;
            } else {
                c.send_anim(w.id, 5);
                w.nsa = 1.0;
                w.npa = 1.0;
            }
        }
        _ if is_grenade(w.id) && c.g.holding_shield && w.start_throw <= 0.0 => shield_toggle(w, c),
        _ => {}
    }
}

/// The knife's and grenades' inlined shield toggle (anims 5 up / 6 down).
fn shield_toggle(w: &mut Weapon, c: &mut Ctx) {
    if w.weapon_state & SHIELD_DRAWN != 0 {
        w.weapon_state &= !SHIELD_DRAWN;
        c.send_anim(w.id, 6);
        c.p.shield_drawn = false;
    } else {
        w.weapon_state |= SHIELD_DRAWN;
        c.send_anim(w.id, 5);
        c.p.shield_drawn = true;
    }
    w.nsa = 0.4;
    w.npa = 0.4;
    w.twi = 0.6;
}

pub fn idle(w: &mut Weapon, c: &mut Ctx) {
    match w.id {
        id::KNIFE => {
            if w.twi <= 0.0 && !c.p.shield_drawn {
                w.twi = 20.0;
                c.send_anim(w.id, 0);
            }
        }
        id::C4 => c4_idle(w, c),
        _ => grenade_idle(w, c),
    }
}

fn grenade_idle(w: &mut Weapon, c: &mut Ctx) {
    if w.release_throw == 0.0 && w.start_throw != 0.0 {
        w.release_throw = c.time;
    }
    if w.twi > 0.0 {
        return;
    }
    if w.start_throw != 0.0 {
        // The throw (the grenade itself is the server's).
        c.send_anim(w.id, 2);
        w.start_throw = 0.0;
        w.npa = 0.5;
        w.twi = 0.75;
        *c.ammo_mut(w) -= 1;
        if c.ammo(w) == 0 {
            w.npa = 0.5;
            w.nsa = 0.5;
            w.twi = 0.5;
        }
        return;
    }
    if w.release_throw > 0.0 {
        w.start_throw = 0.0;
        if w.id == id::FLASHBANG || c.ammo(w) == 0 {
            c.retire();
        } else {
            c.send_anim(w.id, 3);
            w.twi = c.random_float(10.0, 15.0);
            w.release_throw = -1.0;
        }
    } else if c.ammo(w) != 0 {
        if c.g.holding_shield {
            w.twi = 20.0;
            if w.weapon_state & SHIELD_DRAWN != 0 {
                c.send_anim(w.id, 4);
            }
            return;
        }
        if w.id == id::HEGRENADE {
            w.twi = c.random_float(10.0, 15.0);
        } else {
            let r = c.random_float(0.0, 1.0);
            w.twi = if r <= 0.75 { c.random_float(10.0, 15.0) } else { 2.5 };
        }
        c.send_anim(w.id, 0);
    }
}

/// `CC4::PrimaryAttack`: 3 s of held attack on the ground in a bomb zone.
fn c4_primary(w: &mut Weapon, c: &mut Ctx) {
    if c.ammo(w) <= 0 {
        return;
    }
    let ok = c.p.flags & FL_ONGROUND != 0 && c.g.in_bomb_zone;
    if !w.started_arming {
        if !ok {
            w.npa = 1.0;
            return;
        }
        w.started_arming = true;
        w.bomb_placed_anim = false;
        w.armed_time = c.time + 3.0;
        c.send_anim(w.id, 3);
        w.npa = 0.3;
        w.twi = c.random_float(10.0, 15.0);
    } else if ok {
        if c.time >= w.armed_time {
            w.started_arming = false;
            w.armed_time = 0.0;
            *c.ammo_mut(w) -= 1;
            if c.ammo(w) == 0 {
                c.retire();
                return;
            }
        } else if c.time >= w.armed_time - 0.75 && !w.bomb_placed_anim {
            w.bomb_placed_anim = true;
            c.send_anim(w.id, 2);
        }
        w.npa = 0.3;
        w.twi = c.random_float(10.0, 15.0);
    } else {
        w.npa = 1.5;
        w.started_arming = false;
        let anim = if w.bomb_placed_anim { 1 } else { 0 };
        c.send_anim(w.id, anim);
    }
}

fn c4_idle(w: &mut Weapon, c: &mut Ctx) {
    if w.started_arming {
        w.started_arming = false;
        let anim = if w.bomb_placed_anim { 1 } else { 0 };
        c.send_anim(w.id, anim);
    }
    if w.twi <= 0.0 {
        if c.ammo(w) <= 0 {
            c.retire();
        } else {
            c.send_anim(w.id, 1);
            c.send_anim(w.id, 0);
            w.twi = 20.0;
        }
    }
}
