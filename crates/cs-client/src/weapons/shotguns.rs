//! M3 and XM1014 (docs/re/client-sim.md Part C §4): pellet fire with shared-random punch and
//! the shell-by-shell reload state machine.

use super::{Ctx, Weapon, id, view_model};

fn is_m3(w: &Weapon) -> bool {
    w.id == id::M3
}

pub fn deploy(w: &mut Weapon, c: &mut Ctx) -> bool {
    let name = if is_m3(w) { "m3" } else { "xm1014" };
    c.default_deploy(w, &view_model(name, false), 6)
}

pub fn primary(w: &mut Weapon, c: &mut Ctx) {
    if c.p.waterlevel == 3 {
        w.npa = 0.15;
        return;
    }
    if w.clip <= 0 {
        reload(w, c);
        return;
    }
    w.clip -= 1;
    if is_m3(w) {
        c.playback("m3", [0.0; 2], [0, 0], [false, false]);
    } else {
        // m_vVecAiming is never written: zero aim, iparam1 7.
        c.playback("xm1014", [0.0; 2], [7, 0], [w.clip == 0, false]);
    }
    if w.clip != 0 || c.ammo(w) > 0 {
        w.pump_time = if is_m3(w) { 0.5 } else { 0.125 };
    }
    if is_m3(w) {
        w.npa = 0.875;
        w.nsa = 0.875;
        w.twi = if w.clip != 0 { 2.5 } else { 0.875 };
    } else {
        w.npa = 0.25;
        w.nsa = 0.25;
        w.twi = if w.clip != 0 { 2.25 } else { 0.75 };
    }
    w.in_special_reload = 0;
    let (ground, air) = if is_m3(w) { ((4, 6), (8, 11)) } else { ((3, 5), (7, 10)) };
    let (lo, hi) = if c.on_ground() { ground } else { air };
    c.p.punchangle[0] -= c.shared_long(1, lo, hi) as f32;
}

/// `Reload`: 0 → start (0.55 s), 1 → shell animation, 2 → the shell is credited.
pub fn reload(w: &mut Weapon, c: &mut Ctx) {
    let max = if is_m3(w) { 8 } else { 7 };
    if c.ammo(w) <= 0 || w.clip == max || w.npa > 0.0 {
        return;
    }
    match w.in_special_reload {
        0 => {
            w.in_special_reload = 1;
            c.send_anim(w.id, 5);
            w.npa = 0.55;
            w.nsa = 0.55;
            w.next_reload = 0.55;
            w.twi = 0.55;
            if !is_m3(w) {
                c.p.next_attack = 0.55;
            }
        }
        1 if w.twi <= 0.0 => {
            w.in_special_reload = 2;
            c.send_anim(w.id, 3);
            let t = if is_m3(w) { 0.45 } else { 0.30 };
            w.next_reload = t;
            w.twi = t;
        }
        2 if w.twi <= 0.0 => {
            w.clip += 1;
            *c.ammo_mut(w) -= 1;
            c.p.ammo.buckshot -= 1;
            w.in_special_reload = 1;
        }
        _ => {}
    }
}

// `if (m_flTimeWeaponIdle < 0)` around the body: NaN skips it, like the original.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub fn idle(w: &mut Weapon, c: &mut Ctx) {
    if w.pump_time < 0.0 {
        w.pump_time = 0.0;
    }
    if !(w.twi < 0.0) {
        return;
    }
    let max = if is_m3(w) { 8 } else { 7 };
    let ammo = c.ammo(w);
    if w.clip == 0 && w.in_special_reload == 0 && ammo > 0 {
        reload(w, c);
    } else if w.in_special_reload != 0 {
        if w.clip != max && ammo > 0 {
            reload(w, c);
        } else {
            c.send_anim(w.id, 4);
            w.in_special_reload = 0;
            w.twi = 1.5;
        }
    } else {
        c.send_anim(w.id, 0);
        w.twi = 20.0;
    }
}
