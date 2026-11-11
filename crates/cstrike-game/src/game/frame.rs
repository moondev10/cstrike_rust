//! The host frame: the rate limiter, `_Host_Frame`'s tick, the screen update with its swap
//! and `gl_lowlatency` throttle, the profiling cvars, and screenshots.

use super::*;

impl Running {
    /// `Host_FilterTime`'s rate: `fps_max`, 31 while connecting, at least 0.5, at most 100
    /// unless `fps_override`, at least 20 on a multiplayer server, 100 with `gl_vsync` unless
    /// `fps_override`.
    fn host_fps(&self) -> f64 {
        let cv = &self.console.cvars;
        let mut fps = f64::from(cv.value("fps_max"));
        if !matches!(self.client.state(), gs_client::State::Disconnected | gs_client::State::Active) {
            fps = 31.0;
        }
        fps = fps.max(0.5);
        let no_override = cv.value("fps_override") == 0.0;
        if no_override {
            fps = fps.min(100.0);
        }
        if self.client.protocol().maxclients > 1 {
            fps = fps.max(20.0);
        }
        if cv.value("gl_vsync") != 0.0 && no_override {
            fps = 100.0;
        }
        fps
    }

    /// `CEngine::Frame` → `Host_FilterTime`: `realtime` advances by the loop's elapsed time
    /// (scaled) on every iteration; `true` when a host frame runs, with `frametime` set.
    pub(super) fn filter_time(&mut self) -> bool {
        let now = Instant::now();
        let dt = (now - self.last_loop).as_secs_f64();
        self.last_loop = now;
        let timescale = f64::from(self.client.sys_timescale());
        // host_framerate fixes the step in single player and in demo playback: no limiter.
        let framerate = f64::from(self.console.cvars.value("host_framerate"));
        if framerate > 0.0 && (self.client.protocol().maxclients == 1 || self.client.demo_playing()) {
            self.realtime += timescale * framerate;
            self.frametime = timescale * framerate;
            return true;
        }
        self.realtime += dt * timescale;
        if !self.client.demo.timedemo() && self.realtime - self.oldrealtime < timescale / (self.host_fps() + 0.5) {
            return false;
        }
        self.frametime = (self.realtime - self.oldrealtime).min(0.25);
        self.oldrealtime = self.realtime;
        true
    }

    /// When the next loop iteration is due: an inactive app waits up to 20 ms (input wakes it
    /// sooner); an active one spins, here until the limiter would let the next frame through.
    pub(super) fn next_loop(&self) -> Instant {
        if !self.active_app {
            return self.last_loop + Duration::from_millis(20);
        }
        let timescale = f64::from(self.client.sys_timescale());
        let framerate = self.console.cvars.value("host_framerate");
        if self.client.demo.timedemo() || framerate > 0.0 && (self.client.protocol().maxclients == 1 || self.client.demo_playing()) || timescale <= 0.0 {
            return self.last_loop;
        }
        let due = timescale / (self.host_fps() + 0.5) - (self.realtime - self.oldrealtime);
        self.last_loop + Duration::from_secs_f64((due / timescale).max(0.0))
    }

    /// `CL_CreateMove` for this frame into the client's next command.
    fn create_move(&mut self) {
        let frame = self.client.protocol().current_frame();
        let cd = &frame.clientdata;
        let playernum = usize::from(self.client.protocol().playernum);
        let noclip = self.client.entities().get(playernum + 1).is_some_and(|e| e.curstate.movetype() == MOVETYPE_NOCLIP);
        let cvars = &self.console.cvars;
        let default_fov = cvars.value("default_fov") as i32;
        let params = MoveParams {
            cvars,
            frametime: self.frametime as f32,
            active: self.client.state() == State::Active,
            maxspeed: cd.maxspeed(),
            health: self.hud.health(),
            intermission: self.client.level().intermission != 0,
            hud_sensitivity: hud_mouse_sensitivity(cvars, self.last_fov as i32),
            hud_fov: if self.hud.fov != 0 { self.hud.fov } else { default_fov },
            noclip,
            time: self.client.time(),
            cursor_visible: self.vgui_cursor_visible(),
        };
        let mut va = self.angles.to_array();
        let cmd = self.input.create_move(&params, &mut va);
        self.angles = Vec3::from(va);
        self.client.viewangles = va;
        self.client.next_cmd = cmd;
    }

