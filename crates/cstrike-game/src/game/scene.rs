//! A frame's presentation: entity linking, the view, studio models, sprites, effects and
//! particles into the renderer's scene, the level's map and overview, sound spatialization,
//! and the effects' decals.

use super::*;

impl Running {
    /// One frame's presentation: `CL_EmitEntities` (entity linking, link effects, temp entity
    /// update), `V_CalcRefdef`, the studio renderer with its client events, sprites, brush
    /// entities, temp entities, and `R_DrawParticles` geometry.
    pub(super) fn build_scene(&mut self) -> FrameScene {
        self.load_overview_map();
        let time = self.client.time();
        let old_time = self.client.old_time();
        let cd = self.client.protocol().current_frame().clientdata.clone();
        let maxclients = usize::from(self.client.protocol().maxclients);
        let playernum = usize::from(self.client.protocol().playernum);
        // cl.simorg & co. from prediction, or in a playback from the last replayed refdef; the
        // raw clientdata until either exists.
        let sim = *self.client.sim();
        let replay = self.replay.filter(|_| self.client.demo_playing());
        let (simorg, simvel, viewheight, punch, onground, viewmodel) = if let Some((rp, vm)) = replay {
            (rp.simorg, rp.simvel, rp.viewheight, rp.punchangle, rp.onground != 0, vm)
        } else if sim.valid {
            (sim.origin, sim.velocity, sim.viewheight, sim.punchangle, sim.onground != -1, sim.viewmodel)
        } else {
            (cd.origin(), cd.velocity(), cd.view_ofs(), cd.punchangle(), cd.flags() & FL_ONGROUND != 0, cd.viewmodel())
        };
        let waterlevel = if sim.valid { sim.waterlevel } else { cd.waterlevel() };
        // g_iUser1..3 (HUD_ProcessPlayerState, or chosen locally on an HLTV connection).
        let users = [self.hud.state.user1, self.hud.state.user2, self.hud.state.user3];
        let hltv = self.client.spectator();
        let pip = self.console.cvars.value("spec_pip");
        self.client_events.observer = (users[0], users[1]);
        // CL_IsThirdPerson: the camera, or watching one's own body.
        let third_person = self.input.cam.thirdperson || (users[0] != 0 && users[1] == playernum as i32 + 1);
        let linked = self.client.link_entities(simorg, third_person);
        // HUD_AddEntity while observing: CHudSpectator::AddOverviewEntity for every entity the
        // engine offers (before the HLTV PVS cull).
        if users[0] != 0 {
            let offered: Vec<(i32, i32, bool, String)> = linked
                .iter()
                .filter_map(|&(i, kind)| {
                    let kind = match kind {
                        LinkKind::Player => 1,
                        LinkKind::Normal => 0,
                        LinkKind::Beam => return None,
                    };
                    let e = self.client.entities().get(i)?;
                    let model = usize::try_from(e.curstate.modelindex()).ok().and_then(|m| self.client.level().models.get(m).cloned().flatten()).unwrap_or_default();
                    Some((kind, i as i32, e.curstate.solid() != 0, model))
                })
                .collect();
            let (hud, eng) = self.hud_host();
            for (kind, index, solid, model) in offered {
                hud.overview_add_entity(kind, index, solid, &model, &eng);
            }
        }
        // HUD_AddEntity: in-eye hides the observed player's own model; an HLTV client also
        // drops what is outside the PVS around cl.simorg (ClientDLL_AddEntity).
        let hltv_pvs = hltv.then(|| self.client.world().map(|w| (w.clone(), w.leaf_pvs(w.point_leaf(simorg))))).flatten();
        let drawn: Vec<(usize, LinkKind)> = linked
            .iter()
            .copied()
            .filter(|&(i, _)| users[0] == 0 || (users[0] != 4 && pip != 2.0) || i as i32 != users[1])
            .filter(|&(i, kind)| {
                let Some((world, pvs)) = &hltv_pvs else { return true };
                let Some(e) = self.client.entities().get(i) else { return false };
                if kind == LinkKind::Beam || e.curstate.movetype() == 12 {
                    return true;
                }
                let Some(m) = self.registry.precached(e.curstate.modelindex()) else { return false };
                let mins = [0, 1, 2].map(|k| e.origin[k] + m.info.mins[k]);
                let maxs = [0, 1, 2].map(|k| e.origin[k] + m.info.maxs[k]);
                world.box_in_pvs(pvs, mins, maxs)
            })
            .collect();
        let view_entity = match self.client.level().view_entity {
            v if v > 0 => v as usize,
            _ => playernum + 1,
        };
        let movevars = self.client.level().movevars.clone();
        // CL_EmitEntities draws nothing and updates no temporary entity without a valid frame.
        if self.client.emit_frame_valid() {
            let env = ClientEnv { registry: &self.registry, client: &self.client, pvs: &self.pvs };
            effects::link_effects(&mut self.effects, &env, &linked, view_entity, self.last_view_angles, viewheight, time);
            self.effects.update_tents(&env, movevars.gravity);
        }
        // CL_FireEvents, last in CL_EmitEntities.
        let fired = self.client.fire_events();
        if !fired.is_empty() {
            let (show_events, cl_time) = (self.console.cvars.value("cl_showevents") != 0.0, self.client.time());
            let env = ClientEnv { registry: &self.registry, client: &self.client, pvs: &self.pvs };
            let mut host = EventHost::new(&self.client, &mut self.effects, env, viewheight, self.studio.view_model_attachments());
            host.solid_players = self.console.cvars.value("cl_solid_players") != 0.0;
            for e in &fired {
                match e.name.as_deref().filter(|n| cs_client::events::ClientEvents::is_hooked(n)) {
                    Some(name) => {
                        if show_events {
                            let text = format!("{} {cl_time:.6} {name}", e.slot);
                            self.nprint.nxprintf((e.slot & 31) as i32, 4.0, [1.0, 1.0, 1.0], &text, self.realtime);
                        }
                        self.client_events.fire(&mut host, name, &e.args);
                    }
                    None => notify::con_dprintf(&format!("CL_FireEvents:  Unknown event {}:{}\n", e.index, e.name.as_deref().unwrap_or("(null)"))),
                }
            }
            let out = host.out;
            self.play_requests(&out.sounds);
            if let Some((sequence, body)) = out.weapon_anim {
                self.client.entities_and_level().1.set_weapon_anim(sequence, body);
            }
            if out.muzzle_flash {
                self.studio.view_model_muzzle_flash();
            }
            if out.remove_all_decals {
                self.renderer.decal_remove_all();
            }
            for (name, entity, origin) in out.decals {
                let texture = self.renderer.decal_index(&name);
                self.shoot_decal(texture, entity, 0, origin, 0);
            }
        }
        self.client_events.think(time);
        (self.client_events.max_shells, self.client_events.max_smoke_puffs) = self.hud.effect_queue_limits;
        // The MOTD browser's page, once its navigation finished.
        if let Some(page) = self.engine.motd.poll() {
            self.hud.viewport.motd_page(&page);
        }
        // V_CalcNormalRefdef starts with V_DriftPitch; the engine copies rp.cl_viewangles back
        // into cl.viewangles, so the drift reaches the next usercmd.
        if self.client.level().intermission == 0 && users[0] == 0 && !hltv && !self.paused {
            let cv = &self.console.cvars;
            let params = DriftParams {
                time: self.client.time(),
                frametime: self.frametime as f32,
                noclip: self.client.entities().get(playernum + 1).is_some_and(|e| e.curstate.movetype() == MOVETYPE_NOCLIP),
                onground,
                demoplayback: self.client.demo_playing(),
                spectator: hltv,
                forwardmove: self.client.next_cmd.forwardmove,
                forwardspeed: cv.value("cl_forwardspeed"),
                centermove: cv.value("v_centermove"),
                centerspeed: cv.value("v_centerspeed"),
                idealpitch: sim.idealpitch,
            };
            let mut va = self.angles.to_array();
            self.input.drift.drift(&params, &mut va);
            self.angles = Vec3::from(va);
            self.client.viewangles = va;
        }
        let cl_viewangles = self.angles.to_array();
        // CHudSpectator::GetDirectorCamera for the roaming view, then the observer flags the
        // chase cameras read.
        let roaming = if users[0] == 3 {
            let (mut pos, mut ang) = (simorg, cl_viewangles);
            let (hud, mut eng) = self.hud_host();
            hud.director_camera(&mut pos, &mut ang, &mut eng);
            (pos, ang)
        } else {
            (simorg, cl_viewangles)
        };
        self.view_state.observer_flags = self.hud.observer.observer_flags;
        let cone_wanted = users[0] != 0 && pip != 0.0 && self.console.cvars.value("spec_drawcone_internal") != 0.0;
        let force_camera = self.hud.viewport.force_camera;
        // V_CalcShake mutates gVShake; it is lent out of the client while the view is built.
        let mut shake = std::mem::take(&mut self.client.shake);
        let (mut refdef, inset, (cone_main, cone_inset)) = {
            let spec_view = (users[0] != 0 || hltv).then(|| SpecView::new(&self.client));
            let mut view_host = ViewHost { client: &self.client, shake: &mut shake, rng: self.client.rng().clone() };
            let params = RefParams {
                time: time as f32,
                frametime: self.frametime as f32,
                simorg,
                simvel,
                viewheight,
                cl_viewangles,
                punchangle: punch,
                health: cd.health(),
                onground,
                movevars: &movevars,
                viewentity: view_entity as i32,
                viewentity_pose: self.client.entities().get(view_entity).map(|e| (e.origin, e.angles)),
                intermission: self.client.level().intermission != 0,
                paused: self.paused,
                waterlevel,
                crosshair_angle: self.client.level().crosshair_angle,
                idle_scale: self.hud.idle_scale(),
                waterdist: self.console.cvars.value("cl_waterdist"),
                spectator: spec_view.as_ref().map(|env| SpecParams {
                    user1: users[0],
                    user2: users[1],
                    user3: users[2],
                    local_index: playernum as i32 + 1,
                    autodirector: self.console.cvars.value("spec_autodirector_internal") != 0.0,
                    chase_dist: self.console.cvars.value("cl_chasedist"),
                    map_origin: self.hud.observer.map_origin,
                    map_zoom: self.hud.observer.map_zoom,
                    env: env as &dyn SpecEnv,
                    hltv,
                    jump: (self.hud.observer.jump_origin, self.hud.observer.jump_angles),
                    roaming,
                    camera: (self.hud.observer.camera_origin, self.hud.observer.camera_angles),
                    pip: pip as i32,
                }),
                cam_ofs: self.input.cam.thirdperson.then_some(self.input.cam.ofs),
                bob: ["cl_bob", "cl_bobcycle", "cl_bobup"].map(|n| self.console.cvars.value(n)),
                scr_ofs: ["scr_ofsx", "scr_ofsy", "scr_ofsz"].map(|n| self.console.cvars.value(n)),
                maxclients: i32::from(self.client.protocol().maxclients),
            };
            let main = self.view_state.calc_refdef(&params, &mut view_host);
            // V_RenderView's second pass when V_CalcSpectatorRefdef asked for it, and the view
            // cone camera of each map pass (DrawOverviewEntities).
            let inset = if main.next_view { self.view_state.calc_spectator_inset(&params, &main, &mut view_host) } else { None };
            let cone_main = if cone_wanted && matches!(users[0], 5 | 6) { self.view_state.overview_cone(&params) } else { None };
            let map_inset = inset.is_some_and(|r| r.only_client_draw) && pip >= 3.0 && force_camera == 0;
            let cone_inset = if cone_wanted && map_inset { self.view_state.overview_cone(&params) } else { None };
            (main, inset, (cone_main, cone_inset))
        };
        self.client.shake = shake;
        // cl_predlog's frame line (nothing without the log).
        if !self.client.demo_playing() {
            self.client.pred_log_view(refdef.vieworg);
        }
        // v_origin / v_angles (the last pass's) for the director's sounds, spec_decal and the
        // map pan.
        let last = inset.unwrap_or(refdef);
        self.hud.v_origin = last.vieworg;
        self.hud.v_angles = last.viewangles;
        self.apply_observer_requests();
        let mut viewmodel = viewmodel;
        // The pass showing the world draws the view model: the main view, or the inset under a
        // map view. HLTV / in-eye inset: the target's view model (pfnWeaponAnim(0, 0) when it
        // changes).
        let world_pass = if refdef.only_client_draw { inset.filter(|r| !r.only_client_draw) } else { Some(refdef) };
        if let Some(index) = world_pass.and_then(|r| r.view_model_index) {
            viewmodel = index;
            if world_pass.is_some_and(|r| r.weapon_anim_reset) {
                self.client.entities_and_level().1.set_weapon_anim(0, 0);
            }
        }
        // V_RenderView's demo hooks (pass 0, not HLTV): a playback replaces the client's view
        // with the recorded one and copies it back into cl.*; otherwise the snapshot every
        // recorded network frame carries.
        if let Some(replay) = self.client.demo_view() {
            let rp = replay.rp;
            refdef.vieworg = rp.vieworg;
            refdef.viewangles = rp.viewangles;
            refdef.gun_angles = [-rp.viewangles[0], rp.viewangles[1], rp.viewangles[2]];
            if let Some(view) = replay.view {
                refdef.gun_origin = view;
            }
            if let Some(vm) = replay.viewmodel {
                viewmodel = vm;
            }
            if rp.intermission != 0 {
                refdef.view_model = false;
            }
            self.angles = Vec3::from(rp.cl_viewangles);
            self.replay = Some((rp, viewmodel));
        } else if !self.client.demo_playing() {
            let (forward, right, up) = angle_vectors(cl_viewangles);
            let rp = RefParamsRaw {
                vieworg: refdef.vieworg,
                viewangles: refdef.viewangles,
                forward,
                right,
                up,
                frametime: self.frametime as f32,
                time: time as f32,
                intermission: i32::from(self.client.level().intermission != 0),
                paused: i32::from(self.paused),
                spectator: i32::from(self.client.spectator()),
                onground: i32::from(onground),
                waterlevel,
                simvel,
                simorg,
                viewheight,
                idealpitch: sim.idealpitch,
                cl_viewangles,
                health: cd.health() as i32,
                crosshairangle: self.client.level().crosshair_angle,
                viewsize: 120.0,
                punchangle: punch,
                maxclients: maxclients as i32,
                viewentity: view_entity as i32,
                playernum: playernum as i32,
                max_entities: self.client.protocol().max_edicts as i32,
                demoplayback: 0,
                hardware: 1,
                smoothing: 0,
                cmd: 0,
                movevars: 0,
                viewport: [0, 0, self.vid.0 as i32, self.vid.1 as i32],
                next_view: 0,
                only_client_draw: 0,
            };
            let info = DemoInfo::snapshot(rp, &self.client.next_cmd, &movevars, refdef.gun_origin, viewmodel);
            self.client.demo_snapshot(info);
        }
        let level = self.client.level();
        if users[0] != 0 {
            let target = usize::try_from(users[1]).ok().and_then(|i| self.client.entities().get(i)).map(|e| (e.origin, e.angles, e.curstate.solid()));
            log::trace!(
                "observer mode {} target {} ({:?}) second {}: simorg {simorg:?} view {:?} {:?}",
                users[0],
                users[1],
                target,
                users[2],
                refdef.vieworg,
                refdef.viewangles
            );
        }
        self.last_view_angles = refdef.v_angles;
        self.engine.view = (refdef.vieworg, refdef.viewangles);
        {
            // IEngineWrapper::GetViewOrigin / GetViewAngles read r_refdef.
            let mut h = self.gameui.host.borrow_mut();
            h.view_origin = refdef.vieworg;
            h.view_angles = refdef.viewangles;
        }
        let mut view = View::new(Vec3::from(refdef.vieworg), Vec3::from(refdef.viewangles));
        // SCR_CalcRefdef clamps scr_fov_value to 10..150.
        view.fov = self.scr_fov.clamp(10.0, 150.0);
        view.time = time;
        // RenderFog's tri_Fog (on only with gl_fog) and the engine's underwater fog.
        let fog = self.hud.fog;
        view.fog = gs_render::Fog {
            user: (fog.active(sim.waterlevel) && self.console.cvars.value("gl_fog") > 0.0)
                .then(|| (fog.color.map(|c| f32::from(c) / 255.0), fog.density)),
            underwater: sim.waterlevel > 2,
        };
        view.light_styles = animate_styles(&level.lightstyles, time);
        if movevars.zmax > 0.0 {
            view.zfar = movevars.zmax;
        }
        // R_SetupGL's far plane for a client-only (map) view.
        if refdef.only_client_draw {
            view.zfar = 16000.0;
        }
        view.wave_height = movevars.wave_height;

        let (forward, right, up) = angle_vectors(refdef.viewangles);
        // StudioCheckBBox: the frustum in multiplayer with r_cullsequencebox, else the eye plane;
        // nothing culled while a picture-in-picture view reuses these draws.
        let cull = inset.is_none().then(|| StudioCull {
            world_to_screen: gs_render::camera::view_projection(&view, self.vid.0, self.vid.1).to_cols_array(),
            frustum: maxclients > 1 && self.console.cvars.value("r_cullsequencebox") != 0.0,
        });
        let times = FrameTimes { time, old_time, frametime: self.frametime, view_origin: refdef.vieworg, view_forward: forward, cull };
        let muzzleflash = self.client.entities().get(view_entity).is_some_and(|e| e.curstate.effects() & EF_MUZZLEFLASH != 0);
        let gun = if refdef.only_client_draw { inset.filter(|r| !r.only_client_draw) } else { Some(refdef) };
        let vm = ViewModelInput {
            modelindex: viewmodel,
            weapon_id: cd.m_id(),
            fov: view.fov,
            health: cd.health(),
            origin: gun.map_or(refdef.gun_origin, |r| r.gun_origin),
            angles: gun.map_or(refdef.gun_angles, |r| r.gun_angles),
            right_hand: self.console.cvars.value("cl_righthand"),
            muzzleflash,
        };
        // R_DrawViewModel: cl.light_level = (r+g+b)/3 of R_LightPoint at the view entity every
        // rendered frame, drawn or not; the next command carries it.
        let o = vm.origin;
        let light = self.client.world().map(|world| world.light_vec(&view.light_styles, o, [o[0], o[1], o[2] - 2048.0]));
        if let Some(c) = light {
            self.client.light_level = (c[0] + c[1] + c[2]) / 3;
        }
        let (entities, level) = self.client.entities_and_level();
        let mut weapon_starttime = level.weapon_starttime;
        let vm = gun.is_some_and(|r| r.view_model).then_some(&vm);
        let studio_frame = self.studio.draw(entities, level, maxclients, &drawn, &times, vm, &mut weapon_starttime);
        level.weapon_starttime = weapon_starttime;

        let mut brushes = Vec::new();
        let mut pending_sprites = Vec::new();
        for &(index, kind) in &drawn {
            if kind != LinkKind::Normal {
                continue;
            }
            let Some(entry) = entities.get(index).and_then(|e| self.registry.precached(e.curstate.modelindex())) else { continue };
            match entry.data {
                effects::ModelData::Sprite(handle) => {
                    let Some(ent) = entities.get(index) else { continue };
                    // r_entorigin: an attachment of entity `skin` when `body` is set.
                    let (body, skin) = (ent.curstate.body(), i32::from(ent.curstate.skin()));
                    let origin = if body != 0 {
                        usize::try_from(skin)
                            .ok()
                            .and_then(|s| entities.get(s))
                            .and_then(|owner| owner.attachment.get(usize::try_from(body - 1).ok()?).copied())
                            .unwrap_or(ent.origin)
                    } else {
                        ent.origin
                    };
                    let (renderfx, renderamt) = (ent.curstate.renderfx(), ent.curstate.renderamt());
                    let Some(ent) = entities.get_mut(index) else { continue };
                    let blend = self.studio.entity_blend(ent, &times);
                    let draw = SpriteDraw {
                        sprite: handle,
                        frame: ent.curstate.frame(),
                        origin,
                        angles: ent.angles,
                        scale: ent.curstate.scale(),
                        rendermode: RenderMode::from_i32(ent.curstate.rendermode()),
                        blend,
                        color: ent.curstate.rendercolor(),
                    };
                    pending_sprites.push(PendingSprite { draw, renderfx, renderamt });
                }
                effects::ModelData::Brush(model) if model >= 1 => {
                    let Some(ent) = entities.get_mut(index) else { continue };
                    let rendermode = RenderMode::from_i32(ent.curstate.rendermode());
                    let blend = self.studio.entity_blend(ent, &times);
                    brushes.push(BrushInstance {
                        model,
                        origin: Vec3::from(ent.origin),
                        angles: Vec3::from(ent.angles),
                        rendermode,
                        blend,
                        color: ent.curstate.rendercolor(),
                    });
                }
                _ => {}
            }
        }
        {
            let env = ClientEnv { registry: &self.registry, client: &self.client, pvs: &self.pvs };
            effects::studio_events(&mut self.effects, &env, self.audio.as_mut(), &studio_frame.events, time);
        }
        // Temp entities after the studio events, so this frame's muzzle flashes are drawn.
        let tents = effects::tent_draws(&self.effects, &self.registry, &mut self.studio, self.client.level(), &times);
        {
            let env = ClientEnv { registry: &self.registry, client: &self.client, pvs: &self.pvs };
            effects::studio_events(&mut self.effects, &env, self.audio.as_mut(), &tents.events, time);
        }
        brushes.extend(tents.brushes);
        pending_sprites.extend(tents.sprites);
        // HUD_CreateEntities: CVoiceStatusHud's speaker sprite 45 units over every talking player
        // that came in the last packet (players linked this frame), never the local one
        // (cam_thirdperson is off).
        if let Some(effects::ModelData::Sprite(icon)) = self.registry.named("sprites/voiceicon.spr").map(|e| e.data.clone()) {
            let mask = {
                let (hud, eng) = self.hud_host();
                hud.voice_speaking_mask(&eng)
            };
            let entities = self.client.entities();
            for &(index, _) in linked.iter().filter(|(i, k)| *k == LinkKind::Player && *i != playernum + 1 && mask & 1 << (i - 1) != 0) {
                let Some(e) = entities.get(index) else { continue };
                let o = e.origin;
                let draw = SpriteDraw {
                    sprite: icon,
                    frame: 0.0,
                    origin: [o[0], o[1], o[2] + 45.0],
                    angles: [0.0; 3],
                    scale: 0.5,
                    rendermode: RenderMode::TransAdd,
                    blend: 1.0,
                    color: [0; 3],
                };
                pending_sprites.push(PendingSprite { draw, renderfx: 14, renderamt: 255 });
            }
        }
        let sprites: Vec<SpriteDraw> = pending_sprites.into_iter().map(|p| self.glow(p, refdef.vieworg)).collect();
        let shadows = effects::shadow_batches(&studio_frame.shadows, self.shadow_sprite, &self.registry, &self.client);
        let shell_sprite = match self.registry.named("sprites/shellchrome.spr").map(|e| &e.data) {
            Some(effects::ModelData::Sprite(h)) => Some(*h),
            _ => None,
        };

        let dlights = effects::studio_dlights(&self.effects, time);
        let elights = effects::studio_elights(&self.effects, time);
        let mut studios = Vec::with_capacity(studio_frame.draws.len() + tents.studios.len());
        let mut light = StudioLight::FULLBRIGHT;
        let mut local_lights = Vec::new();
        for d in studio_frame.draws.into_iter().chain(tents.studios) {
            if let LightSource::Entity(o) = d.light {
                let target =
                    LightTarget { origin: o.origin, effects: o.effects, model_flags: o.model_flags, renderfx: o.renderfx, iuser4: o.iuser4 };
                light = self
                    .renderer
                    .studio_light(&target, &view.light_styles, movevars.skycolor, movevars.skyvec, &dlights)
                    .unwrap_or(StudioLight::FULLBRIGHT);
                local_lights = select_local_lights(&elights, o.origin, self.renderer.gamma());
                log::trace!(
                    "light {} at {:?}: ambient {} shade {} color {:?} dir {:?} (sky {:?} {:?})",
                    d.model.name,
                    o.origin,
                    light.ambient,
                    light.shade,
                    light.color,
                    light.dir,
                    movevars.skycolor,
                    movevars.skyvec
                );
            }
            let key = Arc::as_ptr(&d.model) as usize;
            let handle = match self.studio_handles.get(&key) {
                Some((_, h)) => *h,
                None => {
                    let h = self.renderer.upload_studio(d.model.clone());
                    self.studio_handles.insert(key, (d.model.clone(), h));
                    h
                }
            };
            studios.push(StudioDraw {
                model: handle,
                bones: d.bones,
                light,
                local_lights: local_lights.clone(),
                origin: d.origin,
                body: d.body,
                skin: d.skin,
                rendermode: RenderMode::from_i32(d.rendermode),
                blend: d.blend,
                view_model: d.view_model,
                mirrored: d.mirrored,
                shell: shell_sprite.filter(|_| d.renderfx == RENDERFX_GLOW_SHELL).map(|sprite| GlowShell {
                    sprite,
                    color: d.rendercolor,
                    renderamt: d.renderamt,
                    time,
                    freq: f64::from(self.console.cvars.value("r_glowshellfreq")),
                }),
            });
        }

        let beam_entities = effects::beam_entities(&mut self.client, &mut self.studio, &linked, &times);
        let (width, height) = self.vid;
        let view_params = ViewParams {
            origin: refdef.vieworg,
            forward,
            right,
            up,
            world_to_screen: gs_render::camera::view_projection(&view, width, height).to_cols_array(),
            gravity: movevars.gravity,
        };
        let geometry = {
            let env = ClientEnv { registry: &self.registry, client: &self.client, pvs: &self.pvs };
            self.effects.draw(&env, &view_params, &beam_entities)
        };
        let (particles, effect_batches) = effects::scene_geometry(&geometry, &self.registry);
        self.apply_effect_events();
        // R_MarkLeaves: the view leaf's PVS, used by the particle manager now and by the next
        // frame's temp entity update.
        if let Some(world) = self.client.world() {
            self.pvs = world.leaf_pvs(world.point_leaf(refdef.vieworg));
        }
        // HUD_DrawTransparentTriangles: the particle manager's SetVariables + Update, then
        // CEnvironment::Update (new weather particles show from the next frame).
        let transparent = {
            let user1 = users[0];
            let input = particleman::FrameInput {
                time: time as f32,
                view_angles: if user1 != 0 && user1 != 3 { refdef.viewangles } else { self.angles.to_array() },
                cl_viewangles: self.angles.to_array(),
                player_origin: simorg,
                view_projection: view_params.world_to_screen,
                gravity: movevars.gravity,
                weather_old_time: self.client_events.environment.old_time(),
            };
            let host = ParticleHost { env: ClientEnv { registry: &self.registry, client: &self.client, pvs: &self.pvs }, styles: &view.light_styles };
            let events = &mut self.client_events;
            let quads = events.particleman.update(&mut self.effects, &host, &input);
            let weather = WeatherInput {
                time: time as f32,
                origin: simorg,
                observer: (users[0], users[1]),
                cl_weather: self.console.cvars.value("cl_weather"),
                player_velocity: simvel,
            };
            if let Some(v) = events.environment.update(&mut events.particleman, &mut self.effects, &host, &weather) {
                self.console.cvars.direct_set("cl_weather", &v.to_string());
            }
            effects::particle_batches(&quads, &self.registry)
        };
        let dlights = effects::world_dlights(&self.effects, time);
        // HUD_DrawNormalTriangles of each pass (the overview in a map view), before HUD_Redraw
        // reads the name anchors.
        let normal = self.overview_batches(0, refdef.viewangles, &view, (width, height), cone_main);
        let inset = inset.map(|r| {
            let rect = self.hud.overview.window;
            let mut iview = View::new(Vec3::from(r.vieworg), Vec3::from(r.viewangles));
            iview.fov = view.fov;
            iview.time = time;
            iview.fog = view.fog;
            iview.light_styles = view.light_styles;
            iview.wave_height = view.wave_height;
            if movevars.zmax > 0.0 {
                iview.zfar = movevars.zmax;
            }
            if r.only_client_draw {
                iview.zfar = 16000.0;
            }
            let proj = (rect[2].max(1) as u32, rect[3].max(1) as u32);
            let normal = self.overview_batches(1, r.viewangles, &iview, proj, cone_inset);
            let scene = if r.only_client_draw {
                Scene { transparent: transparent.clone(), normal, only_client_draw: true, ..Scene::default() }
            } else {
                Scene {
                    brushes: brushes.clone(),
                    studios: studios.clone(),
                    sprites: sprites.clone(),
                    shadows: shadows.clone(),
                    transparent: transparent.clone(),
                    particles: particles.clone(),
                    effects: effect_batches.clone(),
                    normal,
                    only_client_draw: false,
                    dlights: dlights.clone(),
                    overlay: Vec::new(),
                }
            };
            (iview, scene, rect)
        });
        let overlay = self.draw_hud();
        self.update_audio(refdef.vieworg, right, view_entity);
        let only_client_draw = refdef.only_client_draw;
        // V_RenderView needs cls.state == active and signon 2: otherwise only the frame clear and
        // the 2D layers (the loading dialog over black while reconnecting).
        if self.client.state() != State::Active {
            return FrameScene { view, scene: Scene { overlay, only_client_draw: true, ..Scene::default() }, inset: None };
        }
        FrameScene {
            view,
            scene: Scene { brushes, studios, sprites, shadows, transparent, particles, effects: effect_batches, normal, only_client_draw, dlights, overlay },
            inset,
        }
    }

