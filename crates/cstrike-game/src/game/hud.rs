//! The engine's hosts for the console and the client HUD, the 2D frame (`HUD_Redraw`, the
//! VGUI2 paint with GameUI), the demo player modules and the console buffer.

use super::*;

impl Running {
    /// The console with its host borrowed from the other fields.
    pub(super) fn console_host(&mut self) -> (&mut Console, Host<'_>) {
        (
            &mut self.console,
            Host {
                client: &mut self.client,
                fs: &self.fs,
                input: &mut self.input,
                hud: &mut self.hud,
                screen: &mut self.screen,
                renderer: &mut self.renderer,
                titles: &self.titles,
                viewangles: &mut self.angles,
                quit: &mut self.quit,
                engine: &mut self.engine,
                effects: Some(&mut self.effects),
                audio: self.audio.as_mut(),
                chat: &mut self.chat,
                movie: &mut self.movie,
                ui_ops: &mut self.ui_ops,
            },
        )
    }

    /// The HUD with its engine borrowed from the other fields.
    pub(super) fn hud_host(&mut self) -> (&mut Hud, HudHost<'_>) {
        (
            &mut self.hud,
            HudHost {
                screen: &mut self.screen,
                renderer: &mut self.renderer,
                fs: &self.fs,
                console: &mut self.console,
                client: &mut self.client,
                titles: &self.titles,
                audio: self.audio.as_mut(),
                effects: Some(&mut self.effects),
                motd: Some(&mut self.engine.motd),
            },
        )
    }

    /// `ClientDLL_UpdateClientData` (in game, not in a playback, whose type-4 frames stand in)
    /// with the type-4 demo frame of what the client received.
    pub(super) fn hud_think(&mut self) {
        if self.client.state() == State::Active && !self.client.demo_playing() {
            let weapons = self.client.protocol().current_frame().clientdata.weapons();
            // cl_entities[cl.viewentity].origin at frame start (the last packet's origin in
            // first person, cl.simorg once linked in third person).
            let view = match self.client.level().view_entity {
                v if v > 0 => v as usize,
                _ => usize::from(self.client.protocol().playernum) + 1,
            };
            let origin = self.client.entities().get(view).map_or([0.0; 3], |e| e.origin);
            let pre = DemoClientData { origin, viewangles: self.angles.to_array(), weapons, fov: self.scr_fov };
            self.scr_fov = self.hud_update_client_data(origin, pre.viewangles, weapons) as f32;
            self.client.demo_record_client_data(&pre);
        }
    }

    /// `HUD_GetFOV`: a recording gets a type-9 `{1, g_lastFOV}` buffer; a playback takes the
    /// demo zoom.
    fn hud_get_fov(&mut self) -> f32 {
        if self.client.demo_recording() {
            let mut b = Vec::with_capacity(8);
            b.extend_from_slice(&1i32.to_le_bytes());
            b.extend_from_slice(&self.last_fov.to_le_bytes());
            self.client.demo_write_buffer(&b);
        }
        if self.client.demo_playing() {
            self.last_fov = self.demo_zoom;
        }
        self.last_fov
    }

    /// `HUD_UpdateClientData` → `CHud::UpdateClientData`: `CHud::Think` (FOV, owned weapons,
    /// the weapon list's `+attack` confirmation); returns `m_iFOV` (the engine's
    /// `scr_fov_value`).
    pub(super) fn hud_update_client_data(&mut self, origin: [f32; 3], angles: [f32; 3], weapons: i32) -> i32 {
        // g_iPlayerFlags / g_flPlayerSpeed / g_iWeaponFlags come from HUD_WeaponsPostThink;
        // g_iUser1..3 are the HUD's own (not part of this call).
        let g = self.weapons.globals;
        let fov = self.hud_get_fov();
        let state = ClientState {
            weapon_bits: weapons,
            flags: g.player_flags,
            speed: g.player_speed,
            user1: 0,
            user2: 0,
            user3: 0,
            origin,
            angles,
            fov,
            weapon_flags: g.weapon_flags,
            shots_fired: self.client_events.shots_fired,
        };
        let default_fov = self.console.cvars.value("default_fov");
        let attack = self.input.attack_active();
        let (hud, mut eng) = self.hud_host();
        hud.update_client_data(state, default_fov, &eng);
        let out = hud.confirm_selection(attack, &mut eng);
        if let Some(id) = out.weaponselect {
            self.input.weaponselect = id;
        }
        if out.release_attack {
            self.input.release_attack();
        }
        self.hud.fov
    }

