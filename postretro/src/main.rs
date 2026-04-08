// Postretro engine entry point.
// See: context/lib/rendering_pipeline.md

mod bsp;
mod camera;
mod frame_timing;
mod portal_vis;
mod prl;
mod render;
mod visibility;

use std::collections::HashSet;
use std::path::Path;
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
use crate::frame_timing::{FrameTiming, InterpolableState};
use crate::render::Renderer;
use crate::visibility::{VisibilityStats, VisibleFaces};

const FORCE_LINE_LIST_FLAG: &str = "--force-line-list";
const DEFAULT_MAP_PATH: &str = "assets/maps/test.bsp";

/// Loaded level data: either BSP or PRL format.
enum Level {
    Bsp(bsp::BspWorld),
    Prl(prl::LevelWorld),
}

fn resolve_map_path(args: &[String]) -> String {
    args.iter()
        .skip(1)
        .find(|a| *a != FORCE_LINE_LIST_FLAG)
        .cloned()
        .unwrap_or_else(|| DEFAULT_MAP_PATH.to_string())
}

fn load_level(path: &str) -> Result<Option<Level>> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "bsp" => match bsp::load_bsp(path) {
            Ok(world) => {
                log::info!("[Engine] BSP loaded successfully from {path}");
                if world.visdata.is_empty() {
                    log::warn!(
                        "[Visibility] BSP has no visdata — PVS culling disabled, drawing all faces. \
                         Compile the map with vis to enable culling."
                    );
                }
                Ok(Some(Level::Bsp(world)))
            }
            Err(bsp::BspLoadError::FileNotFound(p)) => {
                log::warn!("[Engine] BSP file not found: {p} — starting without map");
                Ok(None)
            }
            Err(err) => anyhow::bail!("failed to load BSP: {err}"),
        },
        "prl" => match prl::load_prl(path) {
            Ok(world) => {
                log::info!("[Engine] PRL loaded successfully from {path}");
                Ok(Some(Level::Prl(world)))
            }
            Err(prl::PrlLoadError::FileNotFound(p)) => {
                log::warn!("[Engine] PRL file not found: {p} — starting without map");
                Ok(None)
            }
            Err(err) => anyhow::bail!("failed to load PRL: {err}"),
        },
        _ => {
            log::warn!("[Engine] Unknown file extension '.{ext}' for {path} — trying BSP loader");
            match bsp::load_bsp(path) {
                Ok(world) => Ok(Some(Level::Bsp(world))),
                Err(bsp::BspLoadError::FileNotFound(p)) => {
                    log::warn!("[Engine] File not found: {p} — starting without map");
                    Ok(None)
                }
                Err(err) => anyhow::bail!("failed to load map: {err}"),
            }
        }
    }
}

