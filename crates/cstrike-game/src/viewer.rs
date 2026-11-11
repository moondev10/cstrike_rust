//! Interactive free-fly map viewer.
//!
//! Controls: click to capture the mouse, WASD move, Space/Ctrl up/down, Shift fast,
//! F12 screenshot (`screenshots/`), P print the current `--pos/--ang`, Esc release mouse / quit.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use glam::Vec3;
use gs_fs::FileSystem;
use gs_render::camera::angle_vectors;
use gs_render::{Gpu, Renderer, View, capture};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::lightstyles::LightStyles;
use crate::scene::LocalMap;
use crate::shot::save_png;

const MOUSE_DEGREES_PER_COUNT: f32 = 0.022 * 3.0;
const MOVE_SPEED: f32 = 320.0;
const FAST_MULTIPLIER: f32 = 3.0;

pub struct ViewerOptions {
    pub map: String,
    pub pos: Option<Vec3>,
    pub angles: Option<Vec3>,
    pub fov: f32,
    pub size: (u32, u32),
}

pub fn run(fs: FileSystem, opts: ViewerOptions) -> Result<()> {
    let event_loop = EventLoop::new().context("creating event loop")?;
    let mut app = Viewer { fs, opts, state: None, error: None };
    event_loop.run_app(&mut app).context("event loop")?;
    app.error.map_or(Ok(()), Err)
}

struct Viewer {
    fs: FileSystem,
    opts: ViewerOptions,
    state: Option<Running>,
    error: Option<anyhow::Error>,
}

struct Running {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    gpu: Gpu,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    scene: LocalMap,
    styles: LightStyles,
    view: View,
    keys: HashSet<KeyCode>,
    grabbed: bool,
    start: Instant,
    last_frame: Instant,
    fps_timer: (Instant, u32),
}

impl Viewer {
    fn fail(&mut self, event_loop: &ActiveEventLoop, err: anyhow::Error) {
        self.error = Some(err);
        event_loop.exit();
    }

    fn start(&self, event_loop: &ActiveEventLoop) -> Result<Running> {
        let attrs = crate::video::with_app_icon(
            Window::default_attributes()
                .with_title(format!("cstrike-game - {}", self.opts.map))
                .with_inner_size(PhysicalSize::new(self.opts.size.0, self.opts.size.1))
                .with_active(false),
        );
        let window = Arc::new(event_loop.create_window(attrs).context("creating window")?);
        let instance = Gpu::new_instance();
        let surface = instance.create_surface(window.clone()).context("creating surface")?;
        let gpu = pollster::block_on(Gpu::new(instance, Some(&surface), None))?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| anyhow!("surface is not supported by the adapter"))?;
        let formats = surface.get_capabilities(&gpu.adapter).formats;
        config.format = formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
            .ok_or_else(|| anyhow!("no linear 8-bit surface format in {formats:?}"))?;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&gpu.device, &config);

        let scene = LocalMap::load(&self.fs, &self.opts.map)?;
        let mut renderer = Renderer::new(&gpu.device, &gpu.queue, config.format, 4);
        renderer.load_map(&scene.map)?;
        let mut view = View::new(
            self.opts.pos.unwrap_or(scene.spawn_origin),
            self.opts.angles.unwrap_or(scene.spawn_angles),
        );
        view.fov = self.opts.fov;
        let now = Instant::now();
        Ok(Running {
            window,
            surface,
            gpu,
            config,
            renderer,
            scene,
            styles: LightStyles::default(),
            view,
            keys: HashSet::new(),
            grabbed: false,
            start: now,
            last_frame: now,
            fps_timer: (now, 0),
        })
    }
}

impl Running {
    fn set_grab(&mut self, grab: bool) {
        let mode = if grab { CursorGrabMode::Locked } else { CursorGrabMode::None };
        let ok = self
            .window
            .set_cursor_grab(mode)
            .or_else(|_| self.window.set_cursor_grab(if grab { CursorGrabMode::Confined } else { CursorGrabMode::None }));
        if let Err(err) = ok {
            log::warn!("cursor grab: {err}");
        }
        self.window.set_cursor_visible(!grab);
        self.grabbed = grab;
    }