    /// The observer logic's `SetViewAngles` and `V_ResetChaseCam`.
    pub(super) fn apply_observer_requests(&mut self) {
        let req = self.hud.take_observer_requests();
        if let Some(a) = req.set_view_angles {
            self.angles = Vec3::from(a);
        }
        if req.reset_chase_cam {
            self.view_state.reset_chase_cam();
        }
    }

    /// `key_dest == key_game`: no chat line and no VGUI menu has the keys.
    pub(super) fn chat_key_dest_game(&self) -> bool {
        !self.chat.is_open() && !self.hud.cursor_visible()
    }

    /// `HUD_Redraw` into this frame's 2D overlay.
    pub(super) fn draw_hud(&mut self) -> Vec<gs_draw::DrawCmd> {
        self.screen.begin_frame();
        // R_DrawScreenFade: drawn by R_RenderView after the view model, so it lies under every
        // 2D layer; not in the overview map modes (onlyClientDraw) or outside the game.
        let map_mode = matches!(self.hud.state.user1, 5 | 6);
        if self.client.state() == State::Active
            && !map_mode
            && let Some((rgba, modulate)) = self.client.screen_fade()
        {
            self.screen.screen_fade(rgba, modulate);
        }
        // The engine text colour from con_color, when it parses as three numbers.
        if let Some(con) = scr::con_color(self.console.cvars.string("con_color")) {
            self.screen.con_color = con;
        }
        // ClientDLL_HudRedraw is skipped while GameUI is visible.
        if self.client.state() == State::Active && !self.gameui.is_game_ui_active() {
            let (time, intermission) = (self.client.time() as f32, self.client.level().intermission != 0);
            let (hud, mut eng) = self.hud_host();
            hud.redraw(time, intermission, &mut eng);
        }
        // SCR_UpdateScreen (not while disconnected). The pre-VGUI center-print check of a
        // finale/cutscene only counts down: the 3D view covers what it draws.
        let chat_open = self.chat.is_open();
        if self.client.state() != State::Disconnected {
            notify::check_resize(self.screen.width as i32);
            // Con_DrawNotify in the viewport pass, after HUD_Redraw (not in intermission 1
            // unless the chat line is open).
            if self.client.level().intermission != 1 || chat_open {
                let chat_y = self.hud.chat_input_y(self.screen.height as i32, self.console.cvars.value("spec_pip") != 0.0);
                let notifytime = self.console.cvars.value("con_notifytime");
                notify::draw_notify(&mut self.screen, &self.nprint, &self.chat, self.realtime, notifytime, chat_y);
            }
            if matches!(self.client.level().intermission, 2 | 3) && !chat_open {
                self.center.countdown(self.frametime);
            }
        }
        // SCR_DrawConnectMsg, before VGui_Paint: in game the 3D view painted inside VGui_Paint
        // covers it, so it shows only without a world view.
        if self.client.state() != State::Active {
            let cv = &self.console.cvars;
            let lines = [cv.string("scr_connectmsg").to_owned(), cv.string("scr_connectmsg1").to_owned(), cv.string("scr_connectmsg2").to_owned()];
            scr::draw_connect_msg(&mut self.screen, [&lines[0], &lines[1], &lines[2]]);
        }
        // VGuiWrap2_Paint: the vgui2 tree with GameUI, also while disconnected.
        self.gameui_frame();
        if self.client.state() != State::Disconnected {
            // After the VGUI2 tree: SCR_DrawFPS, SCR_NetGraph, then (only while GameUI is
            // hidden) the PAUSED plaque and the center print.
            if self.console.cvars.value("cl_showfps") != 0.0 {
                self.fps.draw(&mut self.screen, self.frametime);
            }
            if let Some(stats) = self.client.net_stats() {
                self.net_graph.draw(&mut self.screen, &mut self.console.cvars, &stats, self.input.graph_held(), self.frametime);
            }
            if !self.gameui.is_game_ui_active() {
                if self.console.cvars.value("showpause") != 0.0
                    && self.paused
                    && let Some(pic) = self.paused_pic
                {
                    scr::draw_paused(&mut self.screen, pic);
                }
                let con = scr::con_color(self.console.cvars.string("con_color"));
                let printspeed = self.console.cvars.value("scr_printspeed");
                self.center.check_draw(&mut self.screen, self.frametime, self.client.level().intermission, chat_open, self.client.time(), printspeed, con);
            }
        }
        self.renderer.update_glyphs(&mut self.screen.atlas);
        std::mem::take(&mut self.screen.list)
    }

