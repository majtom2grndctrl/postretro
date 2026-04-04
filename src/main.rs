// Postretro engine entry point.
// See: context/lib/rendering_pipeline.md

mod bsp;
mod camera;
mod render;
mod visibility;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use glam::Vec3;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowAttributes};

use crate::camera::Camera;
use crate::render::Renderer;
use crate::visibility::VisibleFaces;

/// CLI flag to force the line-list wireframe fallback path.
const FORCE_LINE_LIST_FLAG: &str = "--force-line-list";

fn main() -> Result<()> {
    env_logger::init();
    log::info!("[Engine] Postretro starting");

    let args: Vec<String> = std::env::args().collect();
    let force_line_list = args.iter().any(|a| a == FORCE_LINE_LIST_FLAG);

    // Load BSP before entering the event loop (heavy I/O must not block the loop).
    let bsp_path = bsp::resolve_bsp_path(&args);
    let bsp_world = match bsp::load_bsp(&bsp_path) {
        Ok(world) => {
            log::info!("[Engine] BSP loaded successfully from {bsp_path}");
            if world.visdata.is_empty() {
                log::warn!(
                    "[Visibility] BSP has no visdata — PVS culling disabled, drawing all faces. \
                     Compile the map with vis to enable culling."
                );
            }
            Some(world)
        }
        Err(bsp::BspLoadError::FileNotFound(path)) => {
            log::warn!("[Engine] BSP file not found: {path} — starting without map");
            None
        }
        Err(err) => {
            anyhow::bail!("failed to load BSP: {err}");
        }
    };

    let event_loop = EventLoop::new().context("failed to create event loop")?;

    let mut app = App {
        renderer: None,
        window_state: None,
        bsp_world,
        force_line_list,
        exit_result: Ok(()),
        camera: Camera::new(Vec3::new(0.0, 200.0, 500.0), 0.0, 0.0),
        keys_held: HashSet::new(),
        mouse_delta: (0.0, 0.0),
        last_frame: Instant::now(),
    };

    event_loop
        .run_app(&mut app)
        .context("event loop terminated with error")?;

    app.exit_result
}

fn window_attributes() -> WindowAttributes {
    Window::default_attributes()
        .with_title("Postretro")
        .with_inner_size(winit::dpi::LogicalSize::new(1280, 720))
}

// --- Application state ---

struct App {
    renderer: Option<Renderer>,
    window_state: Option<WindowState>,
    /// BSP world data, held until the renderer is created (ownership transfers on resume).
    bsp_world: Option<bsp::BspWorld>,
    force_line_list: bool,
    exit_result: Result<()>,

    camera: Camera,
    keys_held: HashSet<KeyCode>,
    /// Accumulated mouse delta since last frame (dx, dy).
    mouse_delta: (f64, f64),
    last_frame: Instant,
}

struct WindowState {
    window: Arc<Window>,
}

// --- Cursor capture ---

/// Attempt to capture the mouse cursor, trying Locked first then Confined.
fn capture_cursor(window: &Window) {
    if window.set_cursor_grab(CursorGrabMode::Locked).is_err() {
        if let Err(err) = window.set_cursor_grab(CursorGrabMode::Confined) {
            log::warn!("[Input] Failed to grab cursor: {err}");
        }
    }
    window.set_cursor_visible(false);
}

/// Release the mouse cursor.
fn release_cursor(window: &Window) {
    let _ = window.set_cursor_grab(CursorGrabMode::None);
    window.set_cursor_visible(true);
}

