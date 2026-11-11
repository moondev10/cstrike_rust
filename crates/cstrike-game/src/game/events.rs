//! The client's events of a frame (level loads, prints, messages for the HUD, sounds) and the
//! demo player's items.

use super::*;

impl Running {
    pub(super) fn handle_events(&mut self) {
        // The EV_Fire* shell offsets and MsgFunc_Brass read cl_righthand (0 = left hand); the
        // corpse and smoke handlers cl_corpsestay and fastsprites; the studio renderer
        // cl_shadows and the cl_minmodels team models (with g_PlayerExtraInfo's teams).
        {
            let cv = &self.console.cvars;
            self.client_events.right_hand = cv.value("cl_righthand") != 0.0;
            self.client_events.corpse_stay = cv.value("cl_corpsestay");
            self.client_events.fast_sprites = cv.value("fastsprites");
            self.effects.cvars.corpse_stay = cv.value("cl_corpsestay");
            self.studio.shadows = cv.value("cl_shadows") != 0.0;
            let mm = &mut self.studio.min_models;
            mm.value = cv.value("cl_minmodels");
            mm.min_t = cv.value("cl_min_t");
            mm.min_ct = cv.value("cl_min_ct");
            mm.players.resize(self.hud.players.len(), (0, false));
            for (slot, p) in mm.players.iter_mut().zip(&self.hud.players) {
                *slot = (p.teamnumber, p.vip);
            }
        }
        while let Some(event) = self.client.poll_event() {
            match event {
                ClientEvent::Connected(info) => {
                    // CL_ClearState: the replayed view goes with the old level; cl.viewangles
                    // start at zero.
                    self.replay = None;
                    self.angles = Vec3::ZERO;
                    self.studio.new_level();
                    self.studio_handles.clear();
                    self.registry.clear();
                    self.effects.clear();
                    // CHud::VidInit → ResetParticles.
                    self.client_events.particleman.reset();
                    self.pvs.clear();
                    self.renderer.clear_models();
                    // CL_ParseServerInfo: r_decals = mp_decals in multiplayer, sp_decals otherwise.
                    let source = if info.maxclients > 1 { "mp_decals" } else { "sp_decals" };
                    let v = self.console.cvars.value(source);
                    let text = if v == v.trunc() { format!("{}", v as i32) } else { format!("{v:.6}") };
                    self.console.cvars.direct_set("r_decals", &text);
                    // CL_ClearState: cl.paused (the client's level state has cl.intermission).
                    self.paused = false;
                    // SCR_BeginLoadingPlaque → Con_ClearNotify.
                    notify::clear_notify();
                    if let Some(audio) = &mut self.audio {
                        audio.new_level(info.maxclients);
                    }
                    // The renderer's world goes with the old level; R_NewMap loads the new one.
                    self.map = None;
                }
                ClientEvent::NewMap(map) => self.load_map(&map),
                ClientEvent::EnteredGame => {
                    let t = self.realtime;
                    // SCR_EndLoadingPlaque → Con_ClearNotify.
                    notify::clear_notify();
                    self.in_game_since = Some(t);
                    self.next_command_at = t + 1.0;
                    if self.map.is_none()
                        && let Some(map) = self.client.server_info().map(|i| i.map_model.clone())
                    {
                        self.load_map(&map);
                    }
                    let world = self.client.world().cloned();
                    self.registry.load(&self.fs, &mut self.renderer, self.client.level(), world.as_deref());
                    // Mod_LoadStudioModel uploads a precached model's textures at registration;
                    // uploading them at its first draw instead dropped frames mid-fight.
                    let models: Vec<Arc<StudioModel>> = self.client.level().studio.iter().flatten().cloned().collect();
                    for model in models {
                        let key = Arc::as_ptr(&model) as usize;
                        if !self.studio_handles.contains_key(&key) {
                            let handle = self.renderer.upload_studio(model.clone());
                            self.studio_handles.insert(key, (model, handle));
                        }
                    }
                    // The renderer's level sprites are gone: the overview's map image too.
                    self.overview_sprite = None;
                    self.hud.overview_level_loaded();
                    if let Some(audio) = &mut self.audio {
                        audio.precache(&self.client.level().sounds);
                    }
                    // The scheme fonts the panels get, made while loading rather than at a first paint.
                    self.screen.preload_fonts();
                    // HUD_VidInit after the level's resources are known.
                    let (hud, mut eng) = self.hud_host();
                    hud.vid_init(&mut eng);
                    // GameUI's ConnectToServer on becoming active (game dir not "valve").
                    self.console.add_text("mp3 stop\n");
                    // The level's uploads reach the GPU behind the loading plaque, not in the first frame.
                    self.renderer.finish_uploads();
                }
                ClientEvent::StuffText(text) => {
                    log::debug!("[stufftext] {}", text.trim_end());
                    // ValidStuffText's single-player bypass is off in a playback.
                    let single_player = self.client.protocol().maxclients == 1 && !self.client.demo_playing();
                    let policy = StuffPolicy { single_player, ..StuffPolicy::default() };
                    let (console, mut host) = self.console_host();
                    console.stuff_text(&mut host, &text, policy);
                }
                ClientEvent::Disconnected(reason) => {
                    println!("Disconnected: {reason}");
                    // CL_Disconnect → CL_ClearState: cl.viewangles back to zero.
                    self.angles = Vec3::ZERO;
                    if let Some(audio) = &mut self.audio {
                        audio.stop_all();
                    }
                }
                ClientEvent::Print(text) => notify::con_printf(&text),
                ClientEvent::Ui(e) => self.ui_events.push(e),
                ClientEvent::DevPrint(text) => {
                    if self.console.cvars.value("developer") != 0.0 {
                        notify::con_dprintf(&text);
                    }
                }
                ClientEvent::Demo(item) => self.demo_item(item),
                ClientEvent::Execute(text) => {
                    self.console.add_text(&text);
                    self.run_console();
                }
                ClientEvent::InsertText(text) => self.console.insert_text(&text),
                ClientEvent::SetCvar { name, value } => {
                    let (console, mut host) = self.console_host();
                    console.direct_set(&mut host, &name, &value);
                }
                // CL_Move's own Cvar_Set: the client already updated itself and the userinfo.
                ClientEvent::CvarMirror { name, value } => {
                    if self.console.cvars.direct_set(&name, &value) == gs_console::cvar::SetResult::Missing {
                        log::debug!("Cvar_Set: variable {name} not found");
                    }
                }
                ClientEvent::Quit => self.quit = true,
                ClientEvent::AddText(text) => self.console.add_text(&text),
                ClientEvent::CustomizationRemoved(slot) => self.renderer.remove_custom_decals(slot),
                // Con_NPrintf: four seconds, white.
                ClientEvent::NPrintf { slot, text } => self.nprint.nxprintf(slot, 4.0, [1.0, 1.0, 1.0], &text, self.realtime),
                // svc_director → HUD_DirectorMessage.
                ClientEvent::Message(ServerMessage::Director(data)) => {
                    let (hud, mut eng) = self.hud_host();
                    hud.director_message(&data, &mut eng);
                }
                // CL_ParseServerMessage's guard: a WeaponList over 123 bytes is dropped (the
                // exception is Half-Life itself, app 70).
                ClientEvent::Message(ServerMessage::UserMessage { name, data, .. }) if name.eq_ignore_ascii_case("WeaponList") && data.len() > 123 => {
                    notify::con_printf("Malformed WeaponList request, ignoring\n");
                }
                ClientEvent::Message(ServerMessage::UserMessage { name, data, .. }) => {
                    log::debug!("usermsg {name} {data:?}");
                    let handled = {
                        let (hud, mut eng) = self.hud_host();
                        hud.msg(&name, &data, &mut eng)
                    };
                    match name.as_str() {
                        "ShadowIdx" => {
                            if let Some(&b) = data.first_chunk::<4>() {
                                self.shadow_sprite = i32::from_le_bytes(b);
                            }
                        }
                        // CHudSpectator::DeathMessage: the local player's death gets the
                        // dramatic final shot.
                        "DeathMsg" => {
                            if data.get(1).is_some_and(|&victim| usize::from(victim) == usize::from(self.client.protocol().playernum) + 1) {
                                self.hud.observer.observer_flags = cs_client::view::drc_flag::DRAMATIC | cs_client::view::drc_flag::FINAL;
                                self.view_state.reset_chase_cam();
                            }
                        }
                        // CHud::MsgFunc_SetFOV: g_lastFOV.
                        "SetFOV" => {
                            if let Some(&fov) = data.first() {
                                self.last_fov = f32::from(fov);
                            }
                        }
                        "Brass" | "ClCorpse" => {
                            let env = ClientEnv { registry: &self.registry, client: &self.client, pvs: &self.pvs };
                            let (viewheight, attachments) = (self.client.sim().viewheight, self.studio.view_model_attachments());
                            let mut host = EventHost::new(&self.client, &mut self.effects, env, viewheight, attachments);
                            host.solid_players = self.console.cvars.value("cl_solid_players") != 0.0;
                            if name == "Brass" {
                                self.client_events.brass_message(&mut host, &data);
                            } else {
                                self.client_events.corpse_message(&mut host, &data, &self.studio.min_models);
                            }
                            let sounds = host.out.sounds;
                            self.play_requests(&sounds);
                        }
                        // CHudHealth::MsgFunc_ReceiveW.
                        "ReceiveW" => self.client_events.environment.receive_w(&data),
                        // CHud::MsgFunc_InitHUD → CEnvironment::Initialize.
                        "InitHUD" => {
                            let styles = [gs_render::camera::NORMAL_STYLE; 64];
                            let host = ParticleHost { env: ClientEnv { registry: &self.registry, client: &self.client, pvs: &self.pvs }, styles: &styles };
                            let cl_weather = self.console.cvars.value("cl_weather");
                            self.client_events.environment.initialize(&mut self.effects, &host, self.client.time() as f32, cl_weather);
                        }
                        _ if handled => {}
                        _ => log::trace!("unhandled user message {name}"),
                    }
                }
                ClientEvent::Message(ServerMessage::TempEntity(te)) => {
                    log::debug!("{te:?}");
                    let env = ClientEnv { registry: &self.registry, client: &self.client, pvs: &self.pvs };
                    self.effects.temp_entity(&env, &te);
                }
                ClientEvent::Message(ServerMessage::Particle { origin, dir, count, color }) => {
                    self.effects.run_particle_effect(origin, dir, i32::from(color), i32::from(count));
                }
                // svc_finale / svc_cutscene: a center print (cl.intermission is the client's).
                ClientEvent::Message(ServerMessage::Finale(text) | ServerMessage::Cutscene(text)) => {
                    self.center.print(&text, self.console.cvars.value("scr_centertime"), self.client.time());
                }
                ClientEvent::Message(ServerMessage::CenterPrint(text)) => {
                    self.center.print(&text, self.console.cvars.value("scr_centertime"), self.client.time());
                }
                // Parse_svc_sendextrainfo: sv_cheats mirrored, the fallback directory mounted.
                ClientEvent::Message(ServerMessage::SendExtraInfo { fallback_dir, cheats }) => {
                    self.console.cvars.direct_set("sv_cheats", if cheats { "1" } else { "0" });
                    if !fallback_dir.is_empty() && !self.fs.add_fallback_game_dir(&fallback_dir) {
                        log::warn!("svc_sendextrainfo: refused fallback directory {fallback_dir:?}");
                    }
                }
                ClientEvent::Message(ServerMessage::Sound(s)) => {
                    if let Some(audio) = &mut self.audio {
                        audio.precache(&self.client.level().sounds);
                        audio.server_sound(&s);
                    }
                }
                ClientEvent::Message(ServerMessage::SpawnStaticSound(s)) => {
                    if let Some(audio) = &mut self.audio {
                        audio.precache(&self.client.level().sounds);
                        audio.static_sound(&s);
                    }
                }
                ClientEvent::Message(ServerMessage::StopSound { entity, channel }) => {
                    if let Some(audio) = &mut self.audio {
                        audio.stop(i32::from(entity), i32::from(channel));
                    }
                }
                // Parse_svc_cdtrack: the second byte is never read; always looped. A demo's
                // forced track wins while recording or playing.
                ClientEvent::Message(ServerMessage::CdTrack { track, .. }) => {
                    let demo = &self.client.demo;
                    let forced = (demo.playing() || demo.recording()) && demo.forcetrack != -1;
                    let track = if forced { demo.forcetrack } else { i32::from(track) };
                    if let Some(audio) = &mut self.audio {
                        audio.music_play(track, true);
                    }
                }
                // Parse_svc_setpause (not a career match): the music follows the pause.
                ClientEvent::Message(ServerMessage::SetPause(paused)) => {
                    self.paused = paused;
                    if let Some(audio) = &mut self.audio {
                        audio.music_pause(paused);
                    }
                }
                // Parse_svc_voiceinit: the codec name is read and discarded (Voice_Init("", q)).
                ClientEvent::Message(ServerMessage::VoiceInit { .. }) => {
                    if let Some(audio) = &mut self.audio {
                        audio.voice_init(&self.console);
                    }
                }
                // Parse_svc_voicedata: sender + 1 must be below cl.num_entities.
                ClientEvent::Message(ServerMessage::VoiceData { sender, data }) => {
                    let ent = i32::from(sender) + 1;
                    if let Some(audio) = &mut self.audio
                        && (ent as usize) < self.client.entities().num_entities()
                    {
                        let local = ent == i32::from(self.client.protocol().playernum) + 1;
                        audio.voice_data(&self.console, ent, local, &data);
                    }
                }
                // Parse_svc_roomtype: Cvar_SetValue("room_type").
                ClientEvent::Message(ServerMessage::RoomType(v)) => {
                    self.console.cvars.direct_set("room_type", &v.to_string());
                }
                ClientEvent::Message(msg) => log::trace!("{msg:?}"),
            }
        }
        let (set, add_yaw) = self.client.take_view_angles();
        if let Some(a) = set {
            self.angles = Vec3::from(a);
        }
        self.angles.y += add_yaw;
    }