    /// The level's world for the renderer (`Mod_ForName` of the map); `R_NewMap`'s view reset.
    pub(super) fn load_map(&mut self, map_model: &str) {
        let name = map_model.trim_start_matches("maps/").trim_end_matches(".bsp").to_owned();
        match MapAssets::load(&self.fs, &name).map_err(anyhow::Error::from).and_then(|m| {
            self.renderer.load_map(&m)?;
            Ok(m)
        }) {
            Ok(m) => {
                self.map = Some(m);
                // R_NewMap → V_ResetShakeFade; v_dark is one-shot.
                let v_dark = self.console.cvars.value("v_dark") != 0.0;
                self.client.new_map_view_reset(v_dark);
                if v_dark && self.console.cvars.direct_set("v_dark", "0") == gs_console::cvar::SetResult::Missing {
                    log::debug!("Cvar_Set: variable v_dark not found");
                }
            }
            Err(e) => log::error!("loading {name}: {e:#}"),
        }
    }

    /// `DrawOverview` of one pass, as triangle batches with this level's sprites.
    fn overview_batches(&mut self, cycle: i32, angles: [f32; 3], view: &View, proj: (u32, u32), cone: Option<([f32; 3], [f32; 3])>) -> Vec<EffectBatch> {
        let pass = OverviewPass { cycle, angles, world_to_screen: gs_render::camera::view_projection(view, proj.0, proj.1).to_cols_array(), cone };
        let batches = {
            let (hud, mut eng) = self.hud_host();
            hud.draw_overview(&pass, &mut eng)
        };
        batches
            .into_iter()
            .filter_map(|b| {
                let (sprite, frame) = match b.texture {
                    OverviewTexture::Map(f) => (self.overview_sprite?, f),
                    OverviewTexture::Sprite(name) => match self.registry.named(name).map(|e| &e.data) {
                        Some(effects::ModelData::Sprite(h)) => (*h, 0),
                        _ => return None,
                    },
                };
                let vertices = b.vertices.iter().map(|v| StudioVertex { pos: v.pos, uv: v.st, color: b.color }).collect();
                Some(EffectBatch { sprite, frame, blend: if b.additive { EffectBlend::Additive } else { EffectBlend::AlphaDepth }, vertices })
            })
            .collect()
    }

