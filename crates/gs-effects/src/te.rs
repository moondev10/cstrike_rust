//! `CL_ParseTEnt` dispatch (docs/re/render.md §D.4): parsed `svc_temp_entity` payloads to efx
//! calls.

use gs_protocol::messages::{BeamParams, TempEntity};

use crate::beams::kind;
use crate::tents::render;
use crate::{Effects, Env, Event};

fn rgb(c: [u8; 3]) -> [f32; 3] {
    c.map(|v| f32::from(v) / 255.0)
}

impl Effects {
    /// Blood effects are on (`violence_hblood || violence_ablood`).
    fn blood_enabled(&self) -> bool {
        self.cvars.violence_hblood || self.cvars.violence_ablood
    }

    fn decal(&mut self, decal: i32, entity: i32, model: i32, origin: [f32; 3], flags: i32) {
        if self.cvars.r_decals {
            self.events.push(Event::Decal { decal, entity, model, origin, flags });
        }
    }

    /// Runs one temp-entity message.
    pub fn temp_entity(&mut self, env: &dyn Env, te: &TempEntity) {
        use TempEntity as T;
        let beam_tail = |b: &BeamParams| {
            (i32::from(b.model), b.life, b.width, b.noise, b.brightness, b.scroll_speed, i32::from(b.start_frame), b.frame_rate, rgb(b.color))
        };
        match te {
            T::BeamPoints { start, end, beam } => {
                let (m, life, w, amp, br, sp, sf, fr, c) = beam_tail(beam);
                self.beam_points(env, *start, *end, m, life, w, amp, br, sp, sf, fr, c);
            }
            T::BeamEntPoint { start_ent, end, beam } => {
                let (m, life, w, amp, br, sp, sf, fr, c) = beam_tail(beam);
                self.beam_ent_point(env, i32::from(*start_ent), *end, m, life, w, amp, br, sp, sf, fr, c);
            }
            T::Gunshot { origin } => {
                self.run_particle_effect(*origin, [0.0; 3], 0, 20);
                self.ricochet_sound(*origin);
            }
            T::Explosion { origin, sprite, scale, frame_rate, flags } => {
                self.explosion(env, *origin, i32::from(*sprite), *scale, f32::from(*frame_rate), i32::from(*flags));
            }
            T::TarExplosion { origin } => {
                self.blob_explosion(*origin);
                self.sound("weapons/explode3.wav", *origin, 1.0, 1.0, 100);
            }
            T::Smoke { origin, sprite, scale, frame_rate } => {
                if let Some(t) = self.default_sprite(env, *origin, i32::from(*sprite), f32::from(*frame_rate)) {
                    self.sprite_smoke(t, *scale);
                }
            }
            T::Tracer { start, end } => self.tracer_effect(*start, *end),
            T::Lightning { start, end, life, width, amplitude, model } => {
                self.beam_lightning(env, *start, *end, i32::from(*model), *life, *width, *amplitude, 0.6, 3.5);
            }
            T::BeamEnts { start_ent, end_ent, beam } | T::BeamRing { start_ent, end_ent, beam } => {
                let ring = matches!(te, T::BeamRing { .. });
                let (m, life, w, amp, br, sp, sf, fr, c) = beam_tail(beam);
                self.beam_ents(env, ring, i32::from(*start_ent), i32::from(*end_ent), m, life, w, amp, br, sp, sf, fr, c);
            }
            T::Sparks { origin } => self.spark_effect(env, *origin, 8, -200, 200),
            T::LavaSplash { origin } => self.lava_splash(*origin),
            T::Teleport { origin } => self.teleport_splash(*origin),
            T::Explosion2 { origin, color_start, color_count } => {
                self.particle_explosion2(*origin, i32::from(*color_start), i32::from(*color_count));
                self.add_dlight(*origin, 350.0, [0, 0, 0], 0.5, 300.0);
                self.sound("weapons/explode3.wav", *origin, 1.0, 0.6, 100);
            }
            T::BspDecal { origin, decal, entity, model } => {
                self.decal(i32::from(*decal), i32::from(*entity), i32::from(*model), *origin, 1);
            }
            T::Implosion { origin, radius, count, life } => self.implosion(*origin, f32::from(*radius), i32::from(*count), *life),
            T::SpriteTrail { start, end, sprite, count, life, scale, velocity, randomness } => {
                self.sprite_trail(env, *start, *end, i32::from(*sprite), i32::from(*count), *life, *scale, *randomness, 255, *velocity);
            }
            T::Sprite { origin, sprite, scale, brightness } => {
                let a = f32::from(*brightness) / 255.0;
                let model = env.model(i32::from(*sprite));
                self.temp_sprite(*origin, [0.0; 3], *scale, model, render::TRANS_ADD, 0, a, 0.0, crate::tent_flags::SPRANIMATE);
            }
            T::BeamSprite { start, end, beam_model, sprite_model } => {
                let bright = self.rng.float(0.5, 0.655);
                self.beam_points(env, *start, *end, i32::from(*beam_model), 0.01, 0.4, 0.0, bright, 5.0, 0, 0.0, [1.0, 0.0, 0.0]);
                let model = env.model(i32::from(*sprite_model));
                self.temp_sprite(*end, [0.0; 3], 0.1, model, render::TRANS_ADD, 0, 0.35, 0.01, 0);
            }
            T::BeamTorus { center, axis, beam } | T::BeamDisk { center, axis, beam } | T::BeamCylinder { center, axis, beam } => {
                let k = match te {
                    T::BeamTorus { .. } => kind::TORUS,
                    T::BeamDisk { .. } => kind::DISK,
                    _ => kind::CYLINDER,
                };
                let (m, life, w, amp, br, sp, sf, fr, c) = beam_tail(beam);
                self.beam_circle_points(env, k, *center, *axis, m, life, w, amp, br, sp, sf, fr, c);
            }
            T::BeamFollow { entity, model, life, width, color, brightness } => {
                let bright = f32::from(*brightness) / 255.0;
                self.beam_follow(env, i32::from(*entity), i32::from(*model), *life, f32::from(*width), rgb(*color), bright);
            }
            T::GlowSprite { origin, model, life, scale, brightness } => {
                let a = f32::from(*brightness) / 255.0;
                let m = env.model(i32::from(*model));
                let fx = render::FX_NO_DISSIPATION;
                self.temp_sprite(*origin, [0.0; 3], *scale, m, render::GLOW, fx, a, *life, crate::tent_flags::FADEOUT);
            }
            T::StreakSplash { origin, dir, color, count, speed, velocity_range } => {
                let r = i32::from(*velocity_range);
                self.streak_splash(*origin, *dir, i32::from(*color), i32::from(*count), f32::from(*speed), -r, r);
            }
            T::DLight { origin, radius, color, life, decay } => self.add_dlight(*origin, *radius, *color, *life, *decay),
            T::ELight { entity, origin, radius, color, life, decay } => {
                let die = (self.time + f64::from(*life)) as f32;
                let el = self.alloc_elight(i32::from(*entity));
                el.origin = *origin;
                el.radius = *radius;
                el.color = *color;
                el.die = die;
                el.decay = if *life != 0.0 { decay / life } else { *decay };
            }
            T::TextMessage(m) => self.events.push(Event::TextMessage(m.clone())),
            T::Line { start, end, life, color } => self.particle_line(*start, *end, *color, *life),
            T::Box { mins, maxs, life, color } => self.particle_box(*mins, *maxs, *color, *life),
            T::KillBeam { entity } => self.beam_kill(i32::from(*entity)),
            T::LargeFunnel { origin, sprite, flags } => {
                self.large_funnel(*origin, *flags != 0);
                self.funnel_sprite(env, *origin, i32::from(*sprite), *flags != 0);
            }
            T::BloodStream { origin, dir, color, speed } => {
                if self.blood_enabled() {
                    self.blood_stream(*origin, *dir, i32::from(*color), i32::from(*speed));
                }
            }
            T::ShowLine { start, end } => self.show_line(*start, *end),
            T::Blood { origin, dir, color, speed } => {
                if self.blood_enabled() {
                    self.blood(*origin, *dir, i32::from(*color), i32::from(*speed));
                }
            }
            T::Decal { origin, texture, entity } => self.decal(i32::from(*texture), i32::from(*entity), 0, *origin, 0),
            T::WorldDecal { origin, texture } => self.decal(i32::from(*texture), 0, 0, *origin, 0),
            T::Fizz { entity, sprite, density } => self.fizz_effect(env, i32::from(*entity), i32::from(*sprite), i32::from(*density)),
            T::Model { origin, velocity, yaw, model, bounce_sound, life } => {
                let m = env.model(i32::from(*model));
                self.temp_model(*origin, *velocity, [0.0, *yaw, 0.0], *life, m, i32::from(*bounce_sound));
            }
            T::ExplodeModel { origin, velocity, model, count, life } => {
                self.temp_sphere_model(env, *origin, *velocity, *life, i32::from(*count), i32::from(*model));
            }
            T::BreakModel { origin, size, velocity, random_velocity, model, count, life, flags } => {
                let (m, n, f) = (i32::from(*model), i32::from(*count), i32::from(*flags));
                self.break_model(env, *origin, *size, *velocity, *random_velocity, *life, n, m, f);
            }
            T::GunshotDecal { origin, entity, decal } => {
                self.bullet_impact_particles(*origin);
                let n = self.rng.long(0, 0x7FFF);
                if n < 0x3FFF {
                    self.sound(format!("weapons/ric{}.wav", n % 5 + 1), *origin, 1.0, 1.0, 100);
                }
                self.decal(i32::from(*decal), i32::from(*entity), 0, *origin, 0);
            }
            T::SpriteSpray { origin, velocity, sprite, count, speed, noise } => {
                self.sprite_spray(env, *origin, *velocity, i32::from(*sprite), i32::from(*count), *speed, i32::from(*noise));
            }
            T::ArmorRicochet { origin, scale } => {
                self.ricochet_sprite(*origin, env.model_by_name("sprites/richo1.spr"), *scale);
                self.ricochet_sound(*origin);
            }
            T::PlayerDecal { player, origin, entity, decal } => {
                if self.cvars.r_decals {
                    self.events.push(Event::PlayerDecal { player: *player, origin: *origin, entity: i32::from(*entity), decal: *decal });
                }
            }
            T::Bubbles { mins, maxs, height, model, count, speed } => {
                self.bubbles(env, *mins, *maxs, *height, i32::from(*model), i32::from(*count), *speed);
            }
            T::BubbleTrail { mins, maxs, height, model, count, speed } => {
                self.bubble_trail(env, *mins, *maxs, *height, i32::from(*model), i32::from(*count), *speed);
            }
            T::BloodSprite { origin, sprite, drop_sprite, color, scale } => {
                if self.blood_enabled() {
                    let (s, d) = (i32::from(*sprite), i32::from(*drop_sprite));
                    self.blood_sprite(env, *origin, i32::from(*color), s, d, f32::from(*scale));
                }
            }
            T::Projectile { origin, velocity, model, life, owner } => {
                self.projectile(env, *origin, *velocity, i32::from(*model), i32::from(*life), i32::from(*owner));
            }
            T::Spray { origin, dir, model, count, speed, noise, rendermode } => {
                let (m, c, s, n, r) = (i32::from(*model), i32::from(*count), i32::from(*speed), i32::from(*noise), i32::from(*rendermode));
                self.spray(env, *origin, *dir, m, c, s, n, r);
            }
            T::PlayerSprites { player, model, count, variance } => {
                self.player_sprites(env, i32::from(*player), i32::from(*model), i32::from(*count), i32::from(*variance));
            }
            T::ParticleBurst { origin, radius, color, duration } => {
                self.particle_burst(*origin, i32::from(*radius), i32::from(*color), *duration);
            }
            T::FireField { origin, radius, model, count, flags, duration } => {
                let (r, m, c, f) = (i32::from(*radius), i32::from(*model), i32::from(*count), i32::from(*flags));
                self.fire_field(env, *origin, r, m, c, f, *duration);
            }
            T::PlayerAttachment { entity, vertical_offset, model, life } => {
                self.attach_tent_to_player(env, i32::from(*entity), i32::from(*model), *vertical_offset, *life);
            }
            T::KillPlayerAttachments { entity } => self.kill_attached_tents(env, i32::from(*entity)),
            T::MultiGunshot { origin, dir, noise, count, decal } => {
                self.multi_gunshot(env, *origin, *dir, *noise, i32::from(*count), &[i32::from(*decal)]);
            }
            T::UserTracer { origin, velocity, life, color, length } => {
                self.user_tracer(*origin, *velocity, *life, i32::from(*color), *length);
            }
        }
    }
}
