//! `CGameStudioModelRenderer` (docs/re/render.md §B.6–B.8): turns linked entities into posed
//! studio models — players with gait/aim blending and their p_ weapon, corpses, other studio
//! entities, MOVETYPE_FOLLOW attachments and the view model.

use std::collections::HashMap;
use std::sync::Arc;

use gs_client::entities::{ClEntity, Entities, LinkKind, fx_blend, movetype};
use gs_client::level::Level;
use gs_fs::FileSystem;
use gs_studio::StudioModel;
use gs_studio::anim::Mat3x4;
use gs_studio::pose::{
    AnimState, PlayerGait, PoseParams, advance_gait, entity_rotation, merge_bones, player_blend, save_bones, setup_bones,
    setup_player_bones,
};

const MAX_PLAYERS: usize = 32;
/// CS weapon id of the knife (`g_bHoldingKnife` flips `cl_righthand`).
const WEAPON_KNIFE: i32 = 29;
/// Scoped weapons hide the view model when the FOV is zoomed (`gHUD.m_iFOV <= 89`).
const SCOPED_VIEW_MODELS: [&str; 4] = ["v_awp", "v_scout", "v_g3sg1", "v_sg550"];
/// `kRenderFxDeadPlayer`.
const RENDERFX_DEAD_PLAYER: i32 = 17;
/// `EF_MUZZLEFLASH`.
const EF_MUZZLEFLASH: i32 = 2;
/// `STUDIO_LOOPING`.
const STUDIO_LOOPING: i32 = 1;

/// Effects requested while posing models (`studio_StudioClientEvents` → `HUD_StudioEvent`),
/// in draw order.
#[derive(Debug, Clone, PartialEq)]
pub enum StudioEvent {
    /// EF_MUZZLEFLASH: the cstrike elight at `attachment[0]` (radius 16, decay 320,
    /// colour (255,192,64), 0.05 s).
    MuzzleLight { origin: [f32; 3] },
    /// Events 5001/5011/5021/5031: `R_MuzzleFlash(attachment, n)`.
    MuzzleFlash { origin: [f32; 3], kind: i32 },
    /// Event 5002: `R_SparkEffect(attachment[0], n, -100, 100)`.
    Sparks { origin: [f32; 3], count: i32 },
    /// Event 5004: `PlaySoundByNameAtLocation(options, 1.0, attachment[0])`.
    Sound { name: String, origin: [f32; 3] },
}

/// A CS blob shadow request (`StudioDrawShadow(origin, scale)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowRequest {
    pub origin: [f32; 3],
    pub scale: f32,
}

/// Output of one [`StudioRenderer::draw`].
#[derive(Debug, Clone, Default)]
pub struct StudioFrame {
    pub draws: Vec<ModelDraw>,
    pub events: Vec<StudioEvent>,
    pub shadows: Vec<ShadowRequest>,
}

/// `strtol(options)`: leading integer, 0 when none.
fn atoi(s: &str) -> i32 {
    let s = s.trim_start();
    let end = s.char_indices().find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')))).map_or(s.len(), |(i, _)| i);
    s[..end].parse().unwrap_or(0)
}

/// Inputs of `StudioDynamicLight` for one draw.
#[derive(Debug, Clone, Copy)]
pub struct LightOrigin {
    pub origin: [f32; 3],
    pub effects: i32,
    pub model_flags: i32,
    pub renderfx: i32,
    pub iuser4: i32,
}

/// How a draw is lit.
#[derive(Debug, Clone, Copy)]
pub enum LightSource {
    /// Compute `StudioDynamicLight` at this entity.
    Entity(LightOrigin),
    /// Reuse the previous draw's lighting (p_ weapon of a player).
    Previous,
}

/// A posed studio model ready for the renderer.
#[derive(Debug, Clone)]
pub struct ModelDraw {
    pub model: Arc<StudioModel>,
    pub bones: Vec<Mat3x4>,
    pub light: LightSource,
    pub origin: [f32; 3],
    pub body: i32,
    pub skin: i32,
    /// `kRender*`.
    pub rendermode: i32,
    /// `r_blend`.
    pub blend: f32,
    /// `curstate.renderfx` / `renderamt` / `rendercolor` (glow shell inputs).
    pub renderfx: i32,
    pub renderamt: i32,
    pub rendercolor: [u8; 3],
    pub view_model: bool,
    /// Right-handed view model (mirrored bones, no culling).
    pub mirrored: bool,
}