    /// `cle_LoadMapSprite` for the image `CHudSpectator::Reset` asked for: an 8-bit BMP or a
    /// TGA cut into 128×128 tiles.
    fn load_overview_map(&mut self) {
        let Some(image) = self.hud.take_overview_image() else { return };
        let lower = image.to_ascii_lowercase();
        let decoded = if lower.ends_with(".bmp") {
            self.fs.read(&lower).ok().and_then(|d| decode_map_bmp(&d))
        } else if lower.ends_with(".tga") {
            gs_assets::RgbaImage::load(&self.fs, &lower).ok().map(|i| (i.width, i.height, i.pixels))
        } else {
            // The original stops with Sys_Error("Unsupported map image file format.").
            log::error!("{image}: Unsupported map image file format.");
            None
        };
        match decoded.and_then(|(w, h, rgba)| map_tiles(w, h, &rgba)) {
            Some(tiles) => {
                self.overview_sprite = Some(self.renderer.upload_tiles(&lower, &tiles));
                self.hud.set_overview_frames(Some(tiles.len() as u32));
            }
            None => {
                crate::notify::con_printf(&format!("Wrong map image dimensions or file could not be loaded in {lower}\n"));
                self.overview_sprite = None;
                self.hud.set_overview_frames(None);
            }
        }
    }

