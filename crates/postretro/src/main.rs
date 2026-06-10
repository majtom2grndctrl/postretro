// Postretro engine entry point, boot state machine, and level-load orchestration.
// See: context/lib/boot_sequence.md §3 · context/lib/index.md

mod audio;
mod camera;
mod collision;
mod compute_cull;
mod frame_timing;
mod fx;
mod geometry;
mod input;
mod lighting;
mod material;
mod model;
mod movement;
mod options;
mod weapon;

mod portal_vis;
mod prl;
mod render;
mod scripting;
mod shadow_cull;
mod startup;
mod ui_texture;
mod visibility;

// Rooted here (not under `scripting/`) so `gen_script_types.rs` can reuse the
// `scripting` tree via `#[path]` without pulling in wgpu/engine-dependent code.
#[path = "scripting/systems/mod.rs"]
mod scripting_systems;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
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
use crate::input::{Action, ButtonState, DiagnosticAction, InputFocus};
use crate::render::Renderer;
use crate::scripting::builtins::{
    ClassnameDispatch, PLAYER_START_CLASSNAME, apply_classname_dispatch,
    apply_data_archetype_dispatch, register_builtins as register_builtin_classnames,
    spawn_from_player_starts,
};
use crate::scripting::ctx::ScriptCtx;
use crate::scripting::primitives::light::register_sequenced_light_primitives;
use crate::scripting::primitives::register_all;
use crate::scripting::primitives_registry::PrimitiveRegistry;
use crate::scripting::reaction_dispatch::{
    ProgressTracker, fire_named_event, fire_named_event_with_sequences,
    validate_sequence_primitives,
};
use crate::scripting::reactions::registry::{
    ReactionPrimitiveRegistry, register_emitter_reaction_primitives,
    register_fog_reaction_primitives, register_sequenced_fog_primitives,
};
use crate::scripting::runtime::{ReloadSummary, ScriptRuntime, ScriptRuntimeConfig};
use crate::scripting::sequence::SequencedPrimitiveRegistry;
use crate::scripting::state_persistence::{
    STATE_FILE_PATH, StateStoreLifecycle, collect_persisted_state, load_persisted_state,
    overlay_persisted_state, save_persisted_state,
};
use crate::startup::{BootState, LoadOutcome, SplashSource, StartupTimings, spawn_level_worker};
use crate::visibility::{VisibilityPath, VisibilityResult, VisibilityStats, VisibleCells};

const DEFAULT_MAP_PATH: &str = "content/dev/maps/campaign-test.prl";

fn resolve_map_path(args: &[String]) -> String {
    args.iter()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| DEFAULT_MAP_PATH.to_string())
}

fn content_root_from_map(map_path: &str) -> PathBuf {
    // `Path::new("maps/test.prl").parent()` returns `Some("maps")`, and
    // `"maps".parent()` returns `Some("")` — an empty path, not `None`. Filter
    // out the empty case so the `unwrap_or` fallback to `"."` actually fires.
    Path::new(map_path)
        .parent()
        .and_then(|maps_dir| maps_dir.parent())
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf()
}

/// Collect the distinct, non-empty `MeshComponent.model` handles currently in
/// the registry, preserving first-seen order. GPU-free: this is the pure half of
/// the level-load model sweep — the renderer's GPU upload happens in the caller,
/// once per returned handle, so each distinct model is uploaded exactly once.
///
/// Empty handles are skipped: a `prop_mesh` with an absent/empty `model` logs a
/// warning at spawn time and renders nothing; there is nothing to upload for it.
/// Each returned string is the VERBATIM renderer cache key — it matches the
/// per-frame draw planner's `ModelHandle` (built from the same `mesh.model`).
/// `load_skinned_model` caches under this string but opens the glTF from
/// `content_root.join(handle)`, so the caller passes both the handle and the
/// content root (open path and cache key are deliberately decoupled).
fn distinct_mesh_models(registry: &crate::scripting::registry::EntityRegistry) -> Vec<String> {
    use crate::scripting::registry::{ComponentKind, ComponentValue};

    let mut seen = std::collections::HashSet::new();
    let mut ordered = Vec::new();
    for (_id, value) in registry.iter_with_kind(ComponentKind::Mesh) {
        let ComponentValue::Mesh(mesh) = value else {
            continue;
        };
        if mesh.model.is_empty() {
            continue;
        }
        if seen.insert(mesh.model.clone()) {
            ordered.push(mesh.model.clone());
        }
    }
    ordered
}

// Policy chokepoint: the frame loop queues a staged build only when a changed
// path matched the active mod-init dependency set (classified by ScriptRuntime).
fn reload_summary_requires_mod_init(summary: ReloadSummary) -> bool {
    summary.mod_init
}

/// Version/tagline line the boot splash's shaped-text element renders. Sourced
/// from the build's `CARGO_PKG_VERSION` (the simpler of the two options the plan
/// leaves open) so the read-handle snapshot carries a real value. Flows through
/// the `UiReadSnapshot`; the descriptor seam stays intact for Goal B/G1.
fn splash_version_line() -> String {
    format!("postretro v{}", env!("CARGO_PKG_VERSION"))
}