/// Frame inputs.
pub struct FrameTimes {
    /// `cl.time`.
    pub time: f64,
    /// `cl.oldtime`.
    pub old_time: f64,
    /// `host_frametime`.
    pub frametime: f64,
    pub view_origin: [f32; 3],
    pub view_forward: [f32; 3],
    /// `StudioCheckBBox`'s view; `None` draws every model (the spectator's picture-in-picture
    /// view shares this frame's draws).
    pub cull: Option<StudioCull>,
}

/// What `StudioCheckBBox` tests a model's box against (render.md §B.14).
#[derive(Debug, Clone, Copy)]
pub struct StudioCull {
    /// Projection · modelview of the view (column-major), for `R_CullBox`.
    pub world_to_screen: [f32; 16],
    /// Multiplayer with `r_cullsequencebox` on: `R_CullBox`'s four side planes; otherwise only
    /// the plane through the eye facing along `vpn`.
    pub frustum: bool,
}

/// `StudioCheckBBox` after `StudioSetUpTransform`: the header box (`bbmin`, else `min`, else
/// ±16) at the entity origin, grown by the current sequence's box through the bone-root
/// transform `rotation`; `false` when the view culls it.
fn check_bbox(model: &StudioModel, ent: &ClEntity, rotation: &Mat3x4, times: &FrameTimes) -> bool {
    let Some(cull) = times.cull else { return true };
    let o = ent.origin;
    let (lo, hi) = if model.bbmin != [0.0; 3] {
        (model.bbmin, model.bbmax)
    } else if model.min != [0.0; 3] {
        (model.min, model.max)
    } else {
        ([-16.0; 3], [16.0; 3])
    };
    let mut mins: [f32; 3] = std::array::from_fn(|k| lo[k] + o[k]);
    let mut maxs: [f32; 3] = std::array::from_fn(|k| hi[k] + o[k]);
    let Some(seq) = usize::try_from(ent.curstate.sequence()).ok().and_then(|s| model.sequences.get(s)) else { return true };
    for i in 0..8 {
        let c = [
            if i & 1 != 0 { seq.bbmin[0] } else { seq.bbmax[0] },
            if i & 2 != 0 { seq.bbmin[1] } else { seq.bbmax[1] },
            if i & 4 != 0 { seq.bbmin[2] } else { seq.bbmax[2] },
        ];
        for (r, row) in rotation.iter().enumerate() {
            let p = row[0] * c[0] + row[1] * c[1] + row[2] * c[2] + row[3];
            mins[r] = mins[r].min(p);
            maxs[r] = maxs[r].max(p);
        }
    }
    if cull.frustum {
        return !gs_effects::cull_box(&cull.world_to_screen, mins, maxs);
    }
    // BoxOnPlaneSide == 2 for the plane {vpn, vpn·r_origin}: even the box corner farthest
    // along vpn lies behind the eye.
    let (n, eye) = (times.view_forward, times.view_origin);
    let dist = n[0] * eye[0] + n[1] * eye[1] + n[2] * eye[2];
    let far: f32 = (0..3).map(|k| n[k] * if n[k] >= 0.0 { maxs[k] } else { mins[k] }).sum();
    far >= dist
}

/// View model inputs (`cl.viewent` setup in `V_RenderView` / `R_DrawViewModel`).
pub struct ViewModelInput {
    /// `clientdata.viewmodel` (model index).
    pub modelindex: i32,
    /// `clientdata.m_iId`.
    pub weapon_id: i32,
    /// Current FOV (`gHUD.m_iFOV`).
    pub fov: f32,
    pub health: f32,
    pub origin: [f32; 3],
    /// Studio angles from `V_CalcGunAngle`.
    pub angles: [f32; 3],
    /// `cl_righthand->value`.
    pub right_hand: f32,
    /// The view entity has EF_MUZZLEFLASH (`CL_PlayerFlashlight` copies it to the view model
    /// every frame).
    pub muzzleflash: bool,
}

/// Per-player `SetupPlayerModel` cache.
#[derive(Debug, Clone, Default)]
struct PlayerModelSlot {
    name: String,
    model: Option<Arc<StudioModel>>,
}

/// `pplayer` of `_StudioDrawPlayer`: whose player info and gait drive the entity.
#[derive(Debug, Clone, Copy)]
struct PlayerSource {
    number: usize,
    gaitsequence: i32,
    weaponmodel: i32,
}

/// Shared state of one `draw` call.
struct DrawFrame<'a> {
    level: &'a Level,
    maxclients: usize,
    times: &'a FrameTimes,
    out: Vec<ModelDraw>,
    events: Vec<StudioEvent>,
    shadows: Vec<ShadowRequest>,
    /// `StudioSaveBones` per entity, for MOVETYPE_FOLLOW children.
    cached: HashMap<usize, Vec<(String, Mat3x4)>>,
}