    /// Sounds requested by client code (events, temp entities).
    pub(super) fn play_requests(&mut self, requests: &[SoundRequest]) {
        if let Some(audio) = &mut self.audio {
            for r in requests {
                audio.request(r);
            }
        }
    }

    /// `S_Update(r_origin, vpn, vright, vup)` after the frame's view: the predicted movement's
    /// `PM_PlaySound`s on the local player first, then spatialization from the entities of the
    /// latest packet (brush models at their centre) and the mix.
    fn update_audio(&mut self, origin: [f32; 3], right: [f32; 3], view_entity: usize) {
        let pm_sounds = self.client.take_pmove_sounds();
        let Some(audio) = &mut self.audio else { return };
        let player = i32::from(self.client.protocol().playernum) + 1;
        let sim = *self.client.sim();
        for s in pm_sounds {
            let PmSound { channel, sample, volume, attenuation } = s;
            audio.request(&SoundRequest::Play { ent: player, origin: sim.origin, channel, sample, volume, attenuation, flags: 0, pitch: 100 });
        }
        let entities = self.client.entities();
        let mut entity_origins = Vec::new();
        for state in &self.client.protocol().current_frame().entities {
            let Ok(index) = usize::try_from(state.number()) else { continue };
            let Some(ent) = entities.get(index).filter(|e| index > 0 && e.curstate.modelindex() != 0) else { continue };
            let mut o = ent.origin;
            if let Some(m) = self.registry.precached(ent.curstate.modelindex()).filter(|m| m.info.kind == ModelKind::Brush) {
                o = [0, 1, 2].map(|i| o[i] + (m.info.mins[i] + m.info.maxs[i]) * 0.5);
            }
            if entity_origins.len() <= index {
                entity_origins.resize(index + 1, None);
            }
            entity_origins[index] = Some(o);
        }
        let listener = Listener { origin, right, view_entity: view_entity as i32, entity_origins, underwater: sim.waterlevel > 2, paused: self.paused };
        // R_ForceCVars, every rendered view of a multiplayer game: snd_show 0.
        let maxclients = self.client.protocol().maxclients;
        if self.client.state() == State::Active && maxclients > 1 && self.console.cvars.value("snd_show") != 0.0 {
            self.console.cvars.direct_set("snd_show", "0");
        }
        if let Some(p) = audio.update(&self.console, listener, self.client.sys_timescale()) {
            // SX_RoomFX: the new preset's values into the room cvars.
            let values = [
                ("room_lp", p.lp),
                ("room_mod", p.modulation),
                ("room_size", p.size),
                ("room_refl", p.refl),
                ("room_rvblp", p.rvblp),
                ("room_delay", p.delay),
                ("room_feedback", p.feedback),
                ("room_dlylp", p.dlylp),
                ("room_left", p.left),
            ];
            for (name, v) in values {
                self.console.cvars.direct_set(name, &gs_console::cvar::format_value(v));
            }
        }
        // S_PrintChannels, right after S_Update: a fresh 4 s Con_NPrintf line per channel slot.
        if self.console.cvars.value("s_show") != 0.0 && maxclients <= 2 {
            for (i, line) in audio.channel_lines().iter().enumerate() {
                self.nprint.nxprintf((i & 31) as i32, 4.0, [1.0; 3], line.as_deref().unwrap_or(""), self.realtime);
            }
        }
    }