    /// `VGuiWrap2_Paint` → `CBaseUI::Paint`: the connect/disconnect edges
    /// (`VGuiWrap2_ConnectToServer` / `NotifyOfServerDisconnect`), GameUI's view of the engine,
    /// the queued console text and commands, the frame and paint, then GameUI's requests.
    fn gameui_frame(&mut self) {
        let state = self.client.state();
        {
            let level = if state >= State::Connected { self.client.server_info().map(|i| i.map_model.clone()).unwrap_or_default() } else { String::new() };
            let toggle = gameui_host::engine_key_to_vgui(self.toggle_console_engine_key());
            let mut h = self.gameui.host.borrow_mut();
            h.level_name = level;
            h.max_clients = i32::from(self.client.protocol().maxclients);
            h.time = self.start.elapsed().as_secs_f64();
            h.steam = crate::cmdline::argv().iter().any(|a| a.eq_ignore_ascii_case("-steam"));
            h.con_mono = self.console.cvars.value("con_mono");
            h.toggle_console_key = toggle;
            (h.voice_mixer, h.voice_speaking_volume) = self.audio.as_ref().map_or((false, 0), Audio::voice_tweak_state);
            let commands = self.console.command_names().count();
            let cvars = self.console.cvars.iter().count();
            if h.command_names.len() != commands || h.cvar_names.len() != cvars {
                h.command_names = self.console.command_names().map(str::to_owned).collect();
                h.cvar_names = self.console.cvars.iter().map(|c| c.name.clone()).collect();
            }
            // pfnGetCvarString/Float: updated in place, copied only on a change.
            for c in self.console.cvars.iter() {
                match h.cvars.get_mut(&c.name) {
                    Some(e) => {
                        if e.0 != c.string {
                            e.0.clone_from(&c.string);
                        }
                        e.1 = c.value;
                    }
                    None => {
                        h.cvars.insert(c.name.clone(), (c.string.clone(), c.value));
                    }
                }
            }
            // Key_BindingForKey and LocalPlayerInfo_ValueForKey.
            h.bindings.resize(256, String::new());
            for k in 0..=255u8 {
                let b = self.console.bindings.get(k);
                if h.bindings[usize::from(k)] != b {
                    h.bindings[usize::from(k)] = b.to_owned();
                }
            }
            let same = self.client.config.userinfo.pairs().eq(h.userinfo.iter().map(|(k, v)| (k.as_str(), v.as_str())));
            if !same {
                h.userinfo = self.client.config.userinfo.pairs().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
            }
        }
        // IGameClientExports (mute state, server name) and PlayerInfo_ValueForKey.
        {
            let max = usize::from(self.client.protocol().maxclients).min(32);
            let mut muted = [false; 32];
            {
                let (hud, eng) = self.hud_host();
                for (i, m) in muted.iter_mut().enumerate().take(max) {
                    *m = hud.voice_muted(i as i32 + 1, &eng);
                }
            }
            let mut h = self.gameui.host.borrow_mut();
            h.muted.clear();
            h.muted.extend_from_slice(&muted[..max]);
            h.player_infos.resize(max, String::new());
            for (i, slot) in h.player_infos.iter_mut().enumerate() {
                let info = self.client.level().players.get(i).map_or("", |p| p.info.as_str());
                if slot != info {
                    slot.clear();
                    slot.push_str(info);
                }
            }
            let server = self.hud.viewport.server_name();
            if h.server_host_name != server {
                h.server_host_name = server.to_owned();
            }
        }
        // The client's GameUI calls (progress, loading, ConnectToServer, disconnect), in order,
        // with the level name already GameUI's.
        for e in std::mem::take(&mut self.ui_events) {
            self.gameui_engine_event(e);
        }
        self.gameui.ui.time_ms = self.start.elapsed().as_millis() as u32;
        for (dev, text) in notify::take_console_text() {
            if dev {
                self.gameui.con_dprintf(&text);
            } else {
                self.gameui.con_printf(&text);
            }
        }
        let ops = std::mem::take(&mut self.ui_ops);
        let mut activated = false;
        let mut condump = false;
        let requests = {
            let mut loader = hud_host::Loader { renderer: &mut self.renderer, fs: &self.fs };
            let mut s = gameui_host::VguiSurface::new(&mut self.vgui, &mut self.screen, &mut loader);
            for op in ops {
                match op {
                    UiOp::ToggleConsole => {
                        activated |= !self.gameui.is_console_visible();
                        self.gameui.toggle_console(&mut s);
                    }
                    UiOp::HideConsole => self.gameui.hide_console(),
                    UiOp::Clear => self.gameui.clear(),
                    UiOp::Condump => condump = true,
                    // cmd_escape: the chat line takes ESC (Key_Message), else HideGameUI.
                    UiOp::Escape => {
                        if self.chat.is_open() {
                            let _ = self.chat.key(gs_console::keys::K_ESCAPE);
                        } else {
                            self.gameui.hide_game_ui();
                        }
                    }
                    UiOp::LoadingStarted => self.gameui.loading_started("level"),
                    UiOp::LoadingFinished => {
                        self.gameui.loading_finished();
                    }
                }
            }
            self.gameui.paint(&mut s)
        };
        if condump {
            self.condump();
        }
        if activated {
            self.on_game_ui_activated();
        }
        for name in std::mem::take(&mut self.vgui.sounds) {
            let name = name.replace('\\', "/");
            let name = if name.len() > 6 && name[..6].eq_ignore_ascii_case("sound/") { name[6..].to_owned() } else { name };
            if let Some(audio) = &mut self.audio {
                audio.hud_sound(gs_sound::chan::AUTO, &name, 1.0, 100);
            }
        }
        for r in requests {
            match r {
                gs_gameui::HostRequest::ClientCmd(c) => self.console.add_text(&format!("{c}\n")),
                gs_gameui::HostRequest::HideGameUi | gs_gameui::HostRequest::HideGameUiFromConsole => {
                    self.gameui.hide_game_ui();
                }
                gs_gameui::HostRequest::ActivateGameUi => self.activate_game_ui(),
                gs_gameui::HostRequest::CvarSet { name, value } => {
                    let (console, mut host) = self.console_host();
                    console.direct_set(&mut host, &name, &value);
                }
                gs_gameui::HostRequest::SetRegistryString { key, value } => {
                    // vgui2 ISystem keys: `<subkey>\<value>` under HKCU; GameUI only uses User Token 2.
                    if let Some(name) = key.strip_prefix("Software\\Valve\\Half-Life\\Settings\\") {
                        crate::registry::write_string(name, &value);
                    }
                }
                gs_gameui::HostRequest::ConPrintf(text) => notify::con_printf(&text),
                gs_gameui::HostRequest::MutePlayer { index, mute } => {
                    let (hud, mut eng) = self.hud_host();
                    hud.set_voice_muted(index, mute, &mut eng);
                }
                gs_gameui::HostRequest::VoiceTweak(start) => {
                    if let Some(audio) = &mut self.audio {
                        if start {
                            audio.voice_tweak_start();
                        } else {
                            audio.voice_tweak_end();
                        }
                    }
                }
                gs_gameui::HostRequest::VoiceScale(v) => self.console.cvars.set_value_only("voice_scale", v),
            }
        }
    }

