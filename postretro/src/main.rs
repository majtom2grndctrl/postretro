// Postretro engine entry point.
// See: context/lib/rendering_pipeline.md

mod bsp;
mod camera;
mod frame_timing;
mod input;
mod material;

mod portal_vis;
mod prl;
mod render;
mod texture;
mod visibility;

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use glam::Vec3;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes};

use crate::camera::Camera;
use crate::frame_timing::{FrameRateMeter, FrameTiming, InterpolableState};
use crate::input::{Action, DiagnosticAction};
use crate::render::Renderer;
use crate::texture::TextureSet;
use crate::visibility::{DrawRange, VisibilityPath, VisibilityStats, VisibleFaces};

const DEFAULT_MAP_PATH: &str = "assets/maps/test.bsp";

/// Loaded level data: either BSP or PRL format.
enum Level {
    Bsp(bsp::BspWorld),
    Prl(prl::LevelWorld),
}

fn resolve_map_path(args: &[String]) -> String {
    args.iter()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| DEFAULT_MAP_PATH.to_string())
}

/// Resolve the texture root directory from a map file path.
/// For `assets/maps/test.bsp`, the texture root is `assets/textures/`.
/// Navigates up from the map file to the asset root (parent of `maps/`),
/// then appends `textures/`.
fn resolve_texture_root(map_path: &str) -> std::path::PathBuf {
    let map_dir = Path::new(map_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    // Go up one level from the maps directory to get the asset root.
    let asset_root = map_dir.parent().unwrap_or_else(|| Path::new("."));
    asset_root.join("textures")
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

    let map_path = resolve_map_path(&args);
    let mut level = load_level(&map_path)?;

    // Load textures for BSP and PRL levels.
    let texture_set = match &level {
        Some(Level::Bsp(world)) => {
            let texture_root = resolve_texture_root(&map_path);
            log::info!("[Engine] Loading textures from {}", texture_root.display());
            let texture_names = build_texture_names_from_face_meta(&world.face_meta);
            Some(texture::load_textures(&texture_names, &texture_root))
        }
        Some(Level::Prl(world)) if !world.texture_names.is_empty() => {
            let texture_root = resolve_texture_root(&map_path);
            log::info!(
                "[Engine] Loading PRL textures from {}",
                texture_root.display()
            );
            let texture_names: Vec<Option<String>> = world
                .texture_names
                .iter()
                .map(|n| Some(n.clone()))
                .collect();
            Some(texture::load_textures(&texture_names, &texture_root))
        }
        _ => None,
    };

    // Normalize PRL UVs after texture dimensions are known.
    if let (Some(Level::Prl(world)), Some(tex_set)) = (&mut level, &texture_set) {
        normalize_prl_uvs(world, tex_set);
    }

    // Position camera inside the level geometry.
    let initial_camera_pos = match &level {
        Some(Level::Prl(world)) => world.spawn_position(),
        _ => Vec3::new(0.0, 200.0, 500.0),
    };

    let event_loop = EventLoop::new().context("failed to create event loop")?;

    let initial_state = InterpolableState::new(initial_camera_pos);

    let mut app = App {
        renderer: None,
        window_state: None,
        level,
        texture_set,
        exit_result: Ok(()),
        camera: Camera::new(initial_camera_pos, 0.0, 0.0),
        input_system: input::InputSystem::new(input::default_bindings()),
        gamepad_system: input::gamepad::GamepadSystem::new(),
        frame_timing: FrameTiming::new(initial_state),
        diagnostic_inputs: input::DiagnosticInputs::new(input::default_diagnostic_chords()),
        capture_portal_walk_next_frame: false,
        scratch_ranges: Vec::new(),
        frame_rate_meter: FrameRateMeter::new(),
        title_buffer: String::with_capacity(256),
        last_title_update: Instant::now(),
    };

    event_loop
        .run_app(&mut app)
        .context("event loop terminated with error")?;

    app.exit_result
}

/// Build a texture names list indexed by BSP miptexture index from face_meta.
/// Each unique texture_index maps to its texture_name. Missing indices get `None`.
fn build_texture_names_from_face_meta(face_meta: &[bsp::FaceMeta]) -> Vec<Option<String>> {
    let max_tex_idx = face_meta
        .iter()
        .filter_map(|f| f.texture_index)
        .max()
        .unwrap_or(0) as usize;

    let mut names = vec![None; max_tex_idx + 1];
    for face in face_meta {
        if let Some(idx) = face.texture_index {
            let idx = idx as usize;
            if idx < names.len() && names[idx].is_none() && !face.texture_name.is_empty() {
                names[idx] = Some(face.texture_name.clone());
            }
        }
    }
    names
}

/// Normalize PRL texel-space UVs by dividing by texture dimensions.
/// Called after `load_textures()` provides actual dimensions.
/// Iterates faces, collects unique vertex indices, normalizes each vertex exactly once.
fn normalize_prl_uvs(world: &mut prl::LevelWorld, texture_set: &TextureSet) {
    let mut normalized = vec![false; world.vertices.len()];

    for face in &world.face_meta {
        let tex_idx = match face.texture_index {
            Some(idx) => idx as usize,
            None => continue,
        };
        let (w, h) = match texture_set.textures.get(tex_idx) {
            Some(tex) => (tex.width, tex.height),
            None => continue,
        };
        if w == 0 || h == 0 {
            continue;
        }

        let start = face.index_offset as usize;
        let count = face.index_count as usize;
        for i in start..start + count {
            if let Some(&idx) = world.indices.get(i) {
                let vi = idx as usize;
                if vi < normalized.len() && !normalized[vi] {
                    if let Some(vert) = world.vertices.get_mut(vi) {
                        vert.base_uv[0] /= w as f32;
                        vert.base_uv[1] /= h as f32;
                        normalized[vi] = true;
                    }
                }
            }
        }
    }
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
    /// CPU-side textures loaded from disk, consumed by renderer during init.
    texture_set: Option<TextureSet>,
    exit_result: Result<()>,

    camera: Camera,
    input_system: input::InputSystem,
    gamepad_system: Option<input::gamepad::GamepadSystem>,
    frame_timing: FrameTiming,

    /// Diagnostic chord resolver. Parallel to `input_system`; consumes the
    /// same key events but produces engine debug actions, not gameplay
    /// actions. See: context/lib/input.md §7
    diagnostic_inputs: input::DiagnosticInputs,

    /// One-shot flag set by the `DumpPortalWalk` diagnostic chord. The next
    /// redraw consumes it, passes it into `determine_prl_visibility`, and
    /// clears it. The visibility module emits per-portal trace lines under
    /// the `postretro::portal_trace` log target for that one frame only.
    capture_portal_walk_next_frame: bool,

    /// Persistent scratch buffer for per-frame `DrawRange` collection. The
    /// BSP and PRL visibility entry points `std::mem::take` this into
    /// `VisibleFaces::Culled` so no allocation happens in steady state. After
    /// `render_frame` consumes the `VisibleFaces`, the buffer is moved back
    /// into this field with its capacity intact. BSP and PRL cannot be active
    /// on the same frame, so a single scratch suffices.
    scratch_ranges: Vec<DrawRange>,

    /// Rolling ring buffer of per-frame CPU work durations. Sampled every
    /// frame, read at title-update cadence. Reports min/avg/max so hitches
    /// don't vanish into the average. See `frame_timing::FrameRateMeter`.
    frame_rate_meter: FrameRateMeter,

    /// Persistent string used to build the window title each update. Cleared
    /// and rewritten with `write!` to avoid the per-frame allocation a
    /// `format!` would do. Owns its capacity across frames.
    title_buffer: String,

    /// Last time the window title was written. Title updates are rate-limited
    /// to ~4Hz — at 60fps the title would otherwise flicker unreadably and
    /// the OS may throttle rapid `set_title` calls.
    last_title_update: Instant,
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

        // Build geometry for the renderer.
        let geometry = match &self.level {
            Some(Level::Bsp(world)) => Some(render::LevelGeometry {
                vertices: &world.vertices,
                indices: &world.indices,
                leaf_texture_sub_ranges: world
                    .leaves
                    .iter()
                    .map(|l| l.texture_sub_ranges.clone())
                    .collect(),
            }),
            Some(Level::Prl(world)) => Some(render::LevelGeometry {
                vertices: &world.vertices,
                indices: &world.indices,
                leaf_texture_sub_ranges: world
                    .leaves
                    .iter()
                    .map(|l| l.texture_sub_ranges.clone())
                    .collect(),
            }),
            None => None,
        };

        let renderer = match Renderer::new(&window, geometry.as_ref(), self.texture_set.as_ref()) {
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
                    let pressed = key_event.state.is_pressed();

                    // Diagnostic chord resolver runs first. It owns modifier
                    // tracking for the Alt+Shift+ namespace and emits a
                    // diagnostic action only on a clean rising edge.
                    if let Some(action) =
                        self.diagnostic_inputs
                            .handle_key(code, pressed, key_event.repeat)
                    {
                        self.handle_diagnostic_action(action);
                    }

                    self.input_system.handle_keyboard_event(code, pressed);
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
                        self.diagnostic_inputs.clear_modifiers();
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
                let frame_dt = frame_result.frame_dt;
                let ticks = frame_result.ticks;

                // Poll gamepad before taking the snapshot.
                if let Some(gp) = &mut self.gamepad_system {
                    gp.update(&mut self.input_system);
                }

                // drain_look_inputs() must precede snapshot(); both touch
                // mouse_axes and look state belongs to the render-rate path.
                let look = self.input_system.drain_look_inputs();
                let snapshot = self.input_system.snapshot();

                // Apply look rotation once per render frame, before the tick
                // loop. Doing this at render rate (not tick rate) means mouse
                // motion on zero-tick frames is preserved — the bug that
                // motivated decouple-view-from-sim.
                self.camera
                    .rotate(look.yaw_delta(frame_dt), look.pitch_delta(frame_dt));

                // Run fixed-rate game logic ticks.
                for _ in 0..ticks {
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
                    self.frame_timing
                        .push_state(InterpolableState::new(self.camera.position));
                }

                // Interpolate between previous and current state for rendering.
                // Position comes from the tick-state slots; yaw/pitch come from
                // `self.camera` directly so zero-tick frames still reflect this
                // frame's look input.
                let interp = self.frame_timing.interpolated_state();
                let view_proj = interp.view_projection(
                    self.camera.aspect(),
                    self.camera.yaw,
                    self.camera.pitch,
                );

                let capture_portal_walk = std::mem::take(&mut self.capture_portal_walk_next_frame);

                let (visible, stats) = match self.level.as_ref() {
                    Some(Level::Bsp(world)) => visibility::determine_visibility(
                        interp.position,
                        view_proj,
                        world,
                        &mut self.scratch_ranges,
                    ),
                    Some(Level::Prl(world)) => visibility::determine_prl_visibility(
                        interp.position,
                        view_proj,
                        world,
                        capture_portal_walk,
                        &mut self.scratch_ranges,
                    ),
                    None => (
                        VisibleFaces::DrawAll,
                        VisibilityStats {
                            camera_leaf: 0,
                            total_faces: 0,
                            pvs_reach: 0,
                            drawn_faces: 0,
                            path: VisibilityPath::EmptyWorldFallback,
                        },
                    ),
                };

                let pos = interp.position;
                let region_label = "leaf";
                let path_label = match stats.path {
                    VisibilityPath::BspPvs => "bsp-pvs",
                    VisibilityPath::PrlPvs => "prl-pvs",
                    VisibilityPath::PrlPortal { .. } => "prl-portal",
                    VisibilityPath::NoPvsFallback => "no-pvs",
                    VisibilityPath::EmptyWorldFallback => "empty",
                    VisibilityPath::SolidLeafFallback => "solid-leaf",
                };
                let walk_reach_col = match stats.walk_reach() {
                    Some(walk) => format!(" walk:{walk}"),
                    None => String::new(),
                };
                log::debug!(
                    "[Diagnostics] {region_label}:{} path:{path_label} | draw:{} pvs:{} all:{}{walk_reach_col} | pos: ({:.0}, {:.0}, {:.0})",
                    stats.camera_leaf,
                    stats.drawn_faces,
                    stats.pvs_reach,
                    stats.total_faces,
                    pos.x,
                    pos.y,
                    pos.z,
                );

                // Window title update is rate-limited to ~4Hz; the sample
                // recording below happens every frame. A 60Hz title is
                // unreadable and the OS may throttle rapid `set_title`.
                //
                // The vsync state must always be visible in the title so
                // the diagnostic toggle's effect is self-evident. The
                // `vsync:on|off` segment sits adjacent to `frame:` because
                // they're read together, and in a fixed position so it's
                // grep-able (the label is always present, not only in
                // one state).
                let vsync_label = self
                    .renderer
                    .as_ref()
                    .map(|r| if r.vsync_enabled() { "on" } else { "off" });
                if let Some(ws) = self.window_state.as_ref() {
                    if self.last_title_update.elapsed() >= Duration::from_millis(250) {
                        self.last_title_update = Instant::now();
                        self.title_buffer.clear();
                        // Full rewrite into the persistent buffer — no
                        // intermediate `format!` allocation, even on the
                        // update frames.
                        let _ = write!(
                            &mut self.title_buffer,
                            "Postretro | {region_label}:{} path:{path_label} | draw:{} pvs:{} all:{}{walk_reach_col} | pos: ({:.0}, {:.0}, {:.0})",
                            stats.camera_leaf,
                            stats.drawn_faces,
                            stats.pvs_reach,
                            stats.total_faces,
                            pos.x,
                            pos.y,
                            pos.z,
                        );
                        if let Some(label) = vsync_label {
                            let _ = write!(&mut self.title_buffer, " | vsync:{label}");
                        }
                        if let Some(ft) = self.frame_rate_meter.stats() {
                            let _ = write!(
                                &mut self.title_buffer,
                                " frame: {:.1}/{:.1}/{:.1} ms",
                                ft.min_ms, ft.avg_ms, ft.max_ms,
                            );
                        }
                        ws.window.set_title(&self.title_buffer);
                    }
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

                // Reclaim the draw-range buffer from `visible` so its
                // capacity persists across frames. Visibility entry points
                // `std::mem::take` the scratch into `VisibleFaces::Culled`;
                // reclaiming here is the other half of that contract, making
                // the "no per-frame allocation" invariant hold in steady
                // state without depending on render.rs internals.
                if let VisibleFaces::Culled(mut ranges) = visible {
                    ranges.clear();
                    self.scratch_ranges = ranges;
                }

                // Record the CPU-side frame span. Measured from the `now`
                // captured at the top of the handler to this point — covers
                // input polling, fixed-timestep game logic, visibility
                // determination, title update, render, and scratch reclaim.
                // Wall-clock tick-to-tick is useless under vsync (pinned to
                // ~16.6ms regardless of work); this span shows actual load.
                // See: context/lib/rendering_pipeline.md §1 (frame ordering)
                let frame_cpu = Instant::now().duration_since(now);
                self.frame_rate_meter.record(frame_cpu);
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

impl App {
    /// Dispatch a diagnostic action emitted by the chord resolver.
    /// See: context/lib/input.md §7
    fn handle_diagnostic_action(&mut self, action: DiagnosticAction) {
        match action {
            DiagnosticAction::ToggleWireframe => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.cycle_wireframe_mode();
                }
            }
            DiagnosticAction::DumpPortalWalk => {
                self.capture_portal_walk_next_frame = true;
                log::info!(
                    target: "postretro::portal_trace",
                    "[portal_trace] capture armed for next frame",
                );
            }
            DiagnosticAction::ToggleVsync => {
                if let Some(renderer) = self.renderer.as_mut() {
                    let enabled = renderer.toggle_vsync();
                    // Stale frametime samples would keep the title pinned
                    // to pre-toggle numbers for up to two seconds — exactly
                    // when the user is staring at it to see what changed.
                    self.frame_rate_meter.clear();
                    log::info!("[Renderer] vsync {}", if enabled { "on" } else { "off" },);
                }
            }
        }
    }
}

// --- Tests ---
//
// Regression pins for the decouple-view-from-sim fix. The original bug silently
// lost mouse motion on zero-tick frames: `InputSystem::snapshot()` drained the
// accumulated mouse delta, but the tick loop never applied look rotation when
// `ticks == 0`, so the delta vanished. The fix routes look rotation through
// `drain_look_inputs()` at render rate and feeds yaw/pitch into
// `InterpolableState::view_projection` as arguments rather than baking them
// into the tick state. These tests assert on both `camera.yaw` *and* the
// rendered `view_projection` matrix because checking yaw alone would not have
// caught the original rendering staleness.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_timing::TICK_DURATION;
    use crate::input::{InputSystem, default_bindings};

    /// Epsilon for angle and matrix-element comparisons. Mouse-driven yaw
    /// deltas at default sensitivity land around 1e-1 radians, so 1e-5 is
    /// comfortably tight without being flaky on f32 round-off.
    const EPSILON: f32 = 1e-5;

    /// Regression: mouse motion accumulated in a frame that produces zero
    /// ticks must still rotate the camera *and* must change the rendered
    /// view-projection matrix. Asserting only on `camera.yaw` would have
    /// passed against the original bug — rendering read through an
    /// `InterpolableState::view_projection` that ignored the updated yaw.
    #[test]
    fn mouse_delta_applied_on_zero_tick_frame() {
        let mut sys = InputSystem::new(default_bindings());
        let mut camera = Camera::new(Vec3::ZERO, 0.0, 0.0);

        // Accumulate a large horizontal mouse delta. At default sensitivity
        // (0.002 rad/unit) and scale -1.0 this produces yaw_displacement
        // of -0.2 radians — well above EPSILON.
        sys.handle_mouse_delta(100.0, 0.0);
        let look = sys.drain_look_inputs();

        // A 5ms elapsed frame is well below the 16.667ms tick duration, so
        // the accumulator produces zero ticks but still reports a positive
        // frame_dt. This is the exact shape of the frame that triggered the
        // original bug.
        let initial_state = InterpolableState::new(Vec3::ZERO);
        let mut timing = FrameTiming::new(initial_state);
        let result = timing.accumulate(Duration::from_millis(5));
        assert_eq!(result.ticks, 0, "5ms elapsed must not produce a tick");
        assert!(
            result.frame_dt > 0.0,
            "frame_dt must be positive on a non-zero elapsed frame",
        );

        // Mirror production: rotate once per render frame, before the (here
        // absent) tick loop.
        camera.rotate(
            look.yaw_delta(result.frame_dt),
            look.pitch_delta(result.frame_dt),
        );

        // Camera yaw must reflect the mouse motion.
        assert!(
            camera.yaw.abs() > EPSILON,
            "camera.yaw should have changed from 0.0, got {}",
            camera.yaw,
        );

        // View-projection assertion — the load-bearing check. Build the
        // baseline matrix with yaw/pitch = 0 and the post-rotation matrix
        // with the camera's actual yaw/pitch. Position is identical in both
        // cases, so any element-wise difference must come from the rotation.
        let render_state = InterpolableState::new(Vec3::ZERO);
        let aspect = camera.aspect();
        let baseline = render_state.view_projection(aspect, 0.0, 0.0);
        let rotated = render_state.view_projection(aspect, camera.yaw, camera.pitch);

        let baseline_cols = baseline.to_cols_array();
        let rotated_cols = rotated.to_cols_array();
        let any_differs = baseline_cols
            .iter()
            .zip(rotated_cols.iter())
            .any(|(a, b)| (a - b).abs() > EPSILON);
        assert!(
            any_differs,
            "view_projection must differ after applying mouse-driven yaw; \
             baseline={:?} rotated={:?}",
            baseline_cols, rotated_cols,
        );
    }

    /// Regression: on a multi-tick frame, look rotation must be applied
    /// exactly once (at render rate), not once per tick. Applying it in the
    /// tick loop would multiply the delta by `ticks` and send the view
    /// spinning.
    #[test]
    fn mouse_delta_not_multiplied_on_multi_tick_frame() {
        let mut sys = InputSystem::new(default_bindings());
        let mut camera = Camera::new(Vec3::ZERO, 0.0, 0.0);

        sys.handle_mouse_delta(100.0, 0.0);
        let look = sys.drain_look_inputs();

        // Force exactly 3 ticks by advancing the accumulator by 3 * TICK_DURATION.
        let initial_state = InterpolableState::new(Vec3::ZERO);
        let mut timing = FrameTiming::new(initial_state);
        let result = timing.accumulate(TICK_DURATION * 3);
        assert_eq!(result.ticks, 3, "TICK_DURATION * 3 must produce 3 ticks");

        // Production code rotates once before the tick loop and never inside
        // it. Mirror that: one rotation call, regardless of tick count.
        camera.rotate(
            look.yaw_delta(result.frame_dt),
            look.pitch_delta(result.frame_dt),
        );

        // The expected yaw is the single-application delta. Compute it the
        // same way the production code does on a fresh system to avoid
        // analytic drift from the binding table.
        let mut reference_sys = InputSystem::new(default_bindings());
        reference_sys.handle_mouse_delta(100.0, 0.0);
        let reference_look = reference_sys.drain_look_inputs();
        let expected_yaw = reference_look.yaw_delta(result.frame_dt);

        assert!(
            (camera.yaw - expected_yaw).abs() < EPSILON,
            "camera.yaw should equal single-application delta {} (not 3x), got {}",
            expected_yaw,
            camera.yaw,
        );
    }
}