    /// A demo frame the engine hands on (`CL_GetDemoMessage`'s types 3, 4, 8, 9 and the
    /// segment actions).
    fn demo_item(&mut self, item: DemoItem) {
        match item {
            // CL_FilteredCmdAllowed (no single-player bypass in a playback), then the filtered
            // buffer.
            DemoItem::Command(text) => {
                let policy = StuffPolicy { filter: self.console.cvars.value("cl_filterstuffcmd") > 0.0, ..StuffPolicy::default() };
                if gs_console::stufftext::valid_stuff_text(&text, policy) {
                    self.console.add_filtered_text(&text);
                    self.console.add_filtered_text("\n");
                }
            }
            // ClientDLL_DemoUpdateClientData: HUD_UpdateClientData on the recorded data; its
            // angles become cl.viewangles (not HLTV) and its FOV scr_fov_value.
            DemoItem::ClientData(cd) => {
                let fov = self.hud_update_client_data(cd.origin, cd.viewangles, cd.weapons);
                if !self.client.spectator() {
                    self.angles = Vec3::from(cd.viewangles);
                }
                self.scr_fov = fov as f32;
            }
            // CL_DemoReadSound: on the local player at cl.simorg, with the file's volume and
            // attenuation swapped.
            DemoItem::Sound { channel, sample, volume, attenuation, flags, pitch } => {
                let ent = i32::from(self.client.protocol().playernum) + 1;
                let origin = self.replay.map_or(self.client.sim().origin, |(rp, _)| rp.simorg);
                let request = SoundRequest::Play { ent, origin, channel, sample, volume: attenuation, attenuation: volume, flags, pitch };
                self.play_requests(&[request]);
            }
            // Demo_ReadBuffer: type 1 is the zoom HUD_GetFOV replays; type 0 (the HL sniper
            // record) has no reader in CS.
            DemoItem::Buffer(buf) => {
                let word = |o: usize| buf.get(o..o + 4).map_or([0; 4], |b| [b[0], b[1], b[2], b[3]]);
                match i32::from_le_bytes(word(0)) {
                    0 => {}
                    1 => self.demo_zoom = f32::from_le_bytes(word(4)),
                    _ => {
                        if self.console.cvars.value("developer") != 0.0 {
                            notify::con_printf("Unknown demo buffer type, skipping.\n");
                        }
                    }
                }
            }
            DemoItem::EngineText(text) => self.console.add_text(&text),
            // SetDemoMessage + CL_HudMessage("__DEMOMESSAGE__"): HudText with the engine's
            // own titles entry.
            DemoItem::Title(text) => {
                self.titles.set(gs_draw::titles::TextMessage {
                    effect: 0,
                    color1: [255; 4],
                    color2: [255; 4],
                    x: -1.0,
                    y: -1.0,
                    fadein: 0.01,
                    fadeout: 1.0,
                    holdtime: 2.0,
                    fxtime: 0.0,
                    name: "__DEMOMESSAGE__".into(),
                    message: text,
                });
                let (hud, mut eng) = self.hud_host();
                hud.msg("HudText", b"__DEMOMESSAGE__", &mut eng);
            }
            DemoItem::Event { .. } | DemoItem::WeaponAnim { .. } | DemoItem::Fade(_) => {}
        }
    }
}