pub struct StudioRenderer {
    fs: FileSystem,
    files: HashMap<String, Option<Arc<StudioModel>>>,
    slots: Vec<PlayerModelSlot>,
    gait: Vec<PlayerGait>,
    renderframe: Vec<u64>,
    frame_count: u64,
    view_ent: ClEntity,
    rng: gs_effects::EngineRng,
    /// `s_boneNames` lookup of "Bip01 Pelvis", cached on first use and never invalidated.
    pelvis_bone: Option<Option<usize>>,
    /// `cl_shadows`.
    pub shadows: bool,
    /// `cl_minmodels` / `cl_min_t` / `cl_min_ct` with the players' teams.
    pub min_models: MinModels,
}

/// `sPlayerModelFiles` (index 0 is `models/player.mdl`, never chosen by the team rules).
const PLAYER_MODEL_FILES: [&str; 12] = ["", "leet", "gign", "vip", "gsg9", "guerilla", "arctic", "sas", "terror", "urban", "spetsnaz", "militia"];

/// What `_StudioDrawPlayer` and `MsgFunc_ClCorpse` read for the team models: the cvars and
/// `g_PlayerExtraInfo[n]`'s team and VIP flag by player number.
#[derive(Debug, Clone, Default)]
pub struct MinModels {
    /// `cl_minmodels`.
    pub value: f32,
    /// `cl_min_t`, `cl_min_ct`.
    pub min_t: f32,
    pub min_ct: f32,
    /// `(teamnumber, vip)` by player number.
    pub players: Vec<(i32, bool)>,
}

impl MinModels {
    /// `(teamnumber, vip)` of player `number`.
    pub fn player(&self, number: usize) -> (i32, bool) {
        self.players.get(number).copied().unwrap_or((0, false))
    }

    /// The team model name (`None` for neither team): T takes `cl_min_t` when it names a T
    /// model, else leet; a CT VIP the vip model, else `cl_min_ct` when it names a CT model, else
    /// gign.
    pub fn team_model(&self, team: i32, vip: bool) -> Option<&'static str> {
        let pick = |v: f32, max: u32, mask: u32, fallback: usize| {
            let i = v as i32 as u32;
            PLAYER_MODEL_FILES[if i <= max && mask & (1 << i) != 0 { i as usize } else { fallback }]
        };
        match team {
            1 => Some(pick(self.min_t, 11, 0x962, 1)),
            2 if vip => Some(PLAYER_MODEL_FILES[3]),
            2 => Some(pick(self.min_ct, 10, 0x694, 2)),
            _ => None,
        }
    }
}

fn clamp_sequence(model: &StudioModel, seq: i32) -> i32 {
    if (0..model.sequences.len() as i32).contains(&seq) { seq } else { 0 }
}

fn anim_state(ent: &ClEntity) -> AnimState {
    let (s, l) = (&ent.curstate, &ent.latched);
    AnimState {
        sequence: s.sequence(),
        frame: s.frame(),
        animtime: s.animtime(),
        framerate: s.framerate(),
        controller: s.controller(),
        blending: s.blending(),
        mouth: 0,
        prev_sequence: l.prevsequence,
        prev_frame: l.prevframe,
        sequence_time: l.sequencetime,
        prev_animtime: l.prevanimtime,
        prev_controller: l.prevcontroller,
        prev_blending: l.prevblending,
        prev_seq_blending: l.prevseqblending,
    }
}

/// `StudioSetUpTransform`: interpolated angles unless the entity does not move.
fn transform_angles(ent: &ClEntity) -> [f32; 3] {
    if ent.curstate.movetype() != movetype::NONE { ent.angles } else { ent.curstate.angles() }
}

fn light_origin(ent: &ClEntity, model: &StudioModel) -> LightOrigin {
    LightOrigin {
        origin: ent.origin,
        effects: ent.curstate.effects(),
        model_flags: model.flags,
        renderfx: ent.curstate.renderfx(),
        iuser4: ent.curstate.iuser4(),
    }
}

/// `StudioCalcAttachments`: world positions of the model's attachments (at most 4) into `out`.
fn calc_attachments(model: &StudioModel, bones: &[Mat3x4], out: &mut [[f32; 3]; 4]) {
    for (slot, att) in out.iter_mut().zip(&model.attachments) {
        if let Some(m) = usize::try_from(att.bone).ok().and_then(|b| bones.get(b)) {
            *slot = gs_studio::anim::transform(m, att.org);
        }
    }
}

