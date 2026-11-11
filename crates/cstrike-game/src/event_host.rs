//! The engine side of the client DLL's event handlers: [`cs_client::events::EventApi`] over the
//! engine client and its effects.

use cs_client::events::EventApi;
use gs_client::Client;
use gs_effects::{Effects, Env};
use gs_pmove::{Physent, PmTrace};

use crate::audio::SoundRequest;
use crate::effects::ClientEnv;

/// Side effects requested by handlers, applied once they finish.
#[derive(Debug, Default)]
pub struct EventOutput {
    /// `EV_WeaponAnimation(sequence, body)`.
    pub weapon_anim: Option<(i32, i32)>,
    /// `EV_MuzzleFlash`.
    pub muzzle_flash: bool,
    /// `R_DecalShoot(Draw_DecalIndexFromName(name), entity, 0, origin, 0)` requests.
    pub decals: Vec<(String, i32, [f32; 3])>,
    /// `EV_RemoveAllDecals`.
    pub remove_all_decals: bool,
    /// `EV_PlaySound` / `EV_StopSound`.
    pub sounds: Vec<SoundRequest>,
}

pub struct EventHost<'a> {
    pub client: &'a Client,
    pub effects: &'a mut Effects,
    pub env: ClientEnv<'a>,
    /// `cl.viewheight`.
    pub viewheight: [f32; 3],
    /// The view model's attachments from its last draw.
    pub view_attachments: [[f32; 3]; 4],
    pub out: EventOutput,
    /// `pmove->physents` while a handler traces (`EV_PushPMStates` .. `EV_PopPMStates`).
    physents: Vec<Physent>,
    /// `cl_solid_players`: `EV_SetSolidPlayers` adds the players.
    pub solid_players: bool,
}

impl<'a> EventHost<'a> {
    pub fn new(client: &'a Client, effects: &'a mut Effects, env: ClientEnv<'a>, viewheight: [f32; 3], view_attachments: [[f32; 3]; 4]) -> Self {
        let physents = client.event_physents(-1);
        Self { client, effects, env, viewheight, view_attachments, out: EventOutput::default(), physents, solid_players: true }
    }
}

impl EventApi for EventHost<'_> {
    fn max_clients(&self) -> i32 {
        i32::from(self.client.protocol().maxclients)
    }

    fn is_local(&self, playernum: i32) -> bool {
        playernum == i32::from(self.client.protocol().playernum)
    }

    fn local_viewheight(&self) -> [f32; 3] {
        self.viewheight
    }

    fn time(&self) -> f64 {
        self.client.time()
    }

    fn find_model_index(&self, name: &str) -> i32 {
        let models = &self.client.level().models;
        models.iter().enumerate().skip(1).find(|(_, m)| m.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(name))).map_or(0, |(i, _)| i as i32)
    }

    fn entity_eflags(&self, index: i32) -> i32 {
        usize::try_from(index).ok().and_then(|i| self.client.entities().get(i)).map_or(0, |e| e.curstate.eflags())
    }

    fn view_model_attachments(&self) -> [[f32; 3]; 4] {
        self.view_attachments
    }

    fn muzzle_flash(&mut self) {
        self.out.muzzle_flash = true;
    }

    fn weapon_animation(&mut self, sequence: i32, body: i32) {
        self.out.weapon_anim = Some((sequence, body));
    }

    fn play_sound(&mut self, ent: i32, origin: [f32; 3], channel: i32, sample: &str, volume: f32, attenuation: f32, flags: i32, pitch: i32) {
        self.out.sounds.push(SoundRequest::Play { ent, origin, channel, sample: sample.to_owned(), volume, attenuation, flags, pitch });
    }

    fn stop_sound(&mut self, ent: i32, channel: i32, sample: &str) {
        self.out.sounds.push(SoundRequest::Stop { ent, channel, sample: sample.to_owned() });
    }

    fn set_solid_players(&mut self, playernum: i32) {
        self.physents = if self.solid_players { self.client.event_physents(playernum) } else { self.client.base_physents() };
    }

    fn pop_pm_states(&mut self) {
        self.physents = self.client.event_physents(-1);
    }

    fn player_trace(&self, start: [f32; 3], end: [f32; 3], flags: i32) -> PmTrace {
        self.client.trace_physents(&self.physents, start, end, 2, flags).unwrap_or(PmTrace {
            allsolid: false,
            startsolid: false,
            inopen: true,
            inwater: false,
            fraction: 1.0,
            endpos: end,
            plane_normal: [0.0; 3],
            plane_dist: 0.0,
            ent: -1,
            deltavelocity: [0.0; 3],
            hitgroup: 0,
        })
    }

    fn physent(&self, index: i32) -> Option<Physent> {
        self.physents.get(usize::try_from(index).ok()?).cloned()
    }

    fn trace_texture(&self, ent: i32, start: [f32; 3], end: [f32; 3]) -> Option<String> {
        self.client.trace_texture(&self.physents, ent, start, end)
    }

    fn texture_type(&self, name: &str) -> u8 {
        self.client.texture_type(name)
    }

    fn decal_shoot(&mut self, name: &str, entity: i32, origin: [f32; 3]) {
        self.out.decals.push((name.to_owned(), entity, origin));
    }

    fn remove_all_decals(&mut self) {
        self.out.remove_all_decals = true;
    }

    fn efx(&mut self) -> (&mut Effects, &dyn Env) {
        (self.effects, &self.env)
    }
}
