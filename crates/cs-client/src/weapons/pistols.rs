//! Pistols (docs/re/client-sim.md Part C §3): Glock-18 (with burst), USP (silencer), P228,
//! Desert Eagle, Dual Elites, Five-seveN.

use super::{Ctx, SHIELD_DRAWN, Weapon, id, trunc, view_model};

/// Accuracy recovery and spread multipliers (`<W>Fire` / `PrimaryAttack`, Part C §3.0).
struct Table {
    k1: f64,
    k2: f64,
    min: f32,
    max: f32,
    /// Spread multiplier: air, moving, ducking, standing (double constants).
    m: [f64; 4],
    cycle: f32,
    /// What the cycle argument loses before it becomes the next attack time.
    sub: f32,
    idle: f32,
}

fn table(w: &Weapon) -> Table {
    let t = |k1, k2, min, max, m, cycle, sub, idle| Table { k1, k2, min, max, m, cycle, sub, idle };
    match w.id {
        id::GLOCK18 if w.weapon_state & 2 != 0 => t(0.325, 0.275, 0.6, 0.9, [1.2, 0.185, 0.095, 0.3], 0.5, 0.0, 2.5),
        id::GLOCK18 => t(0.325, 0.275, 0.6, 0.9, [1.0, 0.165, 0.075, 0.1], 0.2, 0.05, 2.5),
        id::USP if w.weapon_state & 1 != 0 => t(0.3, 0.275, 0.6, 0.92, [1.3, 0.25, 0.125, 0.15], 0.225, 0.075, 2.0),
        id::USP => t(0.3, 0.275, 0.6, 0.92, [1.2, 0.225, 0.08, 0.1], 0.225, 0.075, 2.0),
        id::P228 => t(0.325, 0.3, 0.6, 0.9, [1.5, 0.255, 0.075, 0.15], 0.2, 0.05, 2.0),
        id::DEAGLE => t(0.4, 0.35, 0.55, 0.9, [1.5, 0.25, 0.115, 0.13], 0.3, 0.075, 1.8),
        id::ELITE => t(0.325, 0.275, 0.55, 0.88, [1.3, 0.175, 0.08, 0.1], 0.2, 0.125, 2.0),
        _ => t(0.275, 0.25, 0.725, 0.92, [1.5, 0.255, 0.075, 0.15], 0.2, 0.05, 2.0),
    }
}

/// The accuracy every Spawn/Deploy/Reload resets to.
fn initial_accuracy(id: i32) -> f32 {
    match id {
        id::USP | id::FIVESEVEN => 0.92,
        id::ELITE => 0.88,
        _ => 0.9,
    }
}

fn has_shield_model(id: i32) -> bool {
    !matches!(id, id::ELITE)
}

pub fn deploy(w: &mut Weapon, c: &mut Ctx) -> bool {
    w.accuracy = initial_accuracy(w.id);
    w.weapon_state &= !SHIELD_DRAWN;
    c.p.shield_drawn = false;
    let shield = c.g.holding_shield && has_shield_model(w.id);
    let (model, anim) = match w.id {
        id::GLOCK18 => {
            w.glock_shots = 0;
            w.glock_shoot = 0.0;
            if shield {
                w.weapon_state &= !2;
                ("glock18", 5)
            } else {
                ("glock18", if c.random_long(0, 1) == 0 { 11 } else { 8 })
            }
        }
        id::USP => {
            if shield {
                w.weapon_state &= !1;
                ("usp", 5)
            } else {
                ("usp", if w.weapon_state & 1 != 0 { 6 } else { 14 })
            }
        }
        id::P228 => ("p228", if shield { 5 } else { 6 }),
        id::DEAGLE => ("deagle", 5),
        id::ELITE => {
            if w.clip & 1 == 0 {
                w.weapon_state |= 8;
            }
            ("elite", 15)
        }
        _ => ("fiveseven", 5),
    };
    let model = view_model(model, shield);
    c.default_deploy(w, &model, anim)
}

pub fn primary(w: &mut Weapon, c: &mut Ctx) {
    let t = table(w);
    let m = if !c.on_ground() {
        t.m[0]
    } else if c.speed2d() > 0.0 {
        t.m[1]
    } else if c.ducking() {
        t.m[2]
    } else {
        t.m[3]
    };
    let spread = (m * (1.0 - f64::from(w.accuracy))) as f32;
    let burst = w.id == id::GLOCK18 && w.weapon_state & 2 != 0;
    fire(w, c, &t, spread, burst);
}