/// `studio_StudioClientEvents` (client-sim.md Part G §6): the EF_MUZZLEFLASH elight, then the
/// sequence's client events (≥ 5000) crossed since the last frame, through `HUD_StudioEvent`.
fn client_events(ent: &mut ClEntity, model: &StudioModel, times: &FrameTimes, out: &mut Vec<StudioEvent>) {
    if times.time == times.old_time {
        return;
    }
    let effects = ent.curstate.effects();
    if effects & EF_MUZZLEFLASH != 0 {
        out.push(StudioEvent::MuzzleLight { origin: ent.attachment[0] });
        ent.curstate.set_effects(effects & !EF_MUZZLEFLASH);
    }
    let Some(seq) = usize::try_from(ent.curstate.sequence()).ok().and_then(|i| model.sequences.get(i)) else { return };
    if seq.events.is_empty() {
        return;
    }
    let animtime = f64::from(ent.curstate.animtime());
    let dfdt = if animtime <= times.time { ((times.time - animtime) as f32) * ent.curstate.framerate() * seq.fps } else { 0.0 };
    let end = gs_studio::anim::estimate_frame(seq, ent.curstate.frame(), dfdt);
    let mut start = end - ent.curstate.framerate() * times.frametime as f32 * seq.fps;
    if ent.latched.sequencetime == ent.curstate.animtime() && seq.flags & STUDIO_LOOPING == 0 {
        start = -0.01;
    }
    for ev in seq.events.iter().filter(|e| e.event >= 5000) {
        let frame = ev.frame as f32;
        if frame <= start || frame > end {
            continue;
        }
        let n = atoi(&ev.options);
        let att = ent.attachment;
        out.push(match ev.event {
            5001 => StudioEvent::MuzzleFlash { origin: att[0], kind: n },
            5011 => StudioEvent::MuzzleFlash { origin: att[1], kind: n },
            5021 => StudioEvent::MuzzleFlash { origin: att[2], kind: n },
            5031 => StudioEvent::MuzzleFlash { origin: att[3], kind: n },
            5002 => StudioEvent::Sparks { origin: att[0], count: n },
            5004 => StudioEvent::Sound { name: ev.options.clone(), origin: att[0] },
            _ => continue,
        });
    }
}

impl StudioRenderer {
    pub fn new(fs: FileSystem, rng: gs_effects::EngineRng) -> Self {
        Self {
            fs,
            files: HashMap::new(),
            slots: vec![PlayerModelSlot::default(); MAX_PLAYERS],
            gait: vec![PlayerGait::default(); MAX_PLAYERS],
            renderframe: vec![u64::MAX; MAX_PLAYERS],
            frame_count: 0,
            view_ent: ClEntity::default(),
            rng,
            pelvis_bone: None,
            shadows: true,
            min_models: MinModels::default(),
        }
    }

    /// Forget per-level state (the model file cache stays, like `Mod_ForName`'s known list).
    pub fn new_level(&mut self) {
        self.slots = vec![PlayerModelSlot::default(); MAX_PLAYERS];
        self.gait = vec![PlayerGait::default(); MAX_PLAYERS];
        self.view_ent = ClEntity::default();
    }

    fn random_long(&mut self, lo: i32, hi: i32) -> i32 {
        self.rng.long(lo, hi)
    }