fn main() -> Result<()> {
    env_logger::init();
    log::info!("[Engine] Postretro starting");

    let args: Vec<String> = std::env::args().collect();
    let force_line_list = args.iter().any(|a| a == FORCE_LINE_LIST_FLAG);

    let map_path = resolve_map_path(&args);
    let level = load_level(&map_path)?;

    // Position camera inside the level geometry.
    let initial_camera_pos = match &level {
        Some(Level::Prl(world)) => world.spawn_position(),
        _ => Vec3::new(0.0, 200.0, 500.0),
    };

    let event_loop = EventLoop::new().context("failed to create event loop")?;

    let initial_state = InterpolableState::new(initial_camera_pos, 0.0, 0.0);

    let mut app = App {
        renderer: None,
        window_state: None,
        level,
        force_line_list,
        exit_result: Ok(()),
        camera: Camera::new(initial_camera_pos, 0.0, 0.0),
        keys_held: HashSet::new(),
        mouse_delta: (0.0, 0.0),
        frame_timing: FrameTiming::new(initial_state),
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
    /// Loaded level data (BSP or PRL), held for the lifetime of the app.
    level: Option<Level>,
    force_line_list: bool,
    exit_result: Result<()>,

    camera: Camera,
    keys_held: HashSet<KeyCode>,
    /// Accumulated mouse delta since last frame (dx, dy).
    mouse_delta: (f64, f64),
    frame_timing: FrameTiming,
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

        let geometry = self.level.as_ref().map(|lvl| match lvl {
            Level::Bsp(world) => render::LevelGeometry {
                vertices: &world.vertices,
                indices: &world.indices,
                face_ranges: world
                    .face_meta
                    .iter()
                    .map(|f| (f.index_offset, f.index_count))
                    .collect(),
                face_cluster_indices: None,
            },
            Level::Prl(world) => render::LevelGeometry {
                vertices: &world.vertices,
                indices: &world.indices,
                face_ranges: world
                    .face_meta
                    .iter()
                    .map(|f| (f.index_offset, f.index_count))
                    .collect(),
                face_cluster_indices: Some(prl::face_leaf_indices(world)),
            },
        });

        let renderer = match Renderer::new(&window, geometry.as_ref(), self.force_line_list) {
            Ok(r) => r,
            Err(err) => {
                self.exit_result = Err(err);
                event_loop.exit();
                return;
            }
        };

        let size = window.inner_size();
        self.camera.update_aspect(size.width, size.height);

        capture_cursor(&window);

        self.renderer = Some(renderer);
        self.window_state = Some(WindowState { window });
        self.frame_timing.last_frame = Instant::now();

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
                // Fixed-timestep loop: accumulate wall-clock time, tick at
                // constant rate, interpolate for rendering.
                // See: context/lib/rendering_pipeline.md §1
                let now = Instant::now();
                let frame_result = self.frame_timing.begin_frame(now);
                let tick_dt = self.frame_timing.tick_dt();

                // Consume mouse delta once per frame — apply evenly across
                // all ticks this frame to avoid input spikes.
                let mouse_dx = self.mouse_delta.0;
                let mouse_dy = self.mouse_delta.1;
                self.mouse_delta = (0.0, 0.0);

                let ticks = frame_result.ticks;
                let yaw_per_tick = if ticks > 0 {
                    -mouse_dx as f32 * camera::SENSITIVITY / ticks as f32
                } else {
                    0.0
                };
                let pitch_per_tick = if ticks > 0 {
                    -mouse_dy as f32 * camera::SENSITIVITY / ticks as f32
                } else {
                    0.0
                };

                // Run fixed-rate game logic ticks.
                for _ in 0..ticks {
                    self.camera.rotate(yaw_per_tick, pitch_per_tick);

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

                    self.camera.position += move_dir * speed * tick_dt;

                    // Push updated camera state for interpolation.
                    self.frame_timing.push_state(InterpolableState::new(
                        self.camera.position,
                        self.camera.yaw,
                        self.camera.pitch,
                    ));
                }

                // Interpolate between previous and current state for rendering.
                let interp = self.frame_timing.interpolated_state();
                let view_proj = interp.view_projection(self.camera.aspect());

                let (visible, stats) = match self.level.as_ref() {
                    Some(Level::Bsp(world)) => {
                        visibility::determine_visibility(interp.position, view_proj, world)
                    }
                    Some(Level::Prl(world)) => {
                        visibility::determine_prl_visibility(interp.position, view_proj, world)
                    }
                    None => (
                        VisibleFaces::DrawAll,
                        VisibilityStats {
                            camera_leaf: 0,
                            total_faces: 0,
                            pvs_faces: 0,
                            frustum_faces: 0,
                        },
                    ),
                };

                let pos = interp.position;
                let region_label = "leaf";
                log::debug!(
                    "[Diagnostics] {region_label}:{} | faces: {}/{}/{} (total/pvs/frustum) | pos: ({:.0}, {:.0}, {:.0})",
                    stats.camera_leaf,
                    stats.total_faces,
                    stats.pvs_faces,
                    stats.frustum_faces,
                    pos.x,
                    pos.y,
                    pos.z,
                );

                if let Some(ws) = self.window_state.as_ref() {
                    ws.window.set_title(&format!(
                        "Postretro | {region_label}:{} | faces: {}/{}/{} (total/pvs/frustum) | pos: ({:.0}, {:.0}, {:.0})",
                        stats.camera_leaf,
                        stats.total_faces,
                        stats.pvs_faces,
                        stats.frustum_faces,
                        pos.x,
                        pos.y,
                        pos.z,
                    ));
                }

                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.update_view_projection(view_proj);

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