    /// Side effects of the engine effects: decals and sounds.
    fn apply_effect_events(&mut self) {
        for event in self.effects.take_events() {
            match event {
                gs_effects::Event::Decal { decal, entity, model, origin, flags } => {
                    if let Some(texture) = self.server_decal(decal) {
                        self.shoot_decal(texture, entity, model, origin, flags as u16);
                    }
                }
                // TE_PLAYERDECAL (engine-misc §5.4): the player's logo; a customization without a
                // valid logo draws nothing; no customization at all uses decals.wad.
                gs_effects::Event::PlayerDecal { player, origin, entity, decal } => {
                    let slot = usize::from(player).wrapping_sub(1);
                    if usize::try_from(entity).is_ok_and(|e| e >= self.client.protocol().max_edicts) {
                        log::warn!("Decal: entity = {entity}");
                        continue;
                    }
                    let decal_for = if slot <= 31 { self.client.player_decal(slot) } else { gs_client::PlayerDecal::None };
                    match decal_for {
                        gs_client::PlayerDecal::Logo(texture, md5) => {
                            let texture = texture.clone();
                            let index = self.renderer.custom_decal(slot, md5, &texture);
                            self.shoot_decal(index, entity, 0, origin, gs_render::decal::flags::CUSTOM);
                        }
                        gs_client::PlayerDecal::Invalid => {}
                        gs_client::PlayerDecal::None => {
                            if let Some(texture) = self.server_decal(i32::from(decal)) {
                                self.shoot_decal(texture, entity, 0, origin, gs_render::decal::flags::CUSTOM);
                            }
                        }
                    }
                }
                // TE sounds: S_StartDynamicSound(-1, CHAN_AUTO, …, flags 0, pitch).
                gs_effects::Event::Sound { sample, origin, volume, attenuation, pitch } => {
                    self.play_requests(&[SoundRequest::Play { ent: -1, origin, channel: 0, sample, volume, attenuation, flags: 0, pitch }]);
                }
                // TE_TEXTMESSAGE (render.md §D.11, client-hud.md §2.3.6): the channel's engine slot
                // `__NETMESSAGE__<channel+1>` takes the message, then CL_HudMessage dispatches
                // HudText with that name; the HUD resolves it through TextMessageGet.
                gs_effects::Event::TextMessage(m) => {
                    const NET_MESSAGES: [&str; 4] = ["__NETMESSAGE__1", "__NETMESSAGE__2", "__NETMESSAGE__3", "__NETMESSAGE__4"];
                    let name = NET_MESSAGES[usize::from(m.channel & 3)];
                    // fxtime is on the wire only for effect 2; otherwise the slot keeps its own.
                    let fxtime = if m.effect == 2 { m.fx_time } else { self.titles.get(name).map_or(0.0, |t| t.fxtime) };
                    self.titles.set(gs_draw::titles::TextMessage {
                        effect: i32::from(m.effect),
                        color1: m.color1,
                        color2: m.color2,
                        x: m.x,
                        y: m.y,
                        fadein: m.fade_in,
                        fadeout: m.fade_out,
                        holdtime: m.hold,
                        fxtime,
                        name: name.into(),
                        message: m.text,
                    });
                    let (hud, mut eng) = self.hud_host();
                    hud.msg("HudText", name.as_bytes(), &mut eng);
                }
            }
        }
    }

