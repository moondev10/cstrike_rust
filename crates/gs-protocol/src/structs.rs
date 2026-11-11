//! Delta-encoded records as C-layout byte images (offsets from docs/re/net-messages.md §4.5) with
//! typed accessors. Keeping the original layout lets server-sent field offsets apply directly.

use crate::delta::size;

macro_rules! record {
    ($(#[$doc:meta])* $name:ident, $size:expr) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name(pub Box<[u8; $size]>);

        impl Default for $name {
            fn default() -> Self {
                Self(Box::new([0u8; $size]))
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }

        impl $name {
            pub const SIZE: usize = $size;

            pub fn bytes(&self) -> &[u8] {
                &self.0[..]
            }

            pub fn bytes_mut(&mut self) -> &mut [u8] {
                &mut self.0[..]
            }

            fn f32_at(&self, off: usize) -> f32 {
                f32::from_le_bytes([self.0[off], self.0[off + 1], self.0[off + 2], self.0[off + 3]])
            }

            fn i32_at(&self, off: usize) -> i32 {
                i32::from_le_bytes([self.0[off], self.0[off + 1], self.0[off + 2], self.0[off + 3]])
            }

            #[allow(dead_code)]
            fn i16_at(&self, off: usize) -> i16 {
                i16::from_le_bytes([self.0[off], self.0[off + 1]])
            }

            #[allow(dead_code)]
            fn vec3_at(&self, off: usize) -> [f32; 3] {
                [self.f32_at(off), self.f32_at(off + 4), self.f32_at(off + 8)]
            }

            fn set_f32(&mut self, off: usize, v: f32) {
                self.0[off..off + 4].copy_from_slice(&v.to_le_bytes());
            }

            #[allow(dead_code)]
            fn set_i32(&mut self, off: usize, v: i32) {
                self.0[off..off + 4].copy_from_slice(&v.to_le_bytes());
            }

            #[allow(dead_code)]
            fn set_vec3(&mut self, off: usize, v: [f32; 3]) {
                for (i, c) in v.iter().enumerate() {
                    self.set_f32(off + i * 4, *c);
                }
            }
        }
    };
}

macro_rules! getters {
    ($ty:ident { $($name:ident: $kind:ident @ $off:expr),* $(,)? }) => {
        impl $ty {
            $(getters!(@one $name $kind $off);)*
        }
    };
    (@one $name:ident f32 $off:expr) => { pub fn $name(&self) -> f32 { self.f32_at($off) } };
    (@one $name:ident i32 $off:expr) => { pub fn $name(&self) -> i32 { self.i32_at($off) } };
    (@one $name:ident i16 $off:expr) => { pub fn $name(&self) -> i16 { self.i16_at($off) } };
    (@one $name:ident u8 $off:expr) => { pub fn $name(&self) -> u8 { self.0[$off] } };
    (@one $name:ident vec3 $off:expr) => { pub fn $name(&self) -> [f32; 3] { self.vec3_at($off) } };
}

macro_rules! setters {
    ($ty:ident { $($name:ident: $kind:ident @ $off:expr),* $(,)? }) => {
        impl $ty {
            $(setters!(@one $name $kind $off);)*
        }
    };
    (@one $name:ident f32 $off:expr) => { pub fn $name(&mut self, v: f32) { self.set_f32($off, v) } };
    (@one $name:ident i32 $off:expr) => { pub fn $name(&mut self, v: i32) { self.set_i32($off, v) } };
}

record!(
    /// `entity_state_t` (340 bytes); also the layout of player and custom (beam) states.
    EntityState,
    size::ENTITY_STATE
);
getters!(EntityState {
    entity_type: i32 @ 0, number: i32 @ 4, msg_time: f32 @ 8, messagenum: i32 @ 12,
    origin: vec3 @ 16, angles: vec3 @ 28, modelindex: i32 @ 40, sequence: i32 @ 44, frame: f32 @ 48,
    colormap: i32 @ 52, skin: i16 @ 56, solid: i16 @ 58, effects: i32 @ 60, scale: f32 @ 64, eflags: i32 @ 68,
    rendermode: i32 @ 72, renderamt: i32 @ 76, renderfx: i32 @ 84, movetype: i32 @ 88, animtime: f32 @ 92,
    framerate: f32 @ 96, body: i32 @ 100, velocity: vec3 @ 112, mins: vec3 @ 124, maxs: vec3 @ 136,
    aiment: i32 @ 148, owner: i32 @ 152, friction: f32 @ 156, gravity: f32 @ 160, team: i32 @ 164,
    playerclass: i32 @ 168, health: i32 @ 172, spectator: i32 @ 176, weaponmodel: i32 @ 180,
    gaitsequence: i32 @ 184, basevelocity: vec3 @ 188, usehull: i32 @ 200, oldbuttons: i32 @ 204,
    onground: i32 @ 208, step_left: i32 @ 212, fall_velocity: f32 @ 216, weaponanim: i32 @ 224,
    startpos: vec3 @ 228, endpos: vec3 @ 240, impacttime: f32 @ 252, starttime: f32 @ 256,
    iuser1: i32 @ 260, iuser2: i32 @ 264, iuser3: i32 @ 268, iuser4: i32 @ 272,
    fuser1: f32 @ 276, fuser2: f32 @ 280, fuser3: f32 @ 284, fuser4: f32 @ 288,
    vuser1: vec3 @ 292, vuser2: vec3 @ 304, vuser3: vec3 @ 316, vuser4: vec3 @ 328,
});

impl EntityState {
    pub fn rendercolor(&self) -> [u8; 3] {
        [self.0[80], self.0[81], self.0[82]]
    }

    pub fn controller(&self) -> [u8; 4] {
        [self.0[104], self.0[105], self.0[106], self.0[107]]
    }

    pub fn blending(&self) -> [u8; 2] {
        [self.0[108], self.0[109]]
    }

    pub fn set_entity_type(&mut self, v: i32) {
        self.set_i32(0, v);
    }

    pub fn set_number(&mut self, v: i32) {
        self.set_i32(4, v);
    }

    pub fn set_msg_time(&mut self, v: f32) {
        self.set_f32(8, v);
    }

    pub fn set_messagenum(&mut self, v: i32) {
        self.set_i32(12, v);
    }

    pub fn set_origin(&mut self, v: [f32; 3]) {
        self.set_vec3(16, v);
    }

    pub fn set_angles(&mut self, v: [f32; 3]) {
        self.set_vec3(28, v);
    }

    pub fn set_movetype(&mut self, v: i32) {
        self.set_i32(88, v);
    }

    pub fn set_animtime(&mut self, v: f32) {
        self.set_f32(92, v);
    }

    pub fn set_controller(&mut self, v: [u8; 4]) {
        self.0[104..108].copy_from_slice(&v);
    }

    pub fn set_blending(&mut self, v: [u8; 2]) {
        self.0[108..110].copy_from_slice(&v);
    }

    pub fn set_renderamt(&mut self, v: i32) {
        self.set_i32(76, v);
    }

    pub fn set_sequence(&mut self, v: i32) {
        self.set_i32(44, v);
    }

    pub fn set_gaitsequence(&mut self, v: i32) {
        self.set_i32(184, v);
    }

    pub fn set_modelindex(&mut self, v: i32) {
        self.set_i32(40, v);
    }

    pub fn set_frame(&mut self, v: f32) {
        self.set_f32(48, v);
    }

    pub fn set_framerate(&mut self, v: f32) {
        self.set_f32(96, v);
    }

    pub fn set_body(&mut self, v: i32) {
        self.set_i32(100, v);
    }

    pub fn set_colormap(&mut self, v: i32) {
        self.set_i32(52, v);
    }

    pub fn set_skin(&mut self, v: i16) {
        self.0[56..58].copy_from_slice(&v.to_le_bytes());
    }

    pub fn set_effects(&mut self, v: i32) {
        self.set_i32(60, v);
    }

    pub fn set_scale(&mut self, v: f32) {
        self.set_f32(64, v);
    }

    pub fn set_rendermode(&mut self, v: i32) {
        self.set_i32(72, v);
    }

    pub fn set_rendercolor(&mut self, v: [u8; 3]) {
        self.0[80..83].copy_from_slice(&v);
    }

    pub fn set_renderfx(&mut self, v: i32) {
        self.set_i32(84, v);
    }
}

record!(
    /// `clientdata_t` (476 bytes): the local player's state from `svc_clientdata`.
    ClientData,
    size::CLIENT_DATA
);
getters!(ClientData {
    origin: vec3 @ 0, velocity: vec3 @ 12, viewmodel: i32 @ 24, punchangle: vec3 @ 28, flags: i32 @ 40,
    waterlevel: i32 @ 44, watertype: i32 @ 48, view_ofs: vec3 @ 52, health: f32 @ 64, in_duck: i32 @ 68,
    weapons: i32 @ 72, time_step_sound: i32 @ 76, duck_time: i32 @ 80, swim_time: i32 @ 84,
    waterjumptime: i32 @ 88, maxspeed: f32 @ 92, fov: f32 @ 96, weaponanim: i32 @ 100, m_id: i32 @ 104,
    ammo_shells: i32 @ 108, ammo_nails: i32 @ 112, ammo_cells: i32 @ 116, ammo_rockets: i32 @ 120,
    next_attack: f32 @ 124, tfstate: i32 @ 128, pushmsec: i32 @ 132, deadflag: i32 @ 136,
    iuser1: i32 @ 396, iuser2: i32 @ 400, iuser3: i32 @ 404, iuser4: i32 @ 408,
    fuser1: f32 @ 412, fuser2: f32 @ 416, fuser3: f32 @ 420, fuser4: f32 @ 424,
    vuser1: vec3 @ 428, vuser2: vec3 @ 440, vuser3: vec3 @ 452, vuser4: vec3 @ 464,
});

impl ClientData {
    /// `physinfo` key/value string (char[256] at 140).
    pub fn physinfo(&self) -> String {
        let raw = &self.0[140..396];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        String::from_utf8_lossy(&raw[..end]).into_owned()
    }
}

record!(
    /// `weapon_data_t` (88 bytes).
    WeaponData,
    size::WEAPON_DATA
);
getters!(WeaponData {
    id: i32 @ 0, clip: i32 @ 4, next_primary_attack: f32 @ 8, next_secondary_attack: f32 @ 12,
    time_weapon_idle: f32 @ 16, in_reload: i32 @ 20, in_special_reload: i32 @ 24, next_reload: f32 @ 28,
    pump_time: f32 @ 32, reload_time: f32 @ 36, aimed_damage: f32 @ 40, next_aim_bonus: f32 @ 44,
    in_zoom: i32 @ 48, weapon_state: i32 @ 52, iuser1: i32 @ 56, iuser2: i32 @ 60, iuser3: i32 @ 64,
    iuser4: i32 @ 68, fuser1: f32 @ 72, fuser2: f32 @ 76, fuser3: f32 @ 80, fuser4: f32 @ 84,
});
setters!(WeaponData {
    set_id: i32 @ 0, set_clip: i32 @ 4, set_next_primary_attack: f32 @ 8, set_next_secondary_attack: f32 @ 12,
    set_time_weapon_idle: f32 @ 16, set_in_reload: i32 @ 20, set_in_special_reload: i32 @ 24, set_next_reload: f32 @ 28,
    set_pump_time: f32 @ 32, set_reload_time: f32 @ 36, set_aimed_damage: f32 @ 40, set_next_aim_bonus: f32 @ 44,
    set_in_zoom: i32 @ 48, set_weapon_state: i32 @ 52, set_iuser1: i32 @ 56, set_iuser2: i32 @ 60, set_iuser3: i32 @ 64,
    set_iuser4: i32 @ 68, set_fuser1: f32 @ 72, set_fuser2: f32 @ 76, set_fuser3: f32 @ 80, set_fuser4: f32 @ 84,
});

record!(
    /// `event_args_t` (72 bytes).
    EventArgs,
    size::EVENT_ARGS
);
getters!(EventArgs {
    flags: i32 @ 0, entindex: i32 @ 4, origin: vec3 @ 8, angles: vec3 @ 20, velocity: vec3 @ 32,
    ducking: i32 @ 44, fparam1: f32 @ 48, fparam2: f32 @ 52, iparam1: i32 @ 56, iparam2: i32 @ 60,
    bparam1: i32 @ 64, bparam2: i32 @ 68,
});
setters!(EventArgs {
    set_flags: i32 @ 0, set_fparam1: f32 @ 48, set_fparam2: f32 @ 52, set_iparam1: i32 @ 56, set_iparam2: i32 @ 60,
    set_bparam1: i32 @ 64, set_bparam2: i32 @ 68,
});

impl EventArgs {
    pub fn set_entindex(&mut self, v: i32) {
        self.set_i32(4, v);
    }

    pub fn set_origin(&mut self, v: [f32; 3]) {
        self.set_vec3(8, v);
    }

    pub fn set_angles(&mut self, v: [f32; 3]) {
        self.set_vec3(20, v);
    }

    pub fn set_velocity(&mut self, v: [f32; 3]) {
        self.set_vec3(32, v);
    }

    pub fn set_ducking(&mut self, v: i32) {
        self.set_i32(44, v);
    }
}

/// `usercmd_t` in typed form; converted to the 52-byte image for encoding.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UserCmd {
    pub lerp_msec: i16,
    pub msec: u8,
    pub viewangles: [f32; 3],
    pub forwardmove: f32,
    pub sidemove: f32,
    pub upmove: f32,
    pub lightlevel: u8,
    pub buttons: u16,
    pub impulse: u8,
    pub weaponselect: u8,
    pub impact_index: i32,
    pub impact_position: [f32; 3],
}

impl UserCmd {
    pub fn to_bytes(&self) -> [u8; size::USERCMD] {
        let mut b = [0u8; size::USERCMD];
        b[0..2].copy_from_slice(&self.lerp_msec.to_le_bytes());
        b[2] = self.msec;
        for (i, a) in self.viewangles.iter().enumerate() {
            b[4 + i * 4..8 + i * 4].copy_from_slice(&a.to_le_bytes());
        }
        b[16..20].copy_from_slice(&self.forwardmove.to_le_bytes());
        b[20..24].copy_from_slice(&self.sidemove.to_le_bytes());
        b[24..28].copy_from_slice(&self.upmove.to_le_bytes());
        b[28] = self.lightlevel;
        b[30..32].copy_from_slice(&self.buttons.to_le_bytes());
        b[32] = self.impulse;
        b[33] = self.weaponselect;
        b[36..40].copy_from_slice(&self.impact_index.to_le_bytes());
        for (i, p) in self.impact_position.iter().enumerate() {
            b[40 + i * 4..44 + i * 4].copy_from_slice(&p.to_le_bytes());
        }
        b
    }

    pub fn from_bytes(b: &[u8; size::USERCMD]) -> Self {
        let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        Self {
            lerp_msec: i16::from_le_bytes([b[0], b[1]]),
            msec: b[2],
            viewangles: [f(4), f(8), f(12)],
            forwardmove: f(16),
            sidemove: f(20),
            upmove: f(24),
            lightlevel: b[28],
            buttons: u16::from_le_bytes([b[30], b[31]]),
            impulse: b[32],
            weaponselect: b[33],
            impact_index: i32::from_le_bytes([b[36], b[37], b[38], b[39]]),
            impact_position: [f(40), f(44), f(48)],
        }
    }
}