fn main() -> Result<()> {
    env_logger::init();
    log::info!("[Engine] Postretro starting");

    // Timing starts at process entry so the first stage captures the
    // args_parsed → wgpu_init gap. See `StartupTimings` doc comment for
    // the per-line stage layout.
    let mut boot_timings = StartupTimings::new();

    let args: Vec<String> = std::env::args().collect();

    let map_path = resolve_map_path(&args);
    let content_root = content_root_from_map(&map_path);
    log::info!("[Engine] Content root: {}", content_root.display());
    boot_timings.record("args_parsed");

    // Camera starts at a placeholder; `install_level_payload` repositions it
    // to the first `player_spawn` or the level geometry center
    // (`spawn_position()`) when no player start exists.
    let initial_camera_pos = Vec3::new(0.0, 200.0, 500.0);

    let initial_state = InterpolableState::new(initial_camera_pos);

    // Scripting bootstrap: primitive registry, runtime construction, and SDK type emission.
    // See: context/lib/scripting.md
    let script_ctx = ScriptCtx::new();
    let mut script_registry = PrimitiveRegistry::new();
    register_all(&mut script_registry, script_ctx.clone());
    let script_runtime = ScriptRuntime::new(
        &script_registry,
        &ScriptRuntimeConfig::default(),
        &script_ctx,
    )
    .context("failed to construct script runtime")?;
    // See `ScriptRuntime::new` for why this runs here rather than in the constructor.
    // See: context/lib/scripting.md §7.
    crate::scripting::typedef::emit_sdk_types_in_debug(&script_registry);
    boot_timings.record("script_runtime_ctor");

    let event_loop = EventLoop::new().context("failed to create event loop")?;
    boot_timings.record("event_loop_created");

    // Rust-only handlers on the sequence-dispatch path — distinct from the
    // script-facing primitive registry (these never run inside QuickJS/Luau).
    let mut sequence_registry = SequencedPrimitiveRegistry::new();
    register_sequenced_light_primitives(&mut sequence_registry, script_ctx.clone());
    register_sequenced_fog_primitives(&mut sequence_registry, script_ctx.clone());

    // Reaction-primitive handlers invoked by name when a `Primitive` reaction
    // fires. Populated once at startup; survives level reloads.
    let mut reaction_registry = ReactionPrimitiveRegistry::new();
    register_emitter_reaction_primitives(&mut reaction_registry);
    register_fog_reaction_primitives(&mut reaction_registry);

    // Built-in classname dispatch — survives level unload because handlers
    // describe engine types, not per-level state. See: context/lib/scripting.md
    let mut classname_dispatch = ClassnameDispatch::new();
    register_builtin_classnames(&mut classname_dispatch);

    // Player options load before `InputSystem` is constructed so the loaded
    // look preferences seed input at startup. On first boot (no file present),
    // write defaults so the human gets an editable starting file — the only
    // `save` call until the M13 settings menu lands. Missing config dir or a
    // save failure is logged, not fatal: boot proceeds on in-memory defaults.
    // See: context/lib/player_options.md §3
    let settings_path = options::settings_path();
    let player_options = match &settings_path {
        Some(path) => {
            // `load` returns defaults for both missing and malformed files, so
            // detect first-run by probing existence before loading. A malformed
            // file exists, so it is never overwritten here.
            let existed = path.exists();
            let options = options::PlayerOptions::load(path);
            if !existed {
                match options.save(path) {
                    Ok(()) => log::info!(
                        "[Options] no settings file found; wrote defaults to {}",
                        path.display()
                    ),
                    Err(err) => log::warn!(
                        "[Options] failed to write default settings to {}: {err}; \
                         running on in-memory defaults",
                        path.display()
                    ),
                }
            }
            options
        }
        None => {
            log::warn!(
                "[Options] no platform config directory; running on in-memory \
                 defaults without persistence"
            );
            options::PlayerOptions::default()
        }
    };

    // Mod init, hot-reload watcher start, and the level-load worker spawn
    // are all deferred to the splash frame loop so the first splash frame
    // paints before any of those run — the user sees pixels before any
    // mod-supplied work executes.
    // See: context/lib/boot_sequence.md §8

    let mut input_system = input::InputSystem::new(input::default_bindings());
    input_system.set_mouse_sensitivity(player_options.mouse_sensitivity);
    input_system.set_invert_y(player_options.invert_y);

    let mut app = App {
        renderer: None,
        audio: None,
        window_state: None,
        level: None,
        map_path,
        content_root,
        exit_result: Ok(()),
        camera: Camera::new(initial_camera_pos, 0.0, 0.0),
        input_system,
        gameplay_input_latch: input::GameplayInputLatch::new(),
        crouch_toggle_active: false,
        player_options,
        settings_path,
        input_focus: InputFocus::Gameplay,
        ui_dispatch: input::UiDispatch::new(),
        gamepad_system: input::gamepad::GamepadSystem::new(),
        frame_timing: FrameTiming::new(initial_state),
        diagnostic_inputs: input::DiagnosticInputs::new(input::default_diagnostic_chords()),
        capture_portal_walk_next_frame: false,
        scratch_cells: Vec::new(),
        frame_rate_meter: FrameRateMeter::new(),
        title_buffer: String::with_capacity(256),
        last_title_update: Instant::now(),
        script_runtime,
        script_ctx,
        state_store_lifecycle: StateStoreLifecycle::default(),
        sequence_registry,
        reaction_registry,
        progress_tracker: ProgressTracker::new(),
        classname_dispatch,
        light_bridge: scripting_systems::light_bridge::LightBridge::new(),
        fog_volume_bridge: scripting_systems::fog_volume_bridge::FogVolumeBridge::new(),
        emitter_bridge: scripting_systems::emitter_bridge::EmitterBridge::new(),
        particle_live_counts: std::collections::HashMap::new(),
        collision_world: collision::CollisionWorld::new(),
        particle_render: scripting_systems::particle_render::ParticleRenderCollector::new(),
        mesh_render: scripting_systems::mesh_render::MeshRenderCollector::new(),
        active_wieldable: None,
        active_wieldable_descriptor: None,
        builtin_handled: None,
        pending_spawn_points: None,
        pending_map_entities: None,
        script_time: 0.0,
        boot_state: BootState::Booting,
        splash_frame: 0,
        pending_level_log: false,
        pending_splash_override: None,
        boot_timings,
        mod_timings: StartupTimings::new(),
        level_timings: StartupTimings::new(),
        level_rx: None,
        level_worker: None,
        #[cfg(feature = "dev-tools")]
        debug_ui: None,
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

/// Resolve the per-tick crouch intent bit from `crouch_mode` and the current
/// `Action::Crouch` button state, advancing the persistent toggle `latch`.
///
/// This is the input-layer toggle-vs-hold resolution (extracted as a free
/// function so the latch/edge logic is unit-testable; the call site in the
/// movement-tick assembly calls it with `&mut self.crouch_toggle_active`):
///   - `Hold`: the intent tracks the button LEVEL (`Pressed | Held`); the latch
///     is left untouched and is inert in this mode.
///   - `Toggle`: a `ButtonState::Pressed` RISING EDGE flips the latch; the
///     latched value is returned. One press latches on, the next latches off.
///
/// The returned bit is the only thing the movement intent ever sees — never the
/// raw button or the mode (the toggle-vs-hold ownership rule).
fn resolve_crouch_intent(mode: options::CrouchMode, button: ButtonState, latch: &mut bool) -> bool {
    match mode {
        options::CrouchMode::Hold => button.is_active(),
        options::CrouchMode::Toggle => {
            if matches!(button, ButtonState::Pressed) {
                *latch = !*latch;
            }
            *latch
        }
    }
}

// --- Application state ---

struct App {
    renderer: Option<Renderer>,

    /// Audio subsystem. `None` until `resumed()` builds it after the renderer,
    /// and stays `None` if kira init fails — the game then runs silent.
    /// See: context/lib/audio.md
    audio: Option<audio::Audio>,

    window_state: Option<WindowState>,
    level: Option<prl::LevelWorld>,

    /// Map path resolved from CLI args. Handed to the level-load worker
    /// when it is spawned during the second splash frame.
    map_path: String,

    /// Derived from the map path at startup. `textures/` and `scripts/`
    /// sibling directories are resolved relative to this root.
    content_root: PathBuf,

    exit_result: Result<()>,

    camera: Camera,
    input_system: input::InputSystem,
    gameplay_input_latch: input::GameplayInputLatch,

    /// Persistent crouch toggle latch for `CrouchMode::Toggle`. Flipped on each
    /// `Action::Crouch` press rising edge by the input layer; fed into
    /// `MovementInput::crouch_intent`. Lives on `App` (the input layer), NEVER on
    /// the movement component. Inert in `CrouchMode::Hold` (hold tracks the
    /// button level directly). See: context/lib/input.md, context/lib/player_options.md
    crouch_toggle_active: bool,

    /// Per-human runtime preferences loaded at boot. Seeds input look
    /// preferences during init; `crouch_mode` is read each input tick by
    /// `resolve_crouch_intent`. Settings-menu UI (M13) remains future.
    /// See: context/lib/player_options.md
    player_options: options::PlayerOptions,

    /// Resolved `settings.toml` path, or `None` when the platform exposes no
    /// config directory (the engine then runs on in-memory defaults). Held for
    /// the future M13 settings menu's save path; no reader yet.
    /// See: context/lib/player_options.md
    #[allow(dead_code)]
    settings_path: Option<PathBuf>,

    /// Coarse owner of keyboard/mouse focus. Drives pointer-lock acquire/release
    /// via `set_input_focus`. See: context/lib/input.md
    input_focus: InputFocus,

    /// Input-stage UI-dispatch seam: decides whether an event is consumed by the
    /// UI layer (capture) or forwarded to gameplay (passthrough), ahead of the
    /// gameplay input forward. Goal A leaves the mode at its inert `Passthrough`
    /// default (no live UI descriptor yet), so the seam does not change gameplay
    /// forwarding; Task 4 sources the mode from the active splash descriptor.
    /// Captured events cross to game logic no earlier than the next frame
    /// (N→N+1). `InputFocus::Menu` is the intended structural home for capture;
    /// Goal A makes no live focus change. See: context/lib/input.md
    ui_dispatch: input::UiDispatch,
    gamepad_system: Option<input::gamepad::GamepadSystem>,
    frame_timing: FrameTiming,

    /// Parallel to `input_system`; same key events, debug actions only.
    /// See: context/lib/input.md §7
    diagnostic_inputs: input::DiagnosticInputs,

    /// One-shot flag: set by `DumpPortalWalk`, consumed and cleared on the
    /// next redraw. Visibility emits per-portal traces under
    /// `postretro::portal_trace` for that one frame only.
    capture_portal_walk_next_frame: bool,

    scratch_cells: Vec<u32>,

    /// Ring buffer of per-frame CPU durations. Reports min/avg/max so
    /// hitches don't vanish into the average.
    frame_rate_meter: FrameRateMeter,

    /// Reused across frames to avoid a per-frame `format!` allocation.
    title_buffer: String,

    /// Rate-limits title writes to ~4Hz — at 60fps rapid `set_title` is
    /// unreadable and the OS may throttle it.
    last_title_update: Instant,

    script_runtime: ScriptRuntime,

    /// Holds the entity registry shared by the light bridge and the script
    /// runtime. Outlives the renderer so device resets preserve scripted
    /// light state. See: context/lib/scripting.md
    script_ctx: ScriptCtx,

    /// Gates the one-time persistence overlay and clean-exit save.
    state_store_lifecycle: StateStoreLifecycle,

    /// Consulted by `fire_named_event_with_sequences` for `Sequence` steps.
    /// No per-level state — entity lookups go through `ScriptCtx`, which the
    /// level-unload path clears separately. See: context/lib/scripting.md §2
    sequence_registry: SequencedPrimitiveRegistry,

    /// Resolved by name when a `Primitive` reaction fires.
    /// See: context/lib/scripting.md §2
    reaction_registry: ReactionPrimitiveRegistry,

    /// Per-tag kill-count subscriptions. Cleared on level unload; survives
    /// hot-reload. See: context/lib/scripting.md §2
    progress_tracker: ProgressTracker,

    /// Maps `classname` strings to engine spawn handlers. Survives level
    /// unload — built-in handlers carry no per-level state.
    /// See: context/lib/scripting.md
    classname_dispatch: ClassnameDispatch,

    /// Runs between Game Logic and Render; uploads repacked GpuLight bytes
    /// when any `LightComponent` is dirty. See: context/lib/scripting.md
    light_bridge: scripting_systems::light_bridge::LightBridge,

    /// Per-level fog-volume registry side-table; packs `FogVolume` GPU bytes
    /// each frame from `FogVolumeComponent` mutations. See:
    /// context/lib/rendering_pipeline.md §7.5
    fog_volume_bridge: scripting_systems::fog_volume_bridge::FogVolumeBridge,

    /// Walks every `BillboardEmitterComponent` after game logic and before
    /// particle sim. See: context/lib/scripting.md
    emitter_bridge: scripting_systems::emitter_bridge::EmitterBridge,

    /// Per-emitter live-particle tally, produced by `particle_sim::tick` and
    /// consumed by the next frame's `emitter_bridge.update` for cap headroom.
    /// Owned here (not re-allocated per frame) so the collapsed pass reuses one
    /// buffer's capacity across frames. See: context/lib/scripting.md §10.1 (Emitter and Particles).
    particle_live_counts: std::collections::HashMap<scripting::registry::EntityId, usize>,

    /// World-space static-geometry collider built from PRL static geometry.
    /// See: context/lib/entity_model.md §7
    collision_world: collision::CollisionWorld,

    /// Packs `SpriteInstance` bytes per collection in the Render stage;
    /// never touches wgpu directly. See: context/lib/scripting.md
    particle_render: scripting_systems::particle_render::ParticleRenderCollector,

    /// Packs per-instance skinned-mesh world matrices in the Render stage
    /// (cull applied via `mesh_pass::mesh_visible`); never touches wgpu.
    /// See: context/lib/scripting.md
    mesh_render: scripting_systems::mesh_render::MeshRenderCollector,

    /// Active wieldable instance equipped by the player. The companion
    /// descriptor name lets mod-init hot reload refresh authored weapon stats
    /// while preserving per-instance cooldown.
    active_wieldable: Option<crate::scripting::registry::EntityId>,
    active_wieldable_descriptor: Option<String>,

    /// Boot state machine: drives the splash → first-level-frame transition.
    /// Subsumes the previous `level_load_fired` one-shot flag.
    boot_state: BootState,

    /// Counts splash frames since `resumed()`. The state machine uses this to
    /// schedule the deferred `mod_init` (frame 1) and the worker spawn
    /// (frame 1, after `mod_init`); frame 2 onward polls the worker channel.
    splash_frame: u32,

    /// Set when `Splash → Running` transitions; consumed at the bottom of the
    /// first `Running` frame after `render_frame_indirect` returns. Ensures
    /// log line C ends with `first_level_frame` covering the cost of the
    /// frame the user actually sees.
    pending_level_log: bool,

    /// Set during `mod_init` if a mod registers a `SplashSource` override.
    /// The consume path in `run_splash_frame` frame 1 is wired; today the field
    /// stays `None` because no mod system yet calls the setter.
    /// See: context/lib/boot_sequence.md §8.
    #[allow(dead_code)]
    pending_splash_override: Option<SplashSource>,

    /// Classnames the built-in dispatch handled at level open. Captured during
    /// install and consumed by the data-archetype sweep on the same frame.
    /// `None` before level load and after the sweep consumes it.
    builtin_handled: Option<std::collections::HashSet<String>>,

    /// `player_spawn` placements partitioned from `world.map_entities` during
    /// install. Consumed on the same frame by `spawn_from_player_starts` — a
    /// separate path from `apply_data_archetype_dispatch`. `None` before level
    /// load and after consumed.
    pending_spawn_points: Option<Vec<crate::scripting::map_entity::MapEntity>>,

    /// Non-player-start map entities partitioned out of `world.map_entities`
    /// during install, awaiting the data-archetype sweep on the same frame.
    /// `None` before level load and after the sweep consumes them.
    pending_map_entities: Option<Vec<crate::scripting::map_entity::MapEntity>>,

    /// Seconds since level load, not wall clock. Resets to zero on level
    /// unload. Maintained for any future engine consumers that need a
    /// level-relative monotonic clock.
    script_time: f64,

    /// Per-stage durations for log line A — engine boot
    /// (args_parsed, script_runtime_ctor, event_loop_created, window_created,
    /// wgpu_init, first_black_frame, splash_decoded, splash_uploaded,
    /// first_splash_frame).
    boot_timings: StartupTimings,

    /// Per-stage durations for log line B — mod init (mod_init,
    /// mod_splash_swap [conditional]).
    mod_timings: StartupTimings,

    /// Per-stage durations for log line C — level load. Worker-thread stages
    /// are merged in between `worker_dispatch` and `worker_delivered`; see
    /// `StartupTimings` doc comment.
    level_timings: StartupTimings,

    /// Receives the worker's `LoadOutcome`. `None` until the second splash
    /// frame spawns the worker; consumed via `try_recv` each frame in
    /// `Splash`.
    level_rx: Option<mpsc::Receiver<LoadOutcome>>,

    /// Owned so the thread is detached (not joined) when App drops.
    /// Detached on shutdown — drop discards the JoinHandle without joining;
    /// the OS thread reaps when its work returns.
    level_worker: Option<JoinHandle<()>>,

    /// CPU-side egui state. `None` until `resumed()` initialises the renderer
    /// (the constructor needs the device's `max_texture_dimension_2d` limit).
    /// GPU half lives on `Renderer` as `debug_ui_gpu`; lazy-initialized on
    /// first panel open.
    #[cfg(feature = "dev-tools")]
    debug_ui: Option<render::debug_ui::DebugUi>,
}

struct WindowState {
    window: Arc<Window>,
}

// --- ApplicationHandler ---

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // On desktop, winit fires resumed() exactly once at startup
        // (Booting → Splash). Guard against the Suspended → Resumed path that
        // some platforms issue during normal operation — re-entering from
        // Running would corrupt the boot state by resetting `splash_frame`,
        // re-installing the splash, and stalling with `level_rx = None`.
        if self.boot_state != BootState::Booting {
            return;
        }
        let window = match event_loop.create_window(window_attributes()) {
            Ok(w) => Arc::new(w),
            Err(err) => {
                self.exit_result = Err(anyhow::anyhow!("failed to create window: {err}"));
                event_loop.exit();
                return;
            }
        };
        self.boot_timings.record("window_created");

        let renderer = match Renderer::new(&window) {
            Ok(r) => r,
            Err(err) => {
                self.exit_result = Err(err);
                event_loop.exit();
                return;
            }
        };
        self.boot_timings.record("wgpu_init");

        // Splash decode + upload is deferred to the first Splash frame's
        // post-paint window so the OS window opens as a black screen as
        // fast as possible. See `run_splash_frame` and
        // `context/lib/boot_sequence.md` §8.

        let size = window.inner_size();
        self.camera.update_aspect(size.width, size.height);

        self.renderer = Some(renderer);
        self.window_state = Some(WindowState { window });

        // Fault-tolerant audio init: on failure log and run silent, never
        // crash. See: context/lib/audio.md §1.
        match audio::Audio::new() {
            Ok(audio) => {
                self.audio = Some(audio);
                log::info!("[Audio] Initialized");
            }
            Err(err) => {
                log::error!("[Audio] Init failed, running silent: {err}");
                self.audio = None;
            }
        }

        #[cfg(feature = "dev-tools")]
        {
            if let (Some(renderer), Some(ws)) = (self.renderer.as_ref(), self.window_state.as_ref())
            {
                let max_texture = renderer.max_texture_dimension_2d();
                self.debug_ui = Some(render::debug_ui::DebugUi::new(&ws.window, max_texture));
            }
        }

        self.set_input_focus(InputFocus::Gameplay);
        self.frame_timing.last_frame = Instant::now();
        self.boot_state = BootState::Splash;

        // Drive the redraw loop so `RedrawRequested` fires the first splash
        // frame and the boot state machine can advance.
        if let Some(ws) = self.window_state.as_ref() {
            ws.window.request_redraw();
        }

        log::info!("[Engine] Window ready");
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.window_state = None;
        self.renderer = None;
        // Re-built on the next `resumed()` since it borrows the new window
        // and reads the new renderer's device limits.
        #[cfg(feature = "dev-tools")]
        {
            self.debug_ui = None;
        }
        // Fog-volume entities live in the script registry; clearing the
        // bridge's id table here keeps it from referencing stale slots if a
        // future surface re-creation re-runs `populate_from_level`.
        // collision_world is reset for the same reason — it must be in a
        // clean placeholder state before populate_from_level runs on resume.
        self.fog_volume_bridge.clear();
        self.collision_world.clear();
        self.active_wieldable = None;
        self.active_wieldable_descriptor = None;
        // Drop any in-flight level-load worker handoff. On resume the splash
        // state machine starts over from frame 0 and will spawn a fresh
        // worker; holding a stale receiver/handle would either block install
        // forever or deliver into the wrong boot phase.
        self.level_rx = None;
        self.level_worker = None;
        // Reset the boot state so `resumed()` re-runs window + renderer
        // creation. Without this, the `Booting` guard in `resumed()` would
        // no-op and the engine would stay permanently renderer-less.
        self.boot_state = BootState::Booting;
        self.splash_frame = 0;
        self.pending_level_log = false;
        log::info!("[Engine] Suspended");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        // Feed every window event to egui-winit to keep its internal state
        // current (scale factor, modifier state, cursor position) regardless
        // of focus. `response.consumed` is honored only in DevTools/Menu
        // focus — gameplay ignores it. ToggleDebugPanel punches through
        // regardless so the panel can always be closed.
        #[cfg(feature = "dev-tools")]
        let egui_consumed: bool = {
            let mut consumed = false;
            if let (Some(debug_ui), Some(ws)) = (self.debug_ui.as_mut(), self.window_state.as_ref())
            {
                let response = debug_ui.on_window_event(&ws.window, &event);
                if self.input_focus != InputFocus::Gameplay {
                    consumed = response.consumed;
                }
            }
            consumed
        };
        #[cfg(not(feature = "dev-tools"))]
        let egui_consumed: bool = false;

        match event {
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
                self.camera.update_aspect(size.width, size.height);
            }
            WindowEvent::CloseRequested => {
                self.release_cursor_for_exit();
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
                self.release_cursor_for_exit();
                log::info!("[Engine] Shutting down");
                event_loop.exit();
            }
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                if let PhysicalKey::Code(code) = key_event.physical_key {
                    let pressed = key_event.state.is_pressed();

                    // Modifier-only key events always feed the diagnostic
                    // resolver — even when egui consumes them — so its
                    // modifier tracking stays current and `Alt+Shift+Backquote`
                    // remains resolvable while the panel has focus.
                    let is_modifier_key = matches!(
                        code,
                        winit::keyboard::KeyCode::ShiftLeft
                            | winit::keyboard::KeyCode::ShiftRight
                            | winit::keyboard::KeyCode::AltLeft
                            | winit::keyboard::KeyCode::AltRight
                            | winit::keyboard::KeyCode::ControlLeft
                            | winit::keyboard::KeyCode::ControlRight
                            | winit::keyboard::KeyCode::SuperLeft
                            | winit::keyboard::KeyCode::SuperRight
                    );

                    if egui_consumed {
                        // egui owns this event. Keep modifier tracking current
                        // so the toggle chord still resolves once the panel is
                        // open, but do not forward to the input system or fire
                        // any other diagnostic chord.
                        if is_modifier_key {
                            let _ =
                                self.diagnostic_inputs
                                    .handle_key(code, pressed, key_event.repeat);
                        }
                        // The toggle chord (`Alt+Shift+Backquote`) is reachable
                        // even when egui consumes the keypress — no egui widget
                        // binds it, so a targeted check here is unambiguous.
                        // See: context/lib/input.md §7
                        #[cfg(feature = "dev-tools")]
                        if !is_modifier_key {
                            if let Some(action) =
                                self.diagnostic_inputs
                                    .handle_key(code, pressed, key_event.repeat)
                            {
                                if action == DiagnosticAction::ToggleDebugPanel {
                                    self.handle_diagnostic_action(action);
                                }
                            }
                        }
                        return;
                    }

                    // Chord resolver runs first: owns Alt+Shift+ modifier
                    // tracking and fires only on a clean rising edge.
                    if let Some(action) =
                        self.diagnostic_inputs
                            .handle_key(code, pressed, key_event.repeat)
                    {
                        self.handle_diagnostic_action(action);
                    }

                    // UI-dispatch seam, ahead of the gameplay forward and
                    // mirroring the `egui_consumed` gate: when the active UI
                    // layer is in Capture mode the event is consumed (queued
                    // for next-frame game logic) and NOT forwarded to the
                    // action system this frame. `InputFocus::Menu` is the
                    // intended structural home for this capture; Goal A makes
                    // no live focus change, so the decision is the mode flag.
                    // See: context/lib/input.md
                    if self.ui_dispatch.dispatch_event().forwards_to_gameplay()
                        && self.input_focus == InputFocus::Gameplay
                    {
                        // Only Gameplay forwards keys to the action system. When
                        // the debug panel (or future menu) owns focus, WASD must
                        // not drive the camera even though egui leaves
                        // `consumed = false` for non-text widgets like sliders.
                        self.input_system.handle_keyboard_event(code, pressed);
                    }
                }
            }
            WindowEvent::MouseInput { button, state, .. } => {
                if egui_consumed {
                    return;
                }
                // Same UI-dispatch seam as the keyboard path: a captured event
                // is consumed by the UI layer and not forwarded to the action
                // system this frame.
                if !self.ui_dispatch.dispatch_event().forwards_to_gameplay() {
                    return;
                }
                // Same focus gate as the keyboard path: mouse-button actions
                // (fire, alt-fire) must not fire while DevTools/Menu owns
                // input. See: context/lib/input.md §5
                if self.input_focus == InputFocus::Gameplay {
                    self.input_system
                        .handle_mouse_button(button, state.is_pressed());
                }
            }
            WindowEvent::Focused(focused) => {
                if focused {
                    // Re-acquire the cursor for whichever focus mode the user
                    // chose; the stored focus is untouched on focus loss so
                    // this restores the pre-blur state.
                    self.reapply_focus();
                } else if let Some(ws) = self.window_state.as_ref() {
                    // Release the cursor while unfocused but leave
                    // `input_focus` alone — the user's chosen focus mode
                    // outlives transient OS focus loss.
                    input::cursor::release_cursor(&ws.window);
                    self.input_system.clear_all();
                    self.gameplay_input_latch.clear();
                    self.diagnostic_inputs.clear_modifiers();
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

                // Drain changed paths every frame — unconditionally — so the
                // watcher channel does not back up even when the summary is
                // empty. ScriptRuntime checks them against the active
                // dependency set before queuing the serialized staged build.
                match self.script_runtime.drain_reload_requests() {
                    Ok(summary) => {
                        if reload_summary_requires_mod_init(summary) {
                            match self
                                .script_runtime
                                .enqueue_staged_manifest_build(&self.content_root)
                            {
                                Ok(Some(generation)) => log::info!(
                                    "[Scripting] active mod-init dependency changed - queued staged generation {generation}",
                                ),
                                Ok(None) => {}
                                Err(err) => {
                                    log::error!(
                                        "[Scripting] failed to queue staged mod-init: {err}",
                                    );
                                }
                            }
                        }
                    }
                    Err(err) => {
                        log::error!("[Scripting] drain_reload_requests failed: {err}");
                    }
                }

                // Boot state machine: while in `Splash`, paint the splash and
                // drive mod-init / worker-spawn / worker-poll. Once the worker
                // delivers, install the level and fall through to the normal
                // frame loop. See: context/lib/boot_sequence.md §8 and
                // `run_splash_frame` for the frame-by-frame schedule.
                match self.boot_state {
                    BootState::Booting => {
                        // A `RedrawRequested` queued before `resumed()` (or
                        // after `suspended()` resets boot_state back to
                        // `Booting`) can legally arrive here. Drop it
                        // silently — `resumed()` will rebuild and request a
                        // fresh redraw.
                        return;
                    }
                    BootState::Splash => {
                        if !self.run_splash_frame(event_loop) {
                            return;
                        }
                    }
                    BootState::Running => {
                        // Steady state — fall through to the normal frame loop.
                    }
                }

                // Game-logic phase begins here. Read the UI captures made
                // available by the *previous* frame, THEN promote this frame's
                // freshly captured events for the next frame. Taking before
                // advancing is what enforces the N→N+1 contract: events captured
                // during THIS frame's Input stage land in `pending` and are only
                // promoted to `ready` by this `advance_frame` call — so they
                // first become visible at the next frame's `take_ready`, never
                // this frame. This holds regardless of winit's event/redraw
                // ordering because both calls run here at game-logic time. Goal A
                // has no intent consumer yet (Goal F defines the vocabulary), so
                // the drained intents are dropped; the drain marks the seam where
                // game logic reads them. See: context/lib/input.md
                let _ui_intents = self.ui_dispatch.take_ready();
                self.ui_dispatch.advance_frame();

                if let Some(gp) = &mut self.gamepad_system {
                    gp.update(&mut self.input_system);
                }

                // drain_look_inputs() must precede snapshot(); both touch
                // mouse_axes and look state belongs to the render-rate path.
                let look = self.input_system.drain_look_inputs();
                let frame_snapshot = self.input_system.snapshot();
                let gameplay_snapshot = self
                    .gameplay_input_latch
                    .snapshot_for_ticks(&frame_snapshot, ticks);

                // Apply look rotation once at render rate, not once per tick —
                // so zero-tick frames still consume accumulated mouse motion.
                self.camera
                    .rotate(look.yaw_delta(frame_dt), look.pitch_delta(frame_dt));

                // Bump the engine frame counter once per Game logic phase.
                // Reserved for primitives that need a per-frame ordering stamp.
                // See: context/lib/scripting.md
                self.script_ctx
                    .frame
                    .set(self.script_ctx.frame.get().wrapping_add(1));

                // Accumulate movement and weapon events across all ticks; drain
                // after the tick loop completes so reactions see fully-settled
                // post-tick world state and event order is never interleaved
                // with ongoing physics simulation. See:
                // context/lib/entity_model.md §5
                let mut pending_movement_events: Vec<&'static str> = Vec::new();
                let mut pending_weapon_events: Vec<&'static str> = Vec::new();

                if let Some(snapshot) = gameplay_snapshot.as_ref() {
                    for _ in 0..ticks {
                        // Order 0: transform snapshot. Copy current→previous for
                        // every already-live entity before any movement/behavior
                        // system mutates transforms this tick, so the renderer can
                        // interpolate each entity between its start-of-tick and
                        // post-tick pose. Entities spawned later this tick seed
                        // previous == current at construction and are skipped here
                        // (no pop on spawn). See: context/lib/entity_model.md §5.
                        self.script_ctx.registry.borrow_mut().snapshot_transforms();

                        let forward_axis = snapshot.axis_value(Action::MoveForward);
                        let right_axis = snapshot.axis_value(Action::MoveRight);
                        let up_axis = snapshot.axis_value(Action::MoveUp);
                        let sprint = snapshot.button(Action::Sprint).is_active();

                        let speed = if sprint {
                            camera::MOVE_SPEED * camera::SPRINT_MULTIPLIER
                        } else {
                            camera::MOVE_SPEED
                        };

                        // Camera-vs-pawn split (entity_model.md §5/§7):
                        //   - If a PlayerMovementComponent entity exists, its
                        //     position drives `camera.position` (yaw/pitch stay
                        //     mouse-driven).
                        //   - Otherwise, fly-cam moves the camera directly so the
                        //     engine is navigable without a player spawn (dev maps,
                        //     levels without a player descriptor).
                        let has_player_pawn = {
                            use crate::scripting::registry::ComponentKind;
                            let registry = self.script_ctx.registry.borrow();
                            registry
                                .iter_with_kind(ComponentKind::PlayerMovement)
                                .next()
                                .is_some()
                        };

                        if !has_player_pawn {
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
                        }

                        // Order 1: movement-component tick (all entities carrying
                        // PlayerMovementComponent, per entity_model.md §5).
                        let jump_pressed = snapshot.button(Action::Jump).is_active();
                        // Dash is a true rising edge (only `Pressed`, not
                        // `Held`): a held dash would re-fire every cooldown-ready
                        // tick. This intentionally differs from `jump_pressed`'s
                        // level signal. See `MovementInput::dash_pressed`.
                        let dash_pressed =
                            matches!(snapshot.button(Action::Dash), ButtonState::Pressed);
                        // Crouch toggle-vs-hold is resolved HERE, in the input
                        // layer, from `PlayerOptions.crouch_mode`. The movement
                        // intent receives only the single resolved bit and never
                        // sees the raw button or the mode. The toggle latch lives
                        // on `App` (`crouch_toggle_active`), never on the movement
                        // component. See `MovementInput::crouch_intent`.
                        let crouch_intent = resolve_crouch_intent(
                            self.player_options.crouch_mode,
                            snapshot.button(Action::Crouch),
                            &mut self.crouch_toggle_active,
                        );
                        let movement_events = self.run_movement_tick(
                            forward_axis,
                            right_axis,
                            jump_pressed,
                            dash_pressed,
                            crouch_intent,
                            sprint,
                            tick_dt,
                        );
                        pending_movement_events.extend(movement_events);

                        // Camera follows the first pawn's position (eye-height
                        // offset above the capsule center). Yaw/pitch are owned by
                        // the mouse-driven look path and are not touched here.
                        if has_player_pawn {
                            use crate::scripting::registry::{
                                ComponentKind, ComponentValue, Transform,
                            };
                            let registry = self.script_ctx.registry.borrow();
                            for (id, value) in
                                registry.iter_with_kind(ComponentKind::PlayerMovement)
                            {
                                let ComponentValue::PlayerMovement(component) = value else {
                                    continue;
                                };
                                if let Ok(transform) = registry.get_component::<Transform>(id) {
                                    self.camera.position = transform.position
                                        + Vec3::new(0.0, component.capsule.eye_height, 0.0);
                                    break;
                                }
                            }
                        }

                        let weapon_events = self.run_weapon_fire_tick(snapshot, tick_dt);
                        pending_weapon_events.extend(weapon_events);

                        self.frame_timing
                            .push_state(InterpolableState::new(self.camera.position));
                    }
                }

                // Drain collected movement and weapon events after all ticks
                // complete so reactions observe the final post-tick state of
                // every entity.
                for event_name in &pending_movement_events {
                    let _ = fire_named_event(event_name, &self.script_ctx.data_registry.borrow());
                }
                for event_name in &pending_weapon_events {
                    let _ = fire_named_event(event_name, &self.script_ctx.data_registry.borrow());
                }

                // Audio step — third in frame order (Input → Game logic →
                // Audio → Render → Present, development_guide.md §4.3). Runs after
                // game logic settles every entity and before render. Convert the
                // glam-typed camera to the primitive `ListenerState` here at the
                // call site (the boundary carries no glam); `forward` uses the
                // aim ray's direction so it includes pitch, unlike yaw-only
                // `forward()`, and `up` is world up per the `ListenerState`
                // contract. Guarded for the silent (init-failed) case.
                if let Some(audio) = &mut self.audio {
                    let listener = audio::ListenerState {
                        position: self.camera.position.to_array(),
                        forward: self.camera.aim_ray().1.to_array(),
                        up: [0.0, 1.0, 0.0],
                    };
                    audio.update(listener, frame_dt);
                }

                // Level-relative monotonic clock consumed by light_bridge.update,
                // the emitter sim, and the map-light collector.
                // Widen to f64 at the accumulation boundary so summing across
                // long sessions (30+ min at 144 Hz) doesn't quantize the
                // millisecond-precision clock the fog volume bridge consumes.
                //
                // Dev-tools freeze must stop BOTH clocks together. The GPU `time`
                // uniform is fed `script_time`, and the CPU light bridge computes
                // `effective_brightness` (which gates shadow-pool eligibility)
                // from the same clock. Freezing only the GPU uniform would let
                // the CPU clock advance, re-creating the CPU/GPU animation-phase
                // desync this branch fixed. Read the freeze flag from the
                // renderer — it owns the toggle (driven by the debug panel) — and
                // skip the increment while frozen so both sides hold one phase.
                #[cfg(feature = "dev-tools")]
                let frozen = self
                    .renderer
                    .as_ref()
                    .is_some_and(|renderer| renderer.freeze_time());
                #[cfg(not(feature = "dev-tools"))]
                let frozen = false;
                if !frozen {
                    self.script_time += frame_dt as f64;
                }

                // Position interpolated from tick-state slots; yaw/pitch from
                // `self.camera` directly so zero-tick frames still see this
                // frame's look rotation.
                let interp = self.frame_timing.interpolated_state();
                let view_proj = interp.view_projection(
                    self.camera.aspect(),
                    self.camera.yaw,
                    self.camera.pitch,
                );

                let capture_portal_walk = std::mem::take(&mut self.capture_portal_walk_next_frame);

                // Portal DFS → cell IDs → visible-cell bitmask → indirect draw buffer.
                let (vis_result, _frustum) = match self.level.as_ref() {
                    Some(world) => visibility::determine_visible_cells(
                        interp.position,
                        view_proj,
                        world,
                        capture_portal_walk,
                        &mut self.scratch_cells,
                    ),
                    None => (
                        VisibilityResult {
                            visible_cells: VisibleCells::DrawAll,
                            fog_reachable: Vec::new(),
                            stats: VisibilityStats {
                                camera_leaf: 0,
                                total_faces: 0,
                                drawn_faces: 0,
                                path: VisibilityPath::EmptyWorldFallback,
                            },
                        },
                        visibility::extract_frustum_planes(view_proj),
                    ),
                };
                let VisibilityResult {
                    visible_cells,
                    fog_reachable,
                    stats,
                } = vis_result;

                // Build the per-leaf bool mask for `update_dynamic_light_slots`
                // from the wider fog/light-reachable set so dynamic lights in
                // empty (face_count == 0) portal-reachable leaves stay
                // eligible. Empty slice = DrawAll sentinel: keep every
                // leaf-assigned light eligible on fallback paths.
                let light_reachable_leaf_mask: Vec<bool> = match self.level.as_ref() {
                    None => Vec::new(),
                    Some(_) if fog_reachable.is_empty() => Vec::new(),
                    Some(world) => {
                        let mut mask = vec![false; world.leaves.len()];
                        for &id in &fog_reachable {
                            let i = id as usize;
                            if i < mask.len() {
                                mask[i] = true;
                            }
                        }
                        mask
                    }
                };

                if let Some(renderer) = self.renderer.as_mut() {
                    // Emitter bridge — after script `tick` handler, before particle
                    // sim. Spawns new particles; the sim advances them the same
                    // frame so they don't appear stuck at origin.
                    {
                        let mut registry = self.script_ctx.registry.borrow_mut();
                        // Cap headroom comes from the previous frame's sim tally
                        // (see particle_sim::tick) — the bridge no longer walks the
                        // ParticleState column itself.
                        self.emitter_bridge.update(
                            &mut registry,
                            frame_dt,
                            self.script_time as f32,
                            &self.particle_live_counts,
                        );
                    }

                    // Particle sim — after emitter bridge, before light bridge.
                    // Pure Rust; scripts never observe individual particles.
                    // Refills `particle_live_counts` with this tick's per-emitter
                    // survivor count for the next frame's bridge headroom.
                    {
                        let mut registry = self.script_ctx.registry.borrow_mut();
                        scripting_systems::particle_sim::tick(
                            &mut registry,
                            frame_dt,
                            self.script_ctx.gravity.get(),
                            &mut self.particle_live_counts,
                        );
                    }

                    // Light bridge — between Game Logic and Render. Uploads
                    // mutated `LightComponent` data before `render_frame_indirect`
                    // allocates slots, so scripted lights reflect their new state.
                    {
                        let mut registry = self.script_ctx.registry.borrow_mut();
                        if let Some(update) = self
                            .light_bridge
                            .update(&mut registry, self.script_time as f32)
                        {
                            if update.has_dirty_data {
                                renderer.upload_bridge_lights(&update.lights_bytes);
                                renderer.upload_bridge_descriptors(&update.descriptor_bytes);
                                renderer.upload_bridge_samples(&update.samples_bytes);
                                // Task 2c: also fan out `_animated` descriptor
                                // updates to the animated-compose buffer.
                                for (slot, bytes) in &update.compose_descriptor_writes {
                                    renderer.write_animated_compose_descriptor(*slot, bytes);
                                }
                            }
                            renderer.set_light_effective_brightness(&update.effective_brightness);
                        }
                    }

                    // Fog volume bridge — alongside the light bridge. Volume
                    // packing reads `FogVolumeComponent`; point-light packing
                    // pre-culls dynamic point lights against fog AABBs. Upload
                    // happens unconditionally so an empty list zeroes the GPU
                    // volume count and skips the pass for the rest of the frame.
                    // Combine static map lights with script-spawned dynamic lights so
                    // fog halos react to lights from both sources. The light bridge
                    // tracks both via `populate_from_level` + `absorb_dynamic_lights`;
                    // `renderer.level_lights()` only covers the static subset.
                    // `collect_all_as_map_lights` pairs each light with its
                    // brightness multiplier so the two cannot drift out of alignment
                    // when a `LightComponent` lookup fails.
                    {
                        // Evaluate fog animation curves (density and saturation)
                        // before `update_volumes` packs the GPU buffer — `tick`
                        // writes sampled values into each `FogVolumeComponent`
                        // so the existing pack path picks them up unchanged.
                        let mut registry = self.script_ctx.registry.borrow_mut();
                        self.fog_volume_bridge.tick(&mut registry, self.script_time);
                    }
                    let all_lights = {
                        let registry = self.script_ctx.registry.borrow();
                        if let Some((bytes, planes, live_mask)) =
                            self.fog_volume_bridge.update_volumes(&registry)
                        {
                            renderer.upload_fog_volumes(bytes, planes, live_mask);
                        } else {
                            renderer.upload_fog_volumes(&[], &[], 0);
                        }
                        renderer.set_fog_aabbs(self.fog_volume_bridge.active_aabbs());
                        self.light_bridge
                            .collect_all_as_map_lights(&registry, self.script_time as f32)
                    };
                    let point_bytes = self.fog_volume_bridge.update_points(&all_lights);
                    renderer.upload_fog_points(point_bytes);

                    renderer.update_per_frame_uniforms(
                        view_proj,
                        interp.position,
                        self.script_time as f32,
                    );

                    if renderer.is_ready() {
                        // Particle render — packs `SpriteInstance` bytes per
                        // collection; the collector never touches wgpu directly.
                        {
                            let registry = self.script_ctx.registry.borrow();
                            // Cull non-visible emitters at render-collect, mirroring
                            // the mesh path below: thread the level world + this
                            // frame's visible-cell set so off-screen / adjacent-room
                            // smoke is never packed for drawing. `visible_cells` is
                            // still live here (reclaimed after the frame).
                            self.particle_render.collect(
                                &registry,
                                self.level.as_ref(),
                                &visible_cells,
                            );
                        }
                        let particle_collections: Vec<(&str, &[u8])> =
                            self.particle_render.iter_collections().collect();

                        // Mesh render — emits per-instance inputs (model handle +
                        // interpolated transform + phase seed) for skinned-mesh
                        // entities, culling each against this frame's visible set
                        // via `mesh_pass::mesh_visible`. Like the particle collector
                        // it never touches wgpu; the renderer consumes the inputs
                        // via `set_mesh_draws`. Runs before `render_frame_indirect`,
                        // while `visible_cells` is still live (it is reclaimed into
                        // scratch after).
                        if let Some(world) = self.level.as_ref() {
                            let registry = self.script_ctx.registry.borrow();
                            // Same frame alpha the player camera reads from
                            // `frame_timing` — interpolate each mesh between its
                            // previous- and current-tick transforms.
                            self.mesh_render.collect(
                                &registry,
                                world,
                                &visible_cells,
                                frame_result.alpha,
                            );
                            renderer.set_mesh_draws(self.mesh_render.instances());
                        }

                        // Build the egui UI before `render_frame_indirect` so
                        // the SH diagnostic overlay can push debug lines that
                        // the frame's debug-line pass will pick up. Tessellated
                        // paint jobs are stashed and consumed after the frame
                        // by `render_debug_ui`.
                        #[cfg(feature = "dev-tools")]
                        let debug_ui_frame: Option<(
                            egui::TexturesDelta,
                            Vec<egui::epaint::ClippedPrimitive>,
                            f32,
                        )> = {
                            let mut out = None;
                            if let (Some(debug_ui), Some(ws)) =
                                (self.debug_ui.as_mut(), self.window_state.as_ref())
                            {
                                if debug_ui.is_visible() {
                                    let window = &ws.window;
                                    let raw_input = debug_ui.winit_state.take_egui_input(window);
                                    let timing_snapshot = renderer.frame_timing_snapshot().cloned();
                                    let panel_state = &mut debug_ui.panel_state;
                                    let sh_state = &mut debug_ui.sh_diagnostics_state;
                                    let ctx_clone = debug_ui.ctx.clone();
                                    let full_output = ctx_clone.run_ui(raw_input, |ui| {
                                        render::debug_ui::draw_diagnostics_panel(
                                            ui.ctx(),
                                            panel_state,
                                            sh_state,
                                            renderer,
                                            timing_snapshot.as_ref(),
                                        );
                                    });
                                    debug_ui.winit_state.handle_platform_output(
                                        window,
                                        full_output.platform_output,
                                    );
                                    let paint_jobs = debug_ui.ctx.tessellate(
                                        full_output.shapes,
                                        full_output.pixels_per_point,
                                    );
                                    out = Some((
                                        full_output.textures_delta,
                                        paint_jobs,
                                        window.scale_factor() as f32,
                                    ));
                                }
                            }
                            // Clear the debug-line buffer unconditionally each
                            // frame so any producer starts fresh. This is the
                            // single lifecycle owner of the buffer: it handles
                            // early-returns in `render_frame_indirect`
                            // (Timeout/Occluded/Outdated) and level unloads
                            // cleanly, and keeps any future debug-line producer
                            // from colliding with the SH diagnostic pass.
                            renderer.clear_debug_lines();
                            // Emit SH diagnostic debug lines now — after UI
                            // mutated state, before `render_frame_indirect`
                            // draws the debug-line pass.
                            if let Some(world) = self.level.as_ref() {
                                if let Some(debug_ui) = self.debug_ui.as_ref() {
                                    renderer.emit_sh_diagnostics(
                                        &debug_ui.sh_diagnostics_state,
                                        interp.position,
                                        world,
                                        &light_reachable_leaf_mask,
                                    );
                                }
                            }
                            out
                        };

                        // Publish the once-per-frame read snapshot just before
                        // the gameplay render call, mirroring the splash path so
                        // the once-per-frame contract holds on both. Game logic and
                        // audio have already run this frame, so the slot snapshot
                        // freezes the settled store state (frame order: Input →
                        // Game logic → Audio → Render). The renderer reads these
                        // cloned values, never the live `SlotTable`. No gameplay UI
                        // producer yet, so the tree-less default carries the map.
                        let slot_values =
                            Self::build_ui_slot_snapshot(&self.script_ctx.slot_table.borrow());
                        renderer.set_ui_snapshot(
                            render::ui::UiReadSnapshot::default().with_slot_values(slot_values),
                        );

                        let surface_texture = match renderer.render_frame_indirect(
                            &visible_cells,
                            &light_reachable_leaf_mask,
                            &fog_reachable,
                            Some(stats.camera_leaf),
                            view_proj,
                            &particle_collections,
                            self.script_time,
                        ) {
                            Ok(opt) => opt,
                            Err(err) => {
                                self.exit_result = Err(err);
                                event_loop.exit();
                                return;
                            }
                        };
                        if let Some(surface_texture) = surface_texture {
                            #[cfg(feature = "dev-tools")]
                            {
                                if let Some((textures_delta, paint_jobs, scale)) = debug_ui_frame {
                                    if let Err(err) = renderer.render_debug_ui(
                                        &surface_texture,
                                        textures_delta,
                                        paint_jobs,
                                        scale,
                                    ) {
                                        self.exit_result = Err(err);
                                        event_loop.exit();
                                        return;
                                    }
                                }
                            }
                            surface_texture.present();
                        }
                        if self.pending_level_log {
                            // First level frame just submitted — close out
                            // log line C with the present-cost of the frame
                            // the user is about to see.
                            self.level_timings.record("first_level_frame");
                            log::info!("{}", self.level_timings.summary());
                            self.pending_level_log = false;
                        }
                    }
                }

                for result in self.script_runtime.poll_staged_manifest_builds() {
                    let _ = self
                        .script_runtime
                        .commit_staged_manifest_result(&result, &self.script_ctx);
                }

                if let VisibleCells::Culled(mut cells) = visible_cells {
                    cells.clear();
                    self.scratch_cells = cells;
                }

                let pos = interp.position;
                let region_label = "leaf";
                let path_label = match stats.path {
                    VisibilityPath::PrlPortal { .. } => "prl-portal",
                    VisibilityPath::NoPortalsFallback => "no-portals",
                    VisibilityPath::EmptyWorldFallback => "empty",
                    VisibilityPath::SolidLeafFallback => "solid-leaf",
                    VisibilityPath::ExteriorCameraFallback => "exterior",
                };
                let walk_reach_col = match stats.walk_reach() {
                    Some(walk) => format!(" walk:{walk}"),
                    None => String::new(),
                };
                log::debug!(
                    "[Diagnostics] {region_label}:{} path:{path_label} | draw:{} all:{}{walk_reach_col} | pos: ({:.0}, {:.0}, {:.0})",
                    stats.camera_leaf,
                    stats.drawn_faces,
                    stats.total_faces,
                    pos.x,
                    pos.y,
                    pos.z,
                );

                // `vsync:` label always present (not toggled) so it's grep-able
                // and the diagnostic toggle's effect is immediately visible.
                let vsync_label = self
                    .renderer
                    .as_ref()
                    .map(|r| if r.vsync_enabled() { "on" } else { "off" });
                if let Some(ws) = self.window_state.as_ref() {
                    if self.last_title_update.elapsed() >= Duration::from_millis(250) {
                        self.last_title_update = Instant::now();
                        self.title_buffer.clear();
                        let _ = write!(
                            &mut self.title_buffer,
                            "Postretro | {region_label}:{} path:{path_label} | draw:{} all:{}{walk_reach_col} | pos: ({:.0}, {:.0}, {:.0})",
                            stats.camera_leaf,
                            stats.drawn_faces,
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

                // Measure from `now` at handler entry so the sample spans all
                // CPU work. Wall-clock tick-to-tick is useless under vsync
                // (pinned to ~16.6ms); this shows actual load.
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
        // UI-dispatch seam, ahead of the gameplay forward: a captured raw
        // delta is consumed by the UI layer and must not reach the look path.
        // Mirrors the `window_event` seam; the decision is the mode flag.
        if !self.ui_dispatch.dispatch_event().forwards_to_gameplay() {
            return;
        }
        // Raw mouse deltas only rotate the camera while gameplay owns input.
        // When the debug panel (DevTools) or a menu is open, the cursor is
        // released and raw deltas must not leak into the look path.
        if self.input_focus != InputFocus::Gameplay {
            return;
        }
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
        // Saving before declarations commit and restore completes could replace
        // a valid state file with an empty or default-only snapshot.
        if self.state_store_lifecycle.can_save() {
            let state_path = Path::new(STATE_FILE_PATH);
            let collected = collect_persisted_state(&self.script_ctx.slot_table.borrow());
            for warning in collected.warnings {
                log::warn!("[State] {warning}");
            }
            match save_persisted_state(state_path, &collected.state) {
                Ok(()) => {
                    log::info!("[State] saved persistent slots to {}", state_path.display())
                }
                Err(error) => log::warn!(
                    "[State] failed to save persistent slots to {}: {error}",
                    state_path.display()
                ),
            }
        }

        // Release the level's sound registry at teardown, mirroring the texture
        // release on level unload (`resource_management.md` §7.2). This engine
        // has a single level for its lifetime, so unload coincides with exit.
        if let Some(audio) = &mut self.audio {
            audio.release_level_sounds();
        }
        self.renderer = None;
        self.window_state = None;
        log::info!("[Engine] Exited");
    }
}

impl App {
    /// Drive one Splash-state frame. Returns `true` if the level payload was
    /// installed this frame (caller falls through to render the first level
    /// frame). Returns `false` if only the splash was painted and the redraw
    /// should otherwise short-circuit.
    ///
    /// Frame schedule:
    /// - frame 0: paint a black frame (no splash bound). After present:
    ///   record `first_black_frame`; decode the base PNG synchronously;
    ///   upload + bind it; record `splash_decoded` / `splash_uploaded`.
    ///   (Source is always `Base` until the mod system ships.)
    /// - frame 1: paint splash (now visible). After paint: record
    ///   `first_splash_frame`; emit log line A; run `mod_init`; optionally
    ///   swap splash on override; emit log line B; spawn level worker.
    /// - frame ≥ 2: poll the worker channel. On `Ok(level=Some)`, install
    ///   and transition to `Running`; otherwise paint splash and stay.
    fn run_splash_frame(&mut self, event_loop: &ActiveEventLoop) -> bool {
        match self.splash_frame {
            0 => {
                // First Splash frame: paint a black screen. The splash
                // texture is not yet decoded; the splash pass clears to
                // black and draws nothing.
                self.paint_splash(event_loop);
                self.boot_timings.record("first_black_frame");

                // Now that the OS window is showing a black frame, decode
                // and upload the splash synchronously. PNG decode is
                // bounded CPU work (~ms); doing it here keeps the boot
                // path single-threaded and ordering causal.
                //
                // Source is always `Base` today; no resolution step exists.
                // Once the mod system ships, override paths will be
                // discovered before this point and set here.
                let source = SplashSource::Base;
                match render::splash::load_splash(&source) {
                    Ok(loaded) => {
                        self.boot_timings.record("splash_decoded");
                        if let Some(renderer) = self.renderer.as_mut() {
                            let dims = renderer.install_splash_from_loaded(&loaded);
                            log::info!("[Engine] Splash loaded: {}×{}", dims[0], dims[1]);
                        }
                        self.boot_timings.record("splash_uploaded");
                    }
                    Err(err) => {
                        // Missing base splash is a packaging bug; record
                        // both stages so log line A always lists the same
                        // set of stage names regardless of success/failure.
                        // Subsequent splash frames stay black.
                        self.boot_timings.record("splash_decoded");
                        self.boot_timings.record("splash_uploaded");
                        log::warn!("[Engine] failed to decode base splash: {err:#}");
                    }
                }

                self.splash_frame += 1;
                self.request_redraw();
                false
            }
            1 => {
                // Second Splash frame: paint the splash so the user sees
                // it before mod scripts touch the engine.
                self.paint_splash(event_loop);
                self.boot_timings.record("first_splash_frame");
                log::info!("{}", self.boot_timings.summary());

                // Reset so the cursor starts at the top of this frame, not at
                // App construction time.
                self.mod_timings = StartupTimings::new();

                // Mod init runs before the worker spawns so declarations and
                // entity descriptors commit together, then persistence
                // overlays defaults once before any level work begins.
                //
                // In debug builds, compile any stale definition scripts first
                // so the hot-reload watcher starts from a consistent baseline.
                // Release builds no-op.
                let script_root = self.content_root.join("scripts");
                self.script_runtime
                    .compile_stale_scripts(&script_root, &self.content_root);
                if let Err(err) = self.script_runtime.run_mod_init(&self.content_root) {
                    log::error!("[Scripting] mod_init failed: {err}");
                } else {
                    let has_manifest = self.script_runtime.mod_manifest().is_some();
                    if let Some(manifest) = self.script_runtime.mod_manifest_mut() {
                        // Drain entity-type descriptors from the validated
                        // `setupMod()` return value into the engine-global
                        // `DataRegistry`. Runtime parses; caller owns lifecycle.
                        // See: context/lib/boot_sequence.md §3.
                        let mut data_registry = self.script_ctx.data_registry.borrow_mut();
                        for desc in std::mem::take(&mut manifest.entities) {
                            data_registry.upsert_entity_type(desc);
                        }
                        drop(data_registry);
                    }

                    if self
                        .state_store_lifecycle
                        .should_restore_after_mod_init(has_manifest)
                    {
                        let state_path = Path::new(STATE_FILE_PATH);
                        match load_persisted_state(state_path) {
                            Ok(Some(persisted)) => {
                                let warnings = overlay_persisted_state(
                                    &mut self.script_ctx.slot_table.borrow_mut(),
                                    &persisted,
                                );
                                for warning in warnings {
                                    log::warn!("[State] {warning}");
                                }
                                log::info!(
                                    "[State] restored persistent slots from {}",
                                    state_path.display()
                                );
                            }
                            Ok(None) => {}
                            Err(error) => log::warn!(
                                "[State] failed to load persistent slots from {}: {error}; using declared defaults",
                                state_path.display()
                            ),
                        }
                        self.state_store_lifecycle.mark_restore_completed();
                    }
                }
                // Hot-reload watcher (debug-only); release builds no-op.
                if let Err(err) = self
                    .script_runtime
                    .start_watcher(&script_root, &self.content_root)
                {
                    log::error!("[Scripting] start_watcher failed: {err}");
                }
                self.mod_timings.record("mod_init");

                // Mod-side override wiring lands with the mod system; today
                // `pending_splash_override` is always `None`. The branch is
                // here so the flow is complete the moment the hook arrives.
                if let Some(source) = self.pending_splash_override.take() {
                    match render::splash::load_splash(&source) {
                        Ok(loaded) => {
                            if let Some(renderer) = self.renderer.as_mut() {
                                let dims = renderer.install_splash_from_loaded(&loaded);
                                log::info!("[Engine] Mod splash loaded: {}×{}", dims[0], dims[1]);
                            }
                            self.mod_timings.record("mod_splash_swap");
                        }
                        Err(err) => {
                            log::error!("[Engine] mod splash override failed: {err:#}");
                        }
                    }
                }

                log::info!("{}", self.mod_timings.summary());

                // Spawn the level worker. PRL parse + texture decode + UV
                // normalize run off the main thread so the splash keeps
                // painting through the wait. Reset the cursor here so the
                // first stage absorbs only worker-spawn overhead, not the
                // full App construction-to-now gap.
                self.level_timings = StartupTimings::new();
                let (tx, rx) = mpsc::channel();
                let map_path = PathBuf::from(&self.map_path);
                let handle = spawn_level_worker(map_path, self.content_root.clone(), tx);
                self.level_rx = Some(rx);
                self.level_worker = Some(handle);
                // Recorded after the spawn call so the delta covers channel
                // creation and thread spawn overhead — recording before the
                // spawn would clock a sub-microsecond no-op.
                self.level_timings.record("worker_dispatch");

                self.splash_frame += 1;
                self.request_redraw();
                false
            }
            _ => {
                // Poll the worker channel non-blockingly.
                use std::sync::mpsc::TryRecvError;
                let outcome = match self.level_rx.as_ref() {
                    Some(rx) => match rx.try_recv() {
                        Ok(payload) => Some(payload),
                        Err(TryRecvError::Empty) => None,
                        Err(TryRecvError::Disconnected) => {
                            log::error!("[Loader] worker channel disconnected before delivery");
                            // Worker panicked — clear both handles so the
                            // engine doesn't loop forever in Splash.
                            // Mirror the Err(e) branch below.
                            self.level_rx = None;
                            self.level_worker = None;
                            None
                        }
                    },
                    None => None,
                };

                match outcome {
                    Some(Ok(payload)) => {
                        self.level_timings.record("worker_delivered");
                        // Splice worker-thread entries between dispatch and
                        // delivered so the summary reads chronologically.
                        let delivered_idx = self.level_timings.entries.len() - 1;
                        let mut worker_entries = payload.timings;
                        // Insert at `delivered_idx + i` rather than appending; each prior
                        // insert shifts the delivered sentinel forward by one, so incrementing
                        // the offset keeps chronological order.
                        for (i, entry) in worker_entries.drain(..).enumerate() {
                            self.level_timings.entries.insert(delivered_idx + i, entry);
                        }
                        // Drop the receiver/handle now — the worker is done.
                        self.level_rx = None;
                        self.level_worker = None;

                        match payload.level {
                            Some(world) => {
                                self.install_level_payload(world, payload.prm_cache_root);
                                if let Some(renderer) = self.renderer.as_mut() {
                                    renderer.clear_splash();
                                }
                                self.boot_state = BootState::Running;
                                // Defer log line C until after the first level
                                // frame's render returns, so `first_level_frame`
                                // captures GPU work the user actually sees.
                                self.pending_level_log = true;
                                // Fall through — the caller paints the first
                                // real level frame this redraw.
                                true
                            }
                            None => {
                                log::warn!(
                                    "[Loader] worker delivered no level payload — staying in splash",
                                );
                                self.paint_splash(event_loop);
                                self.request_redraw();
                                false
                            }
                        }
                    }
                    Some(Err(err)) => {
                        log::error!("[Loader] worker failed: {err:#} — staying in splash");
                        self.level_rx = None;
                        self.level_worker = None;
                        self.paint_splash(event_loop);
                        self.request_redraw();
                        false
                    }
                    None => {
                        // Still loading; keep painting the splash.
                        self.paint_splash(event_loop);
                        self.request_redraw();
                        false
                    }
                }
            }
        }
    }

    /// Paint a single splash-phase frame via `UiPass::encode`. Always clears
    /// to black. When a splash descriptor is installed, the pass also records
    /// a fullscreen background fill, a framed 9-slice panel, the centered logo
    /// image, and a shaped-text line as instanced quads plus glyphon text. The
    /// first frame has no descriptor installed yet and renders only the clear.
    fn paint_splash(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(renderer) = self.renderer.as_mut() {
            if !renderer.is_ready() {
                return;
            }
            // Drive the input-dispatch seam from the active splash descriptor's
            // capture mode (splash is non-interactive -> Passthrough). No-op when
            // no splash is installed (frame 0). Locks the Task 5 seam wiring.
            if let Some(mode) = renderer.splash_capture_mode() {
                self.ui_dispatch.set_mode(mode);
            }
            // Publish the once-per-frame read snapshot just before the render
            // call (the splash phase render path). The version/tagline line the
            // shaped-text element renders rides through here, exercising the
            // once-per-frame contract with a real value.
            renderer.set_ui_snapshot(render::ui::UiReadSnapshot::with_version_line(
                splash_version_line(),
            ));
            if let Err(err) = renderer.render_splash_frame() {
                self.exit_result = Err(err);
                event_loop.exit();
            }
        }
    }

    fn request_redraw(&self) {
        if let Some(ws) = self.window_state.as_ref() {
            ws.window.request_redraw();
        }
    }

    /// Snapshot the live slot table into a frozen dotted-name → value map for the
    /// frame's UI read snapshot. Cloning here decouples the renderer from the live
    /// `SlotTable`: game logic mutates the store, the renderer reads this copy, so
    /// the renderer never borrows engine-side state (renderer/game-logic boundary).
    /// Built once per frame after game logic and before render. Slots without a
    /// current value are skipped, so every entry carries a resolved value.
    ///
    /// Takes the table directly rather than `&self`: the call site holds a mutable
    /// borrow of `self.renderer`, and a `&self` receiver here would conflict with
    /// it. Borrowing only `self.script_ctx.slot_table` keeps the two field borrows
    /// disjoint.
    fn build_ui_slot_snapshot(
        slot_table: &scripting::slot_table::SlotTable,
    ) -> std::collections::HashMap<String, scripting::slot_table::SlotValue> {
        slot_table
            .iter()
            .filter_map(|(name, record)| {
                record.value.clone().map(|value| (name.to_string(), value))
            })
            .collect()
    }

    /// Install a delivered level payload on the main thread: GPU texture
    /// upload (from baked `.prm` mip sidecars), UV normalization, GPU geometry
    /// upload, bridge / fog / collision populate, classname dispatch, data
    /// script, archetype sweep, and `levelLoad` fire. Each stage is recorded
    /// into `self.level_timings` for log line C.
    ///
    /// Texture upload now runs before geometry upload: `.prm` slot dimensions
    /// drive UV normalization, so the renderer must have produced
    /// `LoadedTexture`s before the per-leaf texel-space UVs can be converted
    /// to `[0,1]`.
    ///
    /// Called from the `Splash` state machine on worker delivery; assumes
    /// `self.renderer` is `Some` and `world` is populated.
    fn install_level_payload(&mut self, mut world: prl::LevelWorld, prm_cache_root: PathBuf) {
        // Reset world gravity to the freshly-loaded level's authored value
        // before the data script runs, so any `world.getGravity()` call
        // inside `setupLevel` / `levelLoad` reactions sees the new value.
        self.script_ctx.gravity.set(world.initial_gravity);
        self.active_wieldable = None;
        self.active_wieldable_descriptor = None;

        // Derive material properties from texture names so the renderer can
        // populate per-material uniforms (shininess) without re-parsing.
        let texture_materials: Vec<crate::material::Material> = {
            let mut warned = std::collections::HashSet::new();
            world
                .texture_names
                .iter()
                .map(|n| crate::material::derive_material(n, &mut warned))
                .collect()
        };

        let renderer = match self.renderer.as_mut() {
            Some(r) => r,
            None => {
                log::error!("[Engine] install_level_payload called with no renderer");
                self.level = Some(world);
                return;
            }
        };

        // 1. Textures first — uploaded from the .prm sidecars; their slot
        //    dimensions feed the UV normalize pass.
        renderer.install_textures(
            &world.texture_names,
            &world.texture_cache_keys,
            &prm_cache_root,
            &texture_materials,
        );
        self.level_timings.record("texture_upload");

        // 2. UV normalize using freshly-uploaded diffuse-texture dimensions.
        //    Texel-space UVs on the worker side; converted to `[0,1]` here so
        //    install_level_geometry uploads the final values.
        renderer.normalize_world_uvs(&mut world);
        self.level_timings.record("uv_normalize");

        // 3. Now geometry: vertex_buffer + index_buffer upload to GPU.
        let geometry = render::level_world_to_geometry(&world, &texture_materials);
        renderer.install_level_geometry(&geometry);
        self.level_timings.record("geometry_upload");

        // Reseed the SH diagnostic per-light visibility bitmap to match the
        // freshly-installed level's animated-light count. Reset `seeded` so the
        // panel re-pulls defaults on the next open.
        #[cfg(feature = "dev-tools")]
        if let Some(debug_ui) = self.debug_ui.as_mut() {
            let delta_count = renderer.sh_delta_volumes().len();
            debug_ui.sh_diagnostics_state.per_light_visible.clear();
            debug_ui
                .sh_diagnostics_state
                .per_light_visible
                .resize(delta_count, false);
            debug_ui.sh_diagnostics_state.seeded = false;
        }

        // Stash the world after the mutations so downstream code paths that
        // read from `self.level` see the normalized vertices.
        self.level = Some(world);

        // One `LightComponent` entity per map-authored light; stable
        // `EntityId`s the bridge's dirty tracker keys off for the level's
        // lifetime.
        {
            let level_lights = renderer.level_lights().to_vec();
            let fgd_sample_float_count = (renderer.scripted_sample_byte_offset() / 4) as u32;
            let mut registry = self.script_ctx.registry.borrow_mut();
            self.light_bridge.populate_from_level(
                &level_lights,
                &mut registry,
                fgd_sample_float_count,
            );
        }

        // Fog volumes — one entity per record + a renderer-side pixel-scale
        // push. Done after light bridge populate so the registry's first fog
        // entity-id always lands after the light entities.
        if let Some(world) = self.level.as_ref() {
            let mut registry = self.script_ctx.registry.borrow_mut();
            self.fog_volume_bridge
                .populate_from_level(&mut registry, &world.fog_volumes);
            renderer.set_fog_pixel_scale(world.fog_pixel_scale);
            renderer.install_fog_cell_masks_for_level(world.fog_cell_masks.clone());
        }

        // Populate before the first game tick so movement collision is ready.
        if let Some(world) = self.level.as_ref() {
            self.collision_world.populate_from_level(world);
        }
        self.level_timings.record("bridges_populated");

        // Sound registry follows level lifetime, parallel to textures: load the
        // level's sounds from `sounds/` here, release them at unload. Fault-
        // tolerant — a missing directory or undecodable file warns and is
        // skipped. Silent if audio init failed (`audio` is `None`).
        if let Some(audio) = &mut self.audio {
            audio.load_level_sounds(&self.content_root);
        }
        self.level_timings.record("audio_load");

        // Sweep map entities through classname dispatch. The returned set of
        // handled classnames is stashed and consumed by the data-archetype
        // sweep below, after the data script populates `data_registry.entities`.
        if let Some(world) = self.level.as_ref() {
            let mut registry = self.script_ctx.registry.borrow_mut();
            let all_entities: Vec<crate::scripting::map_entity::MapEntity> =
                world.map_entities.iter().cloned().map(Into::into).collect();
            let (spawn_points, map_entities): (Vec<_>, Vec<_>) = all_entities
                .into_iter()
                .partition(|e| e.classname == PLAYER_START_CLASSNAME);
            self.pending_spawn_points = Some(spawn_points);
            let handled =
                apply_classname_dispatch(&map_entities, &self.classname_dispatch, &mut registry);
            if !map_entities.is_empty() {
                log::info!(
                    "[Loader] dispatched {total} map entities; {built_in} classname(s) handled by built-in handlers",
                    built_in = handled.len(),
                    total = map_entities.len(),
                );
            }
            self.builtin_handled = Some(handled);
            self.pending_map_entities = Some(map_entities);
        }
        self.level_timings.record("classname_dispatch");

        // Level-load model sweep. Classname dispatch above spawned a
        // `MeshComponent` entity per `prop_mesh` placement; now collect the
        // distinct `model` handles off those entities and load + upload each
        // exactly once into the renderer's model cache (renderer owns GPU). This
        // runs at level-load time, never mid-frame, so there is no in-frame
        // hitch. The model handle is the renderer cache key the per-frame draw
        // planner groups by, so it is passed VERBATIM as the cache key; the glTF
        // file itself is opened from `content_root.join(handle)` inside
        // `load_skinned_model` (open path and cache key are decoupled — every
        // other asset joins the content root, but the key must stay the raw
        // handle the planner looks up). A failed/invalid load is non-fatal:
        // `load_skinned_model` already `warn!`s naming the path and returns
        // `None`, the entity then renders nothing, and the load continues.
        {
            let models = {
                let registry = self.script_ctx.registry.borrow();
                distinct_mesh_models(&registry)
            };
            for model in &models {
                renderer.load_skinned_model(model, &self.content_root, &prm_cache_root);
            }
            if !models.is_empty() {
                log::info!(
                    "[Model] uploaded {} distinct mesh model(s) for this level",
                    models.len(),
                );
            }
        }
        self.level_timings.record("model_load");

        // Register sprite collections for every distinct `sprite` name in
        // the registry. Covers map-spawned emitters; descriptor-spawned
        // emitters get a second pass after the data script runs.
        let texture_root = self.content_root.join("textures");
        {
            use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
            use crate::scripting::registry::{ComponentKind, ComponentValue};
            let registry = self.script_ctx.registry.borrow();
            let mut registered: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for (_id, value) in registry.iter_with_kind(ComponentKind::BillboardEmitter) {
                let ComponentValue::BillboardEmitter(c) = value else {
                    continue;
                };
                let _: &BillboardEmitterComponent = c;
                let collection = c.sprite.clone();
                if collection.is_empty() || !registered.insert(collection.clone()) {
                    continue;
                }
                let frames = fx::smoke::load_collection_frames(&texture_root, &collection)
                    .unwrap_or_else(|| {
                        vec![fx::smoke::SpriteFrame {
                            data: vec![255, 255, 255, 255],
                            width: 1,
                            height: 1,
                        }]
                    });
                renderer.register_smoke_collection(&collection, &frames, 0.3, c.lifetime);
                self.particle_render.register_sprite(&collection);
            }
        }

        // Data script runs once at level open. Errors surface as an empty
        // manifest so the level still loads.
        if let Some(world) = &self.level {
            if let Some(data_script) = &world.data_script {
                let mut manifest = self
                    .script_runtime
                    .run_data_script(data_script, &self.content_root);
                manifest.reactions =
                    validate_sequence_primitives(manifest.reactions, &self.sequence_registry);
                self.script_ctx
                    .data_registry
                    .borrow_mut()
                    .populate_from_manifest(manifest);
                self.progress_tracker.initialize(
                    &self.script_ctx.data_registry.borrow(),
                    &self.script_ctx.registry.borrow(),
                );
            }
        }
        self.level_timings.record("data_script");

        // Data-archetype sweep: `data_registry.entities` was populated from
        // `setupMod()`'s return value at mod-init. Materialize every matching
        // map placement that the built-in dispatch did not already handle.
        if self.level.is_some() {
            let handled = self.builtin_handled.take().unwrap_or_default();
            let descriptors = self.script_ctx.data_registry.borrow().entities.clone();
            let mut registry = self.script_ctx.registry.borrow_mut();
            let map_entities = self.pending_map_entities.take().unwrap_or_default();
            let descriptor_handled =
                apply_data_archetype_dispatch(&map_entities, &descriptors, &handled, &mut registry);
            if !descriptor_handled.is_empty() {
                log::info!(
                    "[Loader] dispatched {} map entities through descriptor archetypes",
                    descriptor_handled.len(),
                );
            }

            // Capture the first spawn-point position and facing before take() consumes
            // the vec. Camera move is independent of spawn success — failures inside
            // spawn_from_player_starts log and continue; we still teleport so the user
            // isn't stranded at the boot placeholder.
            let first_spawn: Option<(glam::Vec3, glam::Vec3)> = self
                .pending_spawn_points
                .as_ref()
                .and_then(|v| v.first())
                .map(|e| (e.origin, e.angles));

            // Spawn one entity per `player_spawn` placement, routing
            // each through its `entity_class` (default `"player"`).
            let (active_wieldable, active_wieldable_descriptor) =
                match self.pending_spawn_points.take() {
                    Some(spawn_points) if !spawn_points.is_empty() => {
                        let result =
                            spawn_from_player_starts(&spawn_points, &descriptors, &mut registry);
                        (result.active_wieldable, result.active_wieldable_descriptor)
                    }
                    _ => {
                        log::info!("[Loader] no player_spawn in map; skipping player spawn");
                        (None, None)
                    }
                };
            // Drop the registry borrow before touching `self.level` / `self.camera`.
            drop(registry);
            self.active_wieldable = active_wieldable;
            self.active_wieldable_descriptor = active_wieldable_descriptor;

            if let Some((pos, angles)) = first_spawn {
                self.camera.position = pos;
                // angles is engine-convention radians (YXZ): x=pitch, y=yaw.
                self.camera.yaw = angles.y;
                self.camera.pitch = angles.x;
                self.frame_timing.push_state(InterpolableState::new(pos));
            } else if let Some(world) = self.level.as_ref() {
                // Fallback when no player_spawn: center on level geometry.
                self.camera.position = world.spawn_position();
                self.frame_timing
                    .push_state(InterpolableState::new(self.camera.position));
            }

            // Re-borrow for the dynamic-light absorb step below.
            let registry = self.script_ctx.registry.borrow();

            // Pick up any descriptor-spawned `LightComponent`s so they
            // participate in the per-frame light bridge pack.
            self.light_bridge.absorb_dynamic_lights(&registry);
        }

        // Descriptor-spawned emitters may carry sprite collections not seen
        // during the install-time sweep above. Re-register any new
        // collections so the renderer pass has them ready before the first
        // frame draws.
        if let Some(renderer) = self.renderer.as_mut() {
            use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
            use crate::scripting::registry::{ComponentKind, ComponentValue};
            let texture_root = self.content_root.join("textures");
            let registry = self.script_ctx.registry.borrow();
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for (_id, value) in registry.iter_with_kind(ComponentKind::BillboardEmitter) {
                let ComponentValue::BillboardEmitter(c) = value else {
                    continue;
                };
                let _: &BillboardEmitterComponent = c;
                let collection = c.sprite.clone();
                if collection.is_empty() || !seen.insert(collection.clone()) {
                    continue;
                }
                let frames = fx::smoke::load_collection_frames(&texture_root, &collection)
                    .unwrap_or_else(|| {
                        vec![fx::smoke::SpriteFrame {
                            data: vec![255, 255, 255, 255],
                            width: 1,
                            height: 1,
                        }]
                    });
                renderer.register_smoke_collection(&collection, &frames, 0.3, c.lifetime);
                self.particle_render.register_sprite(&collection);
            }

            let collection = weapon::impact_sprite_collection();
            let frames = fx::smoke::load_collection_frames(&texture_root, collection)
                .unwrap_or_else(|| {
                    vec![fx::smoke::SpriteFrame {
                        data: vec![255, 255, 255, 255],
                        width: 1,
                        height: 1,
                    }]
                });
            renderer.register_smoke_collection(
                collection,
                &frames,
                0.45,
                weapon::impact_lifetime(),
            );
            self.particle_render.register_sprite(collection);
        }
        self.level_timings.record("archetype_sweep");

        fire_named_event_with_sequences(
            "levelLoad",
            &self.script_ctx.data_registry.borrow(),
            &self.sequence_registry,
            &self.reaction_registry,
            &self.script_ctx,
        );
        self.level_timings.record("level_load_event");
        self.script_time = 0.0;
    }

    /// Drive `movement::tick` for every entity carrying a
    /// `PlayerMovementComponent`. Returns the list of event names to fire.
    /// Each entity contributes at most one `landed` and one `jumped` entry per
    /// tick; multiple entities each contribute independently (no cross-entity
    /// deduplication). The caller accumulates these across ticks and drains
    /// them after the tick loop so reactions see the fully-settled post-tick
    /// world state.
    // Threads the per-tick movement inputs (axes + edge/level bits) into the
    // movement substrate; each is an independent signal, not a bundle worth a struct.
    #[allow(clippy::too_many_arguments)]
    fn run_movement_tick(
        &mut self,
        forward_axis: f32,
        right_axis: f32,
        jump_pressed: bool,
        dash_pressed: bool,
        crouch_intent: bool,
        running: bool,
        tick_dt: f32,
    ) -> Vec<&'static str> {
        use crate::movement::{MovementInput, tick as movement_tick};
        use crate::scripting::components::player_movement::PlayerMovementComponent;
        use crate::scripting::registry::{ComponentKind, ComponentValue, EntityId, Transform};

        let mut events_out: Vec<&'static str> = Vec::new();
        let mut snapshots: Vec<(EntityId, PlayerMovementComponent, Vec3)> = Vec::new();
        {
            let registry = self.script_ctx.registry.borrow();
            for (id, value) in registry.iter_with_kind(ComponentKind::PlayerMovement) {
                let ComponentValue::PlayerMovement(component) = value else {
                    continue;
                };
                let position = match registry.get_component::<Transform>(id) {
                    Ok(t) => t.position,
                    Err(_) => continue,
                };
                snapshots.push((id, component.clone(), position));
            }
        }

        if snapshots.is_empty() {
            return events_out;
        }

        let gravity = self.script_ctx.gravity.get();
        let input = MovementInput {
            wish_dir: glam::Vec2::new(right_axis, forward_axis),
            jump_pressed,
            dash_pressed,
            running,
            crouch_intent,
            facing_yaw: self.camera.yaw,
        };

        let mut registry = self.script_ctx.registry.borrow_mut();
        for (id, mut component, position) in snapshots {
            let (new_pos, events) = movement_tick(
                &mut component,
                &input,
                &self.collision_world,
                gravity,
                tick_dt,
                position,
            );
            if let Ok(transform) = registry.get_component::<Transform>(id) {
                let mut t = *transform;
                t.position = new_pos;
                let _ = registry.set_component(id, t);
            }
            let _ = registry.set_component(id, component);
            if events.landed {
                events_out.push("landed");
            }
            if events.jumped {
                events_out.push("jumped");
            }
        }

        events_out
    }

    fn run_weapon_fire_tick(
        &mut self,
        snapshot: &input::ActionSnapshot,
        tick_dt: f32,
    ) -> Vec<&'static str> {
        let mut registry = self.script_ctx.registry.borrow_mut();
        let events = weapon::tick(
            &mut registry,
            self.active_wieldable,
            snapshot,
            &self.camera,
            &self.collision_world,
            tick_dt,
        );
        if let Some(impact) = events.impact {
            weapon::spawn_impact_effect_at(&mut registry, impact.point, impact.normal);
        }
        events.event_names()
    }

    /// Transition input focus, acquiring or releasing the cursor as required
    /// and clearing carry-over input state so keys/mouse held during the
    /// transition do not stick in the new mode.
    fn set_input_focus(&mut self, focus: InputFocus) {
        self.input_focus = focus;
        let Some(ws) = self.window_state.as_ref() else {
            return;
        };
        match focus {
            InputFocus::Gameplay => {
                input::cursor::capture_cursor(&ws.window);
            }
            InputFocus::DevTools | InputFocus::Menu => {
                input::cursor::release_cursor(&ws.window);
            }
        }
        // Both directions clear: returning to Gameplay must not see keys that
        // were "held" by a UI consumer; entering UI must not leak gameplay
        // chords into the overlay.
        //
        // Known minor UX gap: on Gameplay → DevTools, modifiers are cleared
        // even if Alt+Shift are still physically held (the chord that opened
        // the panel). Closing the panel without releasing requires re-pressing
        // those modifiers. Accepted because the symmetric stale-state
        // protection is worth more than the one-keystroke regression.
        self.input_system.clear_all();
        self.gameplay_input_latch.clear();
        self.diagnostic_inputs.clear_modifiers();
    }

    /// Release pointer lock as part of the exit path. Does not mutate
    /// `input_focus` — exiting is not a UI state and future menu code that
    /// inspects `input_focus == Menu` should not see a false positive here.
    fn release_cursor_for_exit(&self) {
        if let Some(ws) = self.window_state.as_ref() {
            input::cursor::release_cursor(&ws.window);
        }
    }

    /// Re-apply the current focus's cursor state without changing the stored
    /// focus. Called on window re-focus so the cursor mode matches the user's
    /// chosen focus after transient OS focus loss.
    fn reapply_focus(&mut self) {
        let Some(ws) = self.window_state.as_ref() else {
            return;
        };
        match self.input_focus {
            InputFocus::Gameplay => input::cursor::capture_cursor(&ws.window),
            InputFocus::DevTools | InputFocus::Menu => input::cursor::release_cursor(&ws.window),
        }
    }

    fn handle_diagnostic_action(&mut self, action: DiagnosticAction) {
        match action {
            DiagnosticAction::ToggleWireframe => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.toggle_wireframe();
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
            // Real-device audio smoke check: play the test tone on the SFX bus
            // so an operator can confirm output reaches the OS. Guarded for the
            // silent (init-failed) case; needs a level loaded for the sound
            // registry to hold the fixture, otherwise `play` warns gracefully.
            DiagnosticAction::PlayTestSfx => {
                if let Some(audio) = &mut self.audio {
                    audio.play(audio::SoundRequest {
                        bus: "sfx".to_string(),
                        sound: "sfx/test_tone".to_string(),
                        looping: false,
                    });
                    log::info!("[Audio] smoke check: played sfx/test_tone on SFX bus");
                }
            }
            // Toggle just flips visibility and shifts InputFocus to gate
            // game input. Lazy GPU init happens inside `render_debug_ui` on
            // the renderer the first time the panel paints; no explicit init
            // call is needed here.
            #[cfg(feature = "dev-tools")]
            DiagnosticAction::ToggleDebugPanel => {
                let now_visible = if let Some(debug_ui) = self.debug_ui.as_mut() {
                    let v = !debug_ui.is_visible();
                    debug_ui.set_visible(v);
                    v
                } else {
                    return;
                };
                self.set_input_focus(if now_visible {
                    InputFocus::DevTools
                } else {
                    InputFocus::Gameplay
                });
            }
        }
    }
}

// --- Tests ---
//
// Pins for the render-rate look / tick-rate sim split:
//
// - On a frame with `ticks == 0`, mouse delta accumulated this frame must
//   still rotate the camera *and* change the rendered view-projection
//   matrix. Yaw/pitch reach the matrix as arguments to `view_projection`,
//   not as fields of `InterpolableState`, so a yaw assertion alone does
//   not cover the rendering path — a matrix assertion is required.
// - On a multi-tick frame, look rotation applies once at render rate,
//   not once per tick.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_timing::TICK_DURATION;
    use crate::input::{InputSystem, default_bindings};
    use crate::options::CrouchMode;

    // --- resolve_crouch_intent (input-layer toggle/hold derivation) ---

    #[test]
    fn crouch_hold_tracks_button_level() {
        // Hold mode: the resolved bit mirrors the button's active level
        // (Pressed | Held), and the latch is never consulted/mutated.
        let mut latch = false;
        assert!(resolve_crouch_intent(
            CrouchMode::Hold,
            ButtonState::Pressed,
            &mut latch
        ));
        assert!(resolve_crouch_intent(
            CrouchMode::Hold,
            ButtonState::Held,
            &mut latch
        ));
        assert!(!resolve_crouch_intent(
            CrouchMode::Hold,
            ButtonState::Released,
            &mut latch
        ));
        assert!(!resolve_crouch_intent(
            CrouchMode::Hold,
            ButtonState::Inactive,
            &mut latch
        ));
        // Latch is inert in hold mode.
        assert!(!latch);
    }

    #[test]
    fn crouch_toggle_flips_on_press_rising_edge() {
        let mut latch = false;
        // First press latches ON.
        assert!(resolve_crouch_intent(
            CrouchMode::Toggle,
            ButtonState::Pressed,
            &mut latch
        ));
        // Held does not re-flip — the latch stays ON across the hold.
        assert!(resolve_crouch_intent(
            CrouchMode::Toggle,
            ButtonState::Held,
            &mut latch
        ));
        // Release does not flip either.
        assert!(resolve_crouch_intent(
            CrouchMode::Toggle,
            ButtonState::Released,
            &mut latch
        ));
        assert!(resolve_crouch_intent(
            CrouchMode::Toggle,
            ButtonState::Inactive,
            &mut latch
        ));
        // A SECOND press (fresh rising edge) latches OFF.
        assert!(!resolve_crouch_intent(
            CrouchMode::Toggle,
            ButtonState::Pressed,
            &mut latch
        ));
        // ...and stays off while held.
        assert!(!resolve_crouch_intent(
            CrouchMode::Toggle,
            ButtonState::Held,
            &mut latch
        ));
    }

    /// Epsilon for angle and matrix-element comparisons. Mouse-driven yaw
    /// deltas at default sensitivity land around 1e-1 radians, so 1e-5 is
    /// comfortably tight without being flaky on f32 round-off.
    const EPSILON: f32 = 1e-5;

    /// On a frame with zero ticks, accumulated mouse delta must rotate the
    /// camera *and* change the rendered view-projection matrix. Both checks
    /// are required: `view_projection` takes yaw/pitch as arguments, so an
    /// updated `camera.yaw` alone does not prove rendering sees it.
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
        // frame_dt — the frame shape the look path must handle.
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

    #[test]
    fn content_root_from_map_returns_grandparent_for_standard_path() {
        assert_eq!(
            content_root_from_map("content/dev/maps/campaign-test.prl"),
            PathBuf::from("content/dev"),
        );
    }

    #[test]
    fn content_root_from_map_returns_grandparent_for_mod_path() {
        assert_eq!(
            content_root_from_map("content/base/maps/e1m1.prl"),
            PathBuf::from("content/base"),
        );
    }

    // Regression: `Path::new("maps/test.prl").parent().and_then(parent)` returns
    // `Some("")` (an empty path), not `None`, so the prior `unwrap_or` fallback
    // was bypassed and the function returned `""` instead of `"."`.
    #[test]
    fn content_root_from_map_returns_dot_for_single_segment_parent() {
        assert_eq!(content_root_from_map("maps/test.prl"), PathBuf::from("."));
    }

    #[test]
    fn content_root_from_map_returns_dot_for_bare_filename() {
        assert_eq!(content_root_from_map("test.prl"), PathBuf::from("."));
    }

    #[test]
    fn dependency_reload_requests_rerun_mod_init() {
        // Dependency classification happens in ScriptRuntime; the frame loop
        // queues staged mod-init only for paths that matched that active set.
        assert!(reload_summary_requires_mod_init(ReloadSummary {
            mod_init: true,
        }));
        assert!(!reload_summary_requires_mod_init(ReloadSummary::default()));
    }

    /// Mirrors the consumed-event gate in `window_event` for keyboard input:
    /// when egui reports `consumed`, only the `ToggleDebugPanel` chord is
    /// allowed to fire; every other resolved diagnostic action is dropped and
    /// no input-system forwarding happens.
    ///
    /// This is a unit test of the gate's *decision* — exercising the full
    /// `App::window_event` path would require a window and GPU, which tests
    /// run without (see context/lib/testing_guide.md §3).
    #[cfg(feature = "dev-tools")]
    #[test]
    fn consumed_event_gate_passes_only_toggle_debug_panel() {
        use crate::input::{DiagnosticAction, DiagnosticInputs, default_diagnostic_chords};
        use winit::keyboard::KeyCode;

        // Helper mirroring the consumed-branch decision in `window_event`:
        // returns `Some(action)` only if the chord is `ToggleDebugPanel`.
        fn consumed_gate(
            diagnostics: &mut DiagnosticInputs,
            code: KeyCode,
            pressed: bool,
            repeat: bool,
        ) -> Option<DiagnosticAction> {
            diagnostics
                .handle_key(code, pressed, repeat)
                .filter(|a| *a == DiagnosticAction::ToggleDebugPanel)
        }

        let mut diagnostics = DiagnosticInputs::new(default_diagnostic_chords());
        // Modifier-only events are still forwarded so the resolver's
        // Alt+Shift state stays current under the consumed gate.
        diagnostics.handle_key(KeyCode::ShiftLeft, true, false);
        diagnostics.handle_key(KeyCode::AltLeft, true, false);

        // Alt+Shift+Backslash (ToggleWireframe) — dropped by the gate.
        let blocked = consumed_gate(&mut diagnostics, KeyCode::Backslash, true, false);
        assert_eq!(
            blocked, None,
            "consumed-event gate must suppress non-toggle diagnostic chords",
        );

        // Alt+Shift+Backquote (ToggleDebugPanel) — passes the gate.
        let allowed = consumed_gate(&mut diagnostics, KeyCode::Backquote, true, false);
        assert_eq!(
            allowed,
            Some(DiagnosticAction::ToggleDebugPanel),
            "consumed-event gate must allow ToggleDebugPanel through",
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

    // --- Level-load model sweep (distinct-model dedup) ---
    //
    // After classname dispatch spawns one `MeshComponent` entity per `prop_mesh`
    // placement, the sweep collects the distinct `model` handles and uploads each
    // exactly once. `distinct_mesh_models` is the GPU-free collection half — the
    // upload itself needs a GPU (untestable per testing_guide), so we pin the
    // dedup/collection as pure logic here. Empty handles (absent/empty `model`)
    // have nothing to upload and are skipped.

    fn spawn_mesh_entity(registry: &mut crate::scripting::registry::EntityRegistry, model: &str) {
        use crate::scripting::components::mesh::MeshComponent;
        use crate::scripting::registry::Transform;

        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                MeshComponent {
                    model: model.to_string(),
                },
            )
            .expect("freshly spawned id is live");
    }

    #[test]
    fn distinct_mesh_models_dedups_repeated_handles() {
        use crate::scripting::registry::EntityRegistry;

        let mut registry = EntityRegistry::new();
        spawn_mesh_entity(&mut registry, "models/a/scene.gltf");
        spawn_mesh_entity(&mut registry, "models/b/scene.gltf");
        spawn_mesh_entity(&mut registry, "models/a/scene.gltf");

        let models = distinct_mesh_models(&registry);
        // Two distinct paths despite three entities — each path uploads once.
        assert_eq!(models.len(), 2);
        assert!(models.contains(&"models/a/scene.gltf".to_string()));
        assert!(models.contains(&"models/b/scene.gltf".to_string()));
    }

    #[test]
    fn distinct_mesh_models_skips_empty_handles() {
        use crate::scripting::registry::EntityRegistry;

        // A `prop_mesh` with an absent/empty `model` spawns with an empty handle
        // (logged at spawn); there is nothing to upload, so the sweep skips it.
        let mut registry = EntityRegistry::new();
        spawn_mesh_entity(&mut registry, "");
        spawn_mesh_entity(&mut registry, "models/a/scene.gltf");

        let models = distinct_mesh_models(&registry);
        assert_eq!(models, vec!["models/a/scene.gltf".to_string()]);
    }

    #[test]
    fn distinct_mesh_models_empty_when_no_mesh_entities() {
        use crate::scripting::registry::EntityRegistry;

        let registry = EntityRegistry::new();
        assert!(distinct_mesh_models(&registry).is_empty());
    }

    #[test]
    fn malformed_gltf_load_returns_err() {
        // The loader contract the degrade AC rides on: a bad/missing model path
        // is `Err`, not a panic — `load_skinned_model` turns that `Err` into a
        // `warn!` + `None`, so the level-load model sweep continues and the
        // `prop_mesh` entity simply renders nothing.
        let bad = std::path::Path::new("definitely/not/a/real/model.gltf");
        assert!(
            crate::model::gltf_loader::load_model(bad).is_err(),
            "loading a missing glTF must return Err, never panic",
        );
    }

    // --- build_ui_slot_snapshot (state-store → UI read-snapshot boundary) ---

    #[test]
    fn ui_slot_snapshot_clones_present_values_and_skips_valueless_slots() {
        use crate::scripting::slot_table::SlotValue;

        // The default table carries engine `player.*` slots with `None` values.
        // Setting one slot is enough to assert the boundary contract: the
        // snapshot clones value-bearing slots and omits value-less ones, so the
        // renderer reads a present key only when it carries a resolved value.
        let mut table = crate::scripting::slot_table::SlotTable::new();
        table
            .get_mut("player.health")
            .expect("default table declares player.health")
            .value = Some(SlotValue::Number(75.0));

        let snapshot = App::build_ui_slot_snapshot(&table);

        assert_eq!(
            snapshot.get("player.health"),
            Some(&SlotValue::Number(75.0)),
            "value-bearing slot is cloned into the snapshot",
        );
        // Every other default slot starts value-less and must be excluded, so
        // the only present key is the one we set.
        assert_eq!(
            snapshot.len(),
            1,
            "value-less slots are skipped; only the set slot appears",
        );
    }
}