    /// `Mod_ForName`: a model the level precached is the one already loaded (`mod_known`), else
    /// the file, loaded once.
    fn load_file(&mut self, path: &str, level: &Level) -> Option<Arc<StudioModel>> {
        let precached = level.models.iter().position(|n| n.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(path)));
        if let Some(model) = precached.and_then(|i| level.studio.get(i)?.clone()) {
            return Some(model);
        }
        let key = path.to_ascii_lowercase();
        if let Some(m) = self.files.get(&key) {
            return m.clone();
        }
        let model = match gs_assets::load_studio(&self.fs, path) {
            Ok(m) => Some(Arc::new(m)),
            Err(e) => {
                log::warn!("{path}: {e}");
                None
            }
        };
        self.files.insert(key, model.clone());
        model
    }

    /// `SetupPlayerModel`: the userinfo `model` (`models/player/<m>/<m>.mdl`) or the entity model.
    fn setup_player_model(&mut self, idx: usize, f: &DrawFrame, ent_model: Option<Arc<StudioModel>>) -> Option<Arc<StudioModel>> {
        let wanted = f.level.players.get(idx).map(|p| p.model.clone()).unwrap_or_default();
        if f.maxclients == 1 || wanted.is_empty() {
            self.slots[idx] = PlayerModelSlot { name: String::new(), model: ent_model.clone() };
            return ent_model;
        }
        if !self.slots[idx].name.eq_ignore_ascii_case(&wanted) || self.slots[idx].model.is_none() {
            let name = gs_common::bytestr::truncate_encoded(&wanted, 259).into_owned();
            let model = self.load_file(&format!("models/player/{name}/{name}.mdl"), f.level).or(ent_model);
            self.slots[idx] = PlayerModelSlot { name, model };
        }
        self.slots[idx].model.clone()
    }

    /// Draws for this frame's linked entities and the view model (`R_RenderScene` studio part
    /// plus `R_DrawViewModel`).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        entities: &mut Entities,
        level: &Level,
        maxclients: usize,
        linked: &[(usize, LinkKind)],
        times: &FrameTimes,
        view_model: Option<&ViewModelInput>,
        weapon_starttime: &mut f64,
    ) -> StudioFrame {
        self.frame_count += 1;
        let mut frame =
            DrawFrame { level, maxclients, times, out: Vec::new(), events: Vec::new(), shadows: Vec::new(), cached: HashMap::new() };
        for &(index, kind) in linked {
            match kind {
                LinkKind::Player => {
                    let Some(ent) = entities.get(index) else { continue };
                    let source = PlayerSource {
                        number: index,
                        gaitsequence: ent.curstate.gaitsequence(),
                        weaponmodel: ent.curstate.weaponmodel(),
                    };
                    self.draw_player(entities, index, source, true, &mut frame);
                }
                LinkKind::Normal => self.draw_entity(entities, index, &mut frame),
                LinkKind::Beam => {}
            }
        }
        if let Some(vm) = view_model {
            self.draw_view_model(vm, weapon_starttime, &mut frame);
        }
        StudioFrame { draws: frame.out, events: frame.events, shadows: frame.shadows }
    }

    /// `StudioDrawModel` for a temp entity's studio model (`tent->entity`, not in
    /// `cl_entities`).
    pub fn draw_temp_entity(&mut self, ent: &mut ClEntity, model: Arc<StudioModel>, level: &Level, times: &FrameTimes) -> StudioFrame {
        let mut frame =
            DrawFrame { level, maxclients: 0, times, out: Vec::new(), events: Vec::new(), shadows: Vec::new(), cached: HashMap::new() };
        self.draw_studio(ent, None, model, &mut frame);
        StudioFrame { draws: frame.out, events: frame.events, shadows: frame.shadows }
    }

    /// `cl.viewent.attachment[]` from the last view model draw (muzzle positions).
    pub fn view_model_attachments(&self) -> [[f32; 3]; 4] {
        self.view_ent.attachment
    }

    /// `EV_MuzzleFlash`: `GetViewModel()->curstate.effects |= EF_MUZZLEFLASH`, consumed by the
    /// next view model draw.
    pub fn view_model_muzzle_flash(&mut self) {
        let effects = self.view_ent.curstate.effects();
        self.view_ent.curstate.set_effects(effects | EF_MUZZLEFLASH);
    }

    /// `r_blend` for an entity: 1 for normal rendering, else `CL_FxBlend / 255`.
    pub fn entity_blend(&mut self, ent: &mut ClEntity, times: &FrameTimes) -> f32 {
        if ent.curstate.rendermode() == 0 {
            return 1.0;
        }
        let mut rng = |lo: i32, hi: i32| self.random_long(lo, hi);
        fx_blend(ent, times.time, times.view_origin, times.view_forward, &mut rng) as f32 / 255.0
    }

    /// `_StudioDrawPlayer(flags, pplayer)`: entity `index` animated as player `source.number`.
    fn draw_player(&mut self, entities: &mut Entities, index: usize, source: PlayerSource, interpolate: bool, f: &mut DrawFrame) {
        let idx = source.number.wrapping_sub(1);
        if idx >= MAX_PLAYERS.min(f.maxclients) {
            return;
        }
        let Some(ent) = entities.get_mut(index) else { return };
        let ent_model = f.level.studio_model(ent.curstate.modelindex()).cloned();
        // cl_minmodels > 0: the team's model (players in neither team are not drawn).
        let model = if self.min_models.value <= 0.0 {
            self.setup_player_model(idx, f, ent_model)
        } else {
            let (team, vip) = self.min_models.player(source.number);
            self.min_models.team_model(team, vip).and_then(|name| self.load_file(&format!("models/player/{name}/{name}.mdl"), f.level))
        };
        let Some(model) = model else { return };
        if model.sequences.is_empty() {
            return;
        }
        ent.curstate.set_sequence(clamp_sequence(&model, ent.curstate.sequence()));
        let gaitsequence = clamp_sequence(&model, source.gaitsequence);

        let dt = (f.times.time - f.times.old_time).clamp(0.0, 1.0);
        let first = self.renderframe[idx] != self.frame_count;
        let gait = &mut self.gait[idx];
        let blend = player_blend(gait, ent.origin, ent.angles, ent.curstate.sequence(), dt, first);
        if gaitsequence != 0 {
            advance_gait(&model, gait, blend.movement, dt, ent.curstate.framerate());
            gait.gaitsequence = gaitsequence;
        } else {
            ent.curstate.set_controller([127; 4]);
            ent.latched.prevcontroller = [127; 4];
            gait.gaitsequence = 0;
            // The modified angles are not restored in this branch.
            ent.angles = blend.angles;
        }
        ent.curstate.set_blending(blend.blending);
        ent.latched.prevblending = blend.blending;
        ent.latched.prevseqblending = blend.blending;
        let angles = if ent.curstate.movetype() != movetype::NONE { blend.angles } else { ent.curstate.angles() };
        let rotation = entity_rotation(ent.origin, angles);
        if !check_bbox(&model, ent, &rotation, f.times) {
            return;
        }
        let params = PoseParams { time: f.times.time, interpolate, mirror: false };
        log::trace!(
            "player {} ent {index}: seq {} frame {:.1} gait {} gaitframe {:.2} gaityaw {:.1} blending {:?} angles {:?}",
            source.number,
            ent.curstate.sequence(),
            ent.curstate.frame(),
            gait.gaitsequence,
            gait.gaitframe,
            gait.gaityaw,
            blend.blending,
            ent.angles
        );
        let setup = setup_player_bones(&model, &anim_state(ent), gait, &rotation, &params);
        ent.latched.prevframe = setup.prev_frame;
        self.renderframe[idx] = self.frame_count;
        if setup.bones.is_empty() {
            return;
        }
        let saved = save_bones(&model, &setup.bones);
        let blend_value = self.entity_blend(ent, f.times);
        let light = light_origin(ent, &model);
        let (body, skin, rendermode) = (ent.curstate.body(), i32::from(ent.curstate.skin()), ent.curstate.rendermode());
        let (renderfx, renderamt, rendercolor) = (ent.curstate.renderfx(), ent.curstate.renderamt(), ent.curstate.rendercolor());
        let weapon = f.level.studio_model(source.weaponmodel).cloned();
        let weapon_bones = weapon.as_ref().map(|w| merge_bones(w, &anim_state(ent), &saved, &rotation, &params));
        // Events wait for the p_ weapon when it has attachments (its muzzle becomes
        // attachment[0]); a weapon without attachments fires them a second time.
        if weapon.as_ref().is_none_or(|w| w.attachments.is_empty()) {
            calc_attachments(&model, &setup.bones, &mut ent.attachment);
            client_events(ent, &model, f.times, &mut f.events);
        }
        if let (Some(w), Some(wb)) = (&weapon, &weapon_bones) {
            calc_attachments(w, wb, &mut ent.attachment);
            client_events(ent, &model, f.times, &mut f.events);
        }
        if self.shadows {
            let pelvis = *self.pelvis_bone.get_or_insert_with(|| model.bones.iter().position(|b| b.name == "Bip01 Pelvis"));
            if let Some(m) = pelvis.and_then(|i| setup.bones.get(i)) {
                f.shadows.push(ShadowRequest { origin: [m[0][3], m[1][3], m[2][3]], scale: 20.0 });
            }
        }
        f.cached.insert(index, saved);
        let origin = ent.origin;
        f.out.push(ModelDraw {
            model,
            bones: setup.bones,
            light: LightSource::Entity(light),
            origin,
            body,
            skin,
            rendermode,
            blend: blend_value,
            renderfx,
            renderamt,
            rendercolor,
            view_model: false,
            mirrored: false,
        });
        if let (Some(model), Some(bones)) = (weapon, weapon_bones)
            && !bones.is_empty()
        {
            f.out.push(ModelDraw {
                model,
                bones,
                light: LightSource::Previous,
                origin,
                body,
                skin,
                rendermode,
                blend: blend_value,
                renderfx,
                renderamt,
                rendercolor,
                view_model: false,
                mirrored: false,
            });
        }
    }

    /// `CStudioModelRenderer::StudioDrawModel` for a non-player entity.
    fn draw_entity(&mut self, entities: &mut Entities, index: usize, f: &mut DrawFrame) {
        let Some(ent) = entities.get_mut(index) else { return };
        // kRenderFxDeadPlayer: a corpse drawn as player `renderamt` (its userinfo model), with
        // no gait, no weapon and no interpolation.
        if ent.curstate.renderfx() == RENDERFX_DEAD_PLAYER {
            let number = ent.curstate.renderamt();
            if !(1..=f.maxclients as i32).contains(&number) {
                return;
            }
            let source = PlayerSource { number: number as usize, gaitsequence: 0, weaponmodel: 0 };
            return self.draw_player(entities, index, source, false, f);
        }
        let Some(model) = f.level.studio_model(ent.curstate.modelindex()).cloned() else { return };
        self.draw_studio(ent, Some(index), model, f);
    }

    /// `StudioDrawModel(STUDIO_RENDER | STUDIO_EVENTS)` on `ent` (entity `index` of
    /// `cl_entities`, or a temp entity).
    fn draw_studio(&mut self, ent: &mut ClEntity, index: Option<usize>, model: Arc<StudioModel>, f: &mut DrawFrame) {
        if model.sequences.is_empty() || model.bones.is_empty() {
            return;
        }
        ent.curstate.set_sequence(clamp_sequence(&model, ent.curstate.sequence()));
        let rotation = entity_rotation(ent.origin, transform_angles(ent));
        if !check_bbox(&model, ent, &rotation, f.times) {
            return;
        }
        let params = PoseParams { time: f.times.time, interpolate: true, mirror: false };
        let bones = if ent.curstate.movetype() == movetype::FOLLOW {
            let parent = usize::try_from(ent.curstate.aiment()).ok();
            let saved = parent.and_then(|p| f.cached.get(&p)).cloned().unwrap_or_default();
            merge_bones(&model, &anim_state(ent), &saved, &rotation, &params)
        } else {
            let setup = setup_bones(&model, &anim_state(ent), &rotation, &params);
            ent.latched.prevframe = setup.prev_frame;
            setup.bones
        };
        if bones.is_empty() {
            return;
        }
        calc_attachments(&model, &bones, &mut ent.attachment);
        client_events(ent, &model, f.times, &mut f.events);
        if let Some(index) = index {
            f.cached.insert(index, save_bones(&model, &bones));
        }
        let blend = self.entity_blend(ent, f.times);
        if self.shadows && model.name.contains("hostage") {
            f.shadows.push(ShadowRequest { origin: ent.origin, scale: 12.0 });
        }
        f.out.push(ModelDraw {
            light: LightSource::Entity(light_origin(ent, &model)),
            model,
            bones,
            origin: ent.origin,
            body: ent.curstate.body(),
            skin: i32::from(ent.curstate.skin()),
            rendermode: ent.curstate.rendermode(),
            blend,
            renderfx: ent.curstate.renderfx(),
            renderamt: ent.curstate.renderamt(),
            rendercolor: ent.curstate.rendercolor(),
            view_model: false,
            mirrored: false,
        });
    }

    /// `R_DrawViewModel` + `StudioDrawModel` on `cl.viewent`.
    fn draw_view_model(&mut self, vm: &ViewModelInput, weapon_starttime: &mut f64, f: &mut DrawFrame) {
        if vm.health <= 0.0 {
            return;
        }
        let Some(model) = f.level.studio_model(vm.modelindex).cloned() else { return };
        let name = f.level.model_name(vm.modelindex).unwrap_or("");
        if vm.fov <= 89.0 && SCOPED_VIEW_MODELS.iter().any(|s| name.contains(s)) {
            return;
        }
        if *weapon_starttime == 0.0 {
            *weapon_starttime = f.times.time;
        }
        let ent = &mut self.view_ent;
        let s = &mut ent.curstate;
        s.set_modelindex(vm.modelindex);
        s.set_sequence(clamp_sequence(&model, f.level.weapon_sequence));
        s.set_frame(0.0);
        s.set_framerate(1.0);
        s.set_animtime(*weapon_starttime as f32);
        s.set_body(f.level.weapon_body);
        s.set_origin(vm.origin);
        s.set_angles(vm.angles);
        ent.origin = vm.origin;
        ent.angles = vm.angles;
        ent.latched.prevorigin = vm.origin;
        ent.latched.prevangles = vm.angles;
        // StudioDrawModel flips the value for the knife (`(int)v ? 0 : 1`, `g_bHoldingKnife`);
        // StudioSetupBones and R_StudioDrawPoints mirror while it is above 0.
        let hand = if vm.weapon_id == WEAPON_KNIFE { if vm.right_hand as i32 != 0 { 0.0 } else { 1.0 } } else { vm.right_hand };
        let right = hand > 0.0;
        let rotation = entity_rotation(vm.origin, vm.angles);
        let params = PoseParams { time: f.times.time, interpolate: true, mirror: right };
        let setup = setup_bones(&model, &anim_state(ent), &rotation, &params);
        ent.latched.prevframe = setup.prev_frame;
        if setup.bones.is_empty() {
            return;
        }
        ent.attachment = [vm.origin; 4];
        calc_attachments(&model, &setup.bones, &mut ent.attachment);
        if vm.muzzleflash {
            ent.curstate.set_effects(ent.curstate.effects() | EF_MUZZLEFLASH);
        }
        client_events(ent, &model, f.times, &mut f.events);
        f.out.push(ModelDraw {
            light: LightSource::Entity(LightOrigin { origin: vm.origin, effects: 0, model_flags: model.flags, renderfx: 0, iuser4: 0 }),
            model,
            bones: setup.bones,
            origin: vm.origin,
            body: f.level.weapon_body,
            skin: 0,
            rendermode: 0,
            blend: 1.0,
            renderfx: 0,
            renderamt: 0,
            rendercolor: [0; 3],
            view_model: true,
            mirrored: right,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model whose header box is `±16` (zero `bbmin`/`min`) and whose one sequence spans
    /// `seq_min..seq_max`.
    fn model(seq_min: [f32; 3], seq_max: [f32; 3]) -> StudioModel {
        let seq = gs_studio::model::SeqDesc {
            label: String::new(),
            fps: 30.0,
            flags: 0,
            activity: 0,
            events: Vec::new(),
            numframes: 1,
            motiontype: 0,
            motionbone: 0,
            linearmovement: [0.0; 3],
            bbmin: seq_min,
            bbmax: seq_max,
            numblends: 1,
            animindex: 0,
            seqgroup: 0,
        };
        StudioModel {
            name: String::new(),
            eyeposition: [0.0; 3],
            min: [0.0; 3],
            max: [0.0; 3],
            bbmin: [0.0; 3],
            bbmax: [0.0; 3],
            flags: 0,
            bones: Vec::new(),
            controllers: Vec::new(),
            hitboxes: Vec::new(),
            sequences: vec![seq],
            seqgroups: Vec::new(),
            bodyparts: Vec::new(),
            attachments: Vec::new(),
            textures: Vec::new(),
            skins: Vec::new(),
            data: Arc::from(Vec::new()),
            group_data: Vec::new(),
        }
    }

    /// The eye at the origin looking down +x with +z up and a 90° square frustum:
    /// clip x = −y, clip y = z, w = x.
    fn times(frustum: bool) -> FrameTimes {
        let mut m = [0.0; 16];
        (m[4], m[9], m[3]) = (-1.0, 1.0, 1.0);
        FrameTimes {
            time: 0.0,
            old_time: 0.0,
            frametime: 0.0,
            view_origin: [0.0; 3],
            view_forward: [1.0, 0.0, 0.0],
            cull: Some(StudioCull { world_to_screen: m, frustum }),
        }
    }

    fn visible(model: &StudioModel, origin: [f32; 3], frustum: bool) -> bool {
        let mut ent = ClEntity::default();
        ent.origin = origin;
        check_bbox(model, &ent, &entity_rotation(origin, [0.0; 3]), &times(frustum))
    }

    #[test]
    fn studio_check_bbox_culls_by_frustum_in_multiplayer_and_by_the_eye_plane_otherwise() {
        let small = model([-16.0; 3], [16.0; 3]);
        assert!(visible(&small, [100.0, 0.0, 0.0], true), "ahead");
        assert!(!visible(&small, [-100.0, 0.0, 0.0], true), "behind, frustum");
        assert!(!visible(&small, [-100.0, 0.0, 0.0], false), "behind, eye plane");
        // Off to the side: only the frustum test drops it.
        assert!(!visible(&small, [100.0, 300.0, 0.0], true));
        assert!(visible(&small, [100.0, 300.0, 0.0], false));
        // The sequence box, placed by the bone-root transform, reaches back into view.
        let reaching = model([-16.0, -250.0, -16.0], [16.0, -200.0, 16.0]);
        assert!(visible(&reaching, [100.0, 300.0, 0.0], true));
    }
}