    fn tick(&mut self, event_loop: &ActiveEventLoop) {
        // host_frametime from Host_FilterTime (filter_time ran first).
        let dt = self.frametime;
        self.engine.frametime = dt;
        self.engine.realtime = self.realtime;
        // SCR_CalcRefdef: viewsize's value is forced back to 120 (the string stays).
        self.console.cvars.set_value_only("viewsize", 120.0);
        let t = self.realtime;
        self.in_game_time = self.in_game_since.map(|since| t - since);
        while let (Some(&(at, pitch, yaw)), Some(now)) = (self.looks.first(), self.in_game_time)
            && now >= at
        {
            self.angles = Vec3::new(pitch, yaw, 0.0);
            self.looks.remove(0);
        }
        // Console input: the terminal, then scripted commands and +command holds.
        while let Ok(line) = self.terminal.try_recv() {
            self.console.add_text(&format!("{line}\n"));
        }
        if let Some(since) = self.in_game_since {
            if t >= self.next_command_at && !self.commands.is_empty() {
                let cmd = self.commands.remove(0);
                log::info!("console: {cmd}");
                self.console.add_text(&format!("{cmd}\n"));
                self.next_command_at = t + 1.0;
            }
            let t = t - since;
            for (command, from, to, phase) in &mut self.holds {
                if *phase == 0 && t >= *from {
                    self.console.add_text(&format!("+{command}\n"));
                    *phase = 1;
                }
                if *phase == 1 && t >= *to {
                    self.console.add_text(&format!("-{command}\n"));
                    *phase = 2;
                }
            }
            while let Some(&(at, key)) = self.keys.first()
                && t >= at
            {
                self.keys.remove(0);
                log::info!("key: {}", gs_console::keys::key_name(key));
                // As `window_event`: the engine's Key_Event, then vgui2's KEYDOWN/KEYUP.
                let vcode = gameui_host::engine_key_to_vgui(i32::from(key));
                for down in [true, false] {
                    self.key_input(key, down, false);
                    self.vgui_input.key(&mut self.gameui.ui, vcode, KeyCode::Fn, down);
                }
            }
            while let Some(&(at, ch)) = self.typed.first()
                && t >= at
            {
                self.typed.remove(0);
                log::info!("type: {ch:?}");
                self.text_input(ch.encode_utf8(&mut [0; 4]));
            }
            while let Some(&(at, x, y, button)) = self.clicks.first()
                && t >= at
            {
                self.clicks.remove(0);
                log::info!("mouse: {button:?} at {x},{y}");
                self.scripted_mouse(x, y, button);
            }
        }
        // _Host_Frame: SystemWrapper_RunFrame, CL_CheckClientState, Cbuf_Execute, HUD think,
        // CL_Move, then packets.
        self.run_demo_modules(dt);
        self.client.check_client_state();
        self.run_console();
        self.drain_module_log();
        self.hud_think();
        self.sync_menu_cursor();
        // ClientDLL_CreateMove runs only inside CL_Move from ca_uninitialized on (impulse and
        // weapon select typed before then wait for it).
        if self.client.state() >= State::Uninitialized {
            self.create_move();
        }
        // The client DLL's globals HUD_PostRunCmd reads during this frame's prediction.
        let cd_user1 = self.hud.state.user1;
        self.weapons.inputs = WeaponInputs {
            v_angles: self.last_view_angles,
            viewheight: self.client.sim().viewheight,
            health: self.hud.health(),
            observer_mode: cd_user1,
            last_fov: self.last_fov as i32,
        };
        // Con_Printf records the notify line with developer on in any client state but 0.
        let record = self.console.cvars.value("developer") != 0.0 && self.client.state() != State::Disconnected;
        notify::set_frame(t, record);
        gs_common::conprint::set_developer(self.console.cvars.value("developer") != 0.0);
        let loopback = self.console.cvars.value("voice_loopback") != 0.0;
        // HUD_TxferPredictionData's source for an HLTV client.
        self.client.hud_users = [self.hud.state.user1, self.hud.state.user2, self.hud.state.user3];
        self.client.filter_stuff_cmd = self.console.cvars.value("cl_filterstuffcmd") > 0.0;
        self.client.send(t, dt, &mut self.weapons, &mut VoiceSendHost { audio: self.audio.as_mut(), loopback });
        // ClientDLL_Frame (HUD_Frame → CVoiceStatus::Frame) after CL_Move: what it queues leaves
        // with the next frame's command.
        {
            let (hud, mut eng) = self.hud_host();
            hud.frame(&mut eng);
        }
        let console = &self.console;
        self.client.receive(&mut self.weapons, &|name| console::cvar_reply(console, name));
        self.drain_module_log();
        if self.client.take_connection_problem() {
            self.nprint.nxprintf(0, 1.0, [1.0, 0.2, 0.0], "WARNING:  Connection Problem", t);
        }
        self.client_events.holding_shield = self.weapons.globals.holding_shield;
        self.client_events.gun_smoke = self.hud.render_gun_smoke;
        // cl.time / cl.oldtime for this frame's effects (CL_DecayLights runs after the screen
        // update); the effect cvars and r_decals as they stand now.
        self.effects.set_time(self.client.time(), self.client.old_time());
        {
            let cv = &self.console.cvars;
            let c = &mut self.effects.cvars;
            c.tracerspeed = cv.value("tracerspeed");
            c.traceroffset = cv.value("traceroffset");
            c.tracerlength = cv.value("tracerlength");
            c.tracerred = cv.value("tracerred");
            c.tracergreen = cv.value("tracergreen");
            c.tracerblue = cv.value("tracerblue");
            c.traceralpha = cv.value("traceralpha");
            c.violence_hblood = cv.value("violence_hblood") != 0.0;
            c.violence_ablood = cv.value("violence_ablood") != 0.0;
            c.r_decals = cv.value("r_decals") != 0.0;
            self.renderer.set_decal_limit((cv.value("r_decals").max(0.0) as usize).min(4096));
        }
        self.handle_events();
        // HUD_ProcessPlayerState for the local player: g_iUser1..3 from its state, whose iuser
        // fields HUD_TxferLocalOverrides took from the clientdata.
        if self.client.take_local_player_update() {
            let cd = &self.client.protocol().current_frame().clientdata;
            let users = [cd.iuser1(), cd.iuser2(), cd.iuser3()];
            self.hud.process_player_state(users);
        }
        // MsgFunc_ViewMode → CAM_ToFirstPerson.
        if std::mem::take(&mut self.hud.view_mode) {
            let effect = self.input.cam.enter_first_person();
            console::apply_cam_effect(&mut self.console, effect);
        }
        // ClientDLL_CAM_Think from cls.state 2 on; the ideal angles and distance are value-only
        // writes.
        if self.client.state() >= gs_client::State::Connecting {
            let maxclients = i32::from(self.client.protocol().maxclients);
            let center = self.window_center();
            let (effect, ideal) = self.input.cam.think(&self.console.cvars, maxclients, self.angles.to_array(), center);
            console::apply_cam_effect(&mut self.console, effect);
            if let Some([pitch, yaw, dist]) = ideal {
                self.console.cvars.set_value_only("cam_idealpitch", pitch);
                self.console.cvars.set_value_only("cam_idealyaw", yaw);
                self.console.cvars.set_value_only("cam_idealdist", dist);
            }
        }
        // CL_MoveSpectatorCamera (after CAM_Think): the HLTV camera pmove.
        let mut jump = self.hud.take_spectator_jump();
        self.client.move_spectator_camera(&mut jump, &mut self.weapons);
        self.hud.restore_spectator_jump(jump);
        self.apply_observer_requests();
        // Voice_Idle after this frame's packets, then the HUD_VoiceStatus calls it queued.
        if let Some(audio) = &mut self.audio {
            audio.voice_idle(&self.console, dt as f32);
        }
        self.voice_output();
        while let Some((at, _)) = self.launch_shots.first()
            && t >= *at
        {
            let (_, path) = self.launch_shots.remove(0);
            self.pending_shots.push(path);
        }
        if let Some(since) = self.in_game_since {
            while let Some((at, _)) = self.shots.first()
                && t - since >= *at
            {
                let (_, path) = self.shots.remove(0);
                self.pending_shots.push(path);
            }
            if self.quit_after.is_some_and(|q| t - since >= q) {
                self.quit = true;
            }
        }
        if self.quit {
            self.shutdown();
            event_loop.exit();
        }
    }

