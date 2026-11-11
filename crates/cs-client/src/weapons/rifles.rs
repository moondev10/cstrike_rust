//! SMGs, rifles, snipers and the M249 (docs/re/client-sim.md Part D): the automatic-weapon
//! fire skeleton with integer-division accuracy, FAMAS burst, M4A1 silencer, the zooms and
//! the snipers' time-based accuracy and predicted punch. `KickBack` is empty on the client.

use super::{Ctx, Weapon, id, trunc, view_model};

/// The automatic weapons' per-shot data (Part D §3.6–§3.8).
struct Auto {
    /// Spread `(base, factor·A)` in the air, moving fast, otherwise: double constants, the sum
    /// rounded to float once (`cvtps2pd A; mulsd; addsd; cvtpd2ps`).
    air: (f64, f64),
    fast: Option<(f64, f64)>,
    slow: (f64, f64),
    threshold: f32,
    cycle: f32,
    idle: f32,
    /// `iparam` scale of the punch.
    scale: f32,
    bparam1: bool,
}

fn auto(w: &Weapon) -> Option<Auto> {
    let a = |air, fast, slow, threshold, cycle, idle, scale, bparam1| Auto { air, fast, slow, threshold, cycle, idle, scale, bparam1 };
    Some(match w.id {
        id::MAC10 => a((0.0, 0.375), None, (0.0, 0.03), 0.0, 0.07, 2.0, 100.0, false),
        id::TMP => a((0.0, 0.25), None, (0.0, 0.03), 0.0, 0.07, 2.0, 100.0, true),
        id::MP5N => a((0.0, 0.2), None, (0.0, 0.04), 0.0, 0.075, 2.0, 100.0, false),
        id::UMP45 => a((0.0, 0.24), None, (0.0, 0.04), 0.0, 0.1, 2.0, 100.0, false),
        id::P90 => a((0.0, 0.3), Some((0.0, 0.115)), (0.0, 0.045), 170.0, 0.066, 2.0, 100.0, true),
        id::GALIL => a((0.04, 0.3), Some((0.04, 0.07)), (0.0, 0.0375), 140.0, 0.0875, 1.28, 1e7, false),
        id::FAMAS => a((0.03, 0.3), Some((0.03, 0.07)), (0.0, 0.02), 140.0, 0.0825, 1.1, 1e7, false),
        id::AK47 => a((0.04, 0.4), Some((0.04, 0.07)), (0.0, 0.0275), 140.0, 0.0955, 1.9, 100.0, false),
        id::M4A1 => {
            let slow = if w.weapon_state & 4 != 0 { 0.025 } else { 0.02 };
            a((0.035, 0.4), Some((0.035, 0.07)), (0.0, slow), 140.0, 0.0875, 1.5, 100.0, w.weapon_state & 4 != 0)
        }
        id::SG552 => a((0.035, 0.45), Some((0.035, 0.075)), (0.0, 0.02), 140.0, 0.0825, 2.0, 100.0, true),
        id::AUG => a((0.035, 0.4), Some((0.035, 0.07)), (0.0, 0.02), 140.0, 0.0825, 1.9, 100.0, false),
        id::M249 => a((0.045, 0.5), Some((0.045, 0.095)), (0.0, 0.03), 140.0, 0.1, 1.6, 100.0, false),
        _ => return None,
    })
}

/// `m_flAccuracy` after the n-th shot (integer division except the MP5N), and its cap.
fn accuracy(id: i32, n: i32) -> f32 {
    let (cube, sq) = (n * n * n, n * n);
    let (v, max) = match id {
        id::MAC10 => (f64::from(cube / 200) + 0.6, 1.65),
        id::TMP => (f64::from(cube / 200) + 0.55, 1.4),
        id::MP5N => (f64::from(sq) / 220.1 + 0.45, 0.75),
        id::UMP45 => (f64::from(sq / 210) + 0.5, 1.0),
        id::P90 => (f64::from(sq / 175) + 0.45, 1.0),
        id::GALIL => (f64::from(cube / 200) + 0.35, 1.25),
        id::FAMAS => (f64::from(cube / 215) + 0.3, 1.0),
        id::AK47 => (f64::from(cube / 200) + 0.35, 1.25),
        id::M4A1 | id::SG552 => (f64::from(cube / 220) + 0.3, 1.0),
        id::AUG => (f64::from(cube / 215) + 0.3, 1.0),
        _ => (f64::from(cube / 175) + 0.4, 0.9),
    };
    (v as f32).min(max)
}