    fn update(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        let (forward, right, _) = angle_vectors(self.view.angles);
        let mut wish = Vec3::ZERO;
        let held = |k| self.keys.contains(&k);
        if held(KeyCode::KeyW) {
            wish += forward;
        }
        if held(KeyCode::KeyS) {
            wish -= forward;
        }
        if held(KeyCode::KeyD) {
            wish += right;
        }
        if held(KeyCode::KeyA) {
            wish -= right;
        }
        if held(KeyCode::Space) {
            wish += Vec3::Z;
        }
        if held(KeyCode::ControlLeft) {
            wish -= Vec3::Z;
        }
        let speed = if held(KeyCode::ShiftLeft) { MOVE_SPEED * FAST_MULTIPLIER } else { MOVE_SPEED };
        self.view.origin += wish.normalize_or_zero() * speed * dt;
        self.view.time = (now - self.start).as_secs_f64();
        self.view.light_styles = self.styles.values(self.view.time);

        self.fps_timer.1 += 1;
        let elapsed = (now - self.fps_timer.0).as_secs_f32();
        if elapsed >= 0.5 {
            let fps = self.fps_timer.1 as f32 / elapsed;
            self.fps_timer = (now, 0);
            let o = self.view.origin;
            self.window.set_title(&format!(
                "cstrike-game - {} | {fps:.0} fps | pos {:.0} {:.0} {:.0}",
                self.scene.map.name, o.x, o.y, o.z
            ));
        }
    }

    fn draw(&mut self) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.gpu.device, &self.config);
                return;
            }
            other => {
                log::debug!("skipping frame: {other:?}");
                return;
            }
        };
        let target = frame.texture.create_view(&Default::default());
        self.renderer.render(&target, (self.config.width, self.config.height), &self.view, &self.scene.draw, None);
        self.window.pre_present_notify();
        self.gpu.queue.present(frame);
    }

    fn screenshot(&mut self) -> Result<PathBuf> {
        let dir = PathBuf::from("screenshots");
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let path = dir.join(format!("{}_{stamp}.png", self.scene.map.name));
        let image = capture::capture(&mut self.renderer, (self.config.width, self.config.height), &self.view, &self.scene.draw, None)?;
        save_png(&image, &path)?;
        Ok(path)
    }

    fn print_position(&self) {
        let (o, a) = (self.view.origin, self.view.angles);
        println!("--pos {:.1} {:.1} {:.1} --ang {:.1} {:.1} {:.1}", o.x, o.y, o.z, a.x, a.y, a.z);
    }
}

impl ApplicationHandler for Viewer {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match self.start(event_loop) {
            Ok(state) => self.state = Some(state),
            Err(err) => self.fail(event_loop, err),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                state.config.width = size.width;
                state.config.height = size.height;
                state.surface.configure(&state.gpu.device, &state.config);
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } if !state.grabbed => {
                state.set_grab(true);
            }
            WindowEvent::Focused(false) if state.grabbed => state.set_grab(false),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else { return };
                if event.state == ElementState::Released {
                    state.keys.remove(&code);
                    return;
                }
                state.keys.insert(code);
                if event.repeat {
                    return;
                }
                match code {
                    KeyCode::Escape if state.grabbed => state.set_grab(false),
                    KeyCode::Escape => event_loop.exit(),
                    KeyCode::KeyP => state.print_position(),
                    KeyCode::F12 => match state.screenshot() {
                        Ok(path) => println!("screenshot {}", path.display()),
                        Err(err) => log::error!("screenshot failed: {err:#}"),
                    },
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                state.update();
                state.draw();
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        let Some(state) = self.state.as_mut().filter(|s| s.grabbed) else { return };
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            let a = &mut state.view.angles;
            a.y -= dx as f32 * MOUSE_DEGREES_PER_COUNT;
            a.x = (a.x + dy as f32 * MOUSE_DEGREES_PER_COUNT).clamp(-89.0, 89.0);
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}
