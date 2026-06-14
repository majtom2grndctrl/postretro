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
// The runtime nav query surface is consumed only by the dev-tools navmesh
// overlay today; the future baked-pathfinding plan extends it. Allow dead code
// so shipping (non-`dev-tools`) builds stay warning-free until that lands.
#[allow(dead_code)]
mod nav;
mod options;
mod weapon;

mod portal_vis;
mod prl;
mod render;
mod scripting;
mod shadow_cull;
mod startup;
mod ui_texture;
mod view_feel;
mod visibility;

// Rooted here (not under `scripting/`) so `gen_script_types.rs` can reuse the
// `scripting` tree via `#[path]` without pulling in wgpu/engine-dependent code.
#[path = "scripting/systems/mod.rs"]
mod scripting_systems;

// Test-only counting global allocator. `#[global_allocator]` must annotate a
// crate-root static, so the static lives here; the allocator type and its
// counters live in `scripting::ir::alloc_probe`. Gated on `#[cfg(test)]` so it
// never touches the production binary — the IR eval pass's zero-allocation
// guarantee is asserted by a test that arms the counters around `eval_value`.
#[cfg(test)]
#[global_allocator]
static COUNTING_ALLOCATOR: scripting::ir::alloc_probe::CountingAllocator =
    scripting::ir::alloc_probe::CountingAllocator;

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
use crate::scripting::components::health::apply_damage;
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
use crate::scripting::reactions::system_commands::{
    SystemReactionCommand, SystemReactionRegistry, register_system_reaction_primitives,
};
use crate::scripting::runtime::{ReloadSummary, ScriptRuntime, ScriptRuntimeConfig};
use crate::scripting::sequence::SequencedPrimitiveRegistry;
use crate::scripting::state_crossings::CrossingDetector;
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

/// Resolve every animated mesh entity's declared state map against the level's
/// clip tables, filling each `AnimationState.clip_index` (name → glTF index). A
/// state naming a clip the model does not carry warns ONCE here (at level load)
/// and stays `clip_index = None` (unusable: switching to it warns + no-ops,
/// switching out of it hard-cuts — both handled by the animation state machine). Stateless `prop_mesh`
/// entities (no animation block) are skipped.
///
/// Runs at level load with a mutable registry, after the model sweep built the
/// clip tables — so every state's index is concrete before the first frame.
fn resolve_mesh_entity_clips(
    registry: &mut crate::scripting::registry::EntityRegistry,
    tables: &scripting_systems::mesh_anim::MeshClipTables,
) {
    use crate::scripting::registry::{ComponentKind, ComponentValue};

    // Collect ids first so the mutable per-entity writes do not alias the
    // immutable iteration borrow. Mesh entity counts are small.
    let animated: Vec<crate::scripting::registry::EntityId> = registry
        .iter_with_kind(ComponentKind::Mesh)
        .filter_map(|(id, value)| match value {
            ComponentValue::Mesh(mesh) if mesh.animation.is_some() => Some(id),
            _ => None,
        })
        .collect();

    for id in animated {
        let Ok(mut component) = registry
            .get_component::<crate::scripting::components::mesh::MeshComponent>(id)
            .cloned()
        else {
            continue;
        };
        let model_name = component.model.clone();
        let handle = crate::model::ModelHandle::from(model_name.clone());
        let Some(anim) = component.animation.as_mut() else {
            continue;
        };
        match tables.get(&handle) {
            Some(table) => {
                let missing =
                    scripting_systems::mesh_anim::resolve_state_clips(&mut anim.states, table);
                for m in &missing {
                    log::warn!(
                        "[Model] animation state '{}' on model '{}' names clip '{}' absent from \
                         the model — state unusable (switching to it no-ops)",
                        m.state,
                        model_name,
                        m.clip,
                    );
                }
            }
            None => {
                // Model never uploaded (load failed): no clips resolve. Warn once
                // for the model, leave every state unresolved.
                log::warn!(
                    "[Model] mesh entity references uncached model '{}' — animation states \
                     unresolved",
                    model_name,
                );
                for state in anim.states.values_mut() {
                    state.clip_index = None;
                }
            }
        }
        let _ = registry.set_component(id, component);
    }
}

// Policy chokepoint: the frame loop queues a staged build only when a changed
// path matched the active mod-init dependency set (classified by ScriptRuntime).
fn reload_summary_requires_mod_init(summary: ReloadSummary) -> bool {
    summary.mod_init
}