fn event_name(id: i32) -> &'static str {
    super::info::item(id).map_or("", |i| i.event)
}

pub fn deploy(w: &mut Weapon, c: &mut Ctx) -> bool {
    let info = super::info::item(w.id);
    let model = view_model(info.map_or("", |i| i.model), false);
    let anim = match w.id {
        id::M4A1 => {
            if w.weapon_state & 4 != 0 {
                5
            } else {
                12
            }
        }
        id::SCOUT | id::G3SG1 | id::SG550 | id::M249 => 4,
        id::AWP => 5,
        _ => 2,
    };
    match w.id {
        id::MAC10 => {
            w.accuracy = 0.15;
            w.delay_fire = false;
        }
        id::MP5N | id::UMP45 => {
            w.accuracy = 0.0;
            w.delay_fire = false;
        }
        id::TMP | id::P90 => {
            w.shots_fired = 0;
            w.accuracy = 0.2;
            w.delay_fire = false;
        }
        id::FAMAS => {
            w.shots_fired = 0;
            w.accuracy = 0.2;
            w.famas_shots = 0;
            w.famas_shoot = 0.0;
        }
        id::M4A1 => {
            w.shots_fired = 0;
            w.accuracy = 0.2;
            w.delay_fire = true;
        }
        id::GALIL | id::AK47 | id::SG552 | id::AUG | id::M249 => {
            w.shots_fired = 0;
            w.accuracy = 0.2;
        }
        id::G3SG1 => w.accuracy = 0.2,
        _ => {}
    }
    let deployed = c.default_deploy(w, &model, anim);
    if deployed {
        match w.id {
            id::SCOUT => {
                w.npa = 1.25;
                c.p.next_attack = 1.25;
                w.nsa = 1.0;
            }
            id::AWP => {
                w.npa = 1.45;
                c.p.next_attack = 1.45;
                w.nsa = 1.0;
            }
            _ => {}
        }
    }
    deployed
}

pub fn primary(w: &mut Weapon, c: &mut Ctx) {
    if matches!(w.id, id::GALIL | id::FAMAS) && c.p.waterlevel == 3 {
        w.npa = 0.15;
        return;
    }
    if let Some(a) = auto(w) {
        let acc = f64::from(w.accuracy);
        let pick = |(base, k): (f64, f64)| (base + k * acc) as f32;
        let spread = if !c.on_ground() {
            pick(a.air)
        } else if let Some(fast) = a.fast.filter(|_| c.speed2d() > a.threshold) {
            pick(fast)
        } else {
            pick(a.slow)
        };
        let zoomed_slow = matches!(w.id, id::SG552 | id::AUG) && c.p.fov != 90.0 && c.on_ground() && c.speed2d() <= a.threshold;
        let cycle = if zoomed_slow { 0.135 } else { a.cycle };
        auto_fire(w, c, &a, spread, cycle);
        return;
    }
    sniper_primary(w, c);
}

/// `<W>Fire(spread, cycle)` of the automatic weapons (Part D §2.1, FAMAS §4.3).
fn auto_fire(w: &mut Weapon, c: &mut Ctx, a: &Auto, spread: f32, cycle: f32) {
    let burst = w.id == id::FAMAS && w.weapon_state & 0x10 != 0;
    let (mut spread, mut cycle) = (spread, cycle);
    if w.id == id::FAMAS {
        if burst {
            w.famas_shots = 0;
            cycle = 0.55;
        } else {
            spread = (f64::from(spread) + 0.01) as f32;
        }
    }
    w.delay_fire = true;
    w.shots_fired += 1;
    w.accuracy = accuracy(w.id, w.shots_fired);
    if w.clip <= 0 {
        if w.fire_on_empty {
            w.npa = 0.2;
        }
        return;
    }
    w.clip -= 1;
    let off = c.spread(spread);
    let punch = c.p.punchangle;
    c.playback(event_name(w.id), off, [trunc(punch[0], a.scale), trunc(punch[1], a.scale)], [a.bparam1, false]);
    if w.id == id::M4A1 {
        c.p.ammo.nato556 -= 1;
    }
    w.npa = cycle;
    w.nsa = cycle;
    w.twi = a.idle;
    if burst {
        w.famas_shoot = c.time + 0.05;
        w.famas_shots += 1;
        w.burst_spread = spread;
    }
}

