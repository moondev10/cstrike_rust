//! `svc_temp_entity` payload parsing (docs/re/net-messages.md §9). Unknown ids are fatal in the
//! original because the payload length cannot be known.

use crate::ProtocolError;
use crate::bitbuf::MsgReader;
use crate::messages::{BeamParams, TempEntity, TextMessage};

/// The common beam tail. `width_scaled` is false for TE_BEAMTORUS/DISK/CYLINDER, whose width
/// byte is used raw; the scroll speed is `(float)(int)(byte * 0.1)`.
fn beam(r: &mut MsgReader, width_scaled: bool) -> BeamParams {
    BeamParams {
        model: r.read_i16(),
        start_frame: r.read_u8(),
        frame_rate: f32::from(r.read_u8()) * 0.1,
        life: f32::from(r.read_u8()) * 0.1,
        width: if width_scaled { f32::from(r.read_u8()) * 0.1 } else { f32::from(r.read_u8()) },
        noise: f32::from(r.read_u8()) * 0.01,
        color: [r.read_u8(), r.read_u8(), r.read_u8()],
        brightness: f32::from(r.read_u8()) / 255.0,
        scroll_speed: ((f32::from(r.read_u8()) * 0.1) as i32) as f32,
    }
}

fn rgb(r: &mut MsgReader) -> [u8; 3] {
    [r.read_u8(), r.read_u8(), r.read_u8()]
}

fn rgba(r: &mut MsgReader) -> [u8; 4] {
    [r.read_u8(), r.read_u8(), r.read_u8(), r.read_u8()]
}

fn tenths(v: u8) -> f32 {
    f32::from(v) * 0.1
}

