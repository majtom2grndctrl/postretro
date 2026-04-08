// Postretro engine entry point.
// See: context/lib/rendering_pipeline.md

mod bsp;
mod camera;
mod frame_timing;
mod input;

mod portal_vis;
mod prl;
mod render;
mod visibility;

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use glam::Vec3;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes};

use crate::camera::Camera;
use crate::frame_timing::{FrameTiming, InterpolableState};
use crate::input::{Action, AxisSource};
use crate::render::Renderer;
use crate::visibility::{VisibilityStats, VisibleFaces};

const FORCE_LINE_LIST_FLAG: &str = "--force-line-list";
const DEFAULT_MAP_PATH: &str = "assets/maps/test.bsp";

/// Gamepad look sensitivity: radians per second at full stick deflection.
const GAMEPAD_LOOK_SENSITIVITY: f32 = 2.5;

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
        input_system: input::InputSystem::new(input::default_bindings()),
        gamepad_system: input::gamepad::GamepadSystem::new(),
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
    input_system: input::InputSystem,
    gamepad_system: Option<input::gamepad::GamepadSystem>,
    frame_timing: FrameTiming,
}

struct WindowState {
    window: Arc<Window>,
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

        input::cursor::capture_cursor(&window);

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
                    input::cursor::release_cursor(&ws.window);
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
                    input::cursor::release_cursor(&ws.window);
                }
                log::info!("[Engine] Shutting down");
                event_loop.exit();
            }
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                if let PhysicalKey::Code(code) = key_event.physical_key {
                    self.input_system
                        .handle_keyboard_event(code, key_event.state.is_pressed());
                }
            }
            WindowEvent::MouseInput { button, state, .. } => {
                self.input_system
                    .handle_mouse_button(button, state.is_pressed());
            }
            WindowEvent::Focused(focused) => {
                if let Some(ws) = self.window_state.as_ref() {
                    input::cursor::handle_focus_change(focused, &ws.window);
                    if !focused {
                        self.input_system.clear_all();
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
                let ticks = frame_result.ticks;

                // Poll gamepad before taking the snapshot.
                if let Some(gp) = &mut self.gamepad_system {
                    gp.update(&mut self.input_system);
                }

                // Take a single snapshot for this frame. All ticks read from it.
                let snapshot = self.input_system.snapshot();

                // Pre-compute look deltas from axis values, split by source.
                let look_yaw_values = snapshot.axis(Action::LookYaw);
                let look_pitch_values = snapshot.axis(Action::LookPitch);

                // Run fixed-rate game logic ticks.
                for _ in 0..ticks {
                    // Look: displacement sources (mouse) divided evenly across
                    // ticks; velocity sources (gamepad) multiplied by tick_dt.
                    let mut yaw_delta = 0.0f32;
                    let mut pitch_delta = 0.0f32;

                    for av in look_yaw_values {
                        match av.source {
                            AxisSource::Displacement => {
                                yaw_delta += av.value / ticks as f32;
                            }
                            AxisSource::Velocity => {
                                yaw_delta += av.value * GAMEPAD_LOOK_SENSITIVITY * tick_dt;
                            }
                        }
                    }
                    for av in look_pitch_values {
                        match av.source {
                            AxisSource::Displacement => {
                                pitch_delta += av.value / ticks as f32;
                            }
                            AxisSource::Velocity => {
                                pitch_delta += av.value * GAMEPAD_LOOK_SENSITIVITY * tick_dt;
                            }
                        }
                    }

                    self.camera.rotate(yaw_delta, pitch_delta);

                    // Movement from action snapshot.
                    let forward_axis = snapshot.axis_value(Action::MoveForward);
                    let right_axis = snapshot.axis_value(Action::MoveRight);
                    let up_axis = snapshot.axis_value(Action::MoveUp);
                    let sprint = snapshot.button(Action::Sprint).is_active();

                    let speed = if sprint {
                        camera::MOVE_SPEED * camera::SPRINT_MULTIPLIER
                    } else {
                        camera::MOVE_SPEED
                    };

                    let forward = self.camera.forward();
                    let right = self.camera.right();
                    let mut move_dir =
                        forward * forward_axis + right * right_axis + Vec3::Y * up_axis;

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
            self.input_system.handle_mouse_delta(delta.0, delta.1);
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