    /// `SystemWrapper_RunFrame(host_frametime)`: every module's `RunFrame` with the accumulated
    /// time; the player's GameUI-listener signals go to the demo dialog.
    pub(super) fn run_demo_modules(&mut self, dt: f64) {
        let Some(p) = self.client.demo_player.clone() else { return };
        let signals = {
            let mut dp = p.borrow_mut();
            let t = dp.sys.time.get() + dt;
            dp.run_modules(t);
            dp.take_ui_signals()
        };
        self.drain_module_log();
        self.demo_ui_signals(signals);
    }

    /// The modules' console output since the last call.
    pub(super) fn drain_module_log(&mut self) {
        if let Some(p) = &self.client.demo_player {
            let sys = p.borrow().sys.clone();
            crate::demoplayer::drain_log(&sys, self.console.cvars.value("developer") != 0.0);
        }
        // Con_Printf / Con_DPrintf from movement, studio and prediction code.
        for (developer, text) in gs_common::conprint::drain() {
            if developer {
                notify::con_dprintf(&text);
            } else {
                notify::con_printf(&text);
            }
        }
    }

    /// `CDemoPlayerDialog::ReceiveSignal` for each queued listener signal.
    fn demo_ui_signals(&mut self, signals: Vec<(i32, i32)>) {
        if signals.is_empty() {
            return;
        }
        let mut loader = hud_host::Loader { renderer: &mut self.renderer, fs: &self.fs };
        let mut s = gameui_host::VguiSurface::new(&mut self.vgui, &mut self.screen, &mut loader);
        self.gameui.demo_signals(&signals, &mut s);
    }