    /// `Draw_DecalIndex`: a server decal index (`svc_decalname`) to the decal list.
    fn server_decal(&mut self, index: i32) -> Option<usize> {
        let name = usize::try_from(index).ok().and_then(|i| self.client.level().decals.get(i)).cloned().flatten();
        match name {
            Some(name) if !name.is_empty() => Some(self.renderer.decal_index(&name)),
            _ => {
                log::warn!("Used decal #{index} without a name");
                None
            }
        }
    }

    /// `R_DecalShoot`: on the brush model of `entity` (the world for 0), or of precache
    /// `model_index`.
    fn shoot_decal(&mut self, texture: usize, entity: i32, model_index: i32, origin: [f32; 3], flags: u16) {
        let ent = usize::try_from(entity).ok().filter(|&e| e != 0).and_then(|e| self.client.entities().get(e));
        let (model, ent_origin, ent_angles) = match ent {
            None if entity == 0 => (Some(0), [0.0; 3], [0.0; 3]),
            None => (None, [0.0; 3], [0.0; 3]),
            Some(e) => {
                let index = if e.curstate.modelindex() != 0 { e.curstate.modelindex() } else { model_index };
                let model = match self.registry.precached(index).map(|m| &m.data) {
                    Some(effects::ModelData::Brush(sub)) => Some(*sub),
                    _ => None,
                };
                (model, e.origin, e.angles)
            }
        };
        let Some(model) = model else {
            log::debug!("Decals must hit mod_brush!");
            return;
        };
        let effects = &mut self.effects;
        self.renderer.decal_shoot(texture, model, entity, ent_origin, ent_angles, origin, flags, &mut |lo, hi| effects.random_float(lo, hi));
    }

    /// Glow sprite visibility (`R_DrawTEntitiesOnList`, render.md §C.3.3): occluded glows vanish,
    /// visible ones fade with distance and grow to `dist · 0.005` scale.
    fn glow(&self, p: PendingSprite, vieworg: [f32; 3]) -> SpriteDraw {
        let mut draw = p.draw;
        if draw.rendermode != RenderMode::Glow {
            return draw;
        }
        let d = [0, 1, 2].map(|i| draw.origin[i] - vieworg[i]);
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        // The point hull; r_traceglow 0 ignores studio models and glass.
        let flags = if self.console.cvars.value("r_traceglow") == 0.0 { 5 } else { 0 };
        let fraction = self.client.trace(vieworg, draw.origin, 2, flags).map_or(1.0, |tr| tr.fraction);
        let g = if (1.0 - fraction) * dist > 8.0 {
            0.0
        } else if p.renderfx == RENDERFX_NO_DISSIPATION {
            p.renderamt as f32 / 255.0
        } else {
            draw.scale = dist * 0.005;
            (19000.0 / (dist * dist)).clamp(0.05, 1.0)
        };
        draw.blend *= g;
        draw
    }
}