// --- ApplicationHandler ---

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = match event_loop.create_window(window_attributes()) {
            Ok(w) => Arc::new(w),
            Err(err) => {
                self.exit_result = Err(anyhow::anyhow!("failed to create window: {err}"));
                event_loop.exit();
                return;
            }
        };

        let renderer = match Renderer::new(
            &window,
            self.bsp_world.as_ref(),
            self.force_line_list,
        ) {
            Ok(r) => r,
            Err(err) => {
                self.exit_result = Err(err);
                event_loop.exit();
                return;
            }
        };

        // Update camera aspect ratio from the initial window size.
        let size = window.inner_size();
        self.camera.update_aspect(size.width, size.height);

        capture_cursor(&window);

        self.renderer = Some(renderer);
        self.window_state = Some(WindowState { window });
        self.last_frame = Instant::now();

        log::info!("[Engine] Window ready");
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.window_state = None;
        self.renderer = None;
        log::info!("[Engine] Suspended");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
                self.camera.update_aspect(size.width, size.height);
            }
            WindowEvent::CloseRequested => {
                if let Some(ws) = self.window_state.as_ref() {
                    release_cursor(&ws.window);
                }
                log::info!("[Engine] Shutting down");
                event_loop.exit();
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        ..
                    },
                ..
            } => {
                if let Some(ws) = self.window_state.as_ref() {
                    release_cursor(&ws.window);
                }
                log::info!("[Engine] Shutting down");
                event_loop.exit();
            }
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                if let PhysicalKey::Code(code) = key_event.physical_key {
                    if key_event.state.is_pressed() {
                        self.keys_held.insert(code);
                    } else {
                        self.keys_held.remove(&code);
                    }
                }
            }
            WindowEvent::Focused(focused) => {
                if let Some(ws) = self.window_state.as_ref() {
                    if focused {
                        capture_cursor(&ws.window);
                    } else {
                        release_cursor(&ws.window);
                        self.keys_held.clear();
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                // Compute delta time.
                let now = Instant::now();
                let dt = now.duration_since(self.last_frame).as_secs_f32();
                self.last_frame = now;

                // Clamp dt to avoid huge jumps after window drag or similar stalls.
                let dt = dt.min(0.1);

                // Apply mouse rotation.
                let yaw_delta = -self.mouse_delta.0 as f32 * camera::SENSITIVITY;
                let pitch_delta = -self.mouse_delta.1 as f32 * camera::SENSITIVITY;
                self.camera.rotate(yaw_delta, pitch_delta);
                self.mouse_delta = (0.0, 0.0);

                // Compute movement from held keys.
                let speed = if self.keys_held.contains(&KeyCode::ShiftLeft)
                    || self.keys_held.contains(&KeyCode::ShiftRight)
                {
                    camera::MOVE_SPEED * camera::SPRINT_MULTIPLIER
                } else {
                    camera::MOVE_SPEED
                };

                let forward = self.camera.forward();
                let right = self.camera.right();
                let mut move_dir = Vec3::ZERO;

                if self.keys_held.contains(&KeyCode::KeyW) {
                    move_dir += forward;
                }
                if self.keys_held.contains(&KeyCode::KeyS) {
                    move_dir -= forward;
                }
                if self.keys_held.contains(&KeyCode::KeyD) {
                    move_dir += right;
                }
                if self.keys_held.contains(&KeyCode::KeyA) {
                    move_dir -= right;
                }
                if self.keys_held.contains(&KeyCode::KeyE) {
                    move_dir += Vec3::Y;
                }
                if self.keys_held.contains(&KeyCode::KeyQ) {
                    move_dir -= Vec3::Y;
                }

                // Normalize to prevent faster diagonal movement, but only
                // if there's actual movement input.
                if move_dir.length_squared() > 0.0 {
                    move_dir = move_dir.normalize();
                }

                self.camera.position += move_dir * speed * dt;

                // Determine visibility: find camera leaf, decompress PVS, collect visible faces.
                let visible = match self.bsp_world.as_ref() {
                    Some(world) => visibility::determine_visibility(self.camera.position, world),
                    None => VisibleFaces::DrawAll,
                };

                // Upload view-projection and render.
                if let Some(renderer) = self.renderer.as_ref() {
                    renderer.update_view_projection(self.camera.view_projection());
                    if renderer.is_ready() {
                        if let Err(err) = renderer.render_frame(&visible) {
                            self.exit_result = Err(err);
                            event_loop.exit();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.mouse_delta.0 += delta.0;
            self.mouse_delta.1 += delta.1;
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(ws) = self.window_state.as_ref() {
            ws.window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.renderer = None;
        self.window_state = None;
        log::info!("[Engine] Exited");
    }
}