    /// `Host_Shutdown`: `Host_WriteConfiguration` (`config.cfg`; the windowless test harness
    /// does not persist its test settings), `Voice_Deinit` (recording stopped, capture files
    /// written), disconnect, the client DLL's static destructors (`CVoiceStatus` saves the voice
    /// bans), then `VGui_Shutdown` (GameUI's platform modules and user config; not persisted by
    /// the test harness either).
    pub(super) fn shutdown(&mut self) {
        // Sys_ShutdownGame: IN_DeactivateMouse before Host_Shutdown.
        self.input.mouse_active = false;
        self.mouse_parms.deactivate();
        if !self.hidden {
            // Host_WriteConfiguration's registry side: `rate`, and the name unless a placeholder.
            crate::registry::set_steam_string("Rate", self.console.cvars.string("rate"));
            let name = self.console.cvars.string("name");
            if !name.is_empty() && !name.eq_ignore_ascii_case("unnamed") && !name.eq_ignore_ascii_case("player") {
                crate::registry::set_steam_string("LastGameNameUsed", name);
            }
            match self.console.config_text(&self.client.config.userinfo) {
                Some(text) => {
                    if let Err(e) = self.fs.write("config.cfg", PathId::GameConfig, &gs_common::bytestr::encode(&text)) {
                        log::warn!("Couldn't write config.cfg: {e}");
                    }
                }
                None => notify::con_printf("skipping config.cfg output, no keys bound\n"),
            }
        }
        if let Some(audio) = &mut self.audio {
            audio.voice_shutdown();
            audio.music_shutdown();
        }
        self.voice_output();
        self.client.disconnect("quit");
        self.client.demo_shutdown();
        let (hud, mut eng) = self.hud_host();
        hud.shutdown(&mut eng);
        if !self.hidden {
            let mut loader = hud_host::Loader { renderer: &mut self.renderer, fs: &self.fs };
            let mut s = gameui_host::VguiSurface::new(&mut self.vgui, &mut self.screen, &mut loader);
            self.gameui.shutdown(&mut s);
        }
    }