pub fn parse(r: &mut MsgReader) -> Result<TempEntity, ProtocolError> {
    use TempEntity as T;
    let kind = r.read_u8();
    let te = match kind {
        0 => T::BeamPoints { start: r.read_coord3(), end: r.read_coord3(), beam: beam(r, true) },
        1 => T::BeamEntPoint { start_ent: r.read_i16(), end: r.read_coord3(), beam: beam(r, true) },
        2 => T::Gunshot { origin: r.read_coord3() },
        3 => T::Explosion {
            origin: r.read_coord3(),
            sprite: r.read_i16(),
            scale: tenths(r.read_u8()),
            frame_rate: r.read_u8(),
            flags: r.read_u8(),
        },
        4 => T::TarExplosion { origin: r.read_coord3() },
        5 => T::Smoke { origin: r.read_coord3(), sprite: r.read_i16(), scale: tenths(r.read_u8()), frame_rate: r.read_u8() },
        6 => T::Tracer { start: r.read_coord3(), end: r.read_coord3() },
        7 => T::Lightning {
            start: r.read_coord3(),
            end: r.read_coord3(),
            life: tenths(r.read_u8()),
            width: tenths(r.read_u8()),
            amplitude: f32::from(r.read_u8()) * 0.01,
            model: r.read_i16(),
        },
        8 => T::BeamEnts { start_ent: r.read_i16(), end_ent: r.read_i16(), beam: beam(r, true) },
        9 => T::Sparks { origin: r.read_coord3() },
        10 => T::LavaSplash { origin: r.read_coord3() },
        11 => T::Teleport { origin: r.read_coord3() },
        12 => T::Explosion2 { origin: r.read_coord3(), color_start: r.read_u8(), color_count: r.read_u8() },
        13 => {
            let origin = r.read_coord3();
            let decal = r.read_i16();
            let entity = r.read_i16();
            let model = if entity != 0 { r.read_i16() } else { 0 };
            T::BspDecal { origin, decal, entity, model }
        }
        14 => T::Implosion { origin: r.read_coord3(), radius: r.read_u8(), count: r.read_u8(), life: tenths(r.read_u8()) },
        15 => {
            let (start, end, sprite, count) = (r.read_coord3(), r.read_coord3(), r.read_i16(), r.read_u8());
            let life = tenths(r.read_u8());
            let scale = match r.read_u8() {
                0 => 1.0,
                s => tenths(s),
            };
            // The 4th byte is the random amplitude, the 5th the speed (opposite to the HLSDK
            // comment order).
            let randomness = f32::from(r.read_u8()) * 10.0;
            let velocity = ((f32::from(r.read_u8()) * 10.0) as i32) as f32;
            T::SpriteTrail { start, end, sprite, count, life, scale, velocity, randomness }
        }
        17 => T::Sprite { origin: r.read_coord3(), sprite: r.read_i16(), scale: tenths(r.read_u8()), brightness: r.read_u8() },
        18 => T::BeamSprite { start: r.read_coord3(), end: r.read_coord3(), beam_model: r.read_i16(), sprite_model: r.read_i16() },
        19 => T::BeamTorus { center: r.read_coord3(), axis: r.read_coord3(), beam: beam(r, false) },
        20 => T::BeamDisk { center: r.read_coord3(), axis: r.read_coord3(), beam: beam(r, false) },
        21 => T::BeamCylinder { center: r.read_coord3(), axis: r.read_coord3(), beam: beam(r, false) },
        22 => T::BeamFollow {
            entity: r.read_i16(),
            model: r.read_i16(),
            life: tenths(r.read_u8()),
            width: r.read_u8(),
            color: rgb(r),
            brightness: r.read_u8(),
        },
        23 => T::GlowSprite {
            origin: r.read_coord3(),
            model: r.read_i16(),
            life: tenths(r.read_u8()),
            scale: tenths(r.read_u8()),
            brightness: r.read_u8(),
        },
        24 => T::BeamRing { start_ent: r.read_i16(), end_ent: r.read_i16(), beam: beam(r, true) },
        25 => T::StreakSplash {
            origin: r.read_coord3(),
            dir: r.read_coord3(),
            color: r.read_u8(),
            count: r.read_i16(),
            speed: r.read_i16(),
            velocity_range: r.read_i16(),
        },
        27 => T::DLight {
            origin: r.read_coord3(),
            radius: f32::from(r.read_u8()) * 10.0,
            color: rgb(r),
            life: tenths(r.read_u8()),
            decay: f32::from(r.read_u8()) * 10.0,
        },
        28 => T::ELight {
            entity: r.read_i16(),
            origin: r.read_coord3(),
            radius: r.read_coord(),
            color: rgb(r),
            life: tenths(r.read_u8()),
            decay: r.read_coord(),
        },
        29 => {
            let channel = r.read_u8() & 3;
            let x = f32::from(r.read_i16()) / 8192.0;
            let y = f32::from(r.read_i16()) / 8192.0;
            let effect = r.read_u8();
            let color1 = rgba(r);
            let color2 = rgba(r);
            let fade_in = f32::from(r.read_u16()) / 256.0;
            let fade_out = f32::from(r.read_u16()) / 256.0;
            let hold = f32::from(r.read_u16()) / 256.0;
            let fx_time = if effect == 2 { f32::from(r.read_u16()) / 256.0 } else { 0.0 };
            let text = gs_common::bytestr::truncate_encoded(&r.read_string(), 511).into_owned();
            T::TextMessage(TextMessage { channel, x, y, effect, color1, color2, fade_in, fade_out, hold, fx_time, text })
        }
        30 => T::Line { start: r.read_coord3(), end: r.read_coord3(), life: f32::from(r.read_i16()) * 0.1, color: rgb(r) },
        31 => T::Box { mins: r.read_coord3(), maxs: r.read_coord3(), life: f32::from(r.read_i16()) * 0.1, color: rgb(r) },
        99 => T::KillBeam { entity: r.read_i16() },
        100 => T::LargeFunnel { origin: r.read_coord3(), sprite: r.read_i16(), flags: r.read_i16() },
        101 => T::BloodStream { origin: r.read_coord3(), dir: r.read_coord3(), color: r.read_u8(), speed: r.read_u8() },
        102 => T::ShowLine { start: r.read_coord3(), end: r.read_coord3() },
        103 => T::Blood { origin: r.read_coord3(), dir: r.read_coord3(), color: r.read_u8(), speed: r.read_u8() },
        104 => T::Decal { origin: r.read_coord3(), texture: u16::from(r.read_u8()), entity: r.read_i16() },
        105 => T::Fizz { entity: r.read_i16(), sprite: r.read_i16(), density: r.read_u8() },
        106 => T::Model {
            origin: r.read_coord3(),
            velocity: r.read_coord3(),
            yaw: r.read_angle(),
            model: r.read_i16(),
            bounce_sound: r.read_u8(),
            life: tenths(r.read_u8()),
        },
        107 => T::ExplodeModel {
            origin: r.read_coord3(),
            velocity: r.read_coord(),
            model: r.read_i16(),
            count: r.read_i16(),
            life: tenths(r.read_u8()),
        },
        108 => T::BreakModel {
            origin: r.read_coord3(),
            size: r.read_coord3(),
            velocity: r.read_coord3(),
            random_velocity: f32::from(r.read_u8()) * 10.0,
            model: r.read_i16(),
            count: r.read_u8(),
            life: tenths(r.read_u8()),
            flags: r.read_u8(),
        },
        109 => T::GunshotDecal { origin: r.read_coord3(), entity: r.read_i16(), decal: r.read_u8() },
        110 => T::SpriteSpray {
            origin: r.read_coord3(),
            velocity: r.read_coord3(),
            sprite: r.read_i16(),
            count: r.read_u8(),
            speed: f32::from(r.read_u8()) * 2.0,
            noise: r.read_u8(),
        },
        111 => T::ArmorRicochet { origin: r.read_coord3(), scale: tenths(r.read_u8()) },
        112 => T::PlayerDecal { player: r.read_u8(), origin: r.read_coord3(), entity: r.read_i16(), decal: r.read_u8() },
        113 | 114 => {
            let (mins, maxs, height, model, count, speed) =
                (r.read_coord3(), r.read_coord3(), r.read_coord(), r.read_i16(), r.read_u8(), r.read_coord());
            if kind == 113 {
                T::Bubbles { mins, maxs, height, model, count, speed }
            } else {
                T::BubbleTrail { mins, maxs, height, model, count, speed }
            }
        }
        115 => T::BloodSprite {
            origin: r.read_coord3(),
            sprite: r.read_i16(),
            drop_sprite: r.read_i16(),
            color: r.read_u8(),
            scale: r.read_u8(),
        },
        116 => T::WorldDecal { origin: r.read_coord3(), texture: u16::from(r.read_u8()) },
        117 => T::WorldDecal { origin: r.read_coord3(), texture: u16::from(r.read_u8()) + 256 },
        118 => T::Decal { origin: r.read_coord3(), texture: u16::from(r.read_u8()) + 256, entity: r.read_i16() },
        119 => T::Projectile {
            origin: r.read_coord3(),
            velocity: r.read_coord3(),
            model: r.read_i16(),
            life: r.read_u8(),
            owner: r.read_u8(),
        },
        120 => T::Spray {
            origin: r.read_coord3(),
            dir: r.read_coord3(),
            model: r.read_i16(),
            count: r.read_u8(),
            speed: r.read_u8(),
            noise: r.read_u8(),
            rendermode: r.read_u8(),
        },
        121 => T::PlayerSprites { player: r.read_i16(), model: r.read_i16(), count: r.read_u8(), variance: r.read_u8() },
        122 => T::ParticleBurst { origin: r.read_coord3(), radius: r.read_i16(), color: r.read_u8(), duration: tenths(r.read_u8()) },
        123 => T::FireField {
            origin: r.read_coord3(),
            radius: r.read_i16(),
            model: r.read_i16(),
            count: r.read_u8(),
            flags: r.read_u8(),
            duration: tenths(r.read_u8()),
        },
        124 => T::PlayerAttachment {
            entity: r.read_u8(),
            vertical_offset: r.read_coord(),
            model: r.read_i16(),
            life: f32::from(r.read_i16()) * 0.1,
        },
        125 => T::KillPlayerAttachments { entity: r.read_u8() },
        126 => {
            let origin = r.read_coord3();
            let dir = r.read_coord3().map(|c| (f64::from(c) / 10.0) as f32);
            let noise = [(f64::from(r.read_coord()) / 100.0) as f32, (f64::from(r.read_coord()) / 100.0) as f32];
            T::MultiGunshot { origin, dir, noise, count: r.read_u8(), decal: r.read_u8() }
        }
        127 => T::UserTracer {
            origin: r.read_coord3(),
            velocity: r.read_coord3(),
            life: tenths(r.read_u8()),
            color: r.read_u8(),
            length: tenths(r.read_u8()),
        },
        other => return Err(ProtocolError::Malformed(format!("CL_ParseTEnt: bad type {other}"))),
    };
    Ok(te)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitbuf::MsgWriter;

    fn parse_bytes(build: impl FnOnce(&mut MsgWriter)) -> TempEntity {
        let mut w = MsgWriter::new();
        build(&mut w);
        parse(&mut MsgReader::new(w.as_bytes())).expect("parses")
    }

    fn coords(w: &mut MsgWriter, v: [i16; 3]) {
        for c in v {
            w.write_i16(c * 8);
        }
    }

    #[test]
    fn sprite_trail_reads_amplitude_before_speed() {
        let te = parse_bytes(|w| {
            w.write_u8(15);
            coords(w, [0, 0, 0]);
            coords(w, [10, 0, 0]);
            w.write_i16(5);
            w.write_bytes(&[4, 20, 0, 3, 7]);
        });
        let TempEntity::SpriteTrail { scale, randomness, velocity, .. } = te else { panic!("{te:?}") };
        assert_eq!((scale, randomness, velocity), (1.0, 30.0, 70.0));
    }

    #[test]
    fn circle_beams_keep_the_raw_width_and_truncate_speed() {
        let te = parse_bytes(|w| {
            w.write_u8(21);
            coords(w, [0, 0, 0]);
            coords(w, [0, 0, 100]);
            w.write_i16(3);
            w.write_bytes(&[0, 0, 5, 16, 0, 255, 255, 255, 255, 25]);
        });
        let TempEntity::BeamCylinder { beam, .. } = te else { panic!("{te:?}") };
        assert_eq!((beam.width, beam.scroll_speed), (16.0, 2.0));
    }

    #[test]
    fn multi_gunshot_scales_direction_and_noise() {
        let te = parse_bytes(|w| {
            w.write_u8(126);
            coords(w, [0, 0, 0]);
            coords(w, [10, 0, -5]);
            w.write_i16(8);
            w.write_i16(16);
            w.write_bytes(&[6, 1]);
        });
        let TempEntity::MultiGunshot { dir, noise, count, .. } = te else { panic!("{te:?}") };
        assert_eq!((dir, noise, count), ([1.0, 0.0, -0.5], [0.01, 0.02], 6));
    }
}
