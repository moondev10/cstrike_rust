//! Window input of the running game: the OS cursor and grab, the harness's scripted mouse,
//! and key and text events on their way to the console, GameUI and the client.

use super::*;

impl Running {
    /// The SDL cursor and window grab (`BaseUISurface::SetCursor`); `mouseactive` is separate.
    /// With `m_rawinput` the grab is SDL's relative mode (the cursor stays put, raw motion
    /// only); without it the cursor is hidden but moves inside the client area
    /// (`SDL_ShowCursor(0)` + `SDL_SetWindowGrab`) and is warped to the centre.
    pub(super) fn os_grab(&mut self, grab: bool) {
        if grab && self.hidden {
            return;
        }
        let relative = self.console.cvars.value("m_rawinput") != 0.0;
        let result = if !grab {
            self.window.set_cursor_grab(CursorGrabMode::None)
        } else if relative {
            self.window.set_cursor_grab(CursorGrabMode::Locked).or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined))
        } else if self.console.cvars.value("cl_mousegrab") != 0.0 {
            // SDL_SetWindowGrab only with cl_mousegrab (vgui-surface.md §7.1).
            self.window.set_cursor_grab(CursorGrabMode::Confined)
        } else {
            self.window.set_cursor_grab(CursorGrabMode::None)
        };
        if let Err(err) = result {
            log::warn!("cursor grab: {err}");
        }
        // winit pins a hidden grabbed cursor to one pixel; a transparent one keeps moving.
        match &self.blank_cursor {
            Some(blank) if grab && !relative => {
                self.window.set_cursor(blank.clone());
                self.window.set_cursor_visible(true);
            }
            _ => {
                self.window.set_cursor(CursorIcon::Default);
                self.window.set_cursor_visible(!grab);
            }
        }
        self.grabbed = grab;
        self.relative_grab = relative;
        // CalculateMouseVisible: hiding the cursor without m_rawinput warps it to the centre.
        if grab && !relative {
            self.center_cursor();
        }
    }

    /// `SetCursorPos(window centre)` (`IN_ResetMouse`, `SDL_WarpMouseInWindow(ww/2, wh/2)`).
    pub(super) fn center_cursor(&self) {
        let size = self.window.inner_size();
        if let Err(err) = self.window.set_cursor_position(winit::dpi::PhysicalPosition::new(size.width / 2, size.height / 2)) {
            log::warn!("cursor warp: {err}");
        }
    }

    /// `vgui2::surface()->IsCursorVisible()`: a popup takes mouse input.
    pub(super) fn vgui_cursor_visible(&self) -> bool {
        self.hud.cursor_visible() || self.gameui.is_game_ui_active()
    }

    /// client.dll `m_bRawInput`, re-read from `m_rawinput` at most once a second.
    pub(super) fn raw_input(&mut self) -> bool {
        if self.realtime - self.raw_input_checked >= 1.0 || self.realtime < self.raw_input_checked {
            self.raw_input_checked = self.realtime;
            self.raw_input = self.console.cvars.value("m_rawinput") != 0.0;
        }
        self.raw_input
    }

    /// `CalculateMouseVisible` with the window's focus: the cursor hides and the window grabs it
    /// while the window is active and no popup takes mouse input. The client's own input skips
    /// mouse motion meanwhile (`MoveParams::cursor_visible`) but `mouseactive` stays set.
    pub(super) fn sync_menu_cursor(&mut self) {
        let grab = self.input.mouse_active && !self.vgui_cursor_visible() && self.window.has_focus();
        // SetCursor also switches SDL's relative mode when m_rawinput changed.
        let relative = self.console.cvars.value("m_rawinput") != 0.0;
        if grab != self.grabbed || grab && relative != self.relative_grab {
            self.os_grab(grab);
        }
    }

    /// `--click`: the window's cursor-move and mouse-button paths for a scripted action
    /// (vgui2 sees the event first; the client menu only while GameUI is hidden).
    pub(super) fn scripted_mouse(&mut self, x: i32, y: i32, button: ScriptedMouse) {
        self.cursor = (x, y);
        self.hud.set_cursor(x, y);
        self.gameui.ui.cursor_moved(x, y);
        if button == ScriptedMouse::Move {
            return;
        }
        let code = if button == ScriptedMouse::Left { gs_vgui::keys::MOUSE_LEFT } else { gs_vgui::keys::MOUSE_RIGHT };
        let (t, s) = (self.console.cvars.value("sdl_double_click_time"), self.console.cvars.value("sdl_double_click_size"));
        self.vgui_input.mouse_down(&mut self.gameui.ui, code, (x, y), t, s);
        self.gameui.ui.mouse_released(code);
        if self.gameui.is_game_ui_active() {
            return;
        }
        if !self.hud.cursor_visible() {
            log::info!("mouse: no cursor, click ignored");
            return;
        }
        let (hud, mut eng) = self.hud_host();
        hud.click(x, y, button == ScriptedMouse::Left, &mut eng);
        if button == ScriptedMouse::Left {
            hud.release(x, y, &mut eng);
        }
    }

    /// The fullscreen cursor mapping (gameui-engine.md §9.2): window pixels to mode pixels,
    /// then through the letterbox scale; windowed it is the identity.
    pub(super) fn window_to_vid(&self, x: i32, y: i32) -> (i32, i32) {
        if self.video.windowed {
            return (x, y);
        }
        let (mw, mh) = (self.vid.0 as i32, self.vid.1 as i32);
        let fx = (mw as f32 / self.config.width as f32 * x as f32) as i32;
        let fy = (mh as f32 / self.config.height as f32 * y as f32) as i32;
        let (sx, sy) = self.letterbox_scale;
        (((sx - 1.0) * (fx - mw / 2) as f32 + fx as f32) as i32, ((sy - 1.0) * (fy - mh / 2) as f32 + fy as f32) as i32)
    }

    /// `CGame::AppActivate`: activation runs ClearIOStates (every key released, the swallow
    /// marks cleared, IN_ClearStates); the client's mouse is activated or deactivated only while
    /// GameUI is hidden.
    pub(super) fn app_activate(&mut self, active: bool) {
        eprintln!("AppActive: {}", if active { "active" } else { "not active" });
        let game_ui = self.gameui.is_game_ui_active();
        if active {
            for key in 0..=255u8 {
                self.key_input(key, false, false);
            }
            self.swallowed = [false; 256];
            self.input.clear_mouse();
            if !game_ui {
                self.input.mouse_active = true;
                self.mouse_parms.activate();
            }
        } else if !game_ui && !self.hidden {
            self.input.mouse_active = false;
            self.mouse_parms.deactivate();
        }
        self.active_app = active || self.hidden;
        if !active && self.grabbed {
            self.os_grab(false);
        }
        if let Some(audio) = &mut self.audio {
            audio.music_pause(!active);
        }
    }

    /// While the MOTD page has the keyboard, the window gets no focus events: leaving the
    /// application and coming back (Windows gives the focus back to the page) show only in the
    /// foreground window.
    pub(super) fn poll_page_focus(&mut self) {
        if self.hidden || !self.engine.motd.shows_pages() {
            return;
        }
        let front = crate::ffi::is_foreground(&self.window);
        if self.page_focus && !front {
            self.page_focus = false;
            self.app_activate(false);
        } else if !self.active_app && front {
            self.page_focus = true;
            self.app_activate(true);
        }
    }

    /// SDL_TEXTINPUT: typed characters (no control codes) for the chat line and vgui2.
    pub(super) fn text_input(&mut self, text: &str) {
        if self.chat.is_open() {
            self.chat.text(text);
        }
        for ch in text.chars() {
            self.gameui.ui.key_typed(ch);
        }
    }

    /// `cle_Key_Event`: with the chat line open (`key_dest == key_message`) presses go to
    /// `Key_Message` (auto-repeat only for backspace). Otherwise `ClientDLL_Key_Event` runs for
    /// presses and for the release of a key it swallowed: BaseUI's VGUI filter, then the
    /// client's HUD menus; a swallowed press records `clientSwallowedDown` and a swallowed
    /// release runs no `-command`.
    pub(super) fn key_input(&mut self, key: u8, down: bool, repeat: bool) {
        if down && self.chat.is_open() {
            if (!repeat || key == 127)
                && let Some((line, privileged)) = self.chat.key(key)
            {
                if privileged { self.console.add_text(&line) } else { self.console.add_filtered_text(&line) }
            }
            return;
        }
        let k = usize::from(key);
        if !self.chat.is_open() && (down || self.swallowed[k]) {
            let toggle_key = self.toggle_console_engine_key();
            let filter = {
                let mut loader = hud_host::Loader { renderer: &mut self.renderer, fs: &self.fs };
                let mut s = gameui_host::VguiSurface::new(&mut self.vgui, &mut self.screen, &mut loader);
                self.gameui.key_event(down, i32::from(key), toggle_key, &mut s)
            };
            let consumed = match filter {
                gs_gameui::KeyFilter::Pass => false,
                gs_gameui::KeyFilter::SwallowedActivate => {
                    self.on_game_ui_activated();
                    true
                }
                gs_gameui::KeyFilter::Swallowed | gs_gameui::KeyFilter::SwallowedHide(_) => true,
            };
            let consumed = consumed || (down && self.hud.cursor_visible() && {
                let binding = self.console.bindings.get(key).to_owned();
                let (hud, mut eng) = self.hud_host();
                hud.menu_key(key, &binding, &mut eng)
            });
            if consumed {
                if down && !repeat {
                    self.swallowed[k] = true;
                }
                if !down {
                    self.swallowed[k] = false;
                }
                return;
            }
        }
        if !down {
            self.swallowed[k] = false;
        }
        // cle_Key_Event in a playback (not HLTV): a console key's press toggles the console
        // instead of running its binding.
        if down && self.client.demo_playing() && !self.client.spectator() && self.chat_key_dest_game() && is_console_key(key) {
            self.ui_ops.push(UiOp::ToggleConsole);
            return;
        }
        self.console.key_event(key, down, repeat);
    }

    /// `g_ToggleConsoleKey`: the first engine key bound to `toggleconsole`.
    pub(super) fn toggle_console_engine_key(&self) -> i32 {
        (0..=255u8).find(|&k| self.console.bindings.get(k).eq_ignore_ascii_case("toggleconsole")).map_or(0, i32::from)
    }

    /// `CBaseUI::ActivateGameUI`'s client side: `HideClientUI` + `HideScoreBoard`.
    pub(super) fn on_game_ui_activated(&mut self) {
        let (hud, mut eng) = self.hud_host();
        hud.score(false, &mut eng);
    }
}