    /// The voice engine's queued `HUD_VoiceStatus` calls (in order) and console prints.
    fn voice_output(&mut self) {
        let Some(audio) = &mut self.audio else { return };
        let (events, prints) = audio.take_voice_output();
        prints.iter().for_each(|p| notify::con_printf(p));
        if events.is_empty() {
            return;
        }
        let (hud, mut eng) = self.hud_host();
        for (ent, talking) in events {
            hud.voice_status(ent, talking, &mut eng);
        }
    }

    /// The change hooks of `r_prefertexturefiltering` (0 → `gl_texturemode GL_NEAREST`, else
    /// `GL_LINEAR_MIPMAP_LINEAR`) and `gl_texturemode` (`Draw_TextureMode_f`: the filters and
    /// `gl_ansio` onto every mipmapped texture), run when either value changed
    /// (render.md §A.13.3).
    fn texture_mode_hooks(&mut self) {
        let prefer = self.console.cvars.value("r_prefertexturefiltering");
        if prefer != self.applied_prefer_filtering {
            self.applied_prefer_filtering = prefer;
            let mode = if prefer == 0.0 { "GL_NEAREST" } else { "GL_LINEAR_MIPMAP_LINEAR" };
            self.console.cvars.direct_set("gl_texturemode", mode);
        }
        let name = self.console.cvars.string("gl_texturemode");
        if name == self.applied_texture_mode {
            return;
        }
        self.applied_texture_mode = name.to_owned();
        let Some(mode) = TextureMode::from_name(name) else {
            notify::con_printf("bad filter name\n");
            return;
        };
        let nearest = name.eq_ignore_ascii_case("GL_NEAREST");
        self.renderer.set_texture_mode(mode, self.console.cvars.value("gl_ansio"));
        // GL_NEAREST turns r_prefertexturefiltering off; its hook sets GL_NEAREST once more.
        if nearest && self.console.cvars.value("r_prefertexturefiltering") == 1.0 {
            self.console.cvars.direct_set("r_prefertexturefiltering", "0");
            self.applied_prefer_filtering = 0.0;
        }
    }