/// The FAMAS burst's follow-up rounds (`ItemPostFrame`'s inlined `FireRemaining`).
pub fn fire_remaining_famas(w: &mut Weapon, c: &mut Ctx) {
    w.clip -= 1;
    if w.clip < 0 {
        w.clip = 0;
        w.famas_shots = 3;
        w.famas_shoot = 0.0;
        return;
    }
    let off = c.spread(w.burst_spread);
    let punch = c.p.punchangle;
    c.playback("famas", off, [trunc(punch[0], 1e7), trunc(punch[1], 1e7)], [false, false]);
    w.famas_shots += 1;
    w.famas_shoot = if w.famas_shots == 3 { 0.0 } else { c.time + 0.1 };
}

/// Scout, AWP, G3SG1, SG550 `PrimaryAttack` + `<S>Fire`.
fn sniper_primary(w: &mut Weapon, c: &mut Ctx) {
    let moving = c.speed2d();
    let a = w.accuracy;
    let one_minus = |k: f64| ((1.0 - f64::from(a)) * k) as f32;
    let mut spread = match w.id {
        id::SCOUT if !c.on_ground() => 0.2,
        id::SCOUT if moving > 170.0 => 0.075,
        id::SCOUT if c.ducking() => 0.0,
        id::SCOUT => 0.007,
        id::AWP if !c.on_ground() => 0.85,
        id::AWP if moving > 140.0 => 0.25,
        id::AWP if moving > 10.0 => 0.1,
        id::AWP if c.ducking() => 0.0,
        id::AWP => 0.001,
        id::G3SG1 if !c.on_ground() => 0.45,
        id::G3SG1 if moving > 0.0 => 0.15,
        id::G3SG1 if c.ducking() => 0.035,
        id::G3SG1 => 0.055,
        _ if !c.on_ground() => one_minus(0.45),
        _ if moving > 0.0 => 0.15,
        _ if c.ducking() => one_minus(0.04),
        _ => one_minus(0.05),
    };
    let fov90 = c.p.fov == 90.0;
    match w.id {
        id::SCOUT | id::AWP => {
            if fov90 {
                spread = (f64::from(spread) + if w.id == id::SCOUT { 0.025 } else { 0.08 }) as f32;
            } else {
                c.p.resume_zoom = true;
                c.p.last_zoom = c.p.i_fov;
                c.p.i_fov = 90;
                if w.id == id::AWP {
                    c.p.fov = 90.0;
                }
            }
        }
        _ => {
            if fov90 {
                spread = (f64::from(spread) + 0.025) as f32;
            }
            let now = c.time;
            if w.id == id::G3SG1 {
                if w.last_fire == 0.0 {
                    w.accuracy = 0.98;
                } else {
                    w.accuracy = ((f64::from(now - w.last_fire) * 0.3 + 0.55) as f32).min(0.98);
                }
            } else if w.last_fire != 0.0 {
                w.accuracy = ((f64::from(now - w.last_fire) * 0.35 + 0.65) as f32).min(0.98);
            }
            w.last_fire = now;
        }
    }
    if w.clip <= 0 {
        if w.fire_on_empty {
            w.npa = 0.2;
        }
        return;
    }
    w.clip -= 1;
    if w.id == id::G3SG1 {
        spread *= 1.0 - w.accuracy;
    }
    let off = c.spread(spread);
    let punch = c.p.punchangle;
    let px = trunc(punch[0], 100.0);
    let (event, fparam, iparam, bparam1, cycle, idle) = match w.id {
        id::SCOUT => ("scout", [off[0] * 1000.0, off[1] * 1000.0], [px, trunc(punch[1], 100.0)], false, 1.25, 1.8),
        id::AWP => ("awp", off, [px, px], false, 1.45, 2.0),
        id::G3SG1 => ("g3sg1", off, [px, trunc(punch[1], 100.0)], false, 0.25, 1.8),
        _ => ("sg550", off, [px, trunc(punch[1], 100.0)], true, 0.25, 1.8),
    };
    c.playback(event, fparam, iparam, [bparam1, false]);
    w.npa = cycle;
    w.nsa = cycle;
    w.twi = idle;
    match w.id {
        id::SCOUT | id::AWP => c.p.punchangle[0] -= 2.0,
        _ => {
            let hi = if w.id == id::G3SG1 { 1.75 } else { 1.25 };
            let px = c.p.punchangle[0];
            c.p.punchangle[0] -= c.shared_float(4, 0.75, hi) + px * 0.25;
            c.p.punchangle[1] += c.shared_float(5, -0.75, 0.75);
        }
    }
}

