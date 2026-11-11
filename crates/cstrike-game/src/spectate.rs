//! The client view code's engine services: the spectator camera's `GetEntityByIndex` /
//! `PM_TraceLine` against the render-time physents, and `V_CalcNormalRefdef`'s screen shake,
//! `PM_WaterEntity` and `PM_PointContents`.

use cs_client::view::{LineTrace, SpecEntity, SpecEnv, ViewEngine};
use gs_client::Client;
use gs_client::viewfx::ScreenShake;
use gs_common::rng::EngineRng;
use gs_pmove::Physent;

/// [`ViewEngine`] over the client; the shake is lent out of the client for the frame.
pub struct ViewHost<'a> {
    pub client: &'a Client,
    pub shake: &'a mut ScreenShake,
    pub rng: EngineRng,
}

impl ViewEngine for ViewHost<'_> {
    fn calc_shake(&mut self) {
        self.shake.calc(self.client.time(), self.client.old_time(), &self.rng);
    }

    fn apply_shake(&self, origin: &mut [f32; 3], angles: &mut [f32; 3], factor: f32) {
        self.shake.apply(origin, angles, factor);
    }

    fn water_entity_scale(&self, p: [f32; 3]) -> Option<f32> {
        let w = usize::try_from(self.client.water_entity(p)).ok()?;
        let e = self.client.entities().get(w)?;
        (e.curstate.modelindex() != 0).then(|| e.curstate.scale())
    }

    fn point_contents(&self, p: [f32; 3]) -> i32 {
        self.client.point_contents(p)
    }
}

pub struct SpecView<'a> {
    client: &'a Client,
    /// `EV_SetUpPlayerPrediction` + all players solid: what `PM_TraceLine` sees.
    physents: Vec<Physent>,
}

impl<'a> SpecView<'a> {
    pub fn new(client: &'a Client) -> Self {
        Self { client, physents: client.event_physents(-1) }
    }
}

/// `GetEntityByIndex` as the client DLL sees an entity.
pub fn spec_entity(client: &Client, index: i32) -> Option<SpecEntity> {
    let e = client.entities().get(usize::try_from(index).ok()?)?;
    Some(SpecEntity {
        index,
        player: e.player,
        origin: e.origin,
        angles: e.angles,
        solid: i32::from(e.curstate.solid()),
        usehull: e.curstate.usehull(),
        msg_time: e.curstate.msg_time(),
        prev_msg_time: e.prevstate.msg_time(),
        cur_origin: e.curstate.origin(),
        prev_origin: e.prevstate.origin(),
        weaponmodel: e.curstate.weaponmodel(),
    })
}

impl SpecEnv for SpecView<'_> {
    fn entity(&self, index: i32) -> Option<SpecEntity> {
        spec_entity(self.client, index)
    }

    /// `V_FindViewModelByWeaponModel`: `…/p_x.mdl` → `…/v_x.mdl`, then `EV_FindModelIndex`.
    fn view_model_for(&self, weaponmodel: i32) -> i32 {
        let models = &self.client.level().models;
        let Some(name) = usize::try_from(weaponmodel).ok().and_then(|i| models.get(i)).and_then(|n| n.as_deref()) else { return 0 };
        let Some(p) = name.find("/p_") else { return 0 };
        let view = format!("{}/v_{}", &name[..p], &name[p + 3..]);
        models.iter().position(|m| m.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(&view))).map_or(0, |i| i as i32)
    }

    fn trace_line(&self, start: [f32; 3], end: [f32; 3], ignore: i32) -> LineTrace {
        let Some(world) = self.client.world() else {
            return LineTrace { endpos: end, normal: [0.0; 3], ent: -1, info: 0 };
        };
        // PM_TRACELINE_PHYSENTSONLY with the point hull: PM_PlayerTraceEx(PM_NORMAL).
        let tr = gs_pmove::trace::player_trace(world, &self.physents, 2, start, end, 0, ignore);
        let info = usize::try_from(tr.ent).ok().and_then(|i| self.physents.get(i)).map_or(0, |pe| pe.info);
        LineTrace { endpos: tr.endpos, normal: tr.plane_normal, ent: tr.ent, info }
    }
}