    /// One host frame: `_Host_Frame`'s work, the screen update, then `CL_DecayLights` and the
    /// frame's end (`startmovie`'s snapshot, `CL_GGSpeeds`, `host_framecount++`,
    /// `CL_AdjustClock`).
    pub(super) fn host_frame(&mut self, event_loop: &ActiveEventLoop) {
        let t1 = Instant::now();
        // No local server: SV_Frame's two stamps are back to back.
        let t2 = Instant::now();
        self.tick(event_loop);
        let t3 = Instant::now();
        self.texture_mode_hooks();
        self.draw();
        let t4 = Instant::now();
        self.effects.decay_lights();
        let end = Instant::now();
        self.host_speeds([self.speeds_prev_end, t1, t2, t3, t4, end]);
        self.speeds_prev_end = end;
        self.client.end_frame(self.realtime as f32, false);
    }

    /// `host_speeds`: the frame's stamps as milliseconds, from the previous frame's end
    /// (engine-core.md §4); `ents` counts a local server's edicts, so it is 0 here.
    fn host_speeds(&self, [prev_end, t1, t2, t3, t4, end]: [Instant; 6]) {
        if self.console.cvars.value("host_speeds") == 0.0 {
            return;
        }
        let ms = |a: Instant, b: Instant| (b.saturating_duration_since(a).as_secs_f64() * 1000.0) as f32;
        let (host, sv, cl, gfx, snd) = (ms(prev_end, t1), ms(t1, t2), ms(t2, t3), ms(t3, t4), ms(t4, end));
        let total = f64::from(host + sv + cl + gfx + snd) / 1000.0;
        let fps = if total >= 0.0001 { 1.0 / total } else { 999.0 };
        // `%3.0f` of the MSVC CRT rounds halves away from zero.
        let f = |v: f32| format!("{:3.0}", f64::from(v).round());
        notify::con_printf(&format!(
            "{:3} fps -- host({}) sv({}) cl({}) gfx({}) snd({}) ents(0)\n",
            fps as i32,
            f(host),
            f(sv),
            f(cl),
            f(gfx),
            f(snd)
        ));
    }

    /// `Host_Frame`'s `host_profile`: every call (also one the limiter skips) is timed, and
    /// each 1000 print their average; a remote client has no server clients to count.
    pub(super) fn host_profile(&mut self, elapsed: Duration) {
        if self.console.cvars.value("host_profile") == 0.0 {
            return;
        }
        self.profile.0 += 1;
        self.profile.1 += elapsed.as_secs_f64();
        if self.profile.0 >= 1000 {
            let avg = (self.profile.1 * 1000.0 / f64::from(self.profile.0)) as i32;
            self.profile = (0, 0.0);
            notify::con_printf(&format!("host_profile: {:2} clients {avg:2} msec\n", 0));
        }
    }