/// `<W>Fire(spread, cycle, fUseSemi)`.
fn fire(w: &mut Weapon, c: &mut Ctx, t: &Table, spread: f32, burst: bool) {
    if burst {
        w.glock_shots = 0;
    } else {
        w.shots_fired += 1;
        if w.shots_fired > 1 {
            return;
        }
    }
    let now = c.time;
    let since = now - w.last_fire;
    if w.last_fire != 0.0 {
        let acc = (f64::from(w.accuracy) - (t.k1 - f64::from(since)) * t.k2) as f32;
        w.accuracy = acc.clamp(t.min, t.max);
    }
    w.last_fire = now;
    if w.clip <= 0 {
        if w.fire_on_empty {
            w.npa = 0.2;
        }
        return;
    }
    w.clip -= 1;
    let off = c.spread(spread);
    let punch = c.p.punchangle;
    let next = t.cycle - t.sub;
    match w.id {
        id::GLOCK18 => c.playback("glock18", off, [trunc(punch[0], 100.0), trunc(punch[1], 100.0)], [w.clip == 0, false]),
        id::USP => c.playback("usp", off, [trunc(punch[0], 100.0), 0], [w.clip == 0, w.weapon_state & 1 != 0]),
        id::P228 => c.playback("p228", off, [trunc(punch[0], 100.0), trunc(punch[1], 100.0)], [w.clip == 0, false]),
        id::DEAGLE => c.playback("deagle", off, [trunc(punch[0], 100.0), trunc(punch[1], 100.0)], [w.clip == 0, false]),
        id::FIVESEVEN => c.playback("fiveseven", off, [trunc(punch[0], 100.0), trunc(punch[1], 100.0)], [w.clip == 0, false]),
        _ => {
            // Elites alternate hands (the bullets leave 5 units to that side, server-side);
            // fparam1 is the time since the previous shot.
            let left = w.weapon_state & 8 != 0;
            w.weapon_state ^= 8;
            let event = if left { "elite_left" } else { "elite_right" };
            c.playback(event, [since, off[0]], [trunc(off[1], 100.0), w.clip], [false, false]);
        }
    }
    w.npa = if burst { 0.5 } else { next };
    w.nsa = w.npa;
    w.twi = t.idle;
    if matches!(w.id, id::DEAGLE | id::ELITE | id::FIVESEVEN) {
        c.p.punchangle[0] -= 2.0;
    }
    if burst {
        w.glock_shoot = now + 0.1;
        w.glock_shots = 1;
    }
}

/// `FireRemaining` for the Glock burst: the next two rounds on the next two weapon frames.
pub fn fire_remaining_glock(w: &mut Weapon, c: &mut Ctx) {
    w.clip -= 1;
    if w.clip < 0 {
        w.clip = 0;
        w.glock_shots = 3;
        w.glock_shoot = 0.0;
        return;
    }
    let off = c.spread(0.05);
    let punch = c.p.punchangle;
    c.playback("glock18", off, [trunc(punch[0], 1e7), trunc(punch[1], 1e7)], [w.clip == 0, false]);
    w.glock_shots += 1;
    w.glock_shoot = if w.glock_shots == 3 { 0.0 } else { c.time + 0.1 };
}

pub fn secondary(w: &mut Weapon, c: &mut Ctx) {
    if c.shield_secondary_fire(w, 7, 8) {
        return;
    }
    match w.id {
        id::GLOCK18 => {
            w.weapon_state ^= 2;
            w.nsa = 0.3;
        }
        id::USP => {
            if w.weapon_state & 1 != 0 {
                w.weapon_state &= !1;
                c.send_anim(w.id, 15);
            } else {
                w.weapon_state |= 1;
                c.send_anim(w.id, 7);
            }
            w.nsa = 3.0;
            w.npa = 3.0;
            w.twi = 3.0;
        }
        _ => {}
    }
}

pub fn reload(w: &mut Weapon, c: &mut Ctx) {
    use super::info::Reserve;
    let reserve = super::info::item(w.id).map_or(Reserve::None, |i| i.reserve);
    if c.p.ammo.get(reserve) <= 0 {
        return;
    }
    let shield = c.g.holding_shield;
    let (clip, anim, delay) = match w.id {
        id::GLOCK18 => (20, if shield { 4 } else if c.random_long(0, 1) != 0 { 7 } else { 12 }, 2.2),
        id::USP => (12, if shield { 4 } else if w.weapon_state & 1 != 0 { 5 } else { 13 }, 2.7),
        id::P228 => (13, if shield { 4 } else { 5 }, 2.7),
        id::DEAGLE => (7, 4, 2.2),
        id::ELITE => (30, 14, 4.5),
        _ => (20, 4, 2.7),
    };
    if c.default_reload(w, clip, anim, delay) {
        w.accuracy = initial_accuracy(w.id);
    }
}

pub fn idle(w: &mut Weapon, c: &mut Ctx) {
    if w.twi > 0.0 {
        return;
    }
    if c.g.holding_shield && has_shield_model(w.id) {
        w.twi = 20.0;
        if w.weapon_state & SHIELD_DRAWN != 0 {
            c.send_anim(w.id, 6);
        }
        return;
    }
    match w.id {
        id::DEAGLE => w.twi = 20.0,
        _ if w.clip == 0 => {}
        id::GLOCK18 => {
            let r = c.random_float(0.0, 1.0);
            let (anim, idle) = if r <= 0.3 {
                (2, 3.0625)
            } else if r <= 0.6 {
                (0, 3.75)
            } else {
                (1, 2.5)
            };
            w.twi = idle;
            c.send_anim(w.id, anim);
        }
        id::USP => {
            w.twi = 60.0;
            c.send_anim(w.id, if w.weapon_state & 1 != 0 { 0 } else { 8 });
        }
        id::ELITE => {
            w.twi = 60.0;
            c.send_anim(w.id, if w.clip == 1 { 1 } else { 0 });
        }
        _ => {
            w.twi = 3.0625;
            c.send_anim(w.id, 0);
        }
    }
}