    /// `CBaseUI::ActivateGameUI` from the engine side.
    fn activate_game_ui(&mut self) {
        {
            let mut loader = hud_host::Loader { renderer: &mut self.renderer, fs: &self.fs };
            let mut s = gameui_host::VguiSurface::new(&mut self.vgui, &mut self.screen, &mut loader);
            self.gameui.activate_game_ui(&mut s);
        }
        self.on_game_ui_activated();
    }

    /// One of the client's GameUI calls (docs/re/gameui-engine.md §8).
    fn gameui_engine_event(&mut self, e: gs_client::UiEvent) {
        use gs_client::UiEvent as U;
        let mut activated = false;
        {
            let mut loader = hud_host::Loader { renderer: &mut self.renderer, fs: &self.fs };
            let mut s = gameui_host::VguiSurface::new(&mut self.vgui, &mut self.screen, &mut loader);
            let g = &mut self.gameui;
            match e {
                U::StartProgress { kind, steps } => g.start_progress_bar(&kind, steps, &mut s),
                U::ContinueProgress(p) => {
                    g.continue_progress_bar(p, &mut s);
                }
                U::Status(t) => {
                    g.set_progress_status(&t);
                }
                U::SecondaryProgress(f) => g.set_secondary_progress(f, &mut s),
                U::SecondaryText(t) => g.set_secondary_text(&t),
                U::LoadingStarted { kind, .. } => g.loading_started(&kind),
                U::LoadingFinished { .. } => {
                    g.loading_finished();
                }
                // COM_ExplainDisconnection prints the text unless it is a `#` token.
                U::Explain { text, extended: false } => {
                    if !text.starts_with('#') {
                        notify::con_printf(&format!("{text}\n"));
                    }
                    self.disconnect_reason = Some(text);
                }
                U::Explain { text, extended: true } => self.disconnect_ext = text,
                // VGuiWrap2_NotifyOfServerDisconnect with cls.state 1: HideAllVGUIMenu,
                // ActivateGameUI, StopProgressBar with the explanation; then it is cleared.
                U::ServerDisconnected => {
                    self.hud.hide_all_vgui_menu();
                    let reason = self.disconnect_reason.take();
                    let ext = std::mem::take(&mut self.disconnect_ext);
                    g.notify_disconnect(reason.is_some(), reason.as_deref().unwrap_or(""), &ext, &mut s);
                    activated = true;
                }
                // VGuiWrap2_ConnectToServer(gamedir, ip, port): the explanation is cleared, the
                // menu closes; the ip has the address bytes in network order.
                U::WentActive => {
                    self.disconnect_reason = None;
                    self.disconnect_ext.clear();
                    let (ip, port) = self.client.netchan().map_or((0, 0), |n| (i32::from_le_bytes(n.remote.ip().octets()), i32::from(n.remote.port())));
                    g.connect_to_server(&self.client.config.game, ip, port, &mut s);
                }
                // DemoPlayer_ShowUI: ActivateGameUI, then IGameUI::ActivateDemoUI.
                U::ShowDemoUi => {
                    g.activate_game_ui(&mut s);
                    g.activate_demo_ui(&mut s);
                    activated = true;
                }
            }
        }
        if activated {
            self.on_game_ui_activated();
        }
    }

    /// `condump`: the console history into the first free `condump%03d.txt`.
    fn condump(&mut self) {
        let Some(name) = (0..1000).map(|i| format!("condump{i:03}.txt")).find(|n| self.fs.read(n).is_err()) else {
            notify::con_printf("Can't condump! Too many existing condump output files in the gamedir!\n");
            return;
        };
        let text = self.gameui.console_text();
        let mut out = Vec::with_capacity(text.len());
        for b in text.bytes() {
            if b == b'\n' {
                out.push(b'\r');
            }
            out.push(b);
        }
        match self.fs.write(&name, PathId::GameConfig, &out) {
            Ok(_) => notify::con_printf(&format!("console dumped to {name}\n")),
            Err(_) => notify::con_printf(&format!("Unable to condump to {name}\n")),
        }
    }

    /// `Cbuf_Execute`, then the cvar side effects the engine applies.
    pub(super) fn run_console(&mut self) {
        let (console, mut host) = self.console_host();
        console.execute(&mut host);
        if !console.take_changed().is_empty() {
            console::apply_client_cvars(console, host.client);
        }
    }
}