    /// `GetWindowCenterX/Y`: the window's client area centre in screen coordinates.
    fn window_center(&self) -> (i32, i32) {
        let pos = self.window.inner_position().unwrap_or_default();
        let size = self.window.inner_size();
        (pos.x + size.width as i32 / 2, pos.y + size.height as i32 / 2)
    }

    fn draw(&mut self) {
        let size = self.vid;
        if let Some(offscreen) = &self.offscreen {
            // Windowless: the same frame into an invisible target, nothing presented.
            let target = offscreen.create_view(&Default::default());
            let frame_scene = self.build_scene();
            self.renderer.render(&target, size, &frame_scene.view, &frame_scene.scene, frame_scene.inset());
            self.take_screenshots(&frame_scene);
            return;
        }
        // SCR_UpdateScreen is skipped while the window is minimised (the host frame still runs).
        if self.window.is_minimized() == Some(true) {
            return;
        }
        // GL_BeginRendering: a changed (int)gl_vsync → SDL_GL_SetSwapInterval(gl_vsync > 0).
        let vsync = self.console.cvars.value("gl_vsync");
        if vsync as i32 != self.swap_interval {
            self.swap_interval = vsync as i32;
            self.config.present_mode = present_mode_for(&self.surface, &self.gpu.adapter, i32::from(vsync > 0.0));
            self.surface.configure(&self.gpu.device, &self.config);
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.gpu.device, &self.config);
                return;
            }
            _ => return,
        };
        let target = frame.texture.create_view(&Default::default());
        let frame_scene = self.build_scene();
        match &self.fbo {
            // GL_EndRendering: the mode-sized FBO into the letterbox rectangle of the window.
            Some((fbo, present)) => {
                let fbo_view = fbo.create_view(&Default::default());
                self.renderer.render(&fbo_view, size, &frame_scene.view, &frame_scene.scene, frame_scene.inset());
                let window = (self.config.width as i32, self.config.height as i32);
                let (rect, scale) = gs_render::present::letterbox((size.0 as i32, size.1 as i32), window, self.keep_aspect);
                self.letterbox_scale = scale;
                present.draw(&self.gpu.device, &self.gpu.queue, &fbo_view, &target, rect);
                self.place_motd_page(Some(rect.into()));
            }
            None => {
                self.renderer.render(&target, size, &frame_scene.view, &frame_scene.scene, frame_scene.inset());
                self.place_motd_page(None);
            }
        }
        // GL_EndRendering: the swap, timed for gl_lowlatency.
        let t0 = self.start.elapsed().as_secs_f64() * 1000.0;
        let frame_ms = if t0 > self.last_swap_end_ms { t0 - self.last_swap_end_ms } else { 0.0 };
        self.window.pre_present_notify();
        self.gpu.queue.present(frame);
        let t1 = self.start.elapsed().as_secs_f64() * 1000.0;
        if self.console.cvars.value("gl_lowlatency") > 0.0 {
            self.low_latency_throttle(t0, t1, frame_ms);
        }
        self.last_swap_end_ms = t1;
        self.take_screenshots(&frame_scene);
    }

    /// The MOTD page over the `Message` control where this frame shows it (through the
    /// letterbox rectangle `letterbox` in fullscreen), hidden while GameUI or the console is up:
    /// the original draws them over it.
    fn place_motd_page(&mut self, letterbox: Option<(i32, i32, i32, i32)>) {
        if !self.engine.motd.shows_pages() {
            return;
        }
        let covered = self.gameui.is_game_ui_active() || self.gameui.is_console_visible();
        let (vw, vh) = (self.vid.0.max(1) as i32, self.vid.1.max(1) as i32);
        let rect = self.hud.viewport.motd_rect().filter(|_| !covered).map(|(x, y, w, h)| match letterbox {
            Some((lx, ly, lw, lh)) => (lx + x * lw / vw, ly + y * lh / vh, w * lw / vw, h * lh / vh),
            None => (x, y, w, h),
        });
        self.engine.motd.place(rect);
        if rect.is_none() && self.page_focus {
            self.engine.motd.focus_game();
        }
    }

    /// `gl_lowlatency` after the swap (engine-video.md §5.3.8): wait for the GPU, then grow the
    /// post-swap sleep by 0.1 ms while more than `maxslop` of the vblank interval goes unused
    /// and cut it by 1 ms when less than `minslop` is left, and sleep that long.
    fn low_latency_throttle(&mut self, t0: f64, t1: f64, frame_ms: f64) {
        // glFinish.
        if let Err(e) = self.gpu.device.poll(wgpu::PollType::wait_indefinitely()) {
            log::debug!("gl_lowlatency wait: {e}");
        }
        // SDL_GetCurrentDisplayMode(0): the primary display's whole-hertz rate, else 60.
        let hz = self.window.primary_monitor().and_then(|m| m.refresh_rate_millihertz()).map_or(0, |mhz| mhz / 1000);
        let vblank = 1.0 / f64::from(if hz > 1 { hz } else { 60 }) * 1000.0;
        let cv = &self.console.cvars;
        let (minslop, maxslop) = (f64::from(cv.value("gl_lowlatency_minslop_ms")), f64::from(cv.value("gl_lowlatency_maxslop_ms")));
        let mut slop = (vblank - frame_ms).max(0.0);
        if t1 - t0 <= slop {
            slop = (t1 - t0).max(0.0);
        }
        if slop > maxslop {
            self.throttle_ms += 0.1;
        } else if slop < minslop {
            self.throttle_ms -= 1.0;
        }
        let cap = (vblank - minslop).max(0.0);
        self.throttle_ms = if self.throttle_ms > cap { cap } else { self.throttle_ms.max(0.0) };
        if cv.value("gl_lowlatency_debugoutput") > 0.0 {
            notify::con_printf(&format!(
                "gl_lowlatency: VBlank Interval: {vblank:.2} Frame Time: {frame_ms:.2}, Slop: {slop:.2} Throttle:{:.2}\n",
                self.throttle_ms
            ));
        }
        std::thread::sleep(Duration::from_millis(self.throttle_ms as u64));
    }

    fn take_screenshots(&mut self, frame_scene: &FrameScene) {
        for path in std::mem::take(&mut self.pending_shots) {
            match self.screenshot(frame_scene, &path) {
                Ok(()) => println!("screenshot {}", path.display()),
                Err(e) => log::error!("screenshot: {e:#}"),
            }
        }
        for op in std::mem::take(&mut self.engine.captures) {
            // The original's write failures are Sys_Errors; here they are printed.
            if let Err(e) = self.engine_capture(op, frame_scene) {
                notify::con_printf(&format!("{e:#}\n"));
            }
        }
        // startmovie: VID_TakeSnapshot("%s%05d.bmp") after each screen update.
        let movie_name = self.movie.as_mut().map(|movie| {
            movie.counter += 1;
            format!("{}{:05}.bmp", movie.name, movie.counter - 1)
        });
        if let Some(name) = movie_name {
            let res = self
                .fs
                .write_path(&name, PathId::GameConfig)
                .map_err(anyhow::Error::from)
                .and_then(|path| Ok((self.capture_window(frame_scene)?, path)))
                .and_then(|(image, path)| save_bmp(&image, &path));
            if let Err(e) = res {
                log::warn!("startmovie {name}: {e:#}");
            }
        }
    }

    /// The picture `VID_TakeSnapshot` / `SCR_ScreenShot_f` read back: the presented window
    /// (the letterboxed FBO in fullscreen), else the frame at `vid`.
    fn capture_window(&mut self, frame: &FrameScene) -> Result<gs_assets::RgbaImage> {
        Ok(match &self.fbo {
            Some((_, present)) => {
                let window = (self.config.width, self.config.height);
                let (rect, _) = gs_render::present::letterbox((self.vid.0 as i32, self.vid.1 as i32), (window.0 as i32, window.1 as i32), self.keep_aspect);
                capture::capture_presented(&mut self.renderer, present, self.vid, window, rect, &frame.view, &frame.scene, frame.inset())?
            }
            None => capture::capture(&mut self.renderer, self.vid, &frame.view, &frame.scene, frame.inset())?,
        })
    }

    /// A capture an engine command queued (`snapshot`, `screenshot`, `envmap`, `timerefresh`),
    /// from the frame just drawn.
    fn engine_capture(&mut self, op: crate::engine_cmds::CaptureOp, frame: &FrameScene) -> Result<()> {
        use crate::engine_cmds::CaptureOp;
        let size = self.vid;
        match op {
            CaptureOp::Snapshot(name) => {
                let path = self.fs.write_path(&name, PathId::GameConfig)?;
                let image = self.capture_window(frame)?;
                save_bmp(&image, &path)
            }
            CaptureOp::Screenshot(name) => {
                let path = self.fs.write_path(&name, PathId::GameConfig)?;
                let image = self.capture_window(frame)?;
                crate::shot::save_tga(&image, &path)?;
                notify::con_printf(&format!("Wrote {name}\n"));
                Ok(())
            }
            // Six S×S views (S = gl_envmapsize, 256) from the current origin without the view
            // model; the angles are left at the last view's (yaw 0, pitch 90).
            CaptureOp::Envmap(base) => {
                const S: u32 = 256;
                let mut scene = frame.scene.clone();
                scene.studios.retain(|s| !s.view_model);
                for (pitch, yaw, suffix) in [(0.0, 0.0, "rt"), (0.0, 90.0, "bk"), (0.0, 180.0, "lf"), (0.0, 270.0, "ft"), (-90.0, 0.0, "up"), (90.0, 0.0, "dn")] {
                    let mut view = frame.view.clone();
                    view.angles = Vec3::new(pitch, yaw, 0.0);
                    let name = format!("{base}{suffix}.bmp");
                    let path = self.fs.write_path(&name, PathId::GameConfig)?;
                    let image = capture::capture(&mut self.renderer, (S, S), &view, &scene, None)?;
                    save_bmp(&image, &path)?;
                }
                Ok(())
            }
            // 128 views around the yaw between two GPU waits; the times are floats.
            CaptureOp::TimeRefresh => {
                let views: Vec<_> = (0..128)
                    .map(|i| {
                        let mut view = frame.view.clone();
                        view.angles.y = (f64::from(i) * 0.0078125 * 360.0) as f32;
                        view
                    })
                    .collect();
                capture::render_views(&mut self.renderer, size, &[], &frame.scene)?;
                let t0 = self.start.elapsed().as_secs_f64() as f32;
                capture::render_views(&mut self.renderer, size, &views, &frame.scene)?;
                let dt = self.start.elapsed().as_secs_f64() as f32 - t0;
                notify::con_printf(&format!("{:.6} seconds ({:.6} fps)\n", f64::from(dt), f64::from(128.0f32 / dt)));
                Ok(())
            }
        }
    }

    fn screenshot(&mut self, frame: &FrameScene, path: &std::path::Path) -> Result<()> {
        let size = self.vid;
        let image = capture::capture(&mut self.renderer, size, &frame.view, &frame.scene, frame.inset())?;
        let (o, a) = (frame.view.origin, frame.view.angles);
        log::info!(
            "{}: pos {:.1} {:.1} {:.1} ang {:.1} {:.1} {:.1}, {} models",
            path.display(),
            o.x,
            o.y,
            o.z,
            a.x,
            a.y,
            a.z,
            frame.scene.studios.len()
        );
        save_png(&image, path)
    }
}