pub fn secondary(w: &mut Weapon, c: &mut Ctx) {
    match w.id {
        id::FAMAS => {
            w.weapon_state ^= 0x10;
            w.nsa = 0.3;
        }
        id::M4A1 => {
            if w.weapon_state & 4 != 0 {
                w.weapon_state &= !4;
                c.send_anim(w.id, 13);
            } else {
                w.weapon_state |= 4;
                c.send_anim(w.id, 6);
            }
            w.nsa = 2.0;
            w.npa = 2.0;
            w.twi = 2.0;
        }
        id::SG552 | id::AUG => {
            c.p.i_fov = if c.p.i_fov == 90 { 55 } else { 90 };
            c.p.fov = c.p.i_fov as f32;
            w.nsa = 0.3;
        }
        id::SCOUT | id::G3SG1 | id::SG550 | id::AWP => {
            let last = if w.id == id::AWP { 10 } else { 15 };
            let next = match c.p.i_fov {
                90 => 40,
                40 => last,
                f if f == last && w.id != id::AWP => 90,
                f if w.id != id::AWP => f,
                _ => 90,
            };
            c.p.i_fov = next;
            c.p.fov = next as f32;
            w.nsa = 0.3;
        }
        _ => {}
    }
}

pub fn reload(w: &mut Weapon, c: &mut Ctx) {
    use super::info::Reserve;
    let reserve = super::info::item(w.id).map_or(Reserve::None, |i| i.reserve);
    if w.id != id::SCOUT && c.p.ammo.get(reserve) <= 0 {
        return;
    }
    let (clip, anim, delay) = match w.id {
        id::MAC10 => (30, 1, 3.15),
        id::TMP => (30, 1, 2.12),
        id::MP5N => (30, 1, 2.63),
        id::UMP45 => (25, 1, 3.5),
        id::P90 => (50, 1, 3.4),
        id::GALIL => (35, 1, 2.45),
        id::FAMAS => (25, 1, 3.3),
        id::AK47 => (30, 1, 2.45),
        id::M4A1 => (30, if w.weapon_state & 4 != 0 { 4 } else { 11 }, 3.05),
        id::SG552 => (30, 1, 3.0),
        id::AUG => (30, 1, 3.3),
        id::SCOUT => (10, 3, 2.0),
        id::AWP => (10, 4, 2.5),
        id::G3SG1 => (20, 3, 3.5),
        id::SG550 => (30, 3, 3.35),
        _ => (100, 3, 4.7),
    };
    if !c.default_reload(w, clip, anim, delay) {
        return;
    }
    // The SMG reloads (MAC10, MP5N, UMP45, TMP, P90) set only the accuracy and the shot count;
    // the rifles and the M249 also clear m_bDelayFire.
    match w.id {
        id::MAC10 | id::MP5N | id::UMP45 => {
            w.accuracy = 0.0;
            w.shots_fired = 0;
        }
        id::TMP | id::P90 => {
            w.accuracy = 0.2;
            w.shots_fired = 0;
        }
        id::AUG | id::FAMAS => {
            w.accuracy = 0.0;
            w.shots_fired = 0;
            w.delay_fire = false;
        }
        id::GALIL | id::AK47 | id::M4A1 | id::SG552 | id::M249 => {
            w.accuracy = 0.2;
            w.shots_fired = 0;
            w.delay_fire = false;
        }
        id::G3SG1 => w.accuracy = 0.2,
        _ => {}
    }
    // Reloading unzooms.
    match w.id {
        id::FAMAS | id::AUG if c.p.i_fov != 90 => secondary(w, c),
        id::SG552 if c.p.fov != 90.0 => secondary(w, c),
        id::SCOUT if c.p.i_fov != 90 => {
            c.p.i_fov = 15;
            c.p.fov = 15.0;
            secondary(w, c);
        }
        id::AWP if c.p.fov != 90.0 => {
            c.p.i_fov = 10;
            c.p.fov = 10.0;
            secondary(w, c);
        }
        id::G3SG1 | id::SG550 if c.p.fov != 90.0 => {
            c.p.i_fov = 15;
            c.p.fov = 15.0;
            secondary(w, c);
        }
        _ => {}
    }
}

pub fn idle(w: &mut Weapon, c: &mut Ctx) {
    if w.twi > 0.0 {
        return;
    }
    match w.id {
        id::SCOUT | id::AWP | id::G3SG1 | id::SG550 => {
            if w.clip != 0 {
                w.twi = 60.0;
                c.send_anim(w.id, 0);
            }
        }
        id::M4A1 => {
            w.twi = 20.0;
            c.send_anim(w.id, if w.weapon_state & 4 != 0 { 0 } else { 7 });
        }
        _ => {
            w.twi = 20.0;
            c.send_anim(w.id, 0);
        }
    }
}