/// Version/tagline line the boot splash's shaped-text element renders. Sourced
/// from the build's `CARGO_PKG_VERSION` so the read-handle snapshot carries a
/// real value. Flows through `UiReadSnapshot::version_line`.
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

    // System-reaction handlers (no entity targets) — the second arm of the
    // shared named-reaction vocabulary. They enqueue typed commands onto
    // `script_ctx.system_commands`, drained once per frame after the post-tick
    // event drains. See: context/lib/scripting.md §10.4.
    let mut system_registry = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut system_registry);

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
        #[cfg(feature = "dev-tools")]
        nav_graph: None,
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
        cursor_pos: None,
        nav_stick_tracker: input::StickNavTracker::new(),
        frame_timing: FrameTiming::new(initial_state),
        view_feel_state: view_feel::ViewFeelState::default(),
        diagnostic_inputs: input::DiagnosticInputs::new(input::default_diagnostic_chords()),
        capture_portal_walk_next_frame: false,
        scratch_cells: Vec::new(),
        frame_rate_meter: FrameRateMeter::new(),
        title_buffer: String::with_capacity(256),
        last_title_update: Instant::now(),
        script_runtime,
        ui_proxy: scripting_systems::ui_proxy::StaticUiProxy::new(script_ctx.clone()),
        flash_decay: scripting_systems::flash_decay::FlashDecay::new(script_ctx.clone()),
        modal_stack: {
            // Register engine built-in trees at boot through the one shared
            // load-and-register path (`tree_asset::register_tree_from_disk`): each
            // built-in screen's `AnchoredTree` is authored in
            // `content/base/ui/<file>.json` and loaded from disk so a layout edit +
            // reload changes it with no Rust change. A missing/malformed asset warns
            // once and skips the registration — that screen is unavailable, the
            // engine still boots.
            //
            // The HUD registers under `HUD_NAME` and is resolved by name as the
            // always-on bottom passthrough layer each frame (the snapshot reads the
            // registry, not a builder). The pause menu registers under
            // `PAUSE_MENU_NAME` so `nav.menu` (gamepad Start / Escape-from-gameplay)
            // can push it; the keyboard under `KEYBOARD_TREE_NAME` so a `showDialog
            // { tree: "keyboard", onCommit }` resolves it.
            let mut stack = render::ui::modal_stack::ModalStack::new();
            let registry = stack.registry_mut();
            render::ui::tree_asset::register_tree_from_disk(
                registry,
                render::ui::tree_asset::HUD_NAME,
                "hud.json",
            );
            render::ui::tree_asset::register_tree_from_disk(
                registry,
                render::ui::demo::PAUSE_MENU_NAME,
                "pauseMenu.json",
            );
            render::ui::tree_asset::register_tree_from_disk(
                registry,
                render::ui::keyboard_asset::KEYBOARD_TREE_NAME,
                "keyboard.json",
            );
            stack
        },
        ui_focus: input::UiFocusEngine::new(),
        ui_focus_rects: None,
        ui_input_mode: input::InputMode::default(),
        input_mode_tracker: scripting_systems::input_mode::InputModeTracker::new(
            script_ctx.clone(),
        ),
        audio_master: scripting_systems::audio_master::AudioMasterConsumer::new(script_ctx.clone()),
        pending_mode_signal: None,
        pending_menu_toggle: false,
        ui_focused_id: None,
        script_ctx,
        state_store_lifecycle: StateStoreLifecycle::default(),
        sequence_registry,
        reaction_registry,
        system_registry,
        progress_tracker: ProgressTracker::new(),
        crossing_detector: CrossingDetector::new(),
        classname_dispatch,
        light_bridge: scripting_systems::light_bridge::LightBridge::new(),
        fog_volume_bridge: scripting_systems::fog_volume_bridge::FogVolumeBridge::new(),
        emitter_bridge: scripting_systems::emitter_bridge::EmitterBridge::new(),
        particle_live_counts: std::collections::HashMap::new(),
        collision_world: collision::CollisionWorld::new(),
        particle_render: scripting_systems::particle_render::ParticleRenderCollector::new(),
        mesh_render: scripting_systems::mesh_render::MeshRenderCollector::new(),
        mesh_clip_tables: scripting_systems::mesh_anim::MeshClipTables::new(),
        hit_zone_store: scripting_systems::hit_zones::HitZoneStore::new(),
        active_wieldable: None,
        active_wieldable_descriptor: None,
        builtin_handled: None,
        pending_spawn_points: None,
        pending_map_entities: None,
        script_time: 0.0,
        anim_time: 0.0,
        anim_time_scale: 1.0,
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
    /// Runtime navigation graph, built once when a level with a baked navmesh
    /// loads. The future pathfinding plan reads this; today only the
    /// `Alt+Shift+N` debug overlay consumes it, so it is dev-tools-gated to
    /// stay dead-code-free in shipping builds.
    #[cfg(feature = "dev-tools")]
    nav_graph: Option<nav::NavGraph>,

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

    /// Last cursor position in device pixels, tracked from winit `CursorMoved`
    /// while the cursor is released (UI mode). Tracked *state*, never queued:
    /// hover never enqueues an intent — the focus engine (Task 3) reads this
    /// position for hit-testing, and a mouse *click* pairs it into a
    /// `PointerClick` intent. `None` until the first `CursorMoved`.
    /// See: context/lib/input.md §7
    cursor_pos: Option<input::PointerPos>,

    /// Edge detector turning the gamepad nav stick (left stick) into discrete
    /// D-pad-style nav intents: one intent per push past the dead zone. Polled
    /// in the input stage before the `take_ready`/`advance_frame` pair so
    /// gamepad nav shares the keyboard's N→N+1 contract. See: context/lib/input.md §7
    nav_stick_tracker: input::StickNavTracker,

    frame_timing: FrameTiming,

    /// Per-camera view-feel integrator (head-bob phase, strafe-tilt spring,
    /// ambient-sway clock). Read AND updated each render frame by
    /// `view_feel::evaluate`; deliberately render-rate state, not on the
    /// fixed-tick `InterpolableState` (movement.md D5). Inert until a pawn
    /// carries `view_feel`. See: context/lib/movement.md
    view_feel_state: view_feel::ViewFeelState,

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

    /// Publishes live pawn HP into the `player.health` slot each frame.
    /// `player.ammo` is a stand-in value until its real producer lands.
    /// `intro.flashColor` is NOT a stand-in — this proxy's flash timer produces it
    /// (and `screen.flash` is produced by `flash_decay`). Flash timer resets on
    /// each level load. See: context/lib/scripting.md §5 for the store contract.
    ui_proxy: scripting_systems::ui_proxy::StaticUiProxy,

    /// App-side flash-decay state for the engine-owned `screen.flash` surface.
    /// A drained `FlashScreen` system-reaction command starts a flash; this
    /// writes the decaying RGBA into `screen.flash` each game-logic tick (beside
    /// `ui_proxy.tick`). Reset on level load. See: context/lib/ui.md §3.
    flash_decay: scripting_systems::flash_decay::FlashDecay,

    /// Gameplay-UI modal stack + named-tree registry. Consumes Goal E's
    /// `PushTree`/`PopTree` system commands (resolving names through its registry,
    /// unknown name warns + no-op) and exposes an engine push/pop API for
    /// pause/dialog. The HUD is republished as the stack's bottom layer each
    /// frame; the top tree's capture mode drives the input seam + `InputFocus`.
    /// Engine built-in trees register at boot. See: context/lib/ui.md §1.
    modal_stack: render::ui::modal_stack::ModalStack,

    /// App-side UI focus engine (M13 Goal F, Task 3). Runs in the game-logic
    /// phase: consumes the drained nav intents + tracked cursor, moves focus
    /// through the top stack tree by policy, runs the dt-clocked hold-to-repeat
    /// timer, and yields the focused node id that rides the next snapshot to drive
    /// the focus ring. See: context/lib/ui.md §4.
    ui_focus: input::UiFocusEngine,

    /// The focus rect list the renderer exported for the top stack tree LAST
    /// frame — the reverse twin of the app→renderer snapshot. The focus engine
    /// consumes it the following game-logic phase (N→N+1 applied in reverse), so
    /// the focus ring may trail a focus change by one frame. `None` until the
    /// first gameplay frame exports one.
    ui_focus_rects: Option<render::ui::tree::FocusRectList>,

    /// Pointer-vs-focus interaction mode taken as an input by the focus engine
    /// (hover moves focus only in `Pointer` mode). Driven each input phase from
    /// `input_mode_tracker` (mouse motion → `Pointer`; nav input → `Focus`).
    ui_input_mode: input::InputMode,

    /// App-side input-mode tracker (M13 Goal F, Task 5). Observes the input
    /// phase's mode signals (mouse motion vs. nav input), debounces them, writes
    /// the engine-owned `input.mode` enum slot, and drives `ui_input_mode`. The
    /// store write is app composition — the input subsystem's contract output
    /// stays the action snapshot. Reset on level load. See: context/lib/input.md §7.
    input_mode_tracker: scripting_systems::input_mode::InputModeTracker,

    /// App-side `audio.master` consumer (M13 Goal F, Task 5). Reads the
    /// mod-declared `audio.master` amplitude slot and applies it to the audio
    /// main-track volume (amplitude → dB) on change, making the demo pause-menu
    /// volume slider audible. No-op when no mod declares the slot. Reset on level
    /// load. See: context/lib/audio.md §1.
    audio_master: scripting_systems::audio_master::AudioMasterConsumer,

    /// The mode signal observed during THIS frame's input phase, resolved into
    /// `input_mode_tracker` at the head of the game-logic phase. Mouse motion
    /// (`CursorMoved`) votes `Pointer`; any nav input (stick edge / D-pad / nav
    /// key) votes `Focus`. Nav wins when both occur in one frame (a deliberate
    /// nav press dominates incidental cursor drift). Cleared each frame after the
    /// tracker consumes it. See: context/lib/input.md §7.
    pending_mode_signal: Option<scripting_systems::input_mode::ModeSignal>,

    /// Punch-through `nav.menu` toggle (M13 Goal F, Task 5): set when a `nav.menu`
    /// intent (gamepad Start, or keyboard Escape-from-gameplay) is produced in the
    /// input phase, then consumed in the game-logic phase to push (open) or pop
    /// (close) the demo pause menu via the engine push/pop API. `nav.menu` opens
    /// the menu from gameplay where the UI-dispatch seam is `Passthrough` and so
    /// queues nothing — hence the dedicated punch-through, mirroring how
    /// `ToggleDebugPanel` bypasses the capture gate. See: context/lib/input.md §7.
    pending_menu_toggle: bool,

    /// The focused node id the focus engine resolved THIS frame's game-logic
    /// phase, published on this frame's snapshot so the UI pass draws the focus
    /// ring around it. `None` when nothing is focused.
    ui_focused_id: Option<String>,

    /// Gates the one-time persistence overlay and clean-exit save.
    state_store_lifecycle: StateStoreLifecycle,

    /// Consulted by `fire_named_event_with_sequences` for `Sequence` steps.
    /// No per-level state — entity lookups go through `ScriptCtx`, which the
    /// level-unload path clears separately. See: context/lib/scripting.md §2
    sequence_registry: SequencedPrimitiveRegistry,

    /// Resolved by name when a `Primitive` reaction fires.
    /// See: context/lib/scripting.md §2
    reaction_registry: ReactionPrimitiveRegistry,

    /// Resolved by name when a `Primitive` reaction with no `tag` fires — the
    /// system-reaction arm. Handlers enqueue typed commands onto
    /// `script_ctx.system_commands`, drained once per frame.
    /// See: context/lib/scripting.md §10.4
    system_registry: SystemReactionRegistry,

    /// Per-tag kill-count subscriptions. Cleared on level unload; survives
    /// hot-reload. See: context/lib/scripting.md §2
    progress_tracker: ProgressTracker,

    /// State-crossing watchers (M13 HUD dynamics). Built from the data
    /// registry's `crossings` at level load; checked each frame after
    /// `ui_proxy.tick` (slot writes settled) and before the UI snapshot build.
    /// Cleared on level unload with the rest of the per-level state.
    /// See: context/lib/scripting.md §10.4
    crossing_detector: CrossingDetector,

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

    /// Game-side per-model animation clip tables (name → glTF index + per-index
    /// duration), built at the level-load model sweep from each uploaded model's
    /// renderer clip metadata. Owned beside `mesh_render`: the collector consults
    /// it to compute per-instance sample times, and the level-load validation
    /// resolves each mesh entity's `AnimationState.clip_index` against it. Cleared
    /// on level unload. See: context/lib/scripting.md §10.3.
    mesh_clip_tables: scripting_systems::mesh_anim::MeshClipTables,

    /// Game-side skeletal hit-zone store: per model TYPE, the CPU skeleton,
    /// clips, authored joint-zone table, and a derived broad-phase bound swept
    /// from the clips. Re-loaded game-side (independent of the renderer's
    /// moved-away copy) at the level-load model sweep and cleared on level
    /// change, beside `mesh_clip_tables`. CPU-only — no wgpu. Nothing consumes it
    /// yet besides tests; the Task 4 raycast facility threads it through.
    /// See: context/lib/entity_model.md §7.
    hit_zone_store: scripting_systems::hit_zones::HitZoneStore,

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

    /// Game-layer animation clock: accumulates `frame_dt × anim_time_scale` each
    /// render frame, advanced beside `script_time` at the same site and gated by
    /// the same dev-tools `freeze_time()` flag. All skeletal-animation timing
    /// (entry stamps, clip-local times, fade windows, the pending-stamp resolve)
    /// reads this clock. Accumulation — not scaling of absolute time — so
    /// changing `anim_time_scale` never jumps existing poses. Resets to zero on
    /// level unload. See: context/lib/scripting.md §10.3.
    anim_time: f64,

    /// Per-frame multiplier on the animation clock's advancement. `1.0` is
    /// real-time; `0.5` half-rate; `0.0` holds every clip and fade (pause). The
    /// slow-motion seam — no script surface yet (engine-side field only).
    anim_time_scale: f64,

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
            } if input::escape_is_dev_quit_chord(self.diagnostic_inputs.shift_held()) => {
                // Escape routing rule: `Shift+Esc` is the dev quit chord (this arm) and
                // takes precedence — even while text entry is open, Shift makes it the
                // developer's unambiguous quit, never a stray menu/cancel. PLAIN `Esc`
                // (no Shift) is NOT a quit: it falls through to the general keyboard arm,
                // which routes Escape-from-gameplay to `nav.menu` (toggles the pause menu,
                // exactly like gamepad Start) and Escape inside a capturing tree —
                // including an open text-entry modal — to `nav.cancel`. The Shift state is
                // the diagnostic resolver's modifier tracking (the Shift key-down was seen
                // by the general arm before this Esc). See: context/lib/input.md §7.
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
                    // intended structural home for this capture.
                    //
                    // Key-down edges resolve to a nav intent (arrows / enter /
                    // escape / tab); the kinded payload rides the queue. Held
                    // repeats and non-nav keys carry no intent (the seam still
                    // suppresses the gameplay forward). Escape's menu-vs-cancel
                    // split needs the "is a capturing tree on the stack?" flag —
                    // owned by Task 2's modal stack; passed `false` here until
                    // that wiring lands. See: context/lib/input.md
                    // A directional key RELEASE stops the focus engine's
                    // hold-to-repeat (the press-edge queue carries no release, so
                    // the focus ring's repeat clock is cleared here). Cancel never
                    // repeats, so only directional keys matter for nav repeat.
                    if !pressed
                        && matches!(
                            code,
                            winit::keyboard::KeyCode::ArrowUp
                                | winit::keyboard::KeyCode::ArrowDown
                                | winit::keyboard::KeyCode::ArrowLeft
                                | winit::keyboard::KeyCode::ArrowRight
                        )
                    {
                        self.ui_focus.release_repeat();
                    }
                    // A confirm key (Enter) RELEASE stops the activation-repeat clock
                    // (M13 Text-Entry, Task 2): a held `repeatOnHold` button stops
                    // re-firing once the confirm key is released, mirroring the
                    // directional release above.
                    if !pressed
                        && matches!(
                            code,
                            winit::keyboard::KeyCode::Enter | winit::keyboard::KeyCode::NumpadEnter
                        )
                    {
                        self.ui_focus.release_confirm_repeat();
                    }
                    // Text-entry routing (M13 Text-Entry, Task 3): while a text-entry
                    // tree is the top of the modal stack, hardware key-down events
                    // drive the edit surface instead of nav. The LOGICAL key resolves
                    // Backspace/Enter/Escape first (so a `\u{8}` Backspace text or a
                    // `\r` Enter text never leaks through the printable channel); only
                    // a non-control printable `KeyEvent.text` becomes a `Text` intent.
                    // Enter/Escape ride the queue as `nav.confirm`/`nav.cancel`, which
                    // the focus-resolution stage intercepts for commit/cancel.
                    let text_entry_open = self.modal_stack.active_text_entry_target().is_some();
                    // Text entry intentionally honors OS key-repeat (Text-Entry AC4:
                    // hardware-key repeat comes from the OS): a held Backspace/letter
                    // appends/deletes on each auto-repeat. All OTHER UI input stays
                    // edge-only (`!key_event.repeat`) — nav intents must not re-fire on
                    // a held key, since the focus engine's own dt clock owns nav repeat.
                    let nav_intent = if pressed && (!key_event.repeat || text_entry_open) {
                        if text_entry_open {
                            // A key inside text entry is always a `focus`-mode signal.
                            self.record_mode_signal(
                                scripting_systems::input_mode::ModeSignal::NavInput,
                            );
                            match input::text_entry_key(
                                &key_event.logical_key,
                                key_event.text.as_deref(),
                            ) {
                                Some(input::TextEntryKey::Append(s)) => {
                                    Some(input::UiIntentPayload::Text(s))
                                }
                                Some(input::TextEntryKey::Backspace) => {
                                    Some(input::UiIntentPayload::Backspace)
                                }
                                Some(input::TextEntryKey::Commit) => {
                                    Some(input::UiIntentPayload::Nav(input::NavIntent::Confirm))
                                }
                                Some(input::TextEntryKey::Cancel) => {
                                    Some(input::UiIntentPayload::Nav(input::NavIntent::Cancel))
                                }
                                None => None,
                            }
                        } else {
                            // Escape's menu-vs-cancel split: a capturing tree on the
                            // stack routes Escape to `nav.cancel`; from gameplay it
                            // opens the menu (`nav.menu`). The seam's `Capture` mode is
                            // set by `reconcile_ui_focus` from the modal stack's top
                            // capture mode, so it IS the "capturing tree present"
                            // predicate. See: context/lib/input.md §7
                            let capturing =
                                self.ui_dispatch.mode() == input::UiCaptureMode::Capture;
                            let intent = input::nav_intent_for_key(code, capturing);
                            if intent.is_some() {
                                // A nav key (arrows/enter/escape/tab) is a `focus`-mode
                                // signal — it switches the interaction mode off pointer.
                                self.record_mode_signal(
                                    scripting_systems::input_mode::ModeSignal::NavInput,
                                );
                            }
                            // Escape-from-gameplay maps to `nav.menu` (opens the pause
                            // menu). The seam is `Passthrough` from gameplay and queues
                            // nothing, so route the toggle through the punch-through flag.
                            if intent == Some(input::NavIntent::Menu) {
                                self.pending_menu_toggle = true;
                            }
                            intent.map(input::UiIntentPayload::Nav)
                        }
                    } else {
                        None
                    };
                    if self
                        .ui_dispatch
                        .dispatch_event(nav_intent)
                        .forwards_to_gameplay()
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
                // system this frame. A *press* (not release) at the tracked
                // cursor position queues a `PointerClick` for hit-testing; a
                // release captures with no payload (suppresses the gameplay
                // forward, queues nothing).
                let click_intent = match (state.is_pressed(), self.cursor_pos) {
                    (true, Some(pos)) => Some(input::UiIntentPayload::PointerClick { pos }),
                    _ => None,
                };
                if !self
                    .ui_dispatch
                    .dispatch_event(click_intent)
                    .forwards_to_gameplay()
                {
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
            WindowEvent::CursorMoved { position, .. } => {
                // Track cursor *position* (not delta) for UI hit-testing while
                // the cursor is released. This is tracked state, never queued —
                // hover never enqueues an intent. A later mouse click pairs this
                // position into a `PointerClick`. Gameplay look uses raw deltas
                // from `device_event`, not this position, so tracking here is
                // independent of the focus gate. See: context/lib/input.md §7
                self.cursor_pos = Some(input::PointerPos {
                    x: position.x,
                    y: position.y,
                });
                // Mouse motion is the `pointer`-mode signal. Recorded as tracked
                // state (resolved into `input.mode` at the game-logic phase head);
                // a same-frame nav press still wins (see `record_mode_signal`).
                self.record_mode_signal(scripting_systems::input_mode::ModeSignal::MouseMotion);
            }
            WindowEvent::CursorLeft { .. } => {
                // Cursor left the window: drop the tracked position so a stale
                // coordinate can't seed a click after re-entry.
                self.cursor_pos = None;
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

                // Tail of the Input stage: poll the gamepad. This must run
                // BEFORE the `take_ready`/`advance_frame` pair below so gamepad
                // nav intents land in `pending` ahead of promotion and share the
                // keyboard's N→N+1 contract — a gamepad nav consumed this frame
                // first reaches game logic next frame, never same-frame. (gilrs
                // previously polled *after* promotion, which would have leaked
                // gamepad intents a frame early.) The intents are enqueued only
                // while a capturing tree owns input (`Capture` mode); under
                // `Passthrough` they are dropped here, exactly as keyboard
                // events forward through the seam. See: context/lib/input.md §7
                if let Some(gp) = &mut self.gamepad_system {
                    let gp_nav = gp.update(&mut self.input_system, &mut self.nav_stick_tracker);
                    // Advance any active rumble's timeout in the input stage and
                    // stop it once its duration elapses (the rumble started by a
                    // drained `Rumble` command on a prior frame).
                    gp.tick_rumble(frame_dt);
                    // A confirm (South) RELEASE stops the focus engine's
                    // activation-repeat clock (M13 Text-Entry, Task 2): a held
                    // `repeatOnHold` button stops re-firing once South releases, the
                    // gamepad twin of the keyboard Enter-release path above.
                    if gp_nav.confirm_released {
                        self.ui_focus.release_confirm_repeat();
                    }
                    // No directional input held (D-pad up + stick in the dead zone)
                    // RELEASES the focus engine's directional hold-to-repeat clock,
                    // mirroring the keyboard arrow-key-up path above. Without it a
                    // press that armed the clock would free-run on dt until the next
                    // stack/intent change (runaway focus-scroll on a `repeat` tree).
                    if gp_nav.directional_released {
                        self.ui_focus.release_repeat();
                    }
                    // Any gamepad nav intent (stick edge, D-pad, face/system
                    // button) is a `focus`-mode signal — recorded regardless of
                    // capture mode so a `nav.menu` opened from gameplay also flips
                    // the interaction mode off pointer.
                    if !gp_nav.nav_intents.is_empty() {
                        self.record_mode_signal(
                            scripting_systems::input_mode::ModeSignal::NavInput,
                        );
                    }
                    // `nav.menu` (gamepad Start) toggles the pause menu. It must
                    // work from gameplay, where the seam is `Passthrough` and queues
                    // nothing — route it through the punch-through flag regardless of
                    // capture mode. Other nav intents only enqueue while a capturing
                    // tree owns input.
                    let capture = self.ui_dispatch.mode() == input::UiCaptureMode::Capture;
                    for intent in gp_nav.nav_intents {
                        if intent == input::NavIntent::Menu {
                            // `pending_menu_toggle` fully handles the toggle; the
                            // focus engine treats a queued `Nav(Menu)` as a no-op, so
                            // enqueuing it would be a dead intent. Skip the enqueue.
                            self.pending_menu_toggle = true;
                            continue;
                        }
                        if capture {
                            self.ui_dispatch
                                .enqueue_intent(input::UiIntentPayload::Nav(intent));
                        }
                    }
                }

                // Resolve this frame's input-mode signal into the engine-owned
                // `input.mode` slot (app composition — the input subsystem's
                // contract output stays the action snapshot). Mouse motion votes
                // `pointer`, nav input votes `focus`, debounced so jitter doesn't
                // flap. Drives `ui_input_mode` (the focus engine's hover gate). The
                // mode is observation-only here; its cursor/ring EFFECT is gated on
                // a capturing tree being on the stack (applied in `reconcile_ui_focus`).
                // See: context/lib/input.md §7.
                self.ui_input_mode = self
                    .input_mode_tracker
                    .update(self.pending_mode_signal.take(), frame_dt);

                // Game-logic phase begins here. Read the UI captures made
                // available by the *previous* frame, THEN promote this frame's
                // freshly captured events for the next frame. Taking before
                // advancing is what enforces the N→N+1 contract: events captured
                // during THIS frame's Input stage (keyboard via `dispatch_event`,
                // gamepad via the poll just above) land in `pending` and are only
                // promoted to `ready` by this `advance_frame` call — so they
                // first become visible at the next frame's `take_ready`, never
                // this frame. This holds regardless of winit's event/redraw
                // ordering because both calls run here at game-logic time. The
                // modal stack (Task 2/Task 3) consumes the drained intents; until
                // then they are dropped, and the drain marks the seam where game
                // logic reads them. See: context/lib/input.md
                let ui_intents = self.ui_dispatch.take_ready();
                self.ui_dispatch.advance_frame();

                // Text-entry resolution (M13 Text-Entry, Task 3): while a text-entry
                // tree is the top of the modal stack, the drained intents drive the
                // edit surface. `Text` appends and `Backspace` deletes against the
                // tree's `text_entry_target` slot (through Task 1's text-edit command
                // path); `nav.confirm` commits (fires the opener's `on_commit`, then
                // pops) and `nav.cancel` cancels (pops, no commit). Those confirm /
                // cancel intents are CONSUMED here so they never reach the focus
                // engine (no stray key-button activation) or the pause-menu logic
                // below. Returns whether a commit or cancel fired so the pause-menu
                // path is skipped this frame.
                let text_entry_consumed_nav = self.resolve_text_entry_intents(&ui_intents);

                // Focus engine (game-logic phase): split the drained intents into
                // nav (directional/confirm/cancel/next/prev) and pointer clicks,
                // then move focus through the TOP stack tree against the focus rect
                // list the renderer exported LAST frame (reverse N→N+1). The
                // focused id is published on this frame's snapshot below so the UI
                // pass draws the ring (it may trail a focus change by one frame).
                // Only the top tree takes focus; lower trees freeze. While text entry
                // is open, confirm/cancel were consumed above and are filtered out so
                // the focus engine sees only directional/next/prev moves (Task 4's
                // on-screen keyboard still navigates between keys).
                let mut nav_intents: Vec<input::NavIntent> = Vec::new();
                let mut click_positions: Vec<input::PointerPos> = Vec::new();
                for intent in &ui_intents {
                    match &intent.payload {
                        input::UiIntentPayload::Nav(nav) => {
                            if text_entry_consumed_nav
                                && matches!(
                                    nav,
                                    input::NavIntent::Confirm | input::NavIntent::Cancel
                                )
                            {
                                // Consumed by the text-entry commit/cancel above.
                                continue;
                            }
                            nav_intents.push(*nav);
                        }
                        input::UiIntentPayload::PointerClick { pos } => click_positions.push(*pos),
                        // Text / Backspace are text-entry edits, resolved above.
                        input::UiIntentPayload::Text(_) | input::UiIntentPayload::Backspace => {}
                    }
                }
                // Slider nav-capture (M13 Goal F, Task 4): the focused slider gets
                // first refusal on its `capturesNav` wire names. A captured nav step
                // adjusts the slider's bound value by `step` within `[min, max]` and
                // emits a `setState` write (applied at the game-logic command drain
                // below → the bound slot changes on the N+1 frame). Captured intents
                // are removed so the focus engine never sees them (focus stays put).
                self.apply_slider_nav_capture(&mut nav_intents);

                // The active (top) tree key: the modal stack's top entry name, else
                // the always-on HUD. `None` is never the gameplay case (the HUD is
                // always present), but the engine handles it.
                let active_key = self
                    .modal_stack
                    .active_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| render::ui::tree_asset::HUD_NAME.to_string());
                let cursor = self.cursor_pos;
                let focus_result = self.ui_focus.tick(
                    Some(active_key.as_str()),
                    self.ui_focus_rects.as_ref(),
                    &nav_intents,
                    cursor,
                    &click_positions,
                    self.ui_input_mode,
                    frame_dt,
                );
                self.ui_focused_id = focus_result.focused.clone();

                // Button activation (M13 Goal F, Task 4): a `confirm` (gamepad
                // confirm or pointer click — the focus engine reports both as
                // `confirmed`) on a focused button fires its `onPress` named reaction
                // through the same reaction path entity/system reactions use, so a
                // click and a gamepad confirm have an identical observable effect.
                if focus_result.confirmed {
                    self.fire_focused_button_activation(focus_result.focused.as_deref());
                }

                // Pause-menu toggle (M13 Goal F, Task 5): `nav.menu` (gamepad Start
                // / Escape-from-gameplay) opens or closes the demo pause menu via
                // the engine push/pop API. A `nav.cancel` (Escape / B inside the
                // menu) also closes it. The capture-mode + cursor effect follows on
                // this frame's `reconcile_ui_focus` below. The toggle flag is a
                // punch-through (it works from gameplay, where the seam queues
                // nothing); `cancelled` rides the captured-intent queue. Guard the
                // cancel close to the pause menu so it never pops an unrelated
                // top tree.
                if self.pending_menu_toggle {
                    self.pending_menu_toggle = false;
                    self.toggle_pause_menu();
                } else if focus_result.cancelled
                    && !text_entry_consumed_nav
                    && self.modal_stack.active_name() == Some(render::ui::demo::PAUSE_MENU_NAME)
                {
                    self.modal_stack.pop();
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
                // Death-event names accumulate here and drain through the
                // sequence-aware dispatcher (a separate sibling loop below), so a
                // `progress` reaction naming a sequence resolves — unlike the
                // plain `fire_named_event` drains, which would no-op it.
                let mut pending_death_events: Vec<String> = Vec::new();

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

                        // Order 2: weapon fire tick.
                        let weapon_events = self.run_weapon_fire_tick(snapshot, tick_dt);
                        pending_weapon_events.extend(weapon_events);

                        // Order 3: death sweep — resolve every entity at zero HP
                        // after this tick's damage has settled. Reports kills and
                        // player death back as owned data; we feed kill tags
                        // through the progress tracker (which returns any events
                        // that crossed their threshold) and accumulate those plus
                        // `playerDied` for the sequence-aware drain below.
                        let death_events = self.run_death_sweep();
                        pending_death_events.extend(death_events);

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
                // Death events drain through the sequence-aware dispatcher in
                // their OWN loop: a `progress` reaction that names a sequence
                // would no-op under plain `fire_named_event`. Chained-event names
                // are discarded (`let _ =`), matching the drains above.
                for event_name in &pending_death_events {
                    let _ = fire_named_event_with_sequences(
                        event_name,
                        &self.script_ctx.data_registry.borrow(),
                        &self.sequence_registry,
                        &self.reaction_registry,
                        &self.system_registry,
                        &self.script_ctx,
                    );
                }

                // System-reaction command drain — runs AFTER every post-tick
                // event drain so commands enqueued by movement/weapon/death
                // reactions (and, later, crossing watchers) are taken in one
                // batch. The typed queue keeps audio/input/UI services out of
                // the scripting surface; the dispatcher routes each command to
                // its subsystem consumer. See: scripting.md §10.4.
                // NOTE: a SECOND drain runs later this frame, after the state
                // crossings fire (see the crossing-detection block below), so
                // crossing-enqueued commands land this frame, not the next.
                if !self.script_ctx.system_commands.is_empty() {
                    self.dispatch_system_commands();
                }

                // Static UI proxy: republish the HUD store slots from
                // the engine side. Runs AFTER game logic settles the store and
                // BEFORE the UI read-snapshot build below, so the snapshot
                // freezes the proxy's values this same frame. Delta-driven from
                // `frame_dt` (not wall-clock) so the flash animation is
                // deterministic. See: context/lib/scripting.md §5.
                self.ui_proxy.tick(frame_dt);
                // Flash-decay state writes the engine-owned `screen.flash`
                // surface at the same game-logic stage as `ui_proxy.tick`, so
                // the UI snapshot below freezes this frame's flash color. Runs
                // after the first command drain so a flash started this frame
                // publishes immediately; the crossing drain below may start
                // another, decayed starting next frame.
                self.flash_decay.tick(frame_dt);

                // State-crossing detection (M13 HUD dynamics). Runs AFTER the
                // frame's slot writes (game logic + `ui_proxy.tick`) settle, so
                // it compares the authoritative slot value — distinct from the
                // eased display value styleRanges read mid-tween. Each watched
                // slot's threshold crossing fires its reaction list synchronously
                // through Task 2's shared named-reaction path; any system
                // reactions thereby enqueued are drained immediately below so
                // crossing-fired commands land in this frame, not the next.
                let crossing_events = self
                    .crossing_detector
                    .detect(&self.script_ctx.slot_table.borrow());
                for event_name in &crossing_events {
                    let _ = fire_named_event_with_sequences(
                        event_name,
                        &self.script_ctx.data_registry.borrow(),
                        &self.sequence_registry,
                        &self.reaction_registry,
                        &self.system_registry,
                        &self.script_ctx,
                    );
                }
                if !self.script_ctx.system_commands.is_empty() {
                    self.dispatch_system_commands();
                }

                // Reconcile the input seam + focus with the modal stack's top
                // capture mode, now that every command drain this frame has
                // settled the stack. A capturing top tree freezes gameplay input
                // (UI-dispatch capture) and releases the cursor (`InputFocus::Menu`);
                // an empty/passthrough top hands input back to gameplay. Runs in
                // the game-logic phase so the capture decision is in force for the
                // next frame's Input stage (the N→N+1 ordering the seam guarantees).
                self.reconcile_ui_focus();

                // Audio step — third in frame order (Input → Game logic →
                // Audio → Render → Present, development_guide.md §4.3). Runs after
                // game logic settles every entity and before render. Convert the
                // glam-typed camera to the primitive `ListenerState` here at the
                // call site (the boundary carries no glam); `forward` uses the
                // aim ray's direction so it includes pitch, unlike yaw-only
                // `forward()`, and `up` is world up per the `ListenerState`
                // contract. Guarded for the silent (init-failed) case.
                if let Some(audio) = &mut self.audio {
                    // App-side `audio.master` consumer (M13 Goal F, Task 5): apply
                    // the mod-declared master amplitude (set by the demo pause-menu
                    // slider via `setState`) to the audio main-track volume on
                    // change — amplitude → dB, `0` → mute floor. No-op when no mod
                    // declares the slot or the value is unchanged. Runs in the audio
                    // phase, after the frame's slot writes settle.
                    if let Some(db) = self.audio_master.poll() {
                        audio.set_main_volume(db);
                    }

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
                    // Animation clock accumulates scaled dt at the same site,
                    // under the same freeze gate. Accumulation (not absolute-time
                    // scaling) keeps a mid-fade scale change from jumping poses;
                    // scale 0 holds every clip and fade. See scripting.md §10.3.
                    self.anim_time = Self::advance_anim_clock(
                        self.anim_time,
                        frame_dt as f64,
                        self.anim_time_scale,
                    );
                }

                // Position interpolated from tick-state slots; yaw/pitch from
                // `self.camera` directly so zero-tick frames still see this
                // frame's look rotation.
                let interp = self.frame_timing.interpolated_state();

                // View-feel assembly (movement.md D1/D5/D6): a render-only,
                // pawn-driven camera effect. When the camera-driving pawn carries
                // `view_feel`, run the render-rate evaluator and fold its output
                // into the look angles, roll, and eye offset. When no pawn drives
                // the camera, or it carries no `view_feel`, take the pass-through
                // path with `roll = 0` / `eye_offset = ZERO` and no angle offsets
                // so the matrix is bit-identical to the no-view-feel render.
                //
                // The evaluator owns the integrator state (`self.view_feel_state`)
                // and never sees the camera basis; we derive its two velocity-space
                // inputs from the pawn velocity and the camera RIGHT vector here,
                // then map its scalar output back onto that basis. `camera.right()`
                // is the yaw-derived, Y-free, unit-length right vector the
                // `view_feel_inputs`/`map_output_to_camera` helpers expect.
                let camera_right = self.camera.right();
                // Match the camera-follow loop above, which drives the camera
                // from the first `PlayerMovement` pawn that ALSO has a
                // `Transform`: select that same pawn here (same
                // `get_component::<Transform>(id).is_ok()` predicate) and run
                // view feel only when IT carries `view_feel`. Selecting on the
                // identical predicate keeps the two readers from diverging — a
                // `PlayerMovement` without a `Transform` ordered first would
                // otherwise drive view feel while the camera follows a different
                // pawn. A later pawn's `view_feel` must not leak onto a camera
                // the driving pawn owns either.
                let view_feel_inputs = {
                    use crate::scripting::registry::{ComponentKind, ComponentValue, Transform};
                    let registry = self.script_ctx.registry.borrow();
                    registry
                        .iter_with_kind(ComponentKind::PlayerMovement)
                        .find(|(id, _value)| registry.get_component::<Transform>(*id).is_ok())
                        .and_then(|(_id, value)| match value {
                            ComponentValue::PlayerMovement(component) => {
                                component.view_feel.as_ref().map(|params| {
                                    (params.clone(), component.velocity, component.is_grounded)
                                })
                            }
                            _ => None,
                        })
                };
                let (vf_roll, vf_yaw_offset, vf_pitch_offset, vf_eye_offset) =
                    if let Some((params, velocity, is_grounded)) = view_feel_inputs {
                        let (horizontal_speed, lateral_velocity) =
                            view_feel::view_feel_inputs(velocity, camera_right);
                        let output = view_feel::evaluate(
                            &params,
                            horizontal_speed,
                            lateral_velocity,
                            is_grounded,
                            &mut self.view_feel_state,
                            // Zero-frame_dt guard: the evaluator leaves the
                            // integrator untouched at `frame_dt == 0` (Task 2
                            // contract), so passing it through is safe — we do
                            // not introduce a separate advance step here.
                            frame_dt,
                            // Accessibility scale (D6): owned/clamped by the
                            // options module; passed verbatim, not re-clamped.
                            self.player_options.view_feel_scale,
                        );
                        view_feel::map_output_to_camera(&output, camera_right)
                    } else {
                        // Pass-through: no driving pawn, or it carries no
                        // `view_feel`. Identical-to-today render path.
                        (0.0, 0.0, 0.0, Vec3::ZERO)
                    };

                let render_camera = camera::RenderCamera::new(
                    interp.position,
                    self.camera.aspect(),
                    self.camera.yaw + vf_yaw_offset,
                    self.camera.pitch + vf_pitch_offset,
                    vf_roll,
                    vf_eye_offset,
                );
                let view_proj = render_camera.view_projection;
                // The render eye and matrix are assembled together.
                // Portal traversal, camera uniforms, and every render-stage
                // distance/leaf query must use the same point. Using the
                // unbobbed interpolated position here can put the visibility
                // apex in a different BSP leaf or on the opposite side of a
                // portal plane, causing one-frame clear-color holes.
                let render_eye_position = render_camera.eye_position;

                let capture_portal_walk = std::mem::take(&mut self.capture_portal_walk_next_frame);

                // Portal DFS → cell IDs → visible-cell bitmask → indirect draw buffer.
                let (vis_result, _frustum) = match self.level.as_ref() {
                    Some(world) => visibility::determine_visible_cells(
                        render_eye_position,
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
                        render_eye_position,
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
                            // Resolve pass: fill every pending animation entry
                            // stamp from this frame's post-advance animation clock
                            // before the collector samples poses. Runs with a
                            // mutable registry, immediately before the (read-only)
                            // collector, so same-tick switches have all landed and
                            // the last target's stamp is concrete. See mesh.rs.
                            {
                                let mut registry = self.script_ctx.registry.borrow_mut();
                                crate::scripting::components::mesh::resolve_pending_animation_stamps(
                                    &mut registry,
                                    self.anim_time,
                                );
                            }
                            let registry = self.script_ctx.registry.borrow();
                            // Same frame alpha the player camera reads from
                            // `frame_timing` — interpolate each mesh between its
                            // previous- and current-tick transforms.
                            self.mesh_render.collect(
                                &registry,
                                world,
                                &visible_cells,
                                frame_result.alpha,
                                self.anim_time,
                                &self.mesh_clip_tables,
                                // Camera eye position — the same value that seeds
                                // the portal flood-fill — drives the per-instance
                                // animation time-slicing distance bucket.
                                interp.position,
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
                                        render_eye_position,
                                        world,
                                        &light_reachable_leaf_mask,
                                    );
                                }
                            }
                            // Navmesh overlay: append region rectangles + portal
                            // edges. No-op unless the `Alt+Shift+N` toggle is on
                            // and the map carried a baked navmesh.
                            if let Some(nav_graph) = self.nav_graph.as_ref() {
                                renderer.emit_nav_diagnostics(nav_graph);
                            }
                            out
                        };

                        // Publish the once-per-frame read snapshot just before
                        // the gameplay render call, mirroring the splash path so
                        // the once-per-frame contract holds on both. Game logic and
                        // audio have already run this frame, so the slot snapshot
                        // freezes the settled store state (frame order: Input →
                        // Game logic → Audio → Render). The renderer reads these
                        // cloned values, never the live `SlotTable`.
                        //
                        // Modal stack: the HUD is the always-on bottom layer
                        // (`trees[0]`), resolved BY NAME from the registry
                        // (`modal_stack.tree(HUD_NAME)`, sourced from
                        // `content/base/ui/hud.json`) — the registry is the single
                        // seam, no builder on the render path. Pushed modal trees
                        // stack above it, drawn bottom→top. The renderer's retained
                        // path lays each layer out and resolves its binds against the
                        // snapshot; each layer's descriptor is structurally stable, so
                        // the retained tree per layer reuses it and only bound values
                        // drive the diff. The HUD's capture mode comes from its
                        // declared envelope (passthrough), so with no modal open the
                        // top mode is passthrough (gameplay keeps input). A missing
                        // `hud.json` resolves to `None` — the HUD is simply absent
                        // that frame and the engine still boots.
                        let slot_values =
                            Self::build_ui_slot_snapshot(&self.script_ctx.slot_table.borrow());
                        let mut trees: Vec<render::ui::UiTreeEntry> = self
                            .modal_stack
                            .tree(render::ui::tree_asset::HUD_NAME)
                            .map(|descriptor| render::ui::UiTreeEntry {
                                name: render::ui::tree_asset::HUD_NAME.to_string(),
                                capture_mode: descriptor.capture_mode.into(),
                                descriptor: descriptor.clone(),
                                on_commit: None,
                            })
                            .into_iter()
                            .collect();
                        trees.extend(self.modal_stack.entries());
                        // Ring-visibility follows the interaction mode WHILE a
                        // capturing tree is on the stack (M13 Goal F, Task 5):
                        // `focus` mode shows the ring, `pointer` mode hides it (the
                        // cursor is the indicator). Inert otherwise — with no
                        // capturing tree the focused id always rides through (the
                        // HUD has no focusable nodes, so it is `None` anyway).
                        let ring_id = if self.modal_stack.top_capture_mode()
                            == input::UiCaptureMode::Capture
                            && !self.ui_input_mode.ring_visible()
                        {
                            None
                        } else {
                            self.ui_focused_id.clone()
                        };
                        renderer.set_ui_snapshot(render::ui::UiReadSnapshot::with_trees(
                            trees,
                            slot_values,
                            self.script_time,
                            ring_id,
                        ));

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
                        // Read back the focus rect list the renderer just exported
                        // for the top stack layer (the gameplay render above laid it
                        // out). The focus engine consumes it next frame's game-logic
                        // phase — the reverse N→N+1 the focus ring's one-frame trail
                        // comes from. See: context/lib/ui.md §4.
                        self.ui_focus_rects = Some(renderer.export_ui_focus_rects());
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

                let pos = render_eye_position;
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
        // Mirrors the `window_event` seam; the decision is the mode flag. A raw
        // delta carries no queueable intent (hover/look is not nav), so the
        // capture suppresses the forward but queues nothing.
        if !self.ui_dispatch.dispatch_event(None).forwards_to_gameplay() {
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

    /// Apply slider nav-capture for the focused slider (M13 Goal F, Task 4).
    ///
    /// The currently focused node
    /// (last frame's `ui_focused_id`, the focus going into this frame) is matched
    /// against the exported focus rects; if it is a `slider`, each nav intent whose
    /// wire name is in the slider's `captures_nav` is REMOVED from `nav_intents`
    /// (the focus engine never sees it) and, when directional, steps the bound value
    /// by `step` clamped to the slider's min/max, enqueuing a `setState` write
    /// applied at the game-logic command drain (the bound slot changes on N+1).
    fn apply_slider_nav_capture(&mut self, nav_intents: &mut Vec<input::NavIntent>) {
        use render::ui::tree::NodeInteraction;

        let Some(focused_id) = self.ui_focused_id.as_deref() else {
            return;
        };
        let Some(rects) = self.ui_focus_rects.as_ref() else {
            return;
        };
        // Resolve the focused slider's interaction + its bound slot (clone out so
        // the immutable borrow of the rect list drops before the slot/queue work).
        let slider = rects
            .rects
            .iter()
            .find(|r| r.id == focused_id)
            .and_then(|r| match &r.interaction {
                Some(interaction @ NodeInteraction::Slider { slot, min, .. }) => {
                    Some((interaction.clone(), slot.clone(), *min))
                }
                _ => None,
            });
        let Some((interaction, slot, min)) = slider else {
            return;
        };

        // The slider's current value: its bound slot reading, or `min` as a floor
        // when the slot is unset or non-numeric (a sane starting point).
        let current = {
            let table = self.script_ctx.slot_table.borrow();
            match table.get(&slot).and_then(|r| r.value.as_ref()) {
                Some(crate::scripting::slot_table::SlotValue::Number(n)) => *n,
                _ => min,
            }
        };

        // Peel off captured nav intents (mutating `nav_intents`) and compute the
        // stepped value; emit one `setState` for the new clamped value.
        if let Some(next) = input::capture_slider_step(&interaction, current, nav_intents) {
            self.script_ctx
                .system_commands
                .push(SystemReactionCommand::SetState {
                    slot,
                    value: serde_json::json!(next),
                });
        }
    }

    /// Fire a focused button's `onPress` named reaction on activation (M13 Goal F,
    /// Task 4). `focused_id` is the focus engine's reported focused node this tick;
    /// when it resolves to a `button` interaction on the exported rect list, the
    /// `onPress` reaction fires through the shared named-reaction path — the same
    /// vocabulary entity/system reactions use — so a gamepad confirm and a pointer
    /// click produce an identical observable effect.
    fn fire_focused_button_activation(&mut self, focused_id: Option<&str>) {
        use render::ui::tree::NodeInteraction;

        let Some(focused_id) = focused_id else {
            return;
        };
        let Some(rects) = self.ui_focus_rects.as_ref() else {
            return;
        };
        let on_press = rects
            .rects
            .iter()
            .find(|r| r.id == focused_id)
            .and_then(|r| match &r.interaction {
                Some(NodeInteraction::Button { on_press, .. }) => Some(on_press.clone()),
                _ => None,
            });
        if let Some(on_press) = on_press {
            // The on-screen keyboard's `done` key carries a reserved sentinel
            // `onPress` (never a registered reaction). Intercept it here and route
            // to the shared commit seam — the same `commit_text_entry` the hardware
            // Enter key reaches (Task 3) — so commit is not keyboard-only and the
            // keyboard stays fully data-driven (the `done` key references the
            // sentinel as data; no Rust change to edit the layout).
            if on_press == render::ui::keyboard_asset::COMMIT_TEXT_ENTRY_SENTINEL {
                self.commit_text_entry();
                return;
            }
            let _ = fire_named_event_with_sequences(
                &on_press,
                &self.script_ctx.data_registry.borrow(),
                &self.sequence_registry,
                &self.reaction_registry,
                &self.system_registry,
                &self.script_ctx,
            );
        }
    }

    /// Resolve drained UI intents against the open text-entry surface (M13
    /// Text-Entry, Task 3). Returns `true` when a `nav.confirm` (commit) or
    /// `nav.cancel` (cancel) was consumed by text entry this frame, so the caller
    /// filters those intents out of the focus engine and skips the pause-menu path.
    ///
    /// No-op (returns `false`) when text entry is closed — the top tree declares no
    /// `text_entry_target`. While open:
    /// - `Text(s)` → an `AppendText { slot, text: s }` edit against the target slot,
    /// - `Backspace` → a `BackspaceText { slot }` edit against the target slot,
    /// - `nav.confirm` → commit: fire the opener's `on_commit`, then `PopTree`,
    /// - `nav.cancel` → cancel: `PopTree` only (edits stay in the slot; the opener
    ///   simply does not act on them — no rollback).
    ///
    /// Edits ride Task 1's text-edit command path (pushed onto the system-command
    /// queue, drained at `dispatch_system_commands`), so they land on the bound slot
    /// on the N+1 frame — the system's defining N→N+1 ordering. Commit and cancel act
    /// on the stack immediately at this game-logic phase; the seam reconciles next.
    fn resolve_text_entry_intents(&mut self, ui_intents: &[input::UiIntent]) -> bool {
        let Some(target) = self
            .modal_stack
            .active_text_entry_target()
            .map(str::to_string)
        else {
            return false;
        };

        // Thread the currently-focused node's interaction (last frame's exported
        // focus, the focus going into this frame — same source `apply_slider_nav_capture`
        // reads) so `resolve_text_entry` can distinguish a confirm that lands on an
        // on-screen keyboard key from a keyboardless hardware Enter. A confirm on a
        // focusable button must flow to the focus engine (Task 4 fires the key's
        // `on_press` — `kbAppend_*` to type, or `done`'s commit sentinel); only a
        // confirm NOT on a button commits here. Without this the confirm was consumed
        // as Commit before the focus engine ran and the keyboard closed instead of
        // typing.
        let confirm_on_button = self.focused_node_is_activatable_button();

        // Pure resolution: drained intents → ordered edits + a terminal disposition.
        let resolution = input::resolve_text_entry(ui_intents, confirm_on_button);

        // Apply the edits through Task 1's text-edit command path (the bound slot
        // changes on the N+1 frame). Edits are queued before commit/cancel acts so a
        // committing reaction observes the slot as last edited.
        for edit in &resolution.edits {
            let command = match edit {
                input::TextEntryEdit::Append(text) => SystemReactionCommand::AppendText {
                    slot: target.clone(),
                    text: text.clone(),
                },
                input::TextEntryEdit::Backspace => SystemReactionCommand::BackspaceText {
                    slot: target.clone(),
                },
            };
            self.script_ctx.system_commands.push(command);
        }

        match resolution.disposition {
            input::TextEntryDisposition::Commit => self.commit_text_entry(),
            input::TextEntryDisposition::Cancel => self.cancel_text_entry(),
            input::TextEntryDisposition::Open => {}
        }
        resolution.consumed_commit_or_cancel()
    }

    /// Whether the currently-focused node (last frame's `ui_focused_id` on the
    /// exported rect list) is an activatable `button`. The on-screen keyboard's
    /// keys are buttons, so this is the predicate `resolve_text_entry_intents` uses
    /// to keep a `nav.confirm` flowing to the focus engine (the key activates)
    /// rather than consuming it as a text-entry commit. Reads the same
    /// `ui_focused_id` + `ui_focus_rects` pair `apply_slider_nav_capture` does.
    fn focused_node_is_activatable_button(&self) -> bool {
        use render::ui::tree::NodeInteraction;
        let Some(focused_id) = self.ui_focused_id.as_deref() else {
            return false;
        };
        let Some(rects) = self.ui_focus_rects.as_ref() else {
            return false;
        };
        rects
            .rects
            .iter()
            .find(|r| r.id == focused_id)
            .is_some_and(|r| matches!(r.interaction, Some(NodeInteraction::Button { .. })))
    }

    /// Commit the open text-entry surface (M13 Text-Entry, Task 3): fire the top
    /// tree's carried `on_commit` reaction (from the `PushTree` that opened it),
    /// THEN pop the tree. This is the shared commit seam — the hardware Enter key
    /// routes here, and Task 4's on-screen `done` button activation calls this same
    /// method so commit is not keyboard-only. A no-op when no tree is open.
    ///
    /// The `on_commit` reaction reads the bound slot's value (the entered text); the
    /// reaction fires synchronously here so it observes the slot as last edited.
    fn commit_text_entry(&mut self) {
        if let Some(on_commit) = self.modal_stack.active_on_commit().map(str::to_string) {
            let _ = fire_named_event_with_sequences(
                &on_commit,
                &self.script_ctx.data_registry.borrow(),
                &self.sequence_registry,
                &self.reaction_registry,
                &self.system_registry,
                &self.script_ctx,
            );
        }
        self.modal_stack.pop();
    }

    /// Cancel the open text-entry surface (M13 Text-Entry, Task 3): pop the tree
    /// WITHOUT firing `on_commit`. Edits already applied to the bound slot are
    /// discarded simply by the opener not acting on them — there is no rollback.
    fn cancel_text_entry(&mut self) {
        self.modal_stack.pop();
    }

    /// Drain the system-reaction command queue and route each typed command to
    /// its subsystem consumer. Runs once per frame after the post-tick event
    /// drains (and again after the crossing detector fires), so audio / input /
    /// UI services stay out of the scripting surface — the queue is the seam.
    /// See: context/lib/scripting.md §10.4.
    ///
    /// - `PlaySound` → the M12 audio module `play()` on the named bus (default
    ///   `sfx`); silent when audio init failed.
    /// - `Rumble` → gilrs force feedback on the active gamepad; warn-once no-op
    ///   when force feedback is unavailable.
    /// - `FlashScreen` → starts the App-side flash-decay state, which writes
    ///   `screen.flash` each game-logic tick.
    /// - `PushTree` / `PopTree` → push/pop the gameplay-UI modal stack, resolving
    ///   `PushTree`'s name through the stack's registry (unknown name warns +
    ///   no-op, never a panic). The top tree's capture mode is reconciled with the
    ///   input seam + focus afterward by `reconcile_ui_focus`.
    /// - `SetState` → readonly-gated JSON write to a writable store slot at the
    ///   game-logic stage (readonly warns + no-ops; unknown/type-mismatch logs).
    /// - `AppendText` / `BackspaceText` / `ClearText` → readonly-gated text edits
    ///   to a writable String slot at the game-logic stage, through the same
    ///   writable-slot gate as `SetState` (readonly warns + no-ops; empty
    ///   backspace is a silent no-op; unknown/non-String slot logs). M13 Text
    ///   Entry, Task 1.
    fn dispatch_system_commands(&mut self) {
        for command in self.script_ctx.system_commands.take() {
            match command {
                SystemReactionCommand::PlaySound { sound, bus } => {
                    if let Some(audio) = &mut self.audio {
                        // The reaction surface has no per-voice volume or looping
                        // yet (deferred); a one-shot on the named bus is the whole
                        // contract. Default to the SFX bus when none is named.
                        let bus = bus.unwrap_or_else(|| "sfx".to_string());
                        // `play` warns-and-drops on an unknown bus or sound, so an
                        // unregistered sound name never panics.
                        let _ = audio.play(audio::SoundRequest {
                            bus,
                            sound,
                            looping: false,
                        });
                    }
                    // Audio init failed ⇒ silent (the game runs without sound).
                }
                SystemReactionCommand::Rumble {
                    strong,
                    weak,
                    duration_ms,
                } => {
                    if let Some(gp) = &mut self.gamepad_system {
                        gp.rumble(strong, weak, duration_ms);
                    }
                    // No gamepad subsystem ⇒ nothing to vibrate.
                }
                SystemReactionCommand::FlashScreen { color, duration_ms } => {
                    self.flash_decay.start(color, duration_ms);
                }
                SystemReactionCommand::PushTree { tree, on_commit } => {
                    // Resolve the registered tree by name onto the modal stack.
                    // An unknown name warns and is a no-op (no panic). The carried
                    // `on_commit` rides the stack entry for a later goal to fire on
                    // commit; the stack does not fire it. The capture mode lives on
                    // the registered tree's envelope (read after the drain by
                    // `reconcile_ui_focus`), not on the command.
                    self.modal_stack.push_named(&tree, on_commit);
                }
                SystemReactionCommand::PopTree => {
                    self.modal_stack.pop();
                }
                SystemReactionCommand::SetState { slot, value } => {
                    // Readonly-gated JSON write at the game-logic stage: a readonly
                    // slot warns and no-ops; an unknown slot or type mismatch logs
                    // and is skipped — never a panic. NEVER the engine bypass.
                    if let Err(err) = crate::scripting::primitives::store::write_state_slot_json(
                        &self.script_ctx,
                        &slot,
                        &value,
                    ) {
                        log::warn!("[Scripting] setState write to `{slot}` failed: {err}");
                    }
                }
                SystemReactionCommand::AppendText { slot, text } => {
                    // Readonly-gated text edit at the game-logic stage (same
                    // writable-slot gate as setState): readonly warns + no-ops;
                    // unknown/non-String slot logs — never a panic.
                    use crate::scripting::primitives::store::{TextEdit, apply_text_edit};
                    if let Err(err) =
                        apply_text_edit(&self.script_ctx, &slot, TextEdit::Append(&text))
                    {
                        log::warn!("[Scripting] appendText to `{slot}` failed: {err}");
                    }
                }
                SystemReactionCommand::BackspaceText { slot } => {
                    // Empty backspace is a silent no-op inside `apply_text_edit`.
                    use crate::scripting::primitives::store::{TextEdit, apply_text_edit};
                    if let Err(err) = apply_text_edit(&self.script_ctx, &slot, TextEdit::Backspace)
                    {
                        log::warn!("[Scripting] backspaceText to `{slot}` failed: {err}");
                    }
                }
                SystemReactionCommand::ClearText { slot } => {
                    use crate::scripting::primitives::store::{TextEdit, apply_text_edit};
                    if let Err(err) = apply_text_edit(&self.script_ctx, &slot, TextEdit::Clear) {
                        log::warn!("[Scripting] clearText to `{slot}` failed: {err}");
                    }
                }
            }
        }
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
        // Restart the static UI proxy's flash timer so `intro.flashColor`
        // replays its level-load pulse from the start on this fresh level.
        self.ui_proxy.reset_timer();
        // Clear any in-flight `screen.flash` decay so a flash never bleeds
        // across a level load.
        self.flash_decay.reset();
        // Reset the input-mode tracker (re-seeds `input.mode` to `focus`) and the
        // `audio.master` consumer (re-applies the freshly-declared volume) so a
        // mid-transition mode or a stale master volume never bleeds across levels.
        self.input_mode_tracker.reset();
        self.audio_master.reset();
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

        // Build the runtime navigation graph once, from the baked navmesh
        // section. `None` when the map has no navmesh bake.
        #[cfg(feature = "dev-tools")]
        {
            self.nav_graph = world.navmesh.as_ref().map(nav::NavGraph::from_section);
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
                // Build the crossing watchers from this level's `crossings`.
                // Clear first so a re-load (or hot reload) does not stack
                // duplicate watchers; the previous value initializes to each
                // slot's value at level start so the initial state never fires.
                self.crossing_detector.clear();
                self.crossing_detector.initialize(
                    &self.script_ctx.data_registry.borrow(),
                    &self.script_ctx.slot_table.borrow(),
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

            // Attach the `player.health` slot's declared range `[0, max]` now
            // that the pawn (and its health component) has materialized. `max`
            // is mod data, so it cannot be declared at `SlotTable` construction.
            //
            // Borrow discipline: `registry` is the live `borrow_mut` taken at
            // the top of this block; read `max` THROUGH it here, before the
            // `drop(registry)` below. A second `self.script_ctx.registry.borrow()`
            // while this `borrow_mut` is live would panic (RefCell). The slot
            // table is a separate `RefCell`, so its `borrow_mut` does not
            // conflict with the registry borrow.
            if let Some((_, health)) =
                crate::scripting::components::health::pawn_with_health(&registry)
            {
                use crate::scripting::slot_table::NumericRange;
                if let Err(err) = self
                    .script_ctx
                    .slot_table
                    .borrow_mut()
                    .set_engine_numeric_range(
                        "player.health",
                        NumericRange {
                            min: 0.0,
                            max: health.max,
                        },
                    )
                {
                    log::warn!("[Loader] failed to set player.health range: {err}");
                }
            }

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

        // Level-load model sweep. Runs AFTER both classname dispatch (which
        // spawned a `MeshComponent` per `prop_mesh` placement) and the
        // data-archetype sweep (which spawned descriptor-declared mesh entities,
        // including animated `components.mesh` placements) so this single sweep
        // sees EVERY mesh entity. Collect the distinct `model` handles off those
        // entities and load + upload each exactly once into the renderer's model
        // cache (renderer owns GPU). This runs at level-load time, never
        // mid-frame, so there is no in-frame hitch. The model handle is the
        // renderer cache key the per-frame draw planner groups by, so it is
        // passed VERBATIM as the cache key; the glTF file itself is opened from
        // `content_root.join(handle)` inside `load_skinned_model` (open path and
        // cache key are decoupled — every other asset joins the content root, but
        // the key must stay the raw handle the planner looks up). A
        // failed/invalid load is non-fatal: `load_skinned_model` already `warn!`s
        // naming the path and returns `None`, the entity then renders nothing,
        // and the load continues.
        //
        // Ordering: this must run after the archetype sweep (so descriptor mesh
        // entities exist before clip resolve) and before the `levelLoad` fire
        // (which can run `setAnimationState`, requiring resolved clip indices).
        if let Some(renderer) = self.renderer.as_mut() {
            // Clear per-level transient mesh-pass state (the `"smooth"`-interrupt
            // snapshot store and the per-entity palette cache — entity seeds are not
            // stable across levels) at the model-cache install seam, and reset the
            // game-side clip tables before rebuilding them for this level.
            renderer.clear_mesh_pass_for_level_load();
            self.mesh_clip_tables.clear();
            // The game-side hit-zone store is per-level transient too — clear it
            // alongside the clip tables before this level's sweep rebuilds it.
            self.hit_zone_store.clear();

            let models = {
                let registry = self.script_ctx.registry.borrow();
                distinct_mesh_models(&registry)
            };
            for model in &models {
                renderer.load_skinned_model(model, &self.content_root, &prm_cache_root);
                // Build this model's game-side clip table from the renderer's clip
                // metadata (glTF index order). A failed load cached nothing, so the
                // metadata is empty and the table maps no clips — every state then
                // warns + stays unresolved below.
                let meta = renderer.skinned_model_clip_metadata(model);
                self.mesh_clip_tables
                    .insert(crate::model::ModelHandle::from(model.clone()), &meta);
                // Build this model's game-side hit-zone entry by re-loading the
                // glTF independently (the renderer moved its own skeleton + clips
                // into the GPU layer). Keeps skeleton + clips + zone table and a
                // derived broad-phase bound; a failed load installs nothing.
                self.hit_zone_store
                    .insert_from_load(model, &self.content_root);
            }
            if !models.is_empty() {
                log::info!(
                    "[Model] uploaded {} distinct mesh model(s) for this level",
                    models.len(),
                );
            }

            // Resolve every animated mesh entity's state map against its model's
            // clip table: fill each `AnimationState.clip_index` (name → index). A
            // state naming a clip the model does not carry warns ONCE here and stays
            // `clip_index = None` (unusable — switching to it warns + no-ops, and
            // switching out of it hard-cuts — both handled by the animation state machine).
            resolve_mesh_entity_clips(
                &mut self.script_ctx.registry.borrow_mut(),
                &self.mesh_clip_tables,
            );
        }
        self.level_timings.record("model_load");

        fire_named_event_with_sequences(
            "levelLoad",
            &self.script_ctx.data_registry.borrow(),
            &self.sequence_registry,
            &self.reaction_registry,
            &self.system_registry,
            &self.script_ctx,
        );
        self.level_timings.record("level_load_event");
        self.script_time = 0.0;
        // Animation clock is level-relative like `script_time`. The scale field
        // is engine config, not level state, so it is not reset here.
        self.anim_time = 0.0;
    }

    /// Accumulate one frame onto the animation clock: `prev + dt × scale`.
    /// Pure so the accumulation contract (scale 0.5 halves advancement; a
    /// mid-accumulation scale change never jumps the clock because we add scaled
    /// deltas rather than scaling absolute time) is unit-verifiable without the
    /// event loop. The freeze gate lives at the call site. See scripting.md §10.3.
    fn advance_anim_clock(prev: f64, frame_dt: f64, scale: f64) -> f64 {
        prev + frame_dt * scale
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
                snapshots.push((id, (**component).clone(), position));
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
            // Additive to the impact burst: when the nearest hit was an entity
            // hitbox, route the payload through the damage chokepoint. Spatial
            // targeting rides on the impact (`target`), never inside the
            // payload. The death sweep (run after this tick) resolves any kill.
            if let (Some(target), weapon::ActivationOutcome::Hit(payload)) =
                (impact.target, impact.outcome)
            {
                apply_damage(&mut registry, target, &payload);
            }
        }
        events.event_names()
    }

    /// Resolve every zero-HP entity for this tick and surface the events its
    /// deaths trigger. The sweep itself only mutates the registry (despawn /
    /// latch) and returns owned data — it cannot reach the progress tracker or
    /// the event-dispatch path. Here on the app side we close that loop:
    ///
    /// - Each killed non-player's tags flow through
    ///   `ProgressTracker::on_entity_killed`, whose returned event names (a
    ///   `progress` reaction crossing its declared fraction) join the drain.
    /// - A player death contributes the `playerDied` event exactly once (the
    ///   sweep's `death_handled` latch guarantees the single report).
    ///
    /// The returned names are accumulated by the caller and drained after the
    /// tick loop via `fire_named_event_with_sequences`.
    fn run_death_sweep(&mut self) -> Vec<String> {
        let report = {
            let mut registry = self.script_ctx.registry.borrow_mut();
            scripting_systems::health::sweep_deaths(&mut registry)
        };

        let mut events = Vec::new();
        for tags in &report.killed_tags {
            events.extend(self.progress_tracker.on_entity_killed(tags));
        }
        if report.player_died {
            events.push(scripting_systems::health::PLAYER_DIED_EVENT.to_string());
        }
        events
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

    /// Record this frame's input-mode signal, with nav input dominating mouse
    /// motion when both occur in one frame: a deliberate nav press should win
    /// over incidental cursor drift, so a `NavInput` vote overwrites a pending
    /// `MouseMotion` but not vice-versa. Cleared each frame after the tracker
    /// consumes it. See: context/lib/input.md §7.
    fn record_mode_signal(&mut self, signal: scripting_systems::input_mode::ModeSignal) {
        use scripting_systems::input_mode::ModeSignal;
        self.pending_mode_signal = match (self.pending_mode_signal, signal) {
            // Nav always wins (overwrite a pending pointer vote; keep nav).
            (_, ModeSignal::NavInput) => Some(ModeSignal::NavInput),
            (Some(ModeSignal::NavInput), ModeSignal::MouseMotion) => Some(ModeSignal::NavInput),
            (_, ModeSignal::MouseMotion) => Some(ModeSignal::MouseMotion),
        };
    }

    /// Toggle the demo pause menu (M13 Goal F, Task 5): pop it if it is the top
    /// tree, otherwise push it via the engine push/pop API (the registered
    /// `pauseMenu` tree). Wired to `nav.menu` (gamepad Start / Escape-from-
    /// gameplay) through `pending_menu_toggle`. The capture-mode + cursor effect
    /// follows on the next `reconcile_ui_focus` (this game-logic phase).
    fn toggle_pause_menu(&mut self) {
        if self.modal_stack.active_name() == Some(render::ui::demo::PAUSE_MENU_NAME) {
            self.modal_stack.pop();
        } else {
            self.modal_stack
                .push_named(render::ui::demo::PAUSE_MENU_NAME, None);
        }
    }

    /// Reconcile the input-dispatch seam and coarse focus with the modal stack's
    /// top capture mode. Called in the game-logic phase after the system-command
    /// drains settle the stack, so the decision is in force for the NEXT frame's
    /// Input stage (the N→N+1 ordering the seam guarantees: a UI event consumed on
    /// frame N reaches game logic no earlier than N+1, and the capture/cursor side
    /// flips here, one game-logic phase before that read).
    ///
    /// - A capturing top tree drives `UiCaptureMode::Capture` (the seam queues
    ///   events for next-frame game logic instead of forwarding to gameplay) and
    ///   `InputFocus::Menu` (cursor released, gameplay input frozen).
    /// - An empty or passthrough top hands input back: `Passthrough` at the seam,
    ///   and focus returns to `Gameplay` if it was `Menu`.
    ///
    /// While a capturing tree is up (Menu focus), the OS cursor's VISIBILITY then
    /// follows the interaction mode (M13 Goal F, Task 5): `pointer` shows it,
    /// `focus` hides it. This is inert when no capturing tree is up — gameplay
    /// owns the cursor (locked + hidden) and dev-tools owns its own.
    ///
    /// DevTools owns focus while the debug panel is open (it released the cursor
    /// and set `DevTools`); this reconcile never overrides that — the modal stack
    /// is gameplay UI, and the debug overlay is a separate, dev-only consumer.
    fn reconcile_ui_focus(&mut self) {
        let mode = self.modal_stack.top_capture_mode();
        self.ui_dispatch.set_mode(mode);

        // The debug overlay owns focus while open — don't fight it.
        if self.input_focus == InputFocus::DevTools {
            return;
        }

        let want_menu = matches!(mode, input::UiCaptureMode::Capture);
        match (want_menu, self.input_focus) {
            // A capturing tree opened (or stayed open): enter Menu, release cursor.
            (true, InputFocus::Gameplay) => self.set_input_focus(InputFocus::Menu),
            // The capturing tree(s) closed: hand the cursor back to gameplay.
            (false, InputFocus::Menu) => self.set_input_focus(InputFocus::Gameplay),
            // Already in the right focus for the current capture mode.
            _ => {}
        }

        // Cursor visibility follows the interaction mode WHILE a capturing tree
        // is up. `set_input_focus(Menu)` released the cursor (visible) above; in
        // `focus` mode we additionally hide it so directional nav isn't cluttered
        // by a stray pointer. Mode is inert otherwise (no capturing tree).
        if want_menu && self.input_focus == InputFocus::Menu {
            if let Some(ws) = self.window_state.as_ref() {
                ws.window
                    .set_cursor_visible(self.ui_input_mode.cursor_visible());
            }
        }
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
            #[cfg(feature = "dev-tools")]
            DiagnosticAction::ToggleNavOverlay => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.toggle_navmesh_overlay();
                }
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
//   matrix. Yaw/pitch reach the matrix through `RenderCamera::new`, not as
//   fields of `InterpolableState`, so a yaw assertion alone does
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
    /// are required: `RenderCamera::new` takes yaw/pitch as arguments, so an
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
        let aspect = camera.aspect();
        let baseline =
            crate::camera::RenderCamera::new(Vec3::ZERO, aspect, 0.0, 0.0, 0.0, Vec3::ZERO)
                .view_projection;
        let rotated = crate::camera::RenderCamera::new(
            Vec3::ZERO,
            aspect,
            camera.yaw,
            camera.pitch,
            0.0,
            Vec3::ZERO,
        )
        .view_projection;

        let baseline_cols = baseline.to_cols_array();
        let rotated_cols = rotated.to_cols_array();
        let any_differs = baseline_cols
            .iter()
            .zip(rotated_cols.iter())
            .any(|(a, b)| (a - b).abs() > EPSILON);
        assert!(
            any_differs,
            "render_camera view projection must differ after applying mouse-driven yaw; \
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
            .set_component(id, MeshComponent::stateless(model.to_string()))
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

    // Regression: the level-load model sweep + clip resolve ran BEFORE the
    // data-archetype dispatch, so descriptor-spawned animated meshes never had
    // their `clip_index` filled (every state stayed `None` → setAnimationState
    // no-ops). The sweep now runs AFTER archetype dispatch. This pins the seam:
    // when resolve runs against a registry that already holds a
    // descriptor-style mesh entity (unresolved `clip_index: None` states), it
    // resolves the indices — proving the resolve sees descriptor-spawned meshes.
    #[test]
    fn resolve_after_archetype_dispatch_fills_descriptor_mesh_clip_index() {
        use crate::scripting::components::mesh::{
            AnimationState, DEFAULT_CROSSFADE_MS, InterruptPolicy, MeshAnimation, MeshComponent,
        };
        use crate::scripting::registry::{EntityRegistry, Transform};
        use std::collections::HashMap;

        // A descriptor-declared animated mesh as it exists right after
        // `apply_data_archetype_dispatch`: states present, every `clip_index`
        // still `None` (the dispatch builds states but does not resolve them).
        let unresolved = |clip: &str| AnimationState {
            clip: clip.into(),
            looping: true,
            crossfade_ms: DEFAULT_CROSSFADE_MS,
            interrupt: InterruptPolicy::Smooth,
            clip_index: None,
        };
        let mut states = HashMap::new();
        states.insert("idle".to_string(), unresolved("idle_clip"));
        states.insert("attack".to_string(), unresolved("attack_clip"));

        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                MeshComponent {
                    model: "models/descriptor_mob/scene.gltf".to_string(),
                    animation: Some(MeshAnimation::new(states, "idle".to_string())),
                },
            )
            .expect("freshly spawned id is live");

        // Before resolve, the descriptor mesh's model is already visible to the
        // sweep — so the single post-dispatch sweep would upload it.
        let models = distinct_mesh_models(&registry);
        assert!(models.contains(&"models/descriptor_mob/scene.gltf".to_string()));

        // Build the clip table the renderer would produce for this model
        // (glTF index order). Hand-built so no GPU is needed.
        let mut tables = scripting_systems::mesh_anim::MeshClipTables::new();
        let meta = vec![
            crate::render::mesh_pass::ClipMetadata {
                name: "idle_clip".to_string(),
                duration: 2.0,
            },
            crate::render::mesh_pass::ClipMetadata {
                name: "attack_clip".to_string(),
                duration: 0.8,
            },
        ];
        tables.insert(
            crate::model::ModelHandle::from("models/descriptor_mob/scene.gltf"),
            &meta,
        );

        resolve_mesh_entity_clips(&mut registry, &tables);

        // The descriptor entity's states are now resolved to concrete glTF
        // indices — the contract that makes `setAnimationState` work at spawn.
        let component = registry
            .get_component::<MeshComponent>(id)
            .expect("mesh component still present");
        let anim = component
            .animation
            .as_ref()
            .expect("animation block present");
        assert_eq!(anim.states.get("idle").unwrap().clip_index, Some(0));
        assert_eq!(anim.states.get("attack").unwrap().clip_index, Some(1));
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

        // The default table carries engine `player.*` slots with `None` values
        // plus two value-bearing engine surfaces: `screen.flash` (resting
        // transparent) and `input.mode` (defaults to `focus`). Setting one of the
        // value-less slots asserts the boundary contract: the snapshot clones
        // value-bearing slots and omits value-less ones.
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
        // `screen.flash` carries its default transparent value, so it is present.
        assert_eq!(
            snapshot.get("screen.flash"),
            Some(&SlotValue::Array(vec![0.0, 0.0, 0.0, 0.0])),
            "engine-owned screen.flash defaults to transparent and is cloned",
        );
        // `input.mode` defaults to `focus`, so it is value-bearing and present.
        assert_eq!(
            snapshot.get("input.mode"),
            Some(&SlotValue::Enum("focus".to_string())),
            "engine-owned input.mode defaults to focus and is cloned",
        );
        // `ui.textEntry` defaults to an empty string, so it is value-bearing and
        // present (the text-edit reactions' writable target).
        assert_eq!(
            snapshot.get("ui.textEntry"),
            Some(&SlotValue::String(String::new())),
            "engine-owned ui.textEntry defaults to empty string and is cloned",
        );
        // `player.ammo` starts value-less and must be excluded, so only the slot
        // we set plus the always-valued `screen.flash`, `input.mode`, and
        // `ui.textEntry` appear.
        assert!(
            snapshot.get("player.ammo").is_none(),
            "value-less slots are skipped",
        );
        assert_eq!(
            snapshot.len(),
            4,
            "only the set player.health and the default-valued screen.flash + input.mode + ui.textEntry appear",
        );
    }

    // --- Animation clock accumulation (scripting.md §10.3) ---

    const CLOCK_EPSILON: f64 = 1e-9;

    #[test]
    fn anim_clock_half_scale_advances_at_half_rate() {
        // With scale 0.5, accumulating the same deltas yields half the elapsed
        // time of a real-time (scale 1.0) clock.
        let dt = 1.0 / 60.0;
        let mut full = 0.0;
        let mut half = 0.0;
        for _ in 0..600 {
            full = App::advance_anim_clock(full, dt, 1.0);
            half = App::advance_anim_clock(half, dt, 0.5);
        }
        assert!(
            (half - full * 0.5).abs() < CLOCK_EPSILON,
            "half-scale clock should be exactly half the real-time clock: full={full}, half={half}"
        );
    }

    #[test]
    fn anim_clock_zero_scale_holds() {
        let dt = 1.0 / 144.0;
        let mut clock = 5.0;
        for _ in 0..100 {
            clock = App::advance_anim_clock(clock, dt, 0.0);
        }
        assert!(
            (clock - 5.0).abs() < CLOCK_EPSILON,
            "scale 0 must hold the clock in place, got {clock}"
        );
    }

    #[test]
    fn anim_clock_mid_accumulation_scale_change_produces_no_jump() {
        // Accumulation (not absolute-time scaling) means changing the scale only
        // affects future deltas — the already-accumulated value is untouched, so
        // there is no discontinuity at the scale-change boundary.
        let dt = 0.01;
        let mut clock = 0.0;
        for _ in 0..50 {
            clock = App::advance_anim_clock(clock, dt, 1.0);
        }
        let before_change = clock; // 50 × 0.01 × 1.0 = 0.5
        // Switch to half scale mid-accumulation. The very next frame advances by
        // dt × 0.5 from `before_change` — no retroactive rescale of the prior 0.5.
        let after_first_half_step = App::advance_anim_clock(clock, dt, 0.5);
        assert!(
            (after_first_half_step - (before_change + dt * 0.5)).abs() < CLOCK_EPSILON,
            "scale change must not retroactively rescale accumulated time"
        );
        assert!(
            after_first_half_step > before_change,
            "clock must keep moving forward (no backward jump) across a scale change"
        );
    }
}
