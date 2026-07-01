// Postretro engine entry point, boot state machine, and level-load orchestration.
// See: context/lib/boot_sequence.md §3 · context/lib/index.md

// Movable navigation agent collide-and-slide harness, driven each tick by the
// steering system in `agent_steering`.
// See: context/lib/movement.md §1, context/lib/entity_model.md §7
mod agent;
// Per-tick navigation-agent steering: replan budget, waypoint following, and
// separation, built on the `agent` harness and `nav::find_path`.
mod agent_steering;
mod audio;
mod camera;
mod candidate_cull;
#[cfg(test)]
mod candidate_cull_mirror;
#[cfg(test)]
mod candidate_cull_probes;
mod collision;
mod compute_cull;
mod frame_timing;
mod fx;
mod health;
mod input;
mod lighting;
mod model;
mod movement;
// The runtime nav graph is built in every build whenever a level carries a
// baked navmesh; pathfinding consumes its query surface.
mod nav;
// Engine-side netcode glue (M15 Phase 3): role selection, the optional endpoint
// held by `App`, the game-logic-owned serialize/apply steps, and client-side
// prediction and reconciliation. The ONLY engine code that touches the registry
// on behalf of replication. See `context/lib/entity_model.md` §6.
mod netcode;
mod options;
mod weapon;

mod portal_vis;
mod render;
mod scripting;
// Live session-lifetime container: all session-lifetime state (scripting core,
// audio, net endpoint, input/UI/modal group, and their bridges and registries),
// held on `App` as `Option<Session>` and built after the first visible frame.
// See: context/lib/boot_sequence.md §1
mod session;
mod shadow_cull;
mod sim;
mod startup;
mod ui_texture;
mod view_feel;
mod visibility;

#[cfg(test)]
mod alloc_probe;

// Rooted here (not under `scripting/`) so `gen_script_types.rs` can reuse the
// `scripting` tree via `#[path]` without pulling in wgpu/engine-dependent code.
#[path = "scripting/systems/mod.rs"]
mod scripting_systems;

// Test-only counting global allocator. `#[global_allocator]` must annotate a
// crate-root static, so the static lives here; the allocator type and its
// counters live in `alloc_probe`. Gated on `#[cfg(test)]` so it never touches
// the production binary — the IR eval pass's zero-allocation guarantee is
// asserted by a test that arms the counters around `eval_value`.
#[cfg(test)]
#[global_allocator]
static COUNTING_ALLOCATOR: alloc_probe::CountingAllocator = alloc_probe::CountingAllocator;

use std::collections::VecDeque;
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
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes};

use crate::camera::Camera;
use crate::frame_timing::{FrameRateMeter, FrameTiming, InterpolableState};
use crate::input::{Action, ButtonState, DiagnosticAction, InputFocus};
use crate::render::Renderer;
use crate::scripting::state_persistence::{
    STATE_FILE_PATH, collect_persisted_state, save_persisted_state,
};
// Session-owned types referenced in `main.rs` only by `#[cfg(test)]` code, so
// they are gated test-only to keep the bin build warning-free.
use crate::startup::{
    BootState, FRONTEND_CLEAR_COLOR, InFlightLevelLoad, LevelRequest, LevelSource, LoadOutcome,
    SplashSource, StartupTimings,
};
#[cfg(test)]
use postretro_entities::ScriptCtx;
#[cfg(test)]
use postretro_scripting_core::reaction_dispatch::ProgressTracker;
#[cfg(test)]
use postretro_scripting_core::runtime::ScriptRuntime;
// Positional-map-path recovery lives with boot construction; re-exported at the
// crate root so `crate::resolve_map_path` keeps resolving for the netcode CLI
// tests. Test-only: the boot path calls it through `startup::session`.
#[cfg(test)]
pub(crate) use crate::startup::session::resolve_map_path;
use crate::visibility::{
    CameraCullVisibility, VisibilityPath, VisibilityResult, VisibilityStats, VisibleCells,
};
use postretro_entities::SystemReactionCommand;
use postretro_foundation::ModThemeTokens;
use postretro_scripting_core::data_descriptors::RegisteredUiTree;
use postretro_scripting_core::reaction_dispatch::{
    fire_named_event, fire_named_event_with_sequences,
};
use postretro_scripting_core::runtime::{
    Frontend, MenuCamera, ReloadSummary, StagedManifestCommitOutcome,
};
use postretro_scripting_core::staged_manifest::{
    StagedManifestBuildResult, StagedManifestBuildStatus,
};

/// Fraction of a vignette reaction's single `durationMs` spent ramping in. The
/// author supplies one duration (mirroring `flashScreen`); the drain splits it
/// into a short rise so the vignette eases in rather than snapping to peak, with
/// the remainder spent decaying back to rest. See `dispatch_system_commands`.
const VIGNETTE_RISE_FRACTION: f32 = 0.2;

fn staged_ui_commit_payload(
    result: &StagedManifestBuildResult,
    outcome: &StagedManifestCommitOutcome,
) -> Option<(Vec<RegisteredUiTree>, ModThemeTokens, Option<Frontend>)> {
    if !matches!(outcome, StagedManifestCommitOutcome::Committed { .. }) {
        return None;
    }

    match &result.status {
        StagedManifestBuildStatus::Built(manifest) => Some((
            manifest.ui_trees.clone(),
            manifest.theme.clone(),
            manifest.frontend.clone(),
        )),
        StagedManifestBuildStatus::NoStartScript => {
            Some((Vec::new(), ModThemeTokens::default(), None))
        }
        StagedManifestBuildStatus::Failed => None,
    }
}

fn apply_menu_camera_pose(
    camera: &mut Camera,
    frame_timing: &mut FrameTiming,
    menu_camera: &MenuCamera,
) {
    let position = Vec3::from_array(menu_camera.position);
    camera.position = position;
    camera.yaw = menu_camera.yaw;
    camera.pitch = menu_camera.pitch;
    frame_timing.hold_state(InterpolableState::new(position));
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
fn distinct_mesh_models(registry: &postretro_entities::EntityRegistry) -> Vec<String> {
    use postretro_entities::{ComponentKind, ComponentValue};

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
    registry: &mut postretro_entities::EntityRegistry,
    tables: &scripting_systems::mesh_anim::MeshClipTables,
) {
    use postretro_entities::{ComponentKind, ComponentValue};

    // Collect ids first so the mutable per-entity writes do not alias the
    // immutable iteration borrow. Mesh entity counts are small.
    let animated: Vec<postretro_entities::EntityId> = registry
        .iter_with_kind(ComponentKind::Mesh)
        .filter_map(|(id, value)| match value {
            ComponentValue::Mesh(mesh) if mesh.animation.is_some() => Some(id),
            _ => None,
        })
        .collect();

    for id in animated {
        let Ok(mut component) = registry
            .get_component::<postretro_entities::components::mesh::MeshComponent>(id)
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

/// Level-load cross-check: for every archetype that declares both a mesh model
/// and `health.zoneMultipliers`, warn ONCE per archetype per declared tag that
/// names no zone on the spawned model. The unknown set is computed by the pure,
/// unit-tested `unknown_zone_multiplier_tags`; this is a thin warn-only caller,
/// modeled on `resolve_mesh_entity_clips`. An archetype whose model has no
/// hit-zone entry (load failed, or an AABB-only model) treats every declared tag
/// as unknown — the model carries no zones to satisfy them.
fn warn_unknown_zone_multipliers(
    descriptors: &[postretro_entities::EntityTypeDescriptor],
    store: &scripting_systems::hit_zones::HitZoneStore,
) {
    for desc in descriptors {
        let (Some(mesh), Some(health)) = (desc.mesh.as_ref(), desc.health.as_ref()) else {
            continue;
        };
        if health.zone_multipliers.is_empty() {
            continue;
        }
        let handle = crate::model::ModelHandle::from(mesh.model.clone());
        let declared = health.zone_multipliers.keys().map(String::as_str);
        // A model with no hit-zone entry carries no zones: every declared tag is
        // unknown. Pass an empty zone table so the cross-check reports them all.
        let empty_zones: Vec<Option<crate::model::gltf_loader::JointZone>> = Vec::new();
        let joint_zones = store
            .get(&handle)
            .map(|m| m.joint_zones.as_slice())
            .unwrap_or(&empty_zones);
        let unknown =
            scripting_systems::hit_zones::unknown_zone_multiplier_tags(declared, joint_zones);
        let archetype = desc.canonical_name.as_deref().unwrap_or("<unnamed>");
        for tag in &unknown {
            log::warn!(
                "[HitZones] archetype '{archetype}' declares health.zoneMultipliers tag '{tag}' \
                 absent from model '{}' — that multiplier never applies",
                mesh.model,
            );
        }
    }
}

// Policy chokepoint: the frame loop queues a staged build only when a changed
// path matched the active mod-init dependency set (classified by ScriptRuntime).
fn reload_summary_requires_mod_init(summary: ReloadSummary) -> bool {
    summary.mod_init
}

fn main() -> Result<()> {
    env_logger::init();
    log::info!("[Engine] Postretro starting");

    // Build boot-lifetime `App` state (args, content root, camera, frame
    // timing, the `pending_session` bundle) and the event loop. The entire
    // `Session` (options I/O, audio, scripting core, input/UI/modal group,
    // net endpoint) is constructed post-first-pixel by `Session::build` via
    // `install_pending_session`. Mod init and the first level-load worker are
    // deferred to the splash loop. See: context/lib/boot_sequence.md §1.
    let startup::BootSession {
        event_loop,
        mut app,
    } = startup::build_session()?;

    event_loop
        .run_app(&mut app)
        .context("event loop terminated with error")?;

    app.exit_result
}

fn window_attributes() -> WindowAttributes {
    // The window is created VISIBLE (winit default). A "create hidden, reveal
    // after first present" scheme was tried to suppress the Windows
    // pre-first-present white flash but caused a boot HANG on Windows: winit's
    // `request_redraw()` uses `RedrawWindow(.., RDW_INTERNALPAINT)`, and Windows
    // does not deliver `WM_PAINT`/`RedrawRequested` to an invisible window — so
    // the redraw-driven splash loop (default `ControlFlow::Wait`, blocking in
    // `MsgWaitForMultipleObjectsEx`) never advanced past frame 0 and never
    // revealed the window. A booting engine with a brief cosmetic flash is
    // strictly better than a hang. A proper flash fix needs a platform approach
    // that does not gate the first frame on an OS paint event delivered to a
    // hidden window (e.g. a Win32 class background brush matching the splash
    // color). See: context/lib/boot_sequence.md §1 (Splash state machine).
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

pub(crate) struct App {
    renderer: Option<Renderer>,

    window_state: Option<WindowState>,
    level: Option<postretro_level_loader::LevelWorld>,
    /// Runtime navigation graph, built once when a level with a baked navmesh
    /// loads. `None` when the map has no navmesh bake. Pathfinding reads this in
    /// every build; the `Alt+Shift+N` debug overlay (dev-tools-only) also
    /// consumes it.
    nav_graph: Option<nav::NavGraph>,

    /// Optional map path resolved from CLI args. When absent, boot lands in
    /// Frontend after the splash instead of spawning the level-load worker.
    map_path: Option<PathBuf>,

    /// Derived from the map path at startup. `textures/` and `scripts/`
    /// sibling directories are resolved relative to this root.
    content_root: PathBuf,

    exit_result: Result<()>,

    camera: Camera,

    /// Live session-lifetime container: all post-first-pixel subsystems
    /// (scripting core, audio, net endpoint, input/UI/modal group, and their
    /// bridges and registries), built by `Session::build` and installed through
    /// `PendingSessionInit::install`. `None` during boot (Booting/Splash before
    /// the install redraw) — boot-phase code physically cannot name a session
    /// field. Becomes `Some` for the rest of the run; a failed build exits boot.
    /// See: context/lib/boot_sequence.md §1.
    session: Option<session::Session>,

    /// Persistent crouch toggle latch for `CrouchMode::Toggle`. Flipped on each
    /// `Action::Crouch` press rising edge by the input layer; fed into
    /// `MovementInput::crouch_intent`. Lives on `App` (the input layer), NEVER on
    /// the movement component. Inert in `CrouchMode::Hold` (hold tracks the
    /// button level directly). See: context/lib/input.md, context/lib/player_options.md
    crouch_toggle_active: bool,

    /// Warn-once latch for the enemy-AI tick. Keyed, namespaced diagnostics fire
    /// exactly once across the run rather than each tick: `anim:<name>` for an
    /// animation state that fails to switch (`UnknownState`/`NotAnimated`, prior
    /// animation kept) and `blocked:<id>` for a chasing enemy whose agent found
    /// no path. Lives on `App` (the AI tick owner), threaded into
    /// `scripting_systems::ai::run_ai_tick`. See: scripting/systems/ai.rs.
    ai_warned: std::collections::HashSet<String>,

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

    /// The currently committed mod theme override. Successful staged mod-init
    /// commits replace this complete snapshot before a fresh merge over engine
    /// defaults reaches the renderer.
    mod_theme_override: ModThemeTokens,

    /// The mode signal observed during THIS frame's input phase, resolved into
    /// `input_mode_tracker` at the head of the game-logic phase. Mouse motion
    /// (`CursorMoved`) votes `Pointer`; any nav input (stick edge / D-pad / nav
    /// key) votes `Focus`. Nav wins when both occur in one frame (a deliberate
    /// nav press dominates incidental cursor drift). Cleared each frame after the
    /// tracker consumes it. See: context/lib/input.md §7.
    pending_mode_signal: Option<scripting_systems::input_mode::ModeSignal>,

    /// Punch-through `nav.menu` toggle: set when a `nav.menu`
    /// intent (gamepad Start, or keyboard Escape-from-gameplay) is produced in the
    /// input phase, then consumed in the game-logic phase to push (open) or pop
    /// (close) the registered `pauseMenu` via the engine push/pop API. `nav.menu` opens
    /// the menu from gameplay where the UI-dispatch seam is `Passthrough` and so
    /// queues nothing — hence the dedicated punch-through, mirroring how
    /// `ToggleDebugPanel` bypasses the capture gate. See: context/lib/input.md §7.
    pending_menu_toggle: bool,

    /// App-local quit request raised by the reserved `ui.exitToDesktop` button
    /// action. The UI action classifier is generic, but only the event-loop owner
    /// actually exits, so this flag is drained in the redraw/game-logic phase
    /// where `ActiveEventLoop` is available.
    pending_exit_to_desktop: bool,

    /// The focused node id the focus engine resolved THIS frame's game-logic
    /// phase, published on this frame's snapshot so the UI pass draws the focus
    /// ring around it. `None` when nothing is focused.
    ui_focused_id: Option<String>,

    /// Per-emitter live-particle tally, produced by `particle_sim::tick` and
    /// consumed by the next frame's `emitter_bridge.update` for cap headroom.
    /// Owned here (not re-allocated per frame) so the collapsed pass reuses one
    /// buffer's capacity across frames. See: context/lib/scripting.md §10.1 (Emitter and Particles).
    particle_live_counts: std::collections::HashMap<postretro_entities::EntityId, usize>,

    /// World-space static-geometry collider built from PRL static geometry.
    /// See: context/lib/entity_model.md §7
    collision_world: collision::CollisionWorld,

    /// Active wieldable instance equipped by the player. The companion
    /// descriptor name lets mod-init hot reload refresh authored weapon stats
    /// while preserving per-instance cooldown.
    active_wieldable: Option<postretro_entities::EntityId>,
    active_wieldable_descriptor: Option<String>,

    /// Boot state machine: drives the splash → first-level-frame transition.
    /// Subsumes the previous `level_load_fired` one-shot flag.
    boot_state: BootState,

    /// Counts splash frames since `resumed()`. The state machine uses this to
    /// schedule the deferred `mod_init` and boot load request after the first
    /// visible splash frame; Loading owns worker polling.
    splash_frame: u32,

    /// Set when `Loading → Running` transitions; consumed at the bottom of the
    /// first `Running` frame after `render_frame_indirect` returns. Ensures
    /// log line C ends with `first_level_frame` covering the cost of the
    /// frame the user actually sees.
    pending_level_log: bool,

    /// Set during `mod_init` if a mod registers a `SplashSource` override.
    /// The consume path in `run_splash_frame` frame 1 is wired; today the field
    /// stays `None` because no mod system yet calls the setter.
    /// See: context/lib/boot_sequence.md §9 (Planned).
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

    /// A retained copy of the level's `player_spawn` placements for the host's
    /// runtime net-slot accept path (M15 Phase 3 Task 4). Unlike
    /// `pending_spawn_points` (consumed at install), this survives so each accepted
    /// client's descriptor-backed remote pawn can be spawned from its deterministically
    /// assigned placement. Empty before level load and on maps with no player_spawn.
    host_spawn_points: Vec<crate::scripting::map_entity::MapEntity>,

    /// Non-player-start map entities partitioned out of `world.map_entities`
    /// during install, awaiting the data-archetype sweep on the same frame.
    /// `None` before level load and after the sweep consumes them.
    pending_map_entities: Option<Vec<crate::scripting::map_entity::MapEntity>>,

    /// Seconds since level load, not wall clock. Resets to zero on level unload
    /// and during level install. Maintained for future engine consumers that need a
    /// level-relative monotonic clock.
    script_time: f64,

    /// Game-layer animation clock: accumulates `frame_dt × anim_time_scale` each
    /// render frame, advanced beside `script_time` at the same site and gated by
    /// the same dev-tools `freeze_time()` flag. All skeletal-animation timing
    /// (entry stamps, clip-local times, fade windows, the pending-stamp resolve)
    /// reads this clock. Accumulation — not scaling of absolute time — so
    /// changing `anim_time_scale` never jumps existing poses. Resets to zero on
    /// level unload and during level install. See: context/lib/scripting.md §10.3.
    anim_time: f64,

    /// Per-frame multiplier on the animation clock's advancement. `1.0` is
    /// real-time; `0.5` half-rate; `0.0` holds every clip and fade (pause). The
    /// slow-motion seam — no script surface yet (engine-side field only).
    anim_time_scale: f64,

    /// Per-stage durations for the boot log line, in record order: args_parsed,
    /// event_loop_created, window_created, wgpu_init, first_black_frame,
    /// splash_decoded, splash_uploaded, first_splash_frame, then the
    /// post-first-pixel deferred-session marks (audio_init_complete,
    /// script_runtime_ctor, net_endpoint_complete, session_init_complete),
    /// renderer_full_init_complete, and (CLI-map boot) boot_worker_dispatch. The
    /// script runtime is constructed inside `Session::build`, so its mark fires
    /// after the logo frame — not in early engine boot.
    /// See: context/lib/boot_sequence.md §1.
    boot_timings: StartupTimings,

    /// Per-stage durations for log line B — mod init (mod_init,
    /// mod_splash_swap [conditional]).
    mod_timings: StartupTimings,

    /// Per-stage durations for log line C — level load. Worker-thread stages
    /// are merged in between `worker_dispatch` and `worker_delivered`; see
    /// `StartupTimings` doc comment.
    level_timings: StartupTimings,

    /// Metadata for the active Loading-state request. Catalog loads retain the
    /// resolved catalog entry here; raw dev-path loads synthesize a non-catalog
    /// entry so install code can read consistent map metadata before data
    /// scripts run.
    level_load: Option<InFlightLevelLoad>,

    /// Catalog classification tags for the installed level. Catalog loads copy
    /// these from the resolved map entry; raw path/dev loads keep this empty.
    active_level_tags: Vec<String>,

    /// Source for the installed level, retained after `level_load` is consumed so
    /// `restartLevel()` can requeue the same catalog id or raw dev path.
    active_level_source: Option<LevelSource>,

    /// Receives the active level worker's `LoadOutcome`. `None` when no load is
    /// in flight; consumed via `try_recv` by the `Loading` state.
    level_rx: Option<mpsc::Receiver<LoadOutcome>>,

    /// Owned so the thread is detached (not joined) when App drops.
    /// Detached on shutdown — drop discards the JoinHandle without joining;
    /// the OS thread reaps when its work returns.
    level_worker: Option<JoinHandle<()>>,

    /// Runtime level lifecycle requests drained by `startup::lifecycle` at the
    /// redraw boundary, before gameplay/world work for the frame runs.
    level_requests: VecDeque<LevelRequest>,

    /// One-shot marker for the CLI boot map load. Runtime load failures fall
    /// back to Frontend; this boot load exits non-zero if the worker fails or
    /// returns an empty payload.
    boot_load: bool,

    /// Deferred-startup owner: the raw inputs (`argv`) needed to construct the
    /// entire `Session` AFTER the first visible logo frame. `Some` from boot
    /// construction until `install_pending_session` consumes it on the first logo
    /// splash frame; `None` afterward. The `Option::take` is the single-commit
    /// guard so a suspend/resume re-entering the splash loop never runs deferred
    /// init twice. See: context/lib/boot_sequence.md §1, §5.
    pending_session: Option<startup::PendingSessionInit>,

    /// The dev-tools "chase me" demo agent (spawned by `Alt+Shift+G`). `None`
    /// until first spawned; spawned at most once per level (cleared on level
    /// unload). Each tick the agent re-targets the player pawn's `Transform`
    /// (or the camera when no pawn exists) so it pathfinds toward the player.
    #[cfg(feature = "dev-tools")]
    debug_chase_agent: Option<postretro_entities::EntityId>,
}

struct WindowState {
    window: Arc<Window>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiButtonAction {
    CommitTextEntry,
    CloseDialog,
    ExitToDesktop,
    QuitToMenu,
    NamedReaction,
}

fn classify_ui_button_action(on_press: &str) -> UiButtonAction {
    match on_press {
        render::ui::actions::COMMIT_TEXT_ENTRY_ACTION => UiButtonAction::CommitTextEntry,
        render::ui::actions::CLOSE_DIALOG_ACTION => UiButtonAction::CloseDialog,
        render::ui::actions::EXIT_TO_DESKTOP_ACTION => UiButtonAction::ExitToDesktop,
        render::ui::actions::QUIT_TO_MENU_ACTION => UiButtonAction::QuitToMenu,
        _ => UiButtonAction::NamedReaction,
    }
}

fn frontend_background_level_source(frontend: Option<&Frontend>) -> Option<LevelSource> {
    frontend
        .and_then(|frontend| frontend.background_level.as_ref())
        .map(|background_level| LevelSource::Catalog(background_level.clone()))
}

fn frontend_return_requests(frontend: Option<&Frontend>) -> Vec<LevelRequest> {
    let mut requests = vec![LevelRequest::Unload];
    if let Some(source) = frontend_background_level_source(frontend) {
        requests.push(LevelRequest::Load(source));
    }
    requests
}

fn focused_button_on_press(
    rects: Option<&render::ui::tree::FocusRectList>,
    focused_id: Option<&str>,
) -> Option<String> {
    use render::ui::tree::NodeInteraction;

    let focused_id = focused_id?;
    rects?
        .rects
        .iter()
        .find(|r| r.id == focused_id)
        // A disabled focused node is non-interactive (M13 G2-T3): block its
        // activation regardless of how the focus arrived (a pre-existing focus
        // that became disabled, or a click that fell through). The focus engine
        // already keeps disabled nodes unreachable; this is the App-side gate on
        // the activation path itself.
        .filter(|r| !r.disabled)
        .and_then(|r| match &r.interaction {
            Some(NodeInteraction::Button { on_press, .. }) => Some(on_press.clone()),
            _ => None,
        })
}

fn route_ui_button_action(
    on_press: &str,
    modal_stack: &mut render::ui::modal_stack::ModalStack,
) -> UiButtonAction {
    match classify_ui_button_action(on_press) {
        UiButtonAction::CloseDialog => {
            modal_stack.pop();
            UiButtonAction::CloseDialog
        }
        other => other,
    }
}

fn apply_pause_menu_nav_policy(modal_stack: &mut render::ui::modal_stack::ModalStack) {
    match modal_stack.active_name() {
        Some(render::ui::demo::PAUSE_MENU_NAME) => modal_stack.pop(),
        None => modal_stack.push_named(render::ui::demo::PAUSE_MENU_NAME, None),
        Some(_) => {}
    }
}

fn gameplay_snapshot_for_capture_state(
    latch: &mut input::GameplayInputLatch,
    frame_snapshot: &input::ActionSnapshot,
    ticks: u32,
    ui_captures_gameplay: bool,
) -> Option<input::ActionSnapshot> {
    if ui_captures_gameplay {
        latch.clear();
        return (ticks > 0).then(input::ActionSnapshot::neutral);
    }

    latch.snapshot_for_ticks(frame_snapshot, ticks)
}

fn gameplay_capture_gate_for_frame(
    ui_captured_gameplay_at_frame_start: bool,
    modal_stack: &render::ui::modal_stack::ModalStack,
) -> bool {
    ui_captured_gameplay_at_frame_start
        || modal_stack.top_capture_mode() == render::ui::descriptor::CaptureMode::Capture
}

fn build_sim_command(
    snapshot: &input::ActionSnapshot,
    camera: &Camera,
    crouch_intent: bool,
    dash_pressed: bool,
    shoot_pressed: bool,
) -> sim::SimCommand {
    let jump_pressed = snapshot.button(Action::Jump).is_active();
    let sprint = snapshot.button(Action::Sprint).is_active();
    let shoot = snapshot.button(Action::Shoot);

    sim::SimCommand {
        movement: movement::MovementInput {
            wish_dir: glam::Vec2::new(
                snapshot.axis_value(Action::MoveRight),
                snapshot.axis_value(Action::MoveForward),
            ),
            jump_pressed,
            dash_pressed,
            running: sprint,
            crouch_intent,
            facing_yaw: camera.yaw,
        },
        fire_button: weapon::FireButtonState {
            pressed: shoot_pressed,
            active: shoot.is_active(),
        },
    }
}

fn build_post_movement_command(camera: &Camera) -> sim::PostMovementCommand {
    let (aim_origin, aim_direction) = camera.aim_ray();
    sim::PostMovementCommand {
        aim_origin,
        aim_direction,
    }
}

fn has_player_pawn(registry: &postretro_entities::EntityRegistry) -> bool {
    use postretro_entities::ComponentKind;

    registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .next()
        .is_some()
}

/// Resolve the followed player pawn: registry marker first, then first
/// `PlayerMovement` entity. See also `local_movement_pawn` (sim/mod.rs)
/// and `player_position` (scripting/systems/ai.rs).
fn followed_player_pawn(
    registry: &postretro_entities::EntityRegistry,
) -> Option<postretro_entities::EntityId> {
    use postretro_entities::ComponentKind;

    if let Some(id) = registry.local_player_pawn() {
        if matches!(
            registry.has_component_kind(id, ComponentKind::PlayerMovement),
            Ok(true)
        ) {
            return Some(id);
        }
    }

    registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .next()
        .map(|(id, _)| id)
}

/// Follow the camera to the local pawn's eye. `presentation_offset` is the M15
/// Phase 3 Task 5 local-pawn correction offset (the decaying difference between the
/// predicted and reconciled pose); it is added to the gameplay-authoritative
/// registry transform so the first-person eye glides smoothly across a reconcile
/// correction without rubber-banding. The offset is always `Vec3::ZERO` at tick rate
/// (both the single-player/host path and the connected-client tick path pass zero);
/// the real offset is read from `ClientPrediction` at render rate by the render seam.
fn follow_camera_to_local_pawn(
    camera: &mut Camera,
    registry: &postretro_entities::EntityRegistry,
    presentation_offset: Vec3,
) {
    use postretro_entities::Transform;

    if let Some(id) = followed_player_pawn(registry) {
        if let (Ok(component), Ok(transform)) = (
            registry.get_component::<postretro_foundation::PlayerMovementComponent>(id),
            registry.get_component::<Transform>(id),
        ) {
            camera.position = transform.position
                + presentation_offset
                + Vec3::new(0.0, component.capsule.eye_height, 0.0);
        }
    }
}

#[cfg(feature = "dev-tools")]
fn update_debug_chase_agent_destination(
    registry: &mut postretro_entities::EntityRegistry,
    debug_chase_agent: Option<postretro_entities::EntityId>,
    fallback_target: Vec3,
) {
    use postretro_entities::Transform;

    let Some(agent) = debug_chase_agent else {
        return;
    };
    let target = followed_player_pawn(registry)
        .and_then(|id| registry.get_component::<Transform>(id).ok())
        .map(|t| t.position)
        .unwrap_or(fallback_target);
    agent_steering::set_destination(registry, agent, target);
}

/// Whether the clean-exit handler should write persistent slots to `state.json`
/// (M15 Phase 3.5 Task 5). The save runs only when the state-store lifecycle has
/// committed declarations and restore (`can_save`) AND this process is not a
/// connected client. A connected client's replicated slots (`player.health`,
/// `player.maxHealth`, shared mod slots) are server-authoritative values applied
/// through the replicated-state path, not local edits — persisting them would write
/// another peer's authoritative state into this client's save file. Single-player
/// (`is_connected_client == false`) and the host both save unchanged.
fn should_save_persisted_state(can_save: bool, is_connected_client: bool) -> bool {
    can_save && !is_connected_client
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
        // The window is created VISIBLE (winit default). The redraw-driven
        // splash loop relies on the OS delivering `RedrawRequested` after the
        // `request_redraw()` below, which on Windows only happens for a visible
        // window (a hidden window gets no `WM_PAINT`). This path also runs on
        // resume (resume resets to Booting and recreates the window).
        // See: context/lib/boot_sequence.md §1.
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
        // post-paint window so the OS window opens and presents its first frame
        // as fast as possible. See `run_splash_frame` and
        // `context/lib/boot_sequence.md` §1 (Splash state machine).

        let size = window.inner_size();
        self.camera.update_aspect(size.width, size.height);

        self.renderer = Some(renderer);
        self.window_state = Some(WindowState { window });
        // NOTE: the committed mod theme is NOT applied here. `Renderer::new`
        // returns a boot-ready renderer with `full: None`, and `set_ui_theme`
        // (reached via `apply_mod_ui_theme_to_renderer`) is a full-ready path
        // that touches `Renderer::full` — calling it now would panic on the
        // full-ready guard (renderer_splash.rs). The full renderer is built
        // later this boot in `run_splash_frame_one::finish_renderer_full_init`,
        // and the committed theme (engine-default or mod override) is installed
        // right after, inside `run_deferred_mod_init`. That path also re-runs on
        // resume (the splash loop replays from frame 0), so the rebuilt full
        // renderer re-receives the theme there — making an apply here redundant
        // as well as unsafe. A no-mod-theme boot needs no apply at all: the
        // full renderer is constructed with `UiTheme::engine_default()`.

        // Audio init, net-endpoint setup, and dev debug-UI creation are deferred
        // out of this pre-redraw path: audio + net build inside `Session::build`
        // (via `install_pending_session`) and the debug UI lazy-builds via
        // `ensure_debug_ui`, all on the first visible logo frame (or the fallback
        // black frame) in `run_splash_frame_one`, so the OS window opens as fast
        // as practical. See: context/lib/boot_sequence.md §1.

        // Input focus is now session-owned: the session is built later this boot
        // (post-first-pixel) with `InputFocus::Gameplay`, and the cursor is
        // captured by the first `reconcile_ui_focus` once gameplay runs. Boot /
        // splash needs no pointer lock, so nothing to set here pre-install.
        self.frame_timing.last_frame = Instant::now();
        self.enter_splash_state();

        // Drive the redraw loop so `RedrawRequested` fires the first splash
        // frame and the boot state machine can advance.
        if let Some(ws) = self.window_state.as_ref() {
            ws.window.request_redraw();
        }

        log::info!("[Engine] Window ready");
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // Audit which boot phase a suspend interrupts. The resume path resets to
        // `Booting` and re-drives the splash loop; the single-commit guards
        // (`pending_session.take`, renderer full-ready idempotence) keep session
        // init and renderer completion from re-running. See: boot_sequence §1, §5.
        log::info!(
            "[Engine] Suspended during boot phase {:?}",
            self.boot_phase()
        );
        self.window_state = None;
        self.renderer = None;
        // Session-owned debug UI is reset here (it borrows the window and reads
        // the renderer's device limits); `ensure_debug_ui` rebuilds it on the next
        // resumed splash loop. The rest of the session survives suspend.
        #[cfg(feature = "dev-tools")]
        if let Some(session) = self.session.as_mut() {
            session.debug_ui = None;
        }
        self.clear_surface_lifetime_level_state();
        // Drop any in-flight level-load worker handoff. On resume the splash
        // state machine starts over from frame 0 and will spawn a fresh
        // worker; holding a stale receiver/handle would either block install
        // forever or deliver into the wrong boot phase.
        self.level_load = None;
        self.level_rx = None;
        self.level_worker = None;
        self.reset_boot_state_after_suspend();
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
            // `input_focus` is session-owned; before the session installs it is
            // effectively `Gameplay` (no UI consumer), so egui consumption is
            // ignored. The debug UI itself only exists post-install.
            let focus = self
                .session
                .as_ref()
                .map(|session| session.input_focus)
                .unwrap_or(InputFocus::Gameplay);
            // `debug_ui` is session-owned; borrow the session for it and the
            // window (a disjoint `self` field) together.
            if let (Some(session), Some(ws)) = (self.session.as_mut(), self.window_state.as_ref())
                && let Some(debug_ui) = session.debug_ui.as_mut()
            {
                let response = debug_ui.on_window_event(&ws.window, &event);
                if focus != InputFocus::Gameplay {
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
                    // split needs the "is a capturing tree on the stack?" flag,
                    // sourced from the modal stack's top capture mode.
                    // See: context/lib/input.md
                    // The UI seam and gameplay forward are session-owned; boot
                    // phase (pre-install) ignores gameplay/UI key input. The
                    // diagnostic resolver above already ran so dev chords still
                    // work during boot. Mode-signal / menu-toggle votes are
                    // collected here and applied after the session borrow ends.
                    let Some(session) = self.session.as_mut() else {
                        return;
                    };
                    let mut record_nav_signal = false;
                    let mut set_menu_toggle = false;

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
                        session.ui_focus.release_repeat();
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
                        session.ui_focus.release_confirm_repeat();
                    }
                    // Text-entry routing (M13 Text-Entry, Task 3): while a text-entry
                    // tree is the top of the modal stack, hardware key-down events
                    // drive the edit surface instead of nav. The LOGICAL key resolves
                    // Backspace/Enter/Escape first (so a `\u{8}` Backspace text or a
                    // `\r` Enter text never leaks through the printable channel); only
                    // a non-control printable `KeyEvent.text` becomes a `Text` intent.
                    // Enter/Escape ride the queue as `nav.confirm`/`nav.cancel`, which
                    // the focus-resolution stage intercepts for commit/cancel.
                    let text_entry_open = session.modal_stack.active_text_entry_target().is_some();
                    // Text entry intentionally honors OS key-repeat (Text-Entry AC4:
                    // hardware-key repeat comes from the OS): a held Backspace/letter
                    // appends/deletes on each auto-repeat. All OTHER UI input stays
                    // edge-only (`!key_event.repeat`) — nav intents must not re-fire on
                    // a held key, since the focus engine's own dt clock owns nav repeat.
                    let nav_intent = if pressed && (!key_event.repeat || text_entry_open) {
                        if text_entry_open {
                            // A key inside text entry is always a `focus`-mode signal.
                            record_nav_signal = true;
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
                                session.ui_dispatch.mode() == input::UiCaptureMode::Capture;
                            let intent = input::nav_intent_for_key(code, capturing);
                            if intent.is_some() {
                                // A nav key (arrows/enter/escape/tab) is a `focus`-mode
                                // signal — it switches the interaction mode off pointer.
                                record_nav_signal = true;
                            }
                            // Escape-from-gameplay maps to `nav.menu` (opens the pause
                            // menu). The seam is `Passthrough` from gameplay and queues
                            // nothing, so route the toggle through the punch-through flag.
                            if intent == Some(input::NavIntent::Menu) {
                                set_menu_toggle = true;
                            }
                            intent.map(input::UiIntentPayload::Nav)
                        }
                    } else {
                        None
                    };
                    if session
                        .ui_dispatch
                        .dispatch_event(nav_intent)
                        .forwards_to_gameplay()
                        && session.input_focus == InputFocus::Gameplay
                    {
                        // Only Gameplay forwards keys to the action system. When
                        // the debug panel (or future menu) owns focus, WASD must
                        // not drive the camera even though egui leaves
                        // `consumed = false` for non-text widgets like sliders.
                        session.input_system.handle_keyboard_event(code, pressed);
                    }

                    if record_nav_signal {
                        self.record_mode_signal(
                            scripting_systems::input_mode::ModeSignal::NavInput,
                        );
                    }
                    if set_menu_toggle {
                        self.pending_menu_toggle = true;
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
                // Boot phase ignores mouse input until the session installs.
                let Some(session) = self.session.as_mut() else {
                    return;
                };
                if !session
                    .ui_dispatch
                    .dispatch_event(click_intent)
                    .forwards_to_gameplay()
                {
                    return;
                }
                // Same focus gate as the keyboard path: mouse-button actions
                // (fire, alt-fire) must not fire while DevTools/Menu owns
                // input. See: context/lib/input.md §5
                if session.input_focus == InputFocus::Gameplay {
                    session
                        .input_system
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
                } else {
                    // Release the cursor while unfocused but leave
                    // `input_focus` alone — the user's chosen focus mode
                    // outlives transient OS focus loss. Input clears are
                    // session-owned; nothing to clear before install.
                    if let Some(ws) = self.window_state.as_ref() {
                        input::cursor::release_cursor(&ws.window);
                    }
                    if let Some(session) = self.session.as_mut() {
                        session.input_system.clear_all();
                        session.gameplay_input_latch.clear();
                    }
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

                // Drain changed paths every frame so the watcher channel does
                // not back up even when the summary is empty. ScriptRuntime
                // checks them against the active dependency set before queuing
                // the serialized staged build.
                //
                // Guarded behind the per-boot signal "the splash logo frame has
                // presented this boot cycle" (`splash_frame >= 2`: frame 0 = black,
                // frame 1 = logo) — so reload draining never runs before the splash
                // logo paints, and a suspend→resume re-blocks it until the resumed
                // logo repaints (suspend resets `splash_frame` to 0). Past the logo
                // also guarantees the script runtime exists: the watcher starts in
                // the deferred mod init on the logo frame, and the runtime is
                // session-lifetime. See: context/lib/boot_sequence.md §1.
                if crate::startup::boot_allows_reload_drain(self.splash_frame >= 2) {
                    self.drain_script_reload_requests();
                }

                if !self.drive_boot_state_for_redraw(event_loop) {
                    return;
                }

                if self.boot_state == BootState::Frontend {
                    if !self.run_frontend_ui_logic(event_loop, frame_dt) {
                        return;
                    }
                    self.render_frontend_frame(event_loop, now);
                    return;
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
                // Reached only in Running (Frontend returned above), so the
                // session is installed. Disjoint borrows of the session group and
                // the non-session `nav_stick_tracker`; mode-signal and menu-toggle
                // votes are collected and applied after the borrow ends.
                let (gamepad_nav_seen, gamepad_menu_toggle) = {
                    let App {
                        session,
                        nav_stick_tracker,
                        ..
                    } = self;
                    let mut nav_seen = false;
                    let mut menu_toggle = false;
                    if let Some(session) = session.as_mut() {
                        if let Some(gp) = session.gamepad_system.as_mut() {
                            let gp_nav = gp.update(&mut session.input_system, nav_stick_tracker);
                            // Advance any active rumble's timeout in the input stage
                            // and stop it once its duration elapses (started by a
                            // drained `Rumble` command on a prior frame).
                            gp.tick_rumble(frame_dt);
                            // A confirm (South) RELEASE stops the activation-repeat
                            // clock — the gamepad twin of the keyboard Enter-release.
                            if gp_nav.confirm_released {
                                session.ui_focus.release_confirm_repeat();
                            }
                            // No directional input held releases the directional
                            // hold-to-repeat clock, mirroring the arrow-key-up path.
                            if gp_nav.directional_released {
                                session.ui_focus.release_repeat();
                            }
                            // Any gamepad nav intent is a `focus`-mode signal.
                            nav_seen = !gp_nav.nav_intents.is_empty();
                            // `nav.menu` (gamepad Start) toggles the pause menu via
                            // the punch-through flag (Passthrough queues nothing);
                            // other nav intents enqueue only while capturing.
                            let capture =
                                session.ui_dispatch.mode() == input::UiCaptureMode::Capture;
                            for intent in gp_nav.nav_intents {
                                if intent == input::NavIntent::Menu {
                                    menu_toggle = true;
                                    continue;
                                }
                                if capture {
                                    session
                                        .ui_dispatch
                                        .enqueue_intent(input::UiIntentPayload::Nav(intent));
                                }
                            }
                        }
                    }
                    (nav_seen, menu_toggle)
                };
                if gamepad_nav_seen {
                    self.record_mode_signal(scripting_systems::input_mode::ModeSignal::NavInput);
                }
                if gamepad_menu_toggle {
                    self.pending_menu_toggle = true;
                }

                // Resolve this frame's input-mode signal into the engine-owned
                // `input.mode` slot (app composition — the input subsystem's
                // contract output stays the action snapshot). Mouse motion votes
                // `pointer`, nav input votes `focus`, debounced so jitter doesn't
                // flap. Drives `ui_input_mode` (the focus engine's hover gate). The
                // mode is observation-only here; its cursor/ring EFFECT is gated on
                // a capturing tree being on the stack (applied in `reconcile_ui_focus`).
                // See: context/lib/input.md §7.
                let mode_signal = self.pending_mode_signal.take();
                if let Some(session) = self.session.as_mut() {
                    let resolved_input_mode = session
                        .scripting
                        .input_mode_tracker
                        .update(mode_signal, frame_dt);
                    session.ui_input_mode = resolved_input_mode;
                }

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
                // modal stack consumes the drained intents; the drain marks the
                // seam where game logic reads them. See: context/lib/input.md
                let (ui_intents, ui_captured_gameplay_at_frame_start) = {
                    let session = self.session.as_mut().expect("running session installed");
                    let ui_intents = session.ui_dispatch.take_ready();
                    session.ui_dispatch.advance_frame();
                    let captured = session.ui_dispatch.mode() == input::UiCaptureMode::Capture;
                    (ui_intents, captured)
                };

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
                let cursor = self.cursor_pos;
                let focus_result = {
                    let session = self.session.as_mut().expect("running session installed");
                    let active_key = session
                        .modal_stack
                        .active_name()
                        .map(str::to_string)
                        .unwrap_or_else(|| render::ui::tree_asset::HUD_NAME.to_string());
                    session.ui_focus.tick(
                        Some(active_key.as_str()),
                        session.ui_focus_rects.as_ref(),
                        &nav_intents,
                        cursor,
                        &click_positions,
                        session.ui_input_mode,
                        frame_dt,
                    )
                };
                self.ui_focused_id = focus_result.focused.clone();

                // Button activation: a `confirm` (gamepad
                // confirm or pointer click — the focus engine reports both as
                // `confirmed`) on a focused button resolves its `onPress` as either
                // a reserved UI action or an ordinary named reaction, so a click and
                // a gamepad confirm have an identical observable effect.
                if focus_result.confirmed {
                    self.fire_focused_button_activation(focus_result.focused.as_deref());
                }

                if self.pending_exit_to_desktop {
                    self.pending_exit_to_desktop = false;
                    self.release_cursor_for_exit();
                    log::info!("[Engine] Shutting down");
                    event_loop.exit();
                    return;
                }

                // Pause-menu toggle: `nav.menu` (gamepad Start /
                // Escape-from-gameplay) opens the registered `pauseMenu` only from
                // an empty modal stack, closes it when it is active, and is ignored
                // while another modal is active. A `nav.cancel` (Escape / B inside
                // the menu) also closes only the active pause menu. The capture-mode
                // + cursor effect follows on this frame's `reconcile_ui_focus`
                // below. The toggle flag is a punch-through from gameplay;
                // `cancelled` rides the captured-intent queue.
                if self.pending_menu_toggle {
                    self.pending_menu_toggle = false;
                    self.toggle_pause_menu();
                } else if focus_result.cancelled && !text_entry_consumed_nav {
                    if let Some(session) = self.session.as_mut() {
                        if session.modal_stack.active_name()
                            == Some(render::ui::demo::PAUSE_MENU_NAME)
                        {
                            session.modal_stack.pop();
                        }
                    }
                }

                let ui_captures_gameplay = {
                    let session = self.session.as_ref().expect("running session installed");
                    gameplay_capture_gate_for_frame(
                        ui_captured_gameplay_at_frame_start,
                        &session.modal_stack,
                    )
                };

                // drain_look_inputs() must precede snapshot(); both touch
                // mouse_axes and look state belongs to the render-rate path.
                // Capturing UI still drains raw input to prevent stale deltas from
                // replaying later, but the consumed look is neutral so player aim
                // cannot move while a modal owns input.
                let gameplay_snapshot = {
                    let session = self.session.as_mut().expect("running session installed");
                    let drained_look = session.input_system.drain_look_inputs();
                    let look = if ui_captures_gameplay {
                        input::LookInputs::default()
                    } else {
                        drained_look
                    };
                    let frame_snapshot = session.input_system.snapshot();
                    let gameplay_snapshot = gameplay_snapshot_for_capture_state(
                        &mut session.gameplay_input_latch,
                        &frame_snapshot,
                        ticks,
                        ui_captures_gameplay,
                    );
                    // Apply look rotation once at render rate, not once per tick —
                    // so zero-tick frames still consume accumulated mouse motion.
                    self.camera
                        .rotate(look.yaw_delta(frame_dt), look.pitch_delta(frame_dt));
                    gameplay_snapshot
                };

                // The script tranche lives on `Session` (built post-first-pixel).
                // Clone the `ScriptCtx` handle once for this Game-logic phase (cheap
                // `Rc` bump) so the many `script_ctx.*` reads below borrow nothing of
                // `self`; the non-`Clone` session subsystems are reached through
                // disjoint scoped `self.session.as_mut()` borrows at each site.
                let script_ctx = self
                    .session
                    .as_ref()
                    .expect("running session installed")
                    .scripting
                    .script_ctx
                    .clone();

                // Bump the engine frame counter once per Game logic phase.
                // Reserved for primitives that need a per-frame ordering stamp.
                // See: context/lib/scripting.md
                script_ctx.frame.set(script_ctx.frame.get().wrapping_add(1));

                // Net poll (M15 Phase 1): non-blocking, once per frame, BEFORE
                // the catch-up tick loop. The client applies received
                // host-authoritative snapshots into the registry here so the
                // render below reflects this frame's replicated state. The host's
                // serialize + send runs AFTER the tick loop (post-loop, beside
                // the other drains). Single-player → inert no-op. See
                // `context/lib/entity_model.md` §6, development_guide §4.3.
                self.net_poll_and_apply(frame_dt);

                // Accumulate post-tick events across all ticks; drain after the
                // tick loop completes so reactions see fully-settled world state
                // and event order is never interleaved with ongoing physics. See:
                // context/lib/entity_model.md §5
                let mut pending_movement_events: Vec<&'static str> = Vec::new();
                let mut pending_ai_events: Vec<&'static str> = Vec::new();
                let mut pending_weapon_events: Vec<&'static str> = Vec::new();
                // Death-event names accumulate here and drain through the
                // sequence-aware dispatcher (a separate sibling loop below), so a
                // `progress` reaction naming a sequence resolves — unlike the
                // plain `fire_named_event` drains, which would no-op it.
                let mut pending_death_events: Vec<String> = Vec::new();

                if let Some(snapshot) = gameplay_snapshot.as_ref() {
                    // `player_options` is session-owned; copy the crouch mode out
                    // before the `&mut self.crouch_toggle_active` borrow.
                    let crouch_mode = self
                        .session
                        .as_ref()
                        .map(|session| session.player_options.crouch_mode)
                        .unwrap_or_default();
                    let crouch_intent = resolve_crouch_intent(
                        crouch_mode,
                        snapshot.button(Action::Crouch),
                        &mut self.crouch_toggle_active,
                    );

                    for tick_index in 0..ticks {
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
                            let registry = script_ctx.registry.borrow();
                            has_player_pawn(&registry)
                        };

                        // A connected client owns ZERO PlayerMovement pawns until the
                        // host's `local_player` baseline arms one (M15 Phase 3). During
                        // that pre-arm window it must NOT fly-cam: it holds the map's
                        // first-spawn pose (seeded at install) so the view is steady
                        // until its net pawn arrives. Without this guard the pawnless
                        // branch below would drift the camera with movement input.
                        let pre_arm_client = self.is_connected_client();

                        if !has_player_pawn && !pre_arm_client {
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

                        let dash_pressed = tick_index == 0
                            && matches!(snapshot.button(Action::Dash), ButtonState::Pressed);
                        let shoot_pressed = tick_index == 0
                            && matches!(snapshot.button(Action::Shoot), ButtonState::Pressed);
                        let command = build_sim_command(
                            snapshot,
                            &self.camera,
                            crouch_intent,
                            dash_pressed,
                            shoot_pressed,
                        );

                        // Connected-client prediction (M15 Phase 3 Task 3): send one
                        // Input command and advance ONLY the local pawn's movement
                        // through the movement-only replay helper — never the full
                        // `simulate_tick` (AI / weapons / death stay host-authoritative
                        // and arrive via snapshots). The camera follows the predicted
                        // pawn; frame timing pushes the predicted camera pose. Task 5
                        // adds reconciliation/smoothing on top of this seam.
                        if self.is_connected_client() {
                            self.client_predict_movement_tick(&command, tick_dt);
                            // Tick-rate camera follow tracks the PRESENTED local pose:
                            // the gameplay-authoritative (snapped) registry pose plus the
                            // decaying presentation offset. Folding the offset in HERE —
                            // before `frame_timing.push_state` — is the fix for the
                            // velocity-proportional first-person shake (M15 Phase 3
                            // playtest bug). Reconcile snaps the registry backward by the
                            // correction each snapshot and seeds the offset forward by the
                            // same amount, so `registry + offset` is continuous across the
                            // snap. If `frame_timing` instead carried the bare (snapped)
                            // registry pose and the offset were re-added only at render,
                            // `frame_timing` would interpolate ACROSS the snap (a backward
                            // arc) while a constant offset over-corrected at alpha 0 — the
                            // exact ∝-velocity oscillation. With the presented pose pushed,
                            // both `frame_timing` endpoints sit in presented space and the
                            // render-rate interpolation between consecutive presented poses
                            // IS the smoother; the offset decays once per tick here.
                            let presentation_offset = netcode::client_local_presentation_offset(
                                self.session
                                    .as_ref()
                                    .and_then(|session| session.net_endpoint.as_ref()),
                            );
                            if has_player_pawn {
                                let registry_ref = script_ctx.registry.borrow();
                                follow_camera_to_local_pawn(
                                    &mut self.camera,
                                    &registry_ref,
                                    presentation_offset,
                                );
                            }
                            // Decay the offset one step now that this tick's camera pose
                            // has baked in the current value. Tick-rate decay (paired with
                            // the presented-pose push) keeps `frame_timing` continuous;
                            // the render stage reads the interpolated presented eye
                            // directly and must NOT re-add the offset (it is already in
                            // the pose), so there is no double-count.
                            netcode::client_decay_local_correction(
                                self.session
                                    .as_mut()
                                    .and_then(|session| session.net_endpoint.as_mut()),
                            );
                            self.frame_timing
                                .push_state(InterpolableState::new(self.camera.position));
                            continue;
                        }

                        // Host: advance remote (owned) pawns through the authoritative
                        // multi-pawn movement seam first (Task 4), then the shared
                        // `simulate_tick` runs the host's own pawn movement + AI /
                        // weapon / death. Remote movement never uses local_movement_pawn.
                        let remote_movement_events = self.host_drive_remote_movement(tick_dt);
                        pending_movement_events.extend(remote_movement_events);

                        // Borrow the two session-owned `simulate_tick` inputs
                        // (hit-zone store, progress tracker) and the boot-owned
                        // `camera` as disjoint field borrows; the post-movement
                        // closure captures these locals (not `self`) so it does not
                        // re-borrow `self.session`.
                        let session = self.session.as_mut().expect("running session installed");
                        let hit_zone_store = &session.hit_zone_store;
                        let progress_tracker = &mut session.progress_tracker;
                        let camera = &mut self.camera;
                        #[cfg(feature = "dev-tools")]
                        let debug_chase_agent = self.debug_chase_agent;
                        let tick_events = sim::simulate_tick(
                            script_ctx.registry.clone(),
                            &self.collision_world,
                            hit_zone_store,
                            self.nav_graph.as_ref(),
                            script_ctx.gravity.get(),
                            self.active_wieldable,
                            self.anim_time,
                            progress_tracker,
                            &mut self.ai_warned,
                            &command,
                            |registry| {
                                // Camera follows the selected local pawn before
                                // weapon fire resolves its aim ray.
                                if has_player_pawn {
                                    let registry_ref = registry.borrow();
                                    // Host / single-player: no client-side correction
                                    // offset (the host pawn is authoritative).
                                    follow_camera_to_local_pawn(camera, &registry_ref, Vec3::ZERO);
                                }

                                #[cfg(feature = "dev-tools")]
                                {
                                    let mut registry_ref = registry.borrow_mut();
                                    update_debug_chase_agent_destination(
                                        &mut registry_ref,
                                        debug_chase_agent,
                                        camera.position,
                                    );
                                }

                                build_post_movement_command(camera)
                            },
                            tick_dt,
                        );
                        pending_movement_events.extend(tick_events.movement);
                        pending_ai_events.extend(tick_events.ai);
                        pending_weapon_events.extend(tick_events.weapon);
                        pending_death_events.extend(tick_events.death);

                        self.frame_timing
                            .push_state(InterpolableState::new(self.camera.position));
                    }
                }

                // Host serialize + send (M15 Phase 1): after the catch-up tick
                // loop, beside the post-loop drains, so the snapshot carries this
                // frame's fully-settled host-authoritative state. No-op for the
                // client and single-player. See `context/lib/entity_model.md` §6.
                self.net_serialize_and_send();

                // Task 6 client remote interpolation: sample each remote entity's
                // buffer at `estimated_server_tick - interpolation_delay` and write the
                // interpolated pose through the registry's remote-presentation helper.
                // Runs AFTER the tick loop (so the stage-0 `snapshot_transforms` does
                // not clobber the previous/current pair this writes) and BEFORE the
                // render stage reads entities, so the renderer stays read-only.
                // No-op for single-player and the host.
                self.net_sample_remote_interpolation(frame_dt);

                // Drain collected post-tick events after all ticks complete so
                // reactions observe the final state of every entity.
                for event_name in &pending_movement_events {
                    let _ = fire_named_event(event_name, &script_ctx.data_registry.borrow());
                }
                for event_name in &pending_ai_events {
                    let _ = fire_named_event(event_name, &script_ctx.data_registry.borrow());
                }
                for event_name in &pending_weapon_events {
                    let _ = fire_named_event(event_name, &script_ctx.data_registry.borrow());
                }
                // Death events drain through the sequence-aware dispatcher in
                // their OWN loop: a `progress` reaction that names a sequence
                // would no-op under plain `fire_named_event`. Chained-event names
                // are discarded (`let _ =`), matching the drains above.
                if let Some(session) = self.session.as_ref() {
                    for event_name in &pending_death_events {
                        let _ = fire_named_event_with_sequences(
                            event_name,
                            &script_ctx.data_registry.borrow(),
                            &session.scripting.sequence_registry,
                            &session.scripting.reaction_registry,
                            &session.scripting.system_registry,
                            &script_ctx,
                        );
                    }
                }

                // System-reaction command drain — runs AFTER every post-tick
                // event drain so commands enqueued by movement/AI/weapon/death
                // reactions (and, later, crossing watchers) are taken in one
                // batch. The typed queue keeps audio/input/UI services out of
                // the scripting surface; the dispatcher routes each command to
                // its subsystem consumer. See: scripting.md §10.4.
                // NOTE: a SECOND drain runs later this frame, after the state
                // crossings fire (see the crossing-detection block below), so
                // crossing-enqueued commands land this frame, not the next.
                if !script_ctx.system_commands.is_empty() {
                    self.dispatch_system_commands();
                }

                // Player HUD state: republish the engine-owned health slots
                // after game logic settles and before crossing detection / UI
                // snapshot construction, so same-frame consumers see the
                // settled pawn HP. See: context/lib/scripting.md §5.
                //
                // M15 Phase 3.5 Task 4: skip on a connected client. `player.health`
                // / `player.maxHealth` are now owner-private replicated slots; the
                // server writes them through the state-slot apply path, so a client
                // must not overwrite the replicated values from its own (non-
                // authoritative) pawn. Host and single-player keep publishing.
                let is_connected_client = self.is_connected_client();
                if let Some(session) = self.session.as_mut() {
                    session
                        .scripting
                        .player_hud_state
                        .tick_for_role(is_connected_client);
                }
                // Flash-decay state writes the engine-owned `screen.flash`
                // surface at the same game-logic stage as the HUD publisher, so
                // the UI snapshot below freezes this frame's flash color. Runs
                // after the first command drain so a flash started this frame
                // publishes immediately; the crossing drain below may start
                // another, decayed starting next frame.
                if let Some(session) = self.session.as_mut() {
                    session.scripting.flash_decay.tick(frame_dt);
                    // Vignette- and shake-decay drivers (SE) write the engine-owned
                    // `screen.vignette` and `screen.shake` surfaces at the same
                    // game-logic stage as `flash_decay.tick`, so the UI snapshot
                    // below freezes this frame's vignette color and shake offset.
                    // Delta-driven from `frame_dt` (not wall-clock) like the flash
                    // decay.
                    session.scripting.vignette_decay.tick(frame_dt);
                    session.scripting.shake_decay.tick(frame_dt);
                }

                // State-crossing detection (M13 HUD dynamics). Runs AFTER the
                // frame's slot writes (game logic + HUD publisher) settle, so
                // it compares the authoritative slot value — distinct from the
                // eased display value styleRanges read mid-tween. Each watched
                // slot's threshold crossing fires its reaction list synchronously
                // through Task 2's shared named-reaction path; any system
                // reactions thereby enqueued are drained immediately below so
                // crossing-fired commands land in this frame, not the next.
                if let Some(session) = self.session.as_mut() {
                    let crossing_events = session
                        .crossing_detector
                        .detect(&script_ctx.slot_table.borrow());
                    for event_name in &crossing_events {
                        let _ = fire_named_event_with_sequences(
                            event_name,
                            &script_ctx.data_registry.borrow(),
                            &session.scripting.sequence_registry,
                            &session.scripting.reaction_registry,
                            &session.scripting.system_registry,
                            &script_ctx,
                        );
                    }
                }
                if !script_ctx.system_commands.is_empty() {
                    self.dispatch_system_commands();
                }

                // Reconcile the input seam + focus with the modal stack's top
                // capture mode, now that every command drain this frame has
                // settled the stack. A capturing top tree gates player controls,
                // freezes lower UI layers, and releases the cursor (`InputFocus::Menu`);
                // an empty/passthrough top hands input back to gameplay.
                self.reconcile_ui_focus();
                self.apply_frontend_menu_camera_pose_if_top();

                // Audio step — third in frame order (Input → Game logic →
                // Audio → Render → Present, development_guide.md §4.3). Runs after
                // game logic settles every entity and before render. Convert the
                // glam-typed camera to the primitive `ListenerState` here at the
                // call site (the boundary carries no glam); `forward` uses the
                // aim ray's direction so it includes pitch, unlike yaw-only
                // `forward()`, and `up` is world up per the `ListenerState`
                // contract. Guarded for the silent (init-failed) case.
                // Audio is session-owned; build the primitive listener from the
                // disjoint `self.camera` field first, then borrow the subsystem.
                let listener = audio::ListenerState {
                    position: self.camera.position.to_array(),
                    forward: self.camera.aim_ray().1.to_array(),
                    up: [0.0, 1.0, 0.0],
                };
                if let Some(audio) = self
                    .session
                    .as_mut()
                    .and_then(|session| session.audio.as_mut())
                {
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

                // M15 Phase 3 Task 5: the connected client's local-pawn presentation
                // offset is already baked into the camera pose `frame_timing` carries
                // (folded in at the tick-rate camera-follow seam above, where the offset
                // also decays once per tick). So the interpolated eye IS the presented
                // eye — re-adding the offset here would double-count it and re-introduce
                // the ∝-velocity oscillation it was moved to fix. `frame_timing`
                // interpolates between consecutive PRESENTED poses, so the smoothed
                // correction reaches the view matrix, camera uniforms, cell locator,
                // and portal apex continuously across each reconcile snap.
                // Single-player and the host carry a ZERO offset, so this is the bare
                // interpolated eye for them, unchanged.
                let presented_eye = interp.position;

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
                // Match the camera-follow resolver above: marked local pawn
                // first, then the legacy first PlayerMovement+Transform
                // fallback. View feel only runs when that driving pawn carries
                // `view_feel`; another pawn's preset must not leak onto the
                // selected camera.
                let view_feel_inputs = {
                    let registry = script_ctx.registry.borrow();
                    followed_player_pawn(&registry).and_then(|id| {
                        registry
                            .get_component::<postretro_foundation::PlayerMovementComponent>(id)
                            .ok()
                            .and_then(|component| {
                                component.view_feel.as_ref().map(|params| {
                                    (params.clone(), component.velocity, component.is_grounded)
                                })
                            })
                    })
                };
                // `player_options` is session-owned; copy the accessibility scale
                // out before the `&mut self.view_feel_state` borrow below.
                let view_feel_scale = self
                    .session
                    .as_ref()
                    .map(|session| session.player_options.view_feel_scale)
                    .unwrap_or(1.0);
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
                            view_feel_scale,
                        );
                        view_feel::map_output_to_camera(&output, camera_right)
                    } else {
                        // Pass-through: no driving pawn, or it carries no
                        // `view_feel`. Identical-to-today render path.
                        (0.0, 0.0, 0.0, Vec3::ZERO)
                    };

                let render_camera = camera::RenderCamera::new(
                    presented_eye,
                    self.camera.aspect(),
                    self.camera.yaw + vf_yaw_offset,
                    self.camera.pitch + vf_pitch_offset,
                    vf_roll,
                    vf_eye_offset,
                );
                let view_proj = render_camera.view_projection;
                // The render eye and matrix are assembled together.
                // Portal traversal, camera uniforms, and every render-stage
                // distance/cell query must use the same point. Using the
                // unbobbed interpolated position here can put the visibility
                // apex in a different cell or on the opposite side of a
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
                                camera_cell: 0,
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

                #[cfg(feature = "dev-tools")]
                if let Some(renderer) = self.renderer.as_mut() {
                    let locator = match self.level.as_ref() {
                        Some(world) => render::LocatorDiagnostics::Trace(
                            world.trace_locate_cell(render_eye_position),
                        ),
                        None => render::LocatorDiagnostics::NoLevel,
                    };
                    renderer.set_spatial_diagnostics(render::SpatialDiagnostics {
                        current_cell: self.level.as_ref().map(|_| stats.camera_cell),
                        portal_drawable_cells:
                            render::SpatialCellSetDiagnostics::from_visible_cells(&visible_cells),
                        fog_reachable_cells: render::SpatialCellSetDiagnostics::from_cell_slice(
                            &fog_reachable,
                        ),
                        locator,
                    });
                    renderer.refresh_camera_cull_diagnostics(
                        CameraCullVisibility {
                            cells: &visible_cells,
                            path: stats.path,
                        },
                        view_proj,
                    );
                }

                // Build the per-cell bool mask for `update_dynamic_light_slots`
                // from the wider fog/light-reachable set so dynamic lights in
                // empty (face_count == 0) portal-reachable cells stay
                // eligible. Empty slice = DrawAll sentinel: keep every
                // cell-assigned light eligible on fallback paths.
                let light_reachable_cell_mask: Vec<bool> = match self.level.as_ref() {
                    None => Vec::new(),
                    Some(_) if fog_reachable.is_empty() => Vec::new(),
                    Some(world) => {
                        let mut mask = vec![false; world.cell_count()];
                        for &id in &fog_reachable {
                            let i = id as usize;
                            if i < mask.len() {
                                mask[i] = true;
                            }
                        }
                        mask
                    }
                };

                // AABBs of the fog/light-reachable cells — the WIDER
                // portal-reachable set (same source as `light_reachable_cell_mask`,
                // built from `fog_reachable`), which deliberately includes empty
                // `face_count == 0` cells. Feeds the dynamic-light shadow-slot
                // eligibility test: a light is shadow-eligible when its influence
                // sphere reaches one of these reachable cells — NOT when its own
                // cell is in the camera PVS (see
                // `lighting::light_reaches_visible_cell`). Intentionally the wider
                // set, not the narrower drawable `visible_cells`, so a light in an
                // empty reachable cell still counts. Empty = DrawAll sentinel
                // (fallback visibility paths): every light eligible.
                let reachable_cell_aabbs: Vec<(glam::Vec3, glam::Vec3)> = match self.level.as_ref()
                {
                    None => Vec::new(),
                    Some(_) if fog_reachable.is_empty() => Vec::new(),
                    Some(world) => fog_reachable
                        .iter()
                        .filter_map(|&id| world.cells.get(id as usize))
                        .map(|cell| (cell.bounds_min, cell.bounds_max))
                        .collect(),
                };

                if let Some(renderer) = self.renderer.as_mut() {
                    // The render-stage bridges + collectors live on `Session`;
                    // borrow it once here (disjoint from the `renderer` borrow of
                    // `self.renderer` and from the other `self` fields read below).
                    let session = self.session.as_mut().expect("running session installed");
                    // Emitter bridge — after script `tick` handler, before particle
                    // sim. Spawns new particles; the sim advances them the same
                    // frame so they don't appear stuck at origin.
                    {
                        let mut registry = script_ctx.registry.borrow_mut();
                        // Cap headroom comes from the previous frame's sim tally
                        // (see particle_sim::tick) — the bridge no longer walks the
                        // ParticleState column itself.
                        session.emitter_bridge.update(
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
                        let mut registry = script_ctx.registry.borrow_mut();
                        scripting_systems::particle_sim::tick(
                            &mut registry,
                            frame_dt,
                            script_ctx.gravity.get(),
                            &mut self.particle_live_counts,
                        );
                    }

                    // Light bridge — between Game Logic and Render. Uploads
                    // mutated `LightComponent` data before `render_frame_indirect`
                    // allocates slots, so scripted lights reflect their new state.
                    {
                        let mut registry = script_ctx.registry.borrow_mut();
                        if let Some(update) = session
                            .light_bridge
                            .update(&mut registry, self.script_time as f32)
                        {
                            if update.has_dirty_data {
                                renderer.upload_bridge_lights(&update.lights_bytes);
                                renderer.upload_bridge_descriptors(&update.descriptor_bytes);
                                renderer.upload_bridge_samples(&update.samples_bytes);
                                // Fan out `_animated` descriptor updates to
                                // the animated-compose buffer.
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
                        let mut registry = script_ctx.registry.borrow_mut();
                        session
                            .fog_volume_bridge
                            .tick(&mut registry, self.script_time);
                    }
                    let all_lights = {
                        let registry = script_ctx.registry.borrow();
                        if let Some((bytes, planes, live_mask)) =
                            session.fog_volume_bridge.update_volumes(&registry)
                        {
                            renderer.upload_fog_volumes(bytes, planes, live_mask);
                        } else {
                            renderer.upload_fog_volumes(&[], &[], 0);
                        }
                        renderer.set_fog_aabbs(session.fog_volume_bridge.active_aabbs());
                        session
                            .light_bridge
                            .collect_all_as_map_lights(&registry, self.script_time as f32)
                    };
                    let point_bytes = session.fog_volume_bridge.update_points(&all_lights);
                    renderer.upload_fog_points(point_bytes);

                    renderer.update_per_frame_uniforms(
                        view_proj,
                        render_eye_position,
                        self.script_time as f32,
                    );

                    // This gameplay block runs only in Running (the redraw
                    // path reaches here solely when `boot_state == Running`,
                    // set after full renderer init), so the renderer is always
                    // full-ready; the mesh-collect + draw submission below runs
                    // unconditionally, like the `full_mut`-backed uploads above.
                    // Particle render — packs `SpriteInstance` bytes per
                    // collection; the collector never touches wgpu directly.
                    {
                        let registry = script_ctx.registry.borrow();
                        // Cull non-visible emitters at render-collect, mirroring
                        // the mesh path below: thread the level world + this
                        // frame's visible-cell set so off-screen / adjacent-room
                        // smoke is never packed for drawing. `visible_cells` is
                        // still live here (reclaimed after the frame).
                        session.particle_render.collect(
                            &registry,
                            self.level.as_ref(),
                            &visible_cells,
                        );
                    }
                    let particle_collections: Vec<(&str, &[u8])> =
                        session.particle_render.iter_collections().collect();

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
                            let mut registry = script_ctx.registry.borrow_mut();
                            postretro_entities::components::mesh::resolve_pending_animation_stamps(
                                &mut registry,
                                self.anim_time,
                            );
                        }
                        let registry = script_ctx.registry.borrow();
                        // Same frame alpha the player camera reads from
                        // `frame_timing` — interpolate each mesh between its
                        // previous- and current-tick transforms.
                        session.mesh_render.collect(
                            &registry,
                            world,
                            &visible_cells,
                            frame_result.alpha,
                            self.anim_time,
                            &session.mesh_clip_tables,
                            // Camera eye position — the same value that seeds
                            // the portal flood-fill — drives the per-instance
                            // animation time-slicing distance bucket.
                            interp.position,
                        );
                        renderer.set_mesh_draws(session.mesh_render.instances());
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
                        // `debug_ui` is session-owned; reach it through the
                        // already-held `session` borrow (the window is a disjoint
                        // `self` field).
                        if let (Some(debug_ui), Some(ws)) =
                            (session.debug_ui.as_mut(), self.window_state.as_ref())
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
                                debug_ui
                                    .winit_state
                                    .handle_platform_output(window, full_output.platform_output);
                                let paint_jobs = debug_ui
                                    .ctx
                                    .tessellate(full_output.shapes, full_output.pixels_per_point);
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
                            if let Some(debug_ui) = session.debug_ui.as_ref() {
                                renderer.emit_sh_diagnostics(
                                    &debug_ui.sh_diagnostics_state,
                                    render_eye_position,
                                    world,
                                    &light_reachable_cell_mask,
                                );
                            }
                            let bvh_visible_cell_mask =
                                drawable_visible_cell_mask(world.cell_count(), &visible_cells);
                            renderer.emit_bvh_overlay_diagnostics(bvh_visible_cell_mask.as_deref());
                            renderer.emit_cell_overlay_diagnostics(world, &visible_cells);
                            renderer.emit_portal_overlay_diagnostics(world);
                        }
                        // Navmesh overlay: append region rectangles + portal
                        // edges. No-op unless the `Alt+Shift+N` toggle is on
                        // and the map carried a baked navmesh.
                        if let Some(nav_graph) = self.nav_graph.as_ref() {
                            renderer.emit_nav_diagnostics(nav_graph);
                        }
                        // Chase-agent path overlay: corridor + funnel
                        // waypoints for the `Alt+Shift+G` demo agent. Reads
                        // the live agent component and hands plain geometry to
                        // the renderer (no wgpu outside the renderer module).
                        // Same toggle as the navmesh overlay.
                        if let Some(agent) = self.debug_chase_agent {
                            use postretro_entities::Transform;
                            use postretro_entities::components::agent::AgentComponent;
                            let registry = script_ctx.registry.borrow();
                            if let Ok(component) = registry.get_component::<AgentComponent>(agent) {
                                let position = registry
                                    .get_component::<Transform>(agent)
                                    .map(|t| t.position)
                                    .unwrap_or(Vec3::ZERO);
                                renderer.emit_agent_path_overlay(
                                    position,
                                    &component.path,
                                    component.waypoint_cursor,
                                    component.radius,
                                );
                            }
                        }
                        // Remote-entity wireframe (M15 Phase 1): on the client
                        // path only, draw a capsule at each replicated remote
                        // entity so the host's moving pawn is visible rather
                        // than an invisible bare-Transform ghost. Thin
                        // delegation — `netcode` collects the centers
                        // (registry read, no wgpu), the renderer owns the draw.
                        // No-op for single-player and the host.
                        if let Some(endpoint) = session.net_endpoint.as_ref() {
                            let registry = script_ctx.registry.borrow();
                            let centers = netcode::remote_entity_positions(endpoint, &registry);
                            renderer.emit_remote_entity_markers(
                                &centers,
                                netcode::REMOTE_CAPSULE_RADIUS,
                                netcode::REMOTE_CAPSULE_HALF_HEIGHT,
                            );
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
                    // Modal stack compose stays behind one helper so normal
                    // gameplay gets always-on HUD/base layers, while a top
                    // frontend menu suppresses those layers and presents only
                    // the menu over its optional backdrop.
                    let frontend_menu_name = session
                        .frontend
                        .as_ref()
                        .map(|frontend| frontend.menu_tree.as_str())
                        .unwrap_or(render::ui::demo::FRONTEND_MENU_NAME);
                    // Reuse the `session` borrow taken at the top of this render
                    // block (the `particle_collections` borrow keeps it alive); a
                    // second `self.session.as_mut()` here would alias it.
                    let frontend_menu_is_top =
                        session.modal_stack.active_name() == Some(frontend_menu_name);
                    let ui_snapshot = Self::build_ui_read_snapshot(
                        &session.modal_stack,
                        &mut session.presentation_cells,
                        &script_ctx.slot_table.borrow(),
                        self.script_time,
                        session.ui_input_mode,
                        self.ui_focused_id.clone(),
                        frontend_menu_is_top,
                    );
                    renderer.set_ui_snapshot(ui_snapshot);

                    let surface_texture = match renderer.render_frame_indirect(
                        CameraCullVisibility {
                            cells: &visible_cells,
                            path: stats.path,
                        },
                        &light_reachable_cell_mask,
                        &reachable_cell_aabbs,
                        &fog_reachable,
                        Some(stats.camera_cell),
                        view_proj,
                        &particle_collections,
                        self.script_time,
                        render::ClearColor {
                            r: 0.05,
                            g: 0.05,
                            b: 0.08,
                            a: 1.0,
                        },
                        true,
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
                    let exported_rects = renderer.export_ui_focus_rects();
                    if let Some(session) = self.session.as_mut() {
                        session.ui_focus_rects = Some(exported_rects);
                    }
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

                self.poll_staged_manifest_results();

                if let VisibleCells::Culled(mut cells) = visible_cells {
                    cells.clear();
                    self.scratch_cells = cells;
                }

                let pos = render_eye_position;
                let region_label = "cell";
                let path_label = match stats.path {
                    VisibilityPath::PrlPortal { .. } => "prl-portal",
                    VisibilityPath::NoPortalsFallback => "no-portals",
                    VisibilityPath::EmptyWorldFallback => "empty",
                    VisibilityPath::SolidCellFallback => "solid-cell",
                    VisibilityPath::ExteriorCellFallback => "exterior",
                };
                let walk_reach_col = match stats.walk_reach() {
                    Some(walk) => format!(" walk:{walk}"),
                    None => String::new(),
                };
                log::debug!(
                    "[Diagnostics] {region_label}:{} path:{path_label} | draw:{} all:{}{walk_reach_col} | pos: ({:.0}, {:.0}, {:.0})",
                    stats.camera_cell,
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
                            stats.camera_cell,
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
        // Boot phase ignores device input until the session is installed.
        let Some(session) = self.session.as_mut() else {
            return;
        };
        // UI-dispatch seam, ahead of the gameplay forward: a captured raw
        // delta is consumed by the UI layer and must not reach the look path.
        // Mirrors the `window_event` seam; the decision is the mode flag. A raw
        // delta carries no queueable intent (hover/look is not nav), so the
        // capture suppresses the forward but queues nothing.
        if !session
            .ui_dispatch
            .dispatch_event(None)
            .forwards_to_gameplay()
        {
            return;
        }
        // Raw mouse deltas only rotate the camera while gameplay owns input.
        // When the debug panel (DevTools) or a menu is open, the cursor is
        // released and raw deltas must not leak into the look path.
        if session.input_focus != InputFocus::Gameplay {
            return;
        }
        if let DeviceEvent::MouseMotion { delta } = event {
            session.input_system.handle_mouse_delta(delta.0, delta.1);
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
        //
        // A connected client must NOT persist replicated slot writes to `state.json`
        // (M15 Phase 3.5 Task 5): its `player.health` / `player.maxHealth` and any
        // shared mod slots are server-authoritative values applied through the
        // replicated-state path, not local edits to save. Bitcode stays live-wire only,
        // and save-game sync for net sessions is a non-goal. Single-player (`None`) and
        // the host (`NetEndpoint::Host`) save unchanged — only `NetEndpoint::Client`
        // skips the clean-exit save.
        let can_save = self
            .session
            .as_ref()
            .is_some_and(|session| session.state_store_lifecycle.can_save());
        if should_save_persisted_state(can_save, self.is_connected_client()) {
            let state_path = Path::new(STATE_FILE_PATH);
            let script_ctx = self
                .session
                .as_ref()
                .expect("session installed at clean exit")
                .scripting
                .script_ctx
                .clone();
            let collected = collect_persisted_state(&script_ctx.slot_table.borrow());
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

        // Release the level's sound registry at teardown too, mirroring the
        // runtime level-unload path. Audio is session-owned.
        if let Some(audio) = self
            .session
            .as_mut()
            .and_then(|session| session.audio.as_mut())
        {
            audio.release_level_sounds();
        }
        self.renderer = None;
        self.window_state = None;
        log::info!("[Engine] Exited");
    }
}

impl App {
    /// Finish deferred session startup on the first visible logo frame. Takes
    /// (and thereby consumes) `pending_session` so the install commits at most
    /// once — a suspend/resume that re-enters the splash loop finds it `None`
    /// and skips re-init. Builds and installs the entire `Session` (options
    /// I/O, audio, scripting core, input/UI/modal group, net endpoint) behind
    /// the logo pixels, via `PendingSessionInit::install` → `Session::build`.
    ///
    /// Returns `true` on success (or when nothing was pending). On a `Session`
    /// build failure it stores the error in `exit_result`, logs it, exits the
    /// event loop, and returns `false`, so the caller early-returns from the
    /// install frame before any later step runs against a `None` session —
    /// mirroring `finish_renderer_full_init`'s failure handling. A failed build
    /// also consumes `pending_session`, so a resumed boot does not retry.
    /// See: context/lib/boot_sequence.md §1, §5; development_guide.md §6.2.
    pub(crate) fn install_pending_session(&mut self, event_loop: &ActiveEventLoop) -> bool {
        // The build-result → action decision is the pure `classify_session_install`
        // classifier; this method only performs the side effects it names, so the
        // boot-abort contract stays testable without a window/GPU/`Session`.
        let build_result = crate::startup::take_once(&mut self.pending_session)
            .map(|pending| pending.install(self));
        let had_pending = build_result.is_some();
        let build_succeeded = !matches!(build_result, Some(Err(_)));
        match crate::startup::classify_session_install(had_pending, build_succeeded) {
            crate::startup::SessionInstallStep::NothingPending
            | crate::startup::SessionInstallStep::Installed => true,
            crate::startup::SessionInstallStep::AbortBoot => {
                // SAFETY of the unwraps: `AbortBoot` is only produced when
                // `had_pending && !build_succeeded`, i.e. `Some(Err(_))`.
                let err = match build_result {
                    Some(Err(err)) => err,
                    _ => unreachable!("AbortBoot implies a failed build result"),
                };
                log::error!("[Engine] session init failed: {err:#}");
                self.exit_result = Err(err);
                event_loop.exit();
                false
            }
        }
    }

    /// Current boot phase for the suspend/resume contract (boot_sequence §1, §5).
    /// Derived purely from the splash schedule, whether the deferred session
    /// bundle is installed (`pending_session` consumed), and renderer full-ready.
    /// Used to log/audit which phase a suspend interrupts; the resume path itself
    /// resets to `Booting` and re-drives the splash loop, where the single-commit
    /// guards keep session init from re-running.
    pub(crate) fn boot_phase(&self) -> crate::startup::BootPhase {
        crate::startup::classify_boot_phase(
            self.splash_frame,
            self.pending_session.is_none(),
            self.renderer.as_ref().is_some_and(Renderer::is_full_ready),
        )
    }

    /// Lazily build the (dev-tools-only) session-owned debug-UI state once the
    /// renderer/window are available, after the session is installed. The
    /// constructor needs the boot-ready device's `max_texture_dimension_2d` limit
    /// and the window — neither is available at `Session::build` time, so this
    /// runs on the first visible logo frame right after `install_pending_session`
    /// and again on resume (which drops the window-derived state).
    ///
    /// The audio subsystem and net endpoint, which used to build alongside this,
    /// now build inside `Session::build` (the sole session construction site), so
    /// the only work left here is the genuinely renderer-dependent debug UI.
    ///
    /// Idempotent across suspend/resume: rebuilt only when absent. `suspended()`
    /// drops `session.debug_ui` (and resets the boot state to `Booting`), so the
    /// re-run of the splash loop on resume reconstructs it here.
    /// See: context/lib/boot_sequence.md §1, §5.
    #[cfg(feature = "dev-tools")]
    pub(crate) fn ensure_debug_ui(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if session.debug_ui.is_some() {
            return;
        }
        if let (Some(renderer), Some(ws)) = (self.renderer.as_ref(), self.window_state.as_ref()) {
            let max_texture = renderer.max_texture_dimension_2d();
            session.debug_ui = Some(render::debug_ui::DebugUi::new(&ws.window, max_texture));
        }
    }

    /// No-op in non-dev-tools builds: debug UI does not exist, audio and net are
    /// built inside `Session::build`.
    #[cfg(not(feature = "dev-tools"))]
    pub(crate) fn ensure_debug_ui(&mut self) {}

    /// Drain the hot-reload watcher's changed-path channel and queue a staged
    /// mod-init build when an active dependency changed. Extracted from the
    /// redraw path so the splash logo frame can gate it behind deferred-session
    /// commit (`pending_session` consumed). See: context/lib/boot_sequence.md §1.
    fn drain_script_reload_requests(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match session.scripting.script_runtime.drain_reload_requests() {
            Ok(summary) => {
                if reload_summary_requires_mod_init(summary) {
                    match session
                        .scripting
                        .script_runtime
                        .enqueue_staged_manifest_build(&self.content_root)
                    {
                        Ok(Some(generation)) => log::info!(
                            "[Scripting] active mod-init dependency changed - queued staged generation {generation}",
                        ),
                        Ok(None) => {}
                        Err(err) => {
                            log::error!("[Scripting] failed to queue staged mod-init: {err}");
                        }
                    }
                }
            }
            Err(err) => {
                log::error!("[Scripting] drain_reload_requests failed: {err}");
            }
        }
    }

    /// Commit staged UI trees and theme only after the matching staged script
    /// manifest has already passed descriptor/store reconciliation.
    fn commit_staged_ui_manifest(
        &mut self,
        result: &StagedManifestBuildResult,
        outcome: &StagedManifestCommitOutcome,
    ) {
        let Some((ui_trees, theme, frontend)) = staged_ui_commit_payload(result, outcome) else {
            return;
        };
        let frontend_was_top = self.frontend_menu_is_top();
        let tree_count = ui_trees.len();
        if let Some(session) = self.session.as_mut() {
            session
                .modal_stack
                .replace_script_tree_tier(ui_trees, render::ui::modal_stack::ScopeTier::Mod);
        }
        self.commit_mod_ui_theme(theme);
        if let Some(session) = self.session.as_mut() {
            session.frontend = frontend;
        }
        if frontend_was_top || self.boot_state == BootState::Frontend {
            self.present_frontend_menu();
        }
        log::info!(
            "[UI] committed staged mod-init generation {} UI snapshot: {} tree(s)",
            result.generation,
            tree_count,
        );
    }

    fn commit_mod_ui_theme(&mut self, theme: ModThemeTokens) {
        self.mod_theme_override = theme;
        self.apply_mod_ui_theme_to_renderer();
    }

    fn apply_mod_ui_theme_to_renderer(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let descriptor = render::ui::theme::ThemeDescriptor {
            colors: self.mod_theme_override.colors.clone(),
            fonts: self.mod_theme_override.fonts.clone(),
            spacing: self.mod_theme_override.spacing.clone(),
        };
        let merged = render::ui::theme::UiTheme::engine_default().with_override(&descriptor);
        renderer.set_ui_theme(merged);
    }

    fn frontend_menu_tree_name(&self) -> &str {
        self.session
            .as_ref()
            .and_then(|session| session.frontend.as_ref())
            .map(|frontend| frontend.menu_tree.as_str())
            .unwrap_or(render::ui::demo::FRONTEND_MENU_NAME)
    }

    fn present_frontend_menu(&mut self) -> bool {
        let menu_tree = self.frontend_menu_tree_name().to_string();
        let presented = self.session.as_mut().and_then(|session| {
            session
                .modal_stack
                .replace_with_frontend_menu(&menu_tree, render::ui::demo::FRONTEND_MENU_NAME)
        });
        self.apply_frontend_menu_camera_pose_if_top();
        self.reconcile_ui_focus();
        presented.is_some()
    }

    fn populate_frontend(&mut self) {
        let presented = self.present_frontend_menu();
        let source = self
            .session
            .as_ref()
            .and_then(|session| frontend_background_level_source(session.frontend.as_ref()));
        if presented && let Some(source) = source {
            self.enqueue_level_request(LevelRequest::Load(source));
        }
    }

    fn return_to_frontend(&mut self) {
        self.present_frontend_menu();
        let requests = self
            .session
            .as_ref()
            .map(|session| frontend_return_requests(session.frontend.as_ref()))
            .unwrap_or_else(|| frontend_return_requests(None));
        for request in requests {
            self.enqueue_level_request(request);
        }
    }

    fn frontend_menu_is_top(&self) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        session.modal_stack.active_name().is_some_and(|active| {
            active == self.frontend_menu_tree_name()
                || active == render::ui::demo::FRONTEND_MENU_NAME
        })
    }

    fn apply_frontend_menu_camera_pose_if_top(&mut self) {
        let Some(frontend) = self
            .session
            .as_ref()
            .and_then(|session| session.frontend.clone())
        else {
            return;
        };
        if !self.frontend_menu_is_top() {
            return;
        }

        apply_menu_camera_pose(&mut self.camera, &mut self.frame_timing, &frontend.camera);
    }

    fn build_ui_read_snapshot(
        modal_stack: &render::ui::modal_stack::ModalStack,
        presentation_cells: &mut scripting_systems::presentation_cells::PresentationCellStore,
        slot_table: &postretro_entities::SlotTable,
        script_time: f64,
        ui_input_mode: input::InputMode,
        ui_focused_id: Option<String>,
        frontend_menu_is_top: bool,
    ) -> render::ui::UiReadSnapshot {
        let slot_values = Self::build_ui_slot_snapshot(slot_table);
        let mut trees: Vec<render::ui::UiTreeEntry> = if frontend_menu_is_top {
            Vec::new()
        } else {
            modal_stack.always_on_layers()
        };
        trees.extend(modal_stack.entries());

        let composed_trees: Vec<&render::ui::descriptor::AnchoredTree> =
            trees.iter().map(|entry| &entry.descriptor).collect();
        presentation_cells.reconcile(&composed_trees);
        let cell_values = presentation_cells.snapshot();

        let ring_id = if modal_stack.top_capture_mode()
            == render::ui::descriptor::CaptureMode::Capture
            && !ui_input_mode.ring_visible()
        {
            None
        } else {
            ui_focused_id
        };

        render::ui::UiReadSnapshot::with_trees(
            trees,
            slot_values,
            cell_values,
            script_time,
            ring_id,
        )
    }

    /// Install a mod manifest's theme tokens and font assets into the live UI
    /// runtime, at the mod-init drain (before the authoring VM context drops). G1b
    /// Task 4. Both halves degrade per `ui.md` §5: a missing/unreadable font file
    /// or a non-registering face produces a named load-time diagnostic and is
    /// skipped; the theme merge tolerates unknown tokens (they degrade visibly at
    /// widget-resolution time — magenta/`primary`/zero, warn-once — never here).
    /// Theme commit is snapshot-style: an empty override resets to engine default.
    fn install_mod_ui_theme_and_fonts(
        &mut self,
        theme: ModThemeTokens,
        fonts: postretro_foundation::ModFontAssets,
    ) {
        self.commit_mod_ui_theme(theme);

        // Fonts: family → TTF path. Resolve each path against the mod content root
        // (itself cwd-relative at runtime per ui.md §5), read the bytes, and
        // register the face. A missing/unreadable file or a non-registering face is
        // logged and skipped — the `font` token then degrades to a system fallback
        // at shape time, but boot never aborts.
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        for (family, rel_path) in fonts.families {
            let path = self.content_root.join(&rel_path);
            let bytes = match render::ui::text::read_font_file(&path) {
                Ok(bytes) => bytes,
                Err(err) => {
                    log::warn!(
                        "[UI] mod font '{family}' file '{}' could not be read ({err}); \
                         skipping — the font token falls back to a system face",
                        path.display(),
                    );
                    continue;
                }
            };
            if renderer.register_ui_font(&family, bytes) {
                log::info!(
                    "[UI] registered mod font '{family}' from '{}'",
                    path.display()
                );
            } else {
                log::warn!(
                    "[UI] mod font '{family}' from '{}' registered no matching face \
                     (malformed file or family-name mismatch); skipping",
                    path.display(),
                );
            }
        }
    }

    /// Paint a single boot-splash frame through the renderer-owned splash pass:
    /// clear to black, then draw the logo quad once one is installed. The boot
    /// splash is independent of the UI system — `paint_splash` publishes no UI
    /// snapshot and does not query the renderer for a capture mode (the input
    /// seam stays passthrough during boot). Returns the present outcome so the
    /// splash schedule advances only on a presented frame; a transient surface
    /// failure (`NeedsRedraw`) requests another redraw without advancing.
    fn paint_splash(
        &mut self,
        _event_loop: &ActiveEventLoop,
    ) -> render::splash_pass::PresentOutcome {
        match self.renderer.as_mut() {
            // Splash requires only boot-ready (surface/device/queue/boot-splash).
            Some(renderer) if renderer.is_boot_ready() => renderer.render_splash_frame(),
            // Surface not yet configured: nothing presented, ask to redraw.
            _ => render::splash_pass::PresentOutcome::NeedsRedraw,
        }
    }

    fn run_frontend_ui_logic(&mut self, event_loop: &ActiveEventLoop, frame_dt: f32) -> bool {
        // Defensive guard: session is present for all normal frontend calls
        // post-install, but a pre-install re-entry edge case could reach here
        // before the session is built. Return a neutral `true` in that case.
        if self.session.is_none() {
            return true;
        }

        // Gamepad poll: disjoint borrows of the session group and the
        // non-session `nav_stick_tracker`. A nav intent votes `focus` mode;
        // recorded after the borrow ends.
        let nav_input_seen = {
            let App {
                session,
                nav_stick_tracker,
                ..
            } = self;
            let session = session.as_mut().expect("frontend session installed");
            let mut nav_input_seen = false;
            if let Some(gp) = session.gamepad_system.as_mut() {
                let gp_nav = gp.update(&mut session.input_system, nav_stick_tracker);
                gp.tick_rumble(frame_dt);
                if gp_nav.confirm_released {
                    session.ui_focus.release_confirm_repeat();
                }
                if gp_nav.directional_released {
                    session.ui_focus.release_repeat();
                }
                nav_input_seen = !gp_nav.nav_intents.is_empty();
                let capture = session.ui_dispatch.mode() == input::UiCaptureMode::Capture;
                for intent in gp_nav.nav_intents {
                    if intent == input::NavIntent::Menu {
                        continue;
                    }
                    if capture {
                        session
                            .ui_dispatch
                            .enqueue_intent(input::UiIntentPayload::Nav(intent));
                    }
                }
            }
            nav_input_seen
        };
        if nav_input_seen {
            self.record_mode_signal(scripting_systems::input_mode::ModeSignal::NavInput);
        }

        let mode_signal = self.pending_mode_signal.take();

        let ui_intents = {
            let session = self.session.as_mut().expect("frontend session installed");
            let ui_input_mode = session
                .scripting
                .input_mode_tracker
                .update(mode_signal, frame_dt);
            session.ui_input_mode = ui_input_mode;
            let ui_intents = session.ui_dispatch.take_ready();
            session.ui_dispatch.advance_frame();
            ui_intents
        };
        let text_entry_consumed_nav = self.resolve_text_entry_intents(&ui_intents);

        let mut nav_intents: Vec<input::NavIntent> = Vec::new();
        let mut click_positions: Vec<input::PointerPos> = Vec::new();
        for intent in &ui_intents {
            match &intent.payload {
                input::UiIntentPayload::Nav(nav) => {
                    if text_entry_consumed_nav
                        && matches!(nav, input::NavIntent::Confirm | input::NavIntent::Cancel)
                    {
                        continue;
                    }
                    nav_intents.push(*nav);
                }
                input::UiIntentPayload::PointerClick { pos } => click_positions.push(*pos),
                input::UiIntentPayload::Text(_) | input::UiIntentPayload::Backspace => {}
            }
        }
        self.apply_slider_nav_capture(&mut nav_intents);

        let frontend_menu_tree_name = self.frontend_menu_tree_name().to_string();
        let cursor_pos = self.cursor_pos;
        let focus_result = {
            let session = self.session.as_mut().expect("frontend session installed");
            let active_key = session
                .modal_stack
                .active_name()
                .map(str::to_string)
                .unwrap_or(frontend_menu_tree_name);
            session.ui_focus.tick(
                Some(active_key.as_str()),
                session.ui_focus_rects.as_ref(),
                &nav_intents,
                cursor_pos,
                &click_positions,
                session.ui_input_mode,
                frame_dt,
            )
        };
        self.ui_focused_id = focus_result.focused.clone();
        if focus_result.confirmed {
            self.fire_focused_button_activation(focus_result.focused.as_deref());
        }
        if focus_result.cancelled && !text_entry_consumed_nav {
            if let Some(session) = self.session.as_mut() {
                session.modal_stack.pop();
            }
        }
        self.pending_menu_toggle = false;

        if self.pending_exit_to_desktop {
            self.pending_exit_to_desktop = false;
            self.release_cursor_for_exit();
            log::info!("[Engine] Shutting down");
            event_loop.exit();
            return false;
        }

        let has_system_commands = self
            .session
            .as_ref()
            .is_some_and(|session| !session.scripting.script_ctx.system_commands.is_empty());
        if has_system_commands {
            self.dispatch_system_commands();
        }
        self.reconcile_ui_focus();
        self.apply_frontend_menu_camera_pose_if_top();
        self.poll_staged_manifest_results();
        true
    }

    fn poll_staged_manifest_results(&mut self) {
        let staged = match self.session.as_mut() {
            Some(session) => session
                .scripting
                .script_runtime
                .poll_staged_manifest_builds(),
            None => return,
        };
        for result in staged {
            // `commit_staged_manifest_result` and the active-set recompose touch
            // the session-owned runtime/ctx/registry; the rebuild + UI commit are
            // App methods. Scope the session borrow to the commit call so the App
            // methods below can re-borrow `self`.
            let outcome = {
                let session = self.session.as_mut().expect("frontend session installed");
                session
                    .scripting
                    .script_runtime
                    .commit_staged_manifest_result(
                        &result,
                        &session.scripting.script_ctx,
                        &session.scripting.sequence_registry,
                    )
            };
            if matches!(outcome, StagedManifestCommitOutcome::Committed { .. })
                && self.has_installed_level()
            {
                if let Some(session) = self.session.as_ref() {
                    session
                        .scripting
                        .script_ctx
                        .data_registry
                        .borrow_mut()
                        .recompose_active_sets(&self.active_level_tags);
                }
                self.rebuild_active_reaction_subscribers();
            }
            self.commit_staged_ui_manifest(&result, &outcome);
        }
    }

    fn render_frontend_frame(&mut self, event_loop: &ActiveEventLoop, frame_start: Instant) {
        self.apply_frontend_menu_camera_pose_if_top();
        self.reconcile_ui_focus();
        let frontend_menu_is_top = self.frontend_menu_is_top();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let ui_snapshot = Self::build_ui_read_snapshot(
            &session.modal_stack,
            &mut session.presentation_cells,
            &session.scripting.script_ctx.slot_table.borrow(),
            self.script_time,
            session.ui_input_mode,
            self.ui_focused_id.clone(),
            frontend_menu_is_top,
        );

        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        // Frontend renders through the full UI/scene path — requires full-ready.
        if !renderer.is_full_ready() {
            return;
        }

        #[cfg(feature = "dev-tools")]
        renderer.clear_debug_lines();

        renderer.set_ui_snapshot(ui_snapshot);
        let surface_texture = match renderer.render_frame_indirect(
            CameraCullVisibility {
                cells: &VisibleCells::DrawAll,
                // Frontend/splash path: no world cull. DrawAll + non-portal
                // provenance keeps the candidate path inert regardless.
                path: VisibilityPath::EmptyWorldFallback,
            },
            &[],
            &[],
            &[],
            None,
            glam::Mat4::IDENTITY,
            &[],
            self.script_time,
            FRONTEND_CLEAR_COLOR,
            false,
        ) {
            Ok(opt) => opt,
            Err(err) => {
                self.exit_result = Err(err);
                event_loop.exit();
                return;
            }
        };
        let exported_rects = renderer.export_ui_focus_rects();
        if let Some(session) = self.session.as_mut() {
            session.ui_focus_rects = Some(exported_rects);
        }
        if let Some(surface_texture) = surface_texture {
            surface_texture.present();
        }

        let frame_cpu = Instant::now().duration_since(frame_start);
        self.frame_rate_meter.record(frame_cpu);
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
    /// it. Borrowing only the script slot table keeps the two field borrows disjoint.
    ///
    /// `pub(crate)` so the netcode state-slot apply tests can drive the REAL UI read
    /// path (the replicated value must surface here), not a hand-mirrored copy.
    pub(crate) fn build_ui_slot_snapshot(
        slot_table: &postretro_entities::SlotTable,
    ) -> std::collections::HashMap<String, postretro_entities::SlotValue> {
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
        let Some(rects) = self
            .session
            .as_ref()
            .and_then(|session| session.ui_focus_rects.as_ref())
        else {
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

        let script_ctx = self
            .session
            .as_ref()
            .expect("frontend session installed")
            .scripting
            .script_ctx
            .clone();
        // The slider's current value: its bound slot reading, or `min` as a floor
        // when the slot is unset or non-numeric (a sane starting point).
        let current = {
            let table = script_ctx.slot_table.borrow();
            match table.get(&slot).and_then(|r| r.value.as_ref()) {
                Some(postretro_entities::SlotValue::Number(n)) => *n,
                _ => min,
            }
        };

        // Peel off captured nav intents (mutating `nav_intents`) and compute the
        // stepped value; emit one `setState` for the new clamped value.
        if let Some(next) = input::capture_slider_step(&interaction, current, nav_intents) {
            script_ctx
                .system_commands
                .push(SystemReactionCommand::SetState {
                    slot,
                    value: serde_json::json!(next),
                });
        }
    }

    /// Fire a focused button's `onPress` on activation. Reserved `ui.*` actions
    /// are handled App-side before ordinary names fall through to the shared
    /// named-reaction path, so gamepad confirm and pointer click produce the same
    /// observable effect.
    fn fire_focused_button_activation(&mut self, focused_id: Option<&str>) {
        let on_press = focused_button_on_press(
            self.session
                .as_ref()
                .and_then(|session| session.ui_focus_rects.as_ref()),
            focused_id,
        );
        if let Some(on_press) = on_press {
            let action = match self.session.as_mut() {
                Some(session) => route_ui_button_action(&on_press, &mut session.modal_stack),
                None => return,
            };
            match action {
                UiButtonAction::CommitTextEntry => self.commit_text_entry(),
                UiButtonAction::CloseDialog => {}
                UiButtonAction::ExitToDesktop => self.pending_exit_to_desktop = true,
                UiButtonAction::QuitToMenu => self.return_to_frontend(),
                UiButtonAction::NamedReaction => {
                    if let Some(session) = self.session.as_ref() {
                        let _ = fire_named_event_with_sequences(
                            &on_press,
                            &session.scripting.script_ctx.data_registry.borrow(),
                            &session.scripting.sequence_registry,
                            &session.scripting.reaction_registry,
                            &session.scripting.system_registry,
                            &session.scripting.script_ctx,
                        );
                    }
                }
            }
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
        let Some(target) = self.session.as_ref().and_then(|session| {
            session
                .modal_stack
                .active_text_entry_target()
                .map(str::to_string)
        }) else {
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
            if let Some(session) = self.session.as_ref() {
                session.scripting.script_ctx.system_commands.push(command);
            }
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
        let Some(rects) = self
            .session
            .as_ref()
            .and_then(|session| session.ui_focus_rects.as_ref())
        else {
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
        let on_commit = self
            .session
            .as_ref()
            .and_then(|session| session.modal_stack.active_on_commit().map(str::to_string));
        if let Some(on_commit) = on_commit {
            if let Some(session) = self.session.as_ref() {
                let _ = fire_named_event_with_sequences(
                    &on_commit,
                    &session.scripting.script_ctx.data_registry.borrow(),
                    &session.scripting.sequence_registry,
                    &session.scripting.reaction_registry,
                    &session.scripting.system_registry,
                    &session.scripting.script_ctx,
                );
            }
        }
        if let Some(session) = self.session.as_mut() {
            session.modal_stack.pop();
        }
    }

    /// Cancel the open text-entry surface (M13 Text-Entry, Task 3): pop the tree
    /// WITHOUT firing `on_commit`. Edits already applied to the bound slot are
    /// discarded simply by the opener not acting on them — there is no rollback.
    fn cancel_text_entry(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.modal_stack.pop();
        }
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
        // `dispatch_system_commands` stays on `App` (it calls App-bound lifecycle
        // methods). The script tranche, audio, and the decay/presentation systems
        // are all session-owned; clone the `ScriptCtx` handle so the queue drain +
        // the store-write arms borrow nothing of `self`, and route the audio and
        // decay/presentation arms through scoped `self.session.as_mut()` borrows.
        // See: context/lib/boot_sequence.md §1.
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return;
        };
        for command in script_ctx.system_commands.take() {
            match command {
                SystemReactionCommand::PlaySound { sound, bus } => {
                    if let Some(audio) = self
                        .session
                        .as_mut()
                        .and_then(|session| session.audio.as_mut())
                    {
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
                    if let Some(gp) = self
                        .session
                        .as_mut()
                        .and_then(|session| session.gamepad_system.as_mut())
                    {
                        gp.rumble(strong, weak, duration_ms);
                    }
                    // No gamepad subsystem ⇒ nothing to vibrate.
                }
                SystemReactionCommand::FlashScreen { color, duration_ms } => {
                    if let Some(session) = self.session.as_mut() {
                        session.scripting.flash_decay.start(color, duration_ms);
                    }
                }
                SystemReactionCommand::Vignette {
                    color,
                    strength,
                    duration_ms,
                } => {
                    // Absent color ⇒ black: a pure strength-only edge-darken. The
                    // single `durationMs` splits into a short rise (so the vignette
                    // ramps in rather than snapping) and the remaining decay,
                    // matching the flash precedent of one author-facing duration.
                    let tint = color.unwrap_or([0.0, 0.0, 0.0]);
                    let rise_ms = duration_ms * VIGNETTE_RISE_FRACTION;
                    let decay_ms = duration_ms - rise_ms;
                    if let Some(session) = self.session.as_mut() {
                        session
                            .scripting
                            .vignette_decay
                            .start(tint, strength, rise_ms, decay_ms);
                    }
                }
                SystemReactionCommand::ScreenShake {
                    amplitude,
                    duration_ms,
                    frequency,
                } => {
                    // Pass the optional frequency straight through: the driver
                    // applies its 18 Hz default when it is `None`.
                    if let Some(session) = self.session.as_mut() {
                        session
                            .scripting
                            .shake_decay
                            .start(amplitude, duration_ms, frequency);
                    }
                }
                SystemReactionCommand::PushTree { tree, on_commit } => {
                    // Resolve the registered tree by name onto the modal stack.
                    // An unknown name warns and is a no-op (no panic). The carried
                    // `on_commit` rides the stack entry; the App fires it from the
                    // text-entry commit path, then pops the entry. The capture mode
                    // lives on the registered tree's envelope (read after the drain by
                    // `reconcile_ui_focus`), not on the command.
                    if let Some(session) = self.session.as_mut() {
                        session.modal_stack.push_named(&tree, on_commit);
                    }
                }
                SystemReactionCommand::LoadLevel { map } => {
                    if let Some(session) = self.session.as_mut() {
                        session.modal_stack.clear_pushed();
                    }
                    self.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(map)));
                }
                SystemReactionCommand::RestartLevel => {
                    if let Some(source) = self.active_level_source.clone() {
                        if let Some(session) = self.session.as_mut() {
                            session.modal_stack.clear_pushed();
                        }
                        self.enqueue_level_request(LevelRequest::Load(source));
                    }
                }
                SystemReactionCommand::ReturnToFrontend => {
                    self.return_to_frontend();
                }
                SystemReactionCommand::PopTree => {
                    if let Some(session) = self.session.as_mut() {
                        session.modal_stack.pop();
                    }
                }
                SystemReactionCommand::SetState { slot, value } => {
                    // Readonly-gated JSON write at the game-logic stage: a readonly
                    // slot warns and no-ops; an unknown slot or type mismatch logs
                    // and is skipped — never a panic. NEVER the engine bypass.
                    if let Err(err) = crate::scripting::primitives::store::write_state_slot_json(
                        &script_ctx,
                        &slot,
                        &value,
                    ) {
                        log::warn!("[Scripting] setState write to `{slot}` failed: {err}");
                    }
                }
                SystemReactionCommand::CellWrite { scope, cell, value } => {
                    // Presentation-cell write at the game-logic stage (M13 G1b,
                    // Task 5): routes into the app-side `PresentationCellStore`,
                    // NEVER the slot table. A value of an unusable shape is skipped
                    // with a warn — never a panic, never a store write.
                    match scripting_systems::presentation_cells::json_to_cell_value(&value) {
                        Some(cell_value) => {
                            if let Some(session) = self.session.as_mut() {
                                session.presentation_cells.write(scope, cell, cell_value);
                            }
                        }
                        None => log::warn!(
                            "[Scripting] cellWrite to `{scope}.{cell}` carried an unusable value; skipped"
                        ),
                    }
                }
                SystemReactionCommand::AppendText { slot, text } => {
                    // Readonly-gated text edit at the game-logic stage (same
                    // writable-slot gate as setState): readonly warns + no-ops;
                    // unknown/non-String slot logs — never a panic.
                    use crate::scripting::primitives::store::{TextEdit, apply_text_edit};
                    if let Err(err) = apply_text_edit(&script_ctx, &slot, TextEdit::Append(&text)) {
                        log::warn!("[Scripting] appendText to `{slot}` failed: {err}");
                    }
                }
                SystemReactionCommand::BackspaceText { slot } => {
                    // Empty backspace is a silent no-op inside `apply_text_edit`.
                    use crate::scripting::primitives::store::{TextEdit, apply_text_edit};
                    if let Err(err) = apply_text_edit(&script_ctx, &slot, TextEdit::Backspace) {
                        log::warn!("[Scripting] backspaceText to `{slot}` failed: {err}");
                    }
                }
                SystemReactionCommand::ClearText { slot } => {
                    use crate::scripting::primitives::store::{TextEdit, apply_text_edit};
                    if let Err(err) = apply_text_edit(&script_ctx, &slot, TextEdit::Clear) {
                        log::warn!("[Scripting] clearText to `{slot}` failed: {err}");
                    }
                }
            }
        }
    }

    /// Net poll plus client apply (M15 Phase 1). Thin delegation to
    /// `crate::netcode`. Drives the endpoint's transport (`update`) once per
    /// frame, then, on the client, applies received host snapshots into the
    /// registry through the game-logic-owned `netcode::apply`. The mutable
    /// registry borrow is threaded in here, so `crate::netcode` never reaches
    /// into `App`. This is a no-op for single-player and for the host, which
    /// serializes post-loop instead.
    fn net_poll_and_apply(&mut self, frame_dt: f32) {
        let dt = std::time::Duration::from_secs_f32(frame_dt);
        // `net_poll_and_apply` stays on `App` (it drives `net_endpoint`, now
        // session-owned). Clone the `ScriptCtx` handle up front so the
        // registry/data-registry/gravity reads borrow nothing of `self`; the
        // `session` re-borrow for `net_endpoint` happens after these owned/disjoint
        // captures. See: context/lib/boot_sequence.md §1.
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return;
        };
        // Capture the host's descriptor-spawn inputs before the `net_endpoint` borrow:
        // the accept arm materializes each accepted client's descriptor-backed remote
        // pawn (M15 Phase 3 Task 4), and these reads alias the session script context /
        // `self.nav_graph` / `self.host_spawn_points`, which the endpoint borrow would
        // otherwise lock out. Cheap on the non-accept path (descriptors clone is the
        // only cost, paid once per frame on the host).
        // Both the host accept arm and the client apply arm need the shared descriptor
        // table: the host materializes each accepted client's descriptor-backed pawn
        // (Task 4), and the client materializes its LOCAL pawn's descriptor-backed
        // `PlayerMovementComponent` from the wire `entity_class` (Task 7). Both peers
        // load the same content, so the same descriptor table serves both roles — clone
        // it for either networked role before the `net_endpoint` borrow.
        let is_networked = matches!(
            self.session
                .as_ref()
                .and_then(|session| session.net_endpoint.as_ref()),
            Some(netcode::NetEndpoint::Host { .. } | netcode::NetEndpoint::Client { .. })
        );
        let net_descriptors: Vec<postretro_entities::EntityTypeDescriptor> = if is_networked {
            script_ctx.data_registry.borrow().entities.clone()
        } else {
            Vec::new()
        };
        let host_agent_params = self.nav_graph.as_ref().map(|g| g.agent_params());
        let host_spawn_points = std::mem::take(&mut self.host_spawn_points);
        // M15 Phase 3 Task 5: the client reconcile replay threads collision + gravity
        // through `client_receive_and_apply`. Capture the gravity scalar before the
        // endpoint borrow (a `Cell` copy); the collision world is read by-reference
        // inside the client arm (a disjoint `self` field from `self.session`).
        let gravity = script_ctx.gravity.get();
        let collision_world = &self.collision_world;
        // `net_endpoint` and `mesh_clip_tables` are both session-owned but distinct
        // fields; bind the session once and reach each as a disjoint field borrow,
        // so the client arm's `mesh_clip_tables` read does not re-borrow the
        // session while the `net_endpoint` match holds it.
        let Some(session) = self.session.as_mut() else {
            self.host_spawn_points = host_spawn_points;
            return;
        };
        match session.net_endpoint.as_mut() {
            None => {}
            Some(netcode::NetEndpoint::Host {
                server,
                allocator,
                replication,
                replicable,
                slot_pawns,
                command_queues,
                owners,
                tick,
                host_pawn: _,
                map_enemies: _,
                demo_mover: _,
                state_slots,
            }) => {
                // Drive the listen server (accept handshakes, drain the socket).
                // Snapshots are sent post-loop in `net_serialize_and_send`.
                match server.update(dt) {
                    // Drive this frame's connection transitions through the game-logic-
                    // owned registry borrow: an accept verdict registers the client and
                    // spawns its slot-owned inert pawn; a lifecycle close despawns it.
                    Ok(poll) => {
                        use postretro_net::transport::HandshakeOutcome;
                        // The accept verdict is the production spawn seam. An accepted
                        // client must get its slot-owned pawn spawned + registered HERE,
                        // so it is in the replicable set before `net_serialize_and_send`
                        // runs `host_replicate` post-loop and the pawn lands in the first
                        // snapshot. `SlotEvent::Accepted` never reaches `poll.lifecycle`
                        // (the transport discards it at `on_accept`); lifecycle carries
                        // `Closed` only. Both paths mutate the registry, so take one
                        // game-logic-owned borrow when either has work.
                        if !poll.handshakes.is_empty() || !poll.lifecycle.is_empty() {
                            let mut registry = script_ctx.registry.borrow_mut();
                            for outcome in &poll.handshakes {
                                match outcome {
                                    HandshakeOutcome::Accepted { client_id } => {
                                        log::info!("[Net] client {client_id} accepted");
                                        replication.register_client(*client_id);
                                        // M15 Phase 3.5: register the accepted client with
                                        // the state tracker too, so its first snapshot
                                        // carries a full state baseline (a late joiner gets
                                        // one without waiting for a value change).
                                        state_slots.register_client(*client_id);
                                        if host_spawn_points.is_empty() {
                                            // No descriptor-backed player spawn on this
                                            // map: fall back to the inert Transform-only
                                            // fixture (dev/test path; never local).
                                            netcode::host_handle_accept(
                                                &mut registry,
                                                allocator,
                                                replicable,
                                                slot_pawns,
                                                *client_id,
                                            );
                                        } else {
                                            // Phase 3 movement session: materialize the
                                            // descriptor-backed remote PlayerMovement pawn
                                            // from the slot's assigned placement.
                                            netcode::host_handle_accept_descriptor(
                                                &mut registry,
                                                allocator,
                                                replicable,
                                                slot_pawns,
                                                command_queues,
                                                owners,
                                                *client_id,
                                                &host_spawn_points,
                                                &net_descriptors,
                                                host_agent_params,
                                            );
                                        }
                                    }
                                    HandshakeOutcome::Rejected { client_id, reason } => {
                                        log::warn!("[Net] client {client_id} rejected: {reason}");
                                    }
                                }
                            }
                            // Lifecycle carries only `Closed` events; the accept-spawn
                            // above is the sole accept seam.
                            netcode::host_handle_lifecycle(
                                &mut registry,
                                replicable,
                                replication,
                                state_slots,
                                slot_pawns,
                                command_queues,
                                owners,
                                &poll.lifecycle,
                            );
                        }
                    }
                    Err(err) => log::error!("[Net] host update failed: {err}"),
                }
                // Drain each accepted client's reliable Channel::Input: apply
                // replication acks and baseline-refresh requests into the tracker,
                // and echo time-sync probes with the current server tick. The echo
                // microseconds are telemetry only, derived from the monotonic tick.
                let server_tick = *tick;
                let server_now_us = u64::from(server_tick) * netcode::SERVER_TICK_MICROS;
                for client_id in server.accepted_clients() {
                    netcode::host_handle_client_messages(
                        server,
                        replication,
                        state_slots,
                        command_queues,
                        client_id,
                        server_tick,
                        server_now_us,
                    );
                }
            }
            Some(netcode::NetEndpoint::Client {
                client,
                replication,
                time_sync,
                prediction,
                state_slots,
                ..
            }) => {
                if let Err(err) = client.update(dt) {
                    log::error!("[Net] client update failed: {err}");
                }
                // Drive the 5 Hz time-sync send loop + echo ingest. The client's
                // local sim tick is the engine frame counter; the estimator reads
                // its own monotonic clock for send/receive microseconds.
                let client_tick = script_ctx.frame.get() as u32;
                netcode::client_drive_time_sync(client, time_sync, client_tick);
                // Decode + apply every snapshot received this frame through the
                // Phase 2 client state machine, arm prediction off any `local_player`
                // baseline, apply replicated state-slot records through the store-write
                // path, send the resulting acks + baseline-refresh requests, and advance
                // the pending-repair 5 Hz cadence. The registry and slot table are
                // disjoint RefCells; both borrows coexist for the duration of the apply.
                let mut registry = script_ctx.registry.borrow_mut();
                let mut slot_table = script_ctx.slot_table.borrow_mut();
                let materialized_remote_enemy_presentation = netcode::client_receive_and_apply(
                    &mut registry,
                    &mut slot_table,
                    client,
                    replication,
                    state_slots,
                    prediction,
                    &net_descriptors,
                    host_agent_params,
                    collision_world,
                    gravity,
                    crate::frame_timing::TICK_DURATION.as_secs_f32(),
                    dt,
                );
                if materialized_remote_enemy_presentation {
                    // `mesh_clip_tables` is a disjoint field of the same `session`
                    // bound for the `net_endpoint` match above.
                    resolve_mesh_entity_clips(&mut registry, &session.mesh_clip_tables);
                }
                // The interpolation-buffer sampling that writes presented remote poses
                // runs in `net_sample_remote_interpolation`, AFTER the catch-up tick
                // loop's stage-0 `snapshot_transforms` — so its previous/current
                // remote-presentation write is the final word before render and is not
                // clobbered by the snapshot pass.
            }
        }
        // Restore the spawn-point cache taken before the endpoint borrow. The host
        // needs it on every future accept; `mem::take` only borrowed it for this call.
        self.host_spawn_points = host_spawn_points;
    }

    /// Host Phase 2 replication step. Thin delegation to `crate::netcode`. Ingests
    /// the replicable set from the registry (immutable borrow) into the per-client
    /// replication tracker every sim tick and, on the 30 Hz cadence, encodes and
    /// sends each accepted client a per-client delta snapshot over the snapshot
    /// channel. No-op for single-player and the client.
    fn net_serialize_and_send(&mut self) {
        // Session-owned `ScriptCtx` cloned before the `net_endpoint` borrow (this
        // method stays on `App`). See: context/lib/boot_sequence.md §1.
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return;
        };
        let Some(netcode::NetEndpoint::Host {
            server,
            allocator,
            tick,
            replication,
            replicable,
            slot_pawns: _,
            command_queues,
            owners,
            host_pawn: _,
            map_enemies: _,
            demo_mover,
            state_slots,
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };

        // Demo path only (POSTRETRO_NET_DEMO_MOVER=1): spawn-and-drive the
        // deterministic Phase 2 net-demo mover for this tick before snapshotting, so
        // its pose is in the replicable set when `host_replicate` ingests below. A
        // no-op on an ordinary host.
        {
            let mut registry = script_ctx.registry.borrow_mut();
            netcode::host_drive_demo_mover(&mut registry, demo_mover, allocator, replicable, *tick);
        }

        {
            // M15 Phase 3.5: borrow the slot table (immutable) alongside the registry so
            // `host_replicate` can collect this frame's replicated-state source values
            // and splice the per-client state records into the snapshot envelope. The
            // two RefCells are disjoint, so both borrows coexist. Game logic and the HUD
            // publisher have already settled the slot table by this post-tick point; the
            // descriptor-fed health projection reads live `HealthComponent`s, so it sees
            // this frame's settled HP regardless of the host HUD publisher's later tick.
            let registry = script_ctx.registry.borrow();
            let slot_table = script_ctx.slot_table.borrow();
            netcode::host_replicate(
                &registry,
                &slot_table,
                server,
                allocator,
                replication,
                state_slots,
                replicable,
                owners,
                command_queues,
                *tick,
            );
        }
        // Advance the monotonic server tick after this tick's ingest+send so a
        // late-joining client never sees a stalled clock.
        *tick = tick.wrapping_add(1);
    }

    /// Client remote-interpolation sampling step (M15 Phase 2 Task 6). Thin delegation
    /// to `crate::netcode`. Samples each remote entity's interpolation buffer at the
    /// adaptive render target tick (jitter delay plus held-newest starvation
    /// feedback) and writes the presented pose through the
    /// registry's remote-presentation helper. That pose is already resolved at the
    /// correct server-time target, so the write is alpha-agnostic (previous ==
    /// current); the render-stage `interpolated_transform` blend reproduces it
    /// verbatim rather than re-blending it by the unrelated sim sub-tick alpha.
    ///
    /// Runs after the catch-up tick loop so the stage-0 `snapshot_transforms` cannot
    /// clobber the presented pose, and before the render stage reads entities.
    /// No-op for single-player and the host (no client interpolation buffers).
    fn net_sample_remote_interpolation(&mut self, frame_dt: f32) {
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return;
        };
        let Some(netcode::NetEndpoint::Client {
            replication,
            time_sync,
            interpolation_delay,
            ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        let mut registry = script_ctx.registry.borrow_mut();
        netcode::client_sample_interpolation(
            &mut registry,
            replication,
            time_sync,
            interpolation_delay,
            f64::from(frame_dt),
        );
    }

    /// Whether this process is a connected client (M15 Phase 3). The connected
    /// client predicts its own movement pawn instead of running the full local
    /// `sim::simulate_tick`; the host and single-player keep the full sim path.
    fn is_connected_client(&self) -> bool {
        matches!(
            self.session
                .as_ref()
                .and_then(|session| session.net_endpoint.as_ref()),
            Some(netcode::NetEndpoint::Client { .. })
        )
    }

    /// Host authoritative movement pre-pass (M15 Phase 3 Task 4). Resolves one
    /// command per OWNED (remote) pawn through the deterministic gap policy, routes
    /// each through the `EntityId -> client_id` map, and advances those pawns through
    /// the multi-pawn movement seam — BEFORE the frame's `simulate_tick` runs AI /
    /// weapon / death. Remote authoritative movement never goes through
    /// `local_movement_pawn`: every owned pawn is named explicitly here. The host's
    /// OWN player pawn (if any) is still driven by `simulate_tick`'s movement stage
    /// from locally-sampled input; folding it into this explicit list alongside the
    /// host's sampled command is the remaining integration seam (it requires the host
    /// to own a queue/owner entry for itself). No-op for single-player and the client.
    ///
    /// Returns the aggregated remote movement events for the caller to fold into the
    /// frame's pending movement-event drain.
    fn host_drive_remote_movement(&mut self, tick_dt: f32) -> Vec<&'static str> {
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return Vec::new();
        };
        let Some(netcode::NetEndpoint::Host {
            command_queues,
            owners,
            ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return Vec::new();
        };
        let pawn_inputs = netcode::host_resolve_movement_inputs(owners, command_queues);
        if pawn_inputs.is_empty() {
            return Vec::new();
        }
        let mut registry = script_ctx.registry.borrow_mut();
        sim::run_host_movement_tick(
            &mut registry,
            &self.collision_world,
            script_ctx.gravity.get(),
            &pawn_inputs,
            tick_dt,
        )
    }

    /// Register the listen host's OWN player pawn for outbound replication after a
    /// level install (M15 Phase 3, issue 3b). The host's boot pawn is spawned by
    /// `install_level_payload` via `spawn_from_player_starts` and marked the
    /// `local_player_pawn`; without registering it in the `ReplicableSet` it never
    /// reaches `produce_owned_snapshots`, so clients draw no host capsule.
    ///
    /// Thin delegation: reads `local_player_pawn` from the registry and hands it to
    /// `netcode::host_register_own_pawn`, which stamps a `NetworkId`, registers it for
    /// replication with NO owner mapping (never `local_player` on any recipient), and
    /// tracks it so a level reload unregisters the stale pawn. No-op for single-player,
    /// the client, and a host whose map has no `player_spawn` (no local pawn to
    /// replicate). The host pawn stays driven locally by `simulate_tick` — this only
    /// replicates its Transform + PlayerMovementState outbound.
    fn host_register_own_pawn_after_install(&mut self) {
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return;
        };
        let Some(netcode::NetEndpoint::Host {
            allocator,
            replicable,
            host_pawn,
            ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        let pawn = {
            let registry = script_ctx.registry.borrow();
            registry.local_player_pawn()
        };
        let Some(pawn) = pawn else {
            // A host on a map with no player_spawn has no own pawn to replicate.
            return;
        };
        netcode::host_register_own_pawn(allocator, replicable, host_pawn, pawn);
    }

    /// Register the listen host's map-placed AI enemies for outbound replication after a
    /// level install (E10 Task 4). Map-placed descriptor enemies carrying `Brain` + `Agent`
    /// are spawned by `apply_data_archetype_dispatch`; without registering them in the
    /// `ReplicableSet` they never reach `produce_owned_snapshots`, so clients see no enemy.
    ///
    /// Host-gated: a no-op for single-player and the connected client (the endpoint is not
    /// the `Host` variant). Thin delegation to `netcode::host_register_map_enemies`, which
    /// sweeps the registry for AI map enemies, stamps each a `NetworkId`, registers it with
    /// NO owner mapping (host-authoritative, never `local_player`), and tracks the ids in
    /// the `Host` endpoint's `map_enemies` set so a level reload unregisters the stale ones
    /// first. The enemies stay driven by the host's AI/steering systems — this only
    /// replicates their `Transform` (and descriptor class) outbound.
    fn host_register_map_enemies_after_install(&mut self) {
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return;
        };
        let Some(netcode::NetEndpoint::Host {
            allocator,
            replicable,
            map_enemies,
            ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        let registry = script_ctx.registry.borrow();
        netcode::host_register_map_enemies(&registry, allocator, replicable, map_enemies);
    }

    /// Connected-client predicted fixed tick (M15 Phase 3 Task 3). Thin delegation
    /// to `crate::netcode`: sends one `ClientMessage::Input` for `command`, then
    /// advances the local pawn through the movement-only replay helper and writes the
    /// predicted state back to the registry. Returns `true` if it drove the local
    /// pawn (prediction armed), `false` if it only sent input (pre-baseline). The
    /// caller skips `simulate_tick`'s local gameplay movement when this path runs —
    /// AI / weapons / death stay host-authoritative and arrive via snapshots.
    fn client_predict_movement_tick(&mut self, command: &sim::SimCommand, tick_dt: f32) -> bool {
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return false;
        };
        let Some(netcode::NetEndpoint::Client {
            client, prediction, ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return false;
        };
        let gravity = script_ctx.gravity.get();
        let mut registry = script_ctx.registry.borrow_mut();
        netcode::client_predict_tick(
            &mut registry,
            client,
            prediction,
            command,
            &self.collision_world,
            gravity,
            tick_dt,
        )
    }

    /// Accumulate one frame onto the animation clock: `prev + dt × scale`.
    /// Pure so the accumulation contract (scale 0.5 halves advancement; a
    /// mid-accumulation scale change never jumps the clock because we add scaled
    /// deltas rather than scaling absolute time) is unit-verifiable without the
    /// event loop. The freeze gate lives at the call site. See scripting.md §10.3.
    fn advance_anim_clock(prev: f64, frame_dt: f64, scale: f64) -> f64 {
        prev + frame_dt * scale
    }

    /// Transition input focus, acquiring or releasing the cursor as required
    /// and clearing carry-over input state so keys/mouse held during the
    /// transition do not stick in the new mode.
    fn set_input_focus(&mut self, focus: InputFocus) {
        // Disjoint field borrows: the session group plus the non-session window
        // and diagnostic state all mutate here. No-op if the session is not yet
        // installed (focus transitions only happen post-install).
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.input_focus = focus;
        if let Some(ws) = self.window_state.as_ref() {
            match focus {
                InputFocus::Gameplay => {
                    input::cursor::capture_cursor(&ws.window);
                }
                InputFocus::DevTools | InputFocus::Menu => {
                    input::cursor::release_cursor(&ws.window);
                }
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
        session.input_system.clear_all();
        session.gameplay_input_latch.clear();
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

    /// Apply the `nav.menu` pause-menu policy: pop the pause menu if it is active,
    /// open it when the modal stack is empty, and ignore the action while another
    /// modal is active. Wired to gamepad Start / Escape-from-gameplay through
    /// `pending_menu_toggle`. The capture-mode + cursor effect follows on the next
    /// `reconcile_ui_focus` (this game-logic phase).
    fn toggle_pause_menu(&mut self) {
        if let Some(session) = self.session.as_mut() {
            apply_pause_menu_nav_policy(&mut session.modal_stack);
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
    ///   `InputFocus::Menu` (cursor released, player controls gated).
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
        // Read the session-owned inputs up front, then drop the borrow before
        // `set_input_focus` (which re-borrows the session). No-op before install.
        let (mode, current_focus) = {
            let Some(session) = self.session.as_mut() else {
                return;
            };
            let mode = session.modal_stack.top_capture_mode();
            session.ui_dispatch.set_mode(mode.into());
            (mode, session.input_focus)
        };

        // The debug overlay owns focus while open — don't fight it.
        if current_focus == InputFocus::DevTools {
            return;
        }

        let want_menu = matches!(mode, render::ui::descriptor::CaptureMode::Capture);
        match (want_menu, current_focus) {
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
        let cursor_visible = self
            .session
            .as_ref()
            .map(|session| (session.input_focus, session.ui_input_mode.cursor_visible()));
        if let Some((InputFocus::Menu, visible)) = cursor_visible {
            if want_menu {
                if let Some(ws) = self.window_state.as_ref() {
                    ws.window.set_cursor_visible(visible);
                }
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
        let Some(focus) = self.session.as_ref().map(|session| session.input_focus) else {
            return;
        };
        let Some(ws) = self.window_state.as_ref() else {
            return;
        };
        match focus {
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
                if let Some(audio) = self
                    .session
                    .as_mut()
                    .and_then(|session| session.audio.as_mut())
                {
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
                let now_visible = if let Some(debug_ui) = self
                    .session
                    .as_mut()
                    .and_then(|session| session.debug_ui.as_mut())
                {
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
            #[cfg(feature = "dev-tools")]
            DiagnosticAction::CycleDevLevel => {
                self.enqueue_dev_level_cycle();
            }
            #[cfg(feature = "dev-tools")]
            DiagnosticAction::SpawnChaseAgent => {
                self.spawn_debug_chase_agent();
            }
        }
    }

    /// Spawn the dev-tools "chase me" demo agent at the camera position, seeded
    /// from the loaded navmesh's baked agent params. Idempotent per level: a
    /// second press re-targets the existing agent instead of stacking spawns.
    /// No-op when the map carries no navmesh (`agent_params` needs the graph).
    ///
    /// The `NavGraph::agent_params()` read and `attach_agent` happen HERE at the
    /// spawn call site (not inside the component constructor): the baked params
    /// describe the capsule the floor was eroded for. The per-tick destination
    /// is then driven by `run_agent_tick`.
    #[cfg(feature = "dev-tools")]
    fn spawn_debug_chase_agent(&mut self) {
        use postretro_entities::Transform;
        use postretro_entities::components::agent::attach_agent;

        let Some(nav_graph) = self.nav_graph.as_ref() else {
            log::warn!("[dev-tools] chase agent: map has no navmesh; cannot spawn");
            return;
        };
        if self.debug_chase_agent.is_some() {
            log::info!("[dev-tools] chase agent already spawned; re-targeting each tick");
            return;
        }

        // Top speed for the demo pursuer (world-units/sec). A brisk-but-readable
        // chase; the capsule itself comes from the baked params below.
        const CHASE_MOVE_SPEED: f32 = 4.0;

        let params = nav_graph.agent_params();
        let spawn_pos = self.camera.position;

        let script_ctx = self
            .session
            .as_ref()
            .expect("running session installed")
            .scripting
            .script_ctx
            .clone();
        let mut registry = script_ctx.registry.borrow_mut();
        let entity = registry.spawn(Transform {
            position: spawn_pos,
            ..Transform::default()
        });
        match attach_agent(&mut registry, entity, &params, CHASE_MOVE_SPEED) {
            Ok(()) => {
                drop(registry);
                self.debug_chase_agent = Some(entity);
                log::info!(
                    "[dev-tools] spawned chase agent {:?} at {:?} (chasing player/camera)",
                    entity,
                    spawn_pos,
                );
            }
            Err(err) => {
                log::warn!("[dev-tools] chase agent attach failed: {err:?}");
            }
        }
    }
}

#[cfg(feature = "dev-tools")]
fn drawable_visible_cell_mask(
    leaf_count: usize,
    visible_cells: &VisibleCells,
) -> Option<Vec<bool>> {
    match visible_cells {
        VisibleCells::DrawAll => None,
        VisibleCells::Culled(cells) => {
            let mut mask = vec![false; leaf_count];
            for &cell in cells {
                if let Some(slot) = mask.get_mut(cell as usize) {
                    *slot = true;
                }
            }
            Some(mask)
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
    use crate::scripting::primitives::register_all;
    use postretro_foundation::{
        AirParams, CapsuleParams, FallParams, ForgivenessParams, GroundParams,
        PlayerMovementDescriptor, SpeedParams,
    };
    use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
    use postretro_scripting_core::runtime::ScriptRuntimeConfig;

    // M15 Phase 3.5 Task 5: a connected client skips the clean-exit `state.json` save;
    // single-player and the host still save. `is_connected_client` is `true` only for
    // `NetEndpoint::Client`, so this gate is the role-aware switch at the save call site.
    #[test]
    fn connected_client_skips_state_save_while_single_player_and_host_save() {
        // Single-player (no endpoint) and host (not a connected client) save.
        assert!(
            should_save_persisted_state(true, false),
            "single-player / host saves when the lifecycle permits"
        );
        // A connected client never saves, even when the lifecycle would otherwise allow.
        assert!(
            !should_save_persisted_state(true, true),
            "a connected client skips the clean-exit save"
        );
        // The lifecycle gate still suppresses the save before commit/restore.
        assert!(!should_save_persisted_state(false, false));
        assert!(!should_save_persisted_state(false, true));
    }

    fn minimal_player_descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.4,
                half_height: 0.8,
                eye_height: 0.5,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 10.0,
                step_height: 0.3,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.0,
                accel: 0.7,
                max_control_speed: 0.5,
                bunny_hop: false,
                jumps: 0,
                jump_velocity: 5.5,
                jump_ceiling: 0.0,
            },
            fall: FallParams {
                terminal_velocity: 40.0,
            },
            stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
            stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
            dash: None,
            forgiveness: Some(ForgivenessParams {
                coyote_ms: 0.0,
                jump_buffer_ms: 0.0,
            }),
            crouch: None,
            view_feel: None,
        }
    }

    #[test]
    fn ui_button_action_classifier_reserves_ui_actions_before_named_reactions() {
        assert_eq!(
            classify_ui_button_action(render::ui::actions::COMMIT_TEXT_ENTRY_ACTION),
            UiButtonAction::CommitTextEntry
        );
        assert_eq!(
            classify_ui_button_action(render::ui::actions::CLOSE_DIALOG_ACTION),
            UiButtonAction::CloseDialog
        );
        assert_eq!(
            classify_ui_button_action(render::ui::actions::EXIT_TO_DESKTOP_ACTION),
            UiButtonAction::ExitToDesktop
        );
        assert_eq!(
            classify_ui_button_action(render::ui::actions::QUIT_TO_MENU_ACTION),
            UiButtonAction::QuitToMenu
        );
        assert_eq!(
            classify_ui_button_action("resumeGame"),
            UiButtonAction::NamedReaction,
            "ordinary button names must keep the named-reaction route",
        );
    }

    #[test]
    fn frontend_return_requests_enqueue_unload_then_optional_backdrop_load() {
        assert_eq!(frontend_return_requests(None), vec![LevelRequest::Unload]);

        let frontend = Frontend {
            menu_tree: "mainMenu".to_string(),
            background_level: Some("menuBackdrop".to_string()),
            camera: MenuCamera {
                position: [0.0, 0.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
            },
        };
        assert_eq!(
            frontend_return_requests(Some(&frontend)),
            vec![
                LevelRequest::Unload,
                LevelRequest::Load(LevelSource::Catalog("menuBackdrop".to_string())),
            ]
        );
    }

    #[test]
    fn gameplay_snapshot_uses_neutral_input_while_ui_captures() {
        let mut latch = input::GameplayInputLatch::new();

        let mut keyboard = InputSystem::new(default_bindings());
        keyboard.handle_keyboard_event(winit::keyboard::KeyCode::Space, true);
        let pressed_before_capture = keyboard.snapshot();
        assert!(
            latch
                .snapshot_for_ticks(&pressed_before_capture, 0)
                .is_none(),
            "zero-tick frame latches a jump press for the next gameplay tick",
        );

        let mut gamepad = InputSystem::new(default_bindings());
        gamepad.set_gamepad_axis(gilrs::Axis::LeftStickY, -1.0);
        gamepad.set_physical_input(
            input::PhysicalInput::GamepadButton(gilrs::Button::South),
            true,
        );
        let captured_raw_snapshot = gamepad.snapshot();

        let captured_snapshot =
            gameplay_snapshot_for_capture_state(&mut latch, &captured_raw_snapshot, 1, true)
                .expect("simulation still ticks while UI captures");
        assert_eq!(
            captured_snapshot.axis_value(Action::MoveForward),
            0.0,
            "capturing UI gates gamepad movement from gameplay",
        );
        assert_eq!(
            captured_snapshot.button(Action::Jump),
            ButtonState::Inactive,
            "capturing UI gates gamepad confirm from gameplay jump",
        );

        let after_capture = gameplay_snapshot_for_capture_state(
            &mut latch,
            &input::ActionSnapshot::neutral(),
            1,
            false,
        )
        .expect("gameplay resumes with a tick");
        assert_eq!(
            after_capture.button(Action::Jump),
            ButtonState::Inactive,
            "capture clears any previously latched button edge so it cannot replay after close",
        );
    }

    #[test]
    fn gameplay_snapshot_stays_neutral_on_gamepad_pause_close_frame() {
        use crate::render::ui::descriptor::{
            Align, AnchoredTree, CaptureMode, ContainerWidget, SpacingValue, Widget,
        };
        use crate::render::ui::layout::Anchor;
        use crate::render::ui::modal_stack::{ModalStack, ScopeTier};

        fn capturing_tree() -> AnchoredTree {
            AnchoredTree {
                anchor: Anchor::Center,
                offset: [0.0, 0.0],
                root: Widget::VStack(ContainerWidget {
                    gap: SpacingValue::Literal(0.0),
                    padding: SpacingValue::Literal(0.0),
                    align: Align::Start,
                    fill: None,
                    border: None,
                    id: None,
                    focus_neighbors: Default::default(),
                    focus: None,
                    restore_on_return: false,
                    local_state: None,
                    visible_when: None,
                    role: None,
                    children: Vec::new(),
                }),
                capture_mode: CaptureMode::Capture,
                initial_focus: Some("pauseResume".to_string()),
                text_entry_target: None,
                accessible_name: None,
                role: None,
            }
        }

        let mut stack = ModalStack::new();
        stack.registry_mut().register(
            render::ui::demo::PAUSE_MENU_NAME,
            capturing_tree(),
            ScopeTier::Engine,
            false,
        );
        stack.push_named(render::ui::demo::PAUSE_MENU_NAME, None);
        let ui_captured_gameplay_at_frame_start =
            stack.top_capture_mode() == render::ui::descriptor::CaptureMode::Capture;

        let routed = route_ui_button_action(render::ui::actions::CLOSE_DIALOG_ACTION, &mut stack);
        assert_eq!(routed, UiButtonAction::CloseDialog);
        assert!(
            stack.is_empty(),
            "Resume closes the pause menu before gameplay snapshots are selected",
        );
        assert_ne!(
            stack.top_capture_mode(),
            render::ui::descriptor::CaptureMode::Capture,
            "the post-pop stack alone would no longer gate gameplay",
        );

        let mut input_system = InputSystem::new(default_bindings());
        input_system.set_gamepad_axis(gilrs::Axis::LeftStickY, -1.0);
        input_system.set_physical_input(
            input::PhysicalInput::GamepadButton(gilrs::Button::South),
            true,
        );
        input_system.set_physical_input(
            input::PhysicalInput::GamepadButton(gilrs::Button::East),
            true,
        );

        let mut latch = input::GameplayInputLatch::new();
        let frame_snapshot = input_system.snapshot();
        let gameplay_snapshot = gameplay_snapshot_for_capture_state(
            &mut latch,
            &frame_snapshot,
            1,
            gameplay_capture_gate_for_frame(ui_captured_gameplay_at_frame_start, &stack),
        )
        .expect("simulation still ticks on the pause-menu close frame");

        // Regression: gamepad Resume/Cancel that closed a capturing pause menu
        // leaked through as Jump/Dash on the same gameplay frame.
        assert_eq!(gameplay_snapshot.axis_value(Action::MoveForward), 0.0);
        assert_eq!(
            gameplay_snapshot.button(Action::Jump),
            ButtonState::Inactive
        );
        assert_eq!(
            gameplay_snapshot.button(Action::Dash),
            ButtonState::Inactive
        );
    }

    #[test]
    fn menu_camera_pose_hold_replaces_interpolation_endpoints() {
        let mut camera = Camera::new(Vec3::new(10.0, 20.0, 30.0), 1.0, 0.5);
        let mut frame_timing =
            FrameTiming::new(InterpolableState::new(Vec3::new(10.0, 20.0, 30.0)));
        frame_timing.push_state(InterpolableState::new(Vec3::new(100.0, 200.0, 300.0)));
        let pose = MenuCamera {
            position: [4.0, 2.0, 8.0],
            yaw: -0.6,
            pitch: -0.1,
        };

        apply_menu_camera_pose(&mut camera, &mut frame_timing, &pose);

        assert_eq!(camera.position, Vec3::new(4.0, 2.0, 8.0));
        assert_eq!(camera.yaw, -0.6);
        assert_eq!(camera.pitch, -0.1);
        assert_eq!(
            frame_timing.interpolated_state().position,
            Vec3::new(4.0, 2.0, 8.0),
            "render interpolation must not blend from the player spawn after the menu pose is reapplied",
        );
    }

    #[test]
    fn sim_catchup_pushes_interpolation_state_per_tick() {
        use std::cell::RefCell;
        use std::collections::HashSet;
        use std::rc::Rc;

        use crate::collision::CollisionWorld;
        use postretro_entities::{EntityRegistry, Transform};
        use postretro_foundation::PlayerMovementComponent;

        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let descriptor = minimal_player_descriptor();
        let start_position = Vec3::new(
            0.0,
            descriptor.capsule.half_height + descriptor.capsule.radius + 0.5,
            0.0,
        );
        {
            let mut registry = registry.borrow_mut();
            let player = registry.spawn(Transform {
                position: start_position,
                ..Transform::default()
            });
            registry
                .set_component(
                    player,
                    PlayerMovementComponent::from_descriptor(&descriptor),
                )
                .expect("player movement component attaches to spawned entity");
        }

        let mut camera = Camera::new(
            start_position + Vec3::new(0.0, descriptor.capsule.eye_height, 0.0),
            0.0,
            0.0,
        );
        let mut frame_timing = FrameTiming::new(InterpolableState::new(camera.position));
        let initial = frame_timing.current_state.position;
        let world = CollisionWorld::new();
        let hit_zones = scripting_systems::hit_zones::HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_warned = HashSet::new();
        let command = sim::SimCommand {
            movement: movement::MovementInput {
                wish_dir: glam::Vec2::ZERO,
                jump_pressed: false,
                dash_pressed: false,
                running: false,
                crouch_intent: false,
                facing_yaw: 0.0,
            },
            fire_button: weapon::FireButtonState {
                pressed: false,
                active: false,
            },
        };

        let mut pushed_states = Vec::new();
        for _ in 0..2 {
            let _events = sim::simulate_tick(
                registry.clone(),
                &world,
                &hit_zones,
                None,
                -9.81,
                None,
                0.0,
                &mut progress,
                &mut ai_warned,
                &command,
                |registry| {
                    follow_camera_to_local_pawn(&mut camera, &registry.borrow(), Vec3::ZERO);
                    build_post_movement_command(&camera)
                },
                TICK_DURATION.as_secs_f32(),
            );
            frame_timing.push_state(InterpolableState::new(camera.position));
            pushed_states.push(frame_timing.current_state.position);
        }

        assert_eq!(pushed_states.len(), 2);
        assert_ne!(pushed_states[0], initial);
        assert_ne!(
            pushed_states[1], pushed_states[0],
            "catch-up frames must push interpolation state after each simulated tick",
        );
        assert_eq!(
            frame_timing.previous_state.position, pushed_states[0],
            "the second push must shift the first tick's camera state into previous_state",
        );
    }

    #[test]
    fn camera_follow_does_not_fallback_when_marked_movement_pawn_lacks_transform() {
        use postretro_entities::{EntityRegistry, Transform};
        use postretro_foundation::PlayerMovementComponent;

        let mut registry = EntityRegistry::new();
        let descriptor = minimal_player_descriptor();
        let marked = registry.spawn(Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            ..Transform::default()
        });
        registry
            .set_component(
                marked,
                PlayerMovementComponent::from_descriptor(&descriptor),
            )
            .expect("marked pawn receives movement");
        registry
            .remove_component::<Transform>(marked)
            .expect("test strips transform from marked pawn");
        registry.mark_local_player_pawn(marked).unwrap();

        let fallback = registry.spawn(Transform {
            position: Vec3::new(50.0, 0.0, 0.0),
            ..Transform::default()
        });
        registry
            .set_component(
                fallback,
                PlayerMovementComponent::from_descriptor(&descriptor),
            )
            .expect("fallback pawn receives movement");

        let mut camera = Camera::new(Vec3::new(9.0, 8.0, 7.0), 0.0, 0.0);

        assert_eq!(
            followed_player_pawn(&registry),
            Some(marked),
            "valid marked movement pawn remains selected even without transform"
        );
        follow_camera_to_local_pawn(&mut camera, &registry, Vec3::ZERO);
        let post = build_post_movement_command(&camera);

        assert_eq!(
            camera.position,
            Vec3::new(9.0, 8.0, 7.0),
            "camera must not silently follow a different pawn"
        );
        assert_eq!(
            post.aim_origin, camera.position,
            "aim resolves from the unchanged camera when selected pawn lacks transform"
        );
    }

    #[test]
    fn camera_follow_no_marker_fallback_does_not_skip_transformless_first_pawn() {
        use postretro_entities::{EntityRegistry, Transform};
        use postretro_foundation::PlayerMovementComponent;

        let mut registry = EntityRegistry::new();
        let descriptor = minimal_player_descriptor();
        let first = registry.spawn(Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            ..Transform::default()
        });
        registry
            .set_component(first, PlayerMovementComponent::from_descriptor(&descriptor))
            .expect("first pawn receives movement");
        registry
            .remove_component::<Transform>(first)
            .expect("test strips transform from first pawn");

        let fallback = registry.spawn(Transform {
            position: Vec3::new(50.0, 0.0, 0.0),
            ..Transform::default()
        });
        registry
            .set_component(
                fallback,
                PlayerMovementComponent::from_descriptor(&descriptor),
            )
            .expect("fallback pawn receives movement");

        let mut camera = Camera::new(Vec3::new(9.0, 8.0, 7.0), 0.0, 0.0);

        assert_eq!(
            followed_player_pawn(&registry),
            Some(first),
            "legacy no-marker fallback must pick the same first movement pawn as sim systems"
        );
        follow_camera_to_local_pawn(&mut camera, &registry, Vec3::ZERO);

        assert_eq!(
            camera.position,
            Vec3::new(9.0, 8.0, 7.0),
            "camera must not silently follow a later pawn"
        );
    }

    #[test]
    fn sim_command_reuses_frame_resolved_crouch_toggle_across_catchup_ticks() {
        let mut input_system = InputSystem::new(default_bindings());
        input_system.set_physical_input(
            input::PhysicalInput::Key(winit::keyboard::KeyCode::KeyC),
            true,
        );
        let snapshot = input_system.snapshot();
        assert_eq!(snapshot.button(Action::Crouch), ButtonState::Pressed);

        let mut crouch_toggle_active = false;
        let crouch_intent = resolve_crouch_intent(
            CrouchMode::Toggle,
            snapshot.button(Action::Crouch),
            &mut crouch_toggle_active,
        );
        assert!(crouch_intent);
        assert!(crouch_toggle_active);

        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let commands: Vec<sim::SimCommand> = (0..2)
            .map(|_| build_sim_command(&snapshot, &camera, crouch_intent, false, false))
            .collect();

        assert_eq!(commands.len(), 2);
        assert!(
            commands
                .iter()
                .all(|command| command.movement.crouch_intent)
        );
        assert!(
            crouch_toggle_active,
            "a catch-up frame must not re-resolve the same Pressed snapshot and flip the toggle off",
        );
    }

    #[test]
    fn sim_command_strips_dash_edge_after_first_catchup_tick() {
        let mut input_system = InputSystem::new(default_bindings());
        input_system.set_physical_input(
            input::PhysicalInput::Key(winit::keyboard::KeyCode::KeyF),
            true,
        );
        let snapshot = input_system.snapshot();
        assert_eq!(snapshot.button(Action::Dash), ButtonState::Pressed);

        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let commands: Vec<sim::SimCommand> = (0..2)
            .map(|tick_index| {
                let dash_pressed = tick_index == 0
                    && matches!(snapshot.button(Action::Dash), ButtonState::Pressed);
                build_sim_command(&snapshot, &camera, false, dash_pressed, false)
            })
            .collect();

        assert!(commands[0].movement.dash_pressed);
        assert!(
            !commands[1].movement.dash_pressed,
            "one physical dash press must not replay as a new dash edge on every catch-up tick",
        );
    }

    #[test]
    fn sim_command_strips_shoot_pressed_edge_after_first_catchup_tick() {
        let mut input_system = InputSystem::new(default_bindings());
        input_system.set_physical_input(
            input::PhysicalInput::MouseButton(winit::event::MouseButton::Left),
            true,
        );
        let snapshot = input_system.snapshot();
        assert_eq!(snapshot.button(Action::Shoot), ButtonState::Pressed);

        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let commands: Vec<sim::SimCommand> = (0..2)
            .map(|tick_index| {
                let shoot_pressed = tick_index == 0
                    && matches!(snapshot.button(Action::Shoot), ButtonState::Pressed);
                build_sim_command(&snapshot, &camera, false, false, shoot_pressed)
            })
            .collect();

        assert!(commands[0].fire_button.pressed);
        assert!(commands[0].fire_button.active);
        assert!(
            !commands[1].fire_button.pressed,
            "one physical shoot press must not replay as a new pressed edge on every catch-up tick",
        );
        assert!(
            commands[1].fire_button.active,
            "held shoot state must remain active across later catch-up ticks",
        );
    }

    #[test]
    fn simulate_tick_resolves_weapon_aim_after_movement_camera_follow() {
        use std::cell::RefCell;
        use std::collections::HashSet;
        use std::rc::Rc;

        use crate::collision::CollisionWorld;
        use postretro_entities::{EntityRegistry, Transform};
        use postretro_foundation::PlayerMovementComponent;

        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let descriptor = minimal_player_descriptor();
        let start_position = Vec3::new(
            0.0,
            descriptor.capsule.half_height + descriptor.capsule.radius + 0.5,
            0.0,
        );
        {
            let mut registry = registry.borrow_mut();
            let player = registry.spawn(Transform {
                position: start_position,
                ..Transform::default()
            });
            registry
                .set_component(
                    player,
                    PlayerMovementComponent::from_descriptor(&descriptor),
                )
                .expect("player movement component attaches to spawned entity");
        }

        let mut camera = Camera::new(Vec3::new(99.0, 99.0, 99.0), 0.0, 0.0);
        let world = CollisionWorld::new();
        let hit_zones = scripting_systems::hit_zones::HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_warned = HashSet::new();
        let command = sim::SimCommand {
            movement: movement::MovementInput {
                wish_dir: glam::Vec2::ZERO,
                jump_pressed: false,
                dash_pressed: false,
                running: false,
                crouch_intent: false,
                facing_yaw: 0.0,
            },
            fire_button: weapon::FireButtonState {
                pressed: true,
                active: true,
            },
        };
        let mut resolved_aim_origin = None;

        let _events = sim::simulate_tick(
            registry.clone(),
            &world,
            &hit_zones,
            None,
            -9.81,
            None,
            0.0,
            &mut progress,
            &mut ai_warned,
            &command,
            |registry| {
                follow_camera_to_local_pawn(&mut camera, &registry.borrow(), Vec3::ZERO);
                let post = build_post_movement_command(&camera);
                resolved_aim_origin = Some(post.aim_origin);
                post
            },
            TICK_DURATION.as_secs_f32(),
        );

        assert_eq!(resolved_aim_origin, Some(camera.position));
        assert_ne!(
            camera.position,
            Vec3::new(99.0, 99.0, 99.0),
            "weapon aim must be resolved from the post-movement followed camera, not the stale frame-start camera",
        );
    }

    fn widget_contains_text(widget: &render::ui::descriptor::Widget, needle: &str) -> bool {
        use render::ui::descriptor::Widget;

        match widget {
            Widget::Text(text) => text.content == needle,
            Widget::VStack(container) | Widget::HStack(container) => container
                .children
                .iter()
                .any(|child| widget_contains_text(child, needle)),
            Widget::Grid(grid) => grid
                .children
                .iter()
                .any(|child| widget_contains_text(child, needle)),
            _ => false,
        }
    }

    fn button_action<'a>(widget: &'a render::ui::descriptor::Widget, id: &str) -> Option<&'a str> {
        use render::ui::descriptor::Widget;

        match widget {
            Widget::Button(button) if button.id == id => Some(button.on_press.as_str()),
            Widget::VStack(container) | Widget::HStack(container) => container
                .children
                .iter()
                .find_map(|child| button_action(child, id)),
            Widget::Grid(grid) => grid
                .children
                .iter()
                .find_map(|child| button_action(child, id)),
            _ => None,
        }
    }

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root exists")
    }

    fn focus_button_action(
        rects: &render::ui::tree::FocusRectList,
        result: &input::FocusTickResult,
    ) -> String {
        assert!(
            result.confirmed,
            "activation should confirm the focused button"
        );
        focused_button_on_press(Some(rects), result.focused.as_deref())
            .expect("focused button exposes an onPress action")
    }

    #[cfg(debug_assertions)]
    fn install_scripts_build_next_to_current_exe() -> bool {
        let Ok(current_exe) = std::env::current_exe() else {
            return false;
        };
        let Some(target_dir) = current_exe.parent() else {
            return false;
        };
        let name = if cfg!(windows) {
            "scripts-build.exe"
        } else {
            "scripts-build"
        };
        let dest = target_dir.join(name);
        if dest.is_file() {
            return true;
        }
        let source = ensure_scripts_build();
        if let (Ok(cs), Ok(cd)) = (source.canonicalize(), dest.canonicalize()) {
            if cs == cd {
                return true;
            }
        }
        std::fs::copy(&source, &dest).unwrap_or_else(|e| {
            panic!(
                "scripts-build found at {} but copy to {} failed: {e}",
                source.display(),
                dest.display()
            )
        });
        true
    }

    fn ensure_scripts_build() -> PathBuf {
        fn scripts_build_binary() -> Option<PathBuf> {
            let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let name = if cfg!(windows) {
                "scripts-build.exe"
            } else {
                "scripts-build"
            };
            let mut dir: Option<&Path> = Some(manifest.as_path());
            while let Some(d) = dir {
                for profile in ["debug", "release"] {
                    let candidate = d.join("target").join(profile).join(name);
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
                dir = d.parent();
            }
            None
        }

        if let Some(path) = scripts_build_binary() {
            return path;
        }
        let status = std::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "-p",
                "postretro-script-compiler",
                "--bin",
                "scripts-build",
            ])
            .status()
            .expect("cargo build scripts-build");
        assert!(status.success(), "failed to build scripts-build");
        scripts_build_binary().expect("scripts-build should exist after build")
    }

    #[cfg(debug_assertions)]
    #[test]
    fn production_pause_menu_sdk_tree_drives_cpu_interaction_end_to_end() {
        use crate::input::{InputMode, NavIntent, PointerPos, UiFocusEngine};
        use crate::render::ui::descriptor::CaptureMode;
        use crate::render::ui::layout::Anchor;
        use crate::render::ui::modal_stack::{ModalStack, ScopeTier};
        use crate::render::ui::tree::CellValues;
        use postretro_scripting_core::data_descriptors::RegisteredUiTree;

        if !install_scripts_build_next_to_current_exe() {
            eprintln!("skipping: could not install scripts-build next to test binary");
            return;
        }

        let mut rt = test_runtime();
        let content_dev = workspace_root().join("content/dev");
        rt.run_mod_init(&content_dev)
            .expect("development TypeScript mod entry bundles and initializes");

        let pause_entry = rt
            .mod_manifest()
            .expect("dev mod manifest exists")
            .ui_trees
            .iter()
            .find(|tree| tree.name == render::ui::demo::PAUSE_MENU_NAME)
            .expect("dev mod manifest exports the pauseMenu tree")
            .clone();
        assert!(
            !pause_entry.always_on,
            "the mod pause menu is pushed-only, never always-on",
        );

        let mod_pause = pause_entry.tree.clone();
        assert_eq!(mod_pause.anchor, Anchor::Center);
        assert_eq!(mod_pause.offset, [0.0, 0.0]);
        assert_eq!(mod_pause.capture_mode, CaptureMode::Capture);
        assert_eq!(mod_pause.initial_focus.as_deref(), Some("pauseResume"));
        assert_eq!(mod_pause.accessible_name.as_deref(), Some("Pause menu"));
        assert_eq!(mod_pause.role, Some(render::ui::descriptor::Role::Group));
        assert!(widget_contains_text(&mod_pause.root, "PAUSED"));
        let render::ui::descriptor::Widget::VStack(pause_root) = &mod_pause.root else {
            panic!("pause menu root is a vstack");
        };
        assert_eq!(
            pause_root.focus.as_ref().map(|focus| focus.kind()),
            Some(render::ui::descriptor::FocusKind::Linear)
        );
        assert_eq!(
            pause_root.focus.as_ref().map(|focus| focus.wrap()),
            Some(true)
        );
        assert_eq!(
            button_action(&mod_pause.root, "pauseResume"),
            Some(render::ui::actions::CLOSE_DIALOG_ACTION),
            "Resume resolves to the reserved close action wire value",
        );
        assert_eq!(
            button_action(&mod_pause.root, "pauseExitDesktop"),
            Some(render::ui::actions::EXIT_TO_DESKTOP_ACTION),
            "Exit to Desktop resolves to the generic reserved quit action wire value",
        );

        let theme = render::ui::theme::UiTheme::engine_default();
        let mut retained = render::ui::tree::UiTree::from_descriptor(&mod_pause, &theme);
        let mut font_system = render::ui::text::build_font_system();
        let empty_slots = std::collections::HashMap::new();
        let empty_cells = CellValues::new();
        let draw = retained.build_draw_data_retained(
            [1280, 720],
            &mut font_system,
            &render::ui::tree::ImageSizes::new(),
            &empty_slots,
            &empty_cells,
            0.0,
        );
        assert_eq!(
            retained.recompute_count(),
            1,
            "retained descriptor builds layout after the mod-init context has dropped",
        );
        assert!(
            draw.texts.iter().any(|text| text.content == "RESUME"),
            "retained draw data includes the SDK-authored Resume label",
        );
        let focus_rects =
            retained.export_focus_rects(&mod_pause, [1280, 720], &empty_slots, &empty_cells);
        assert_eq!(focus_rects.initial_focus.as_deref(), Some("pauseResume"));
        let resume_rect = focus_rects
            .rects
            .iter()
            .find(|rect| rect.id == "pauseResume")
            .expect("Resume button exports a focus rect");
        assert!(
            resume_rect.rect[2] > 0.0 && resume_rect.rect[3] > 0.0,
            "focus rect proves layout produced usable hit geometry",
        );

        let fallback_path =
            workspace_root().join(render::ui::tree_asset::ui_asset_path("pauseMenu.json"));
        let fallback = render::ui::tree_asset::load_named_tree(&fallback_path)
            .expect("engine pause fallback loads");
        assert!(
            widget_contains_text(&fallback.root, "PRESS ESC OR B TO RESUME"),
            "fallback marker distinguishes the engine JSON fallback",
        );

        let mut stack = ModalStack::new();
        stack.registry_mut().register(
            render::ui::demo::PAUSE_MENU_NAME,
            fallback.clone(),
            ScopeTier::Engine,
            false,
        );
        stack.register_script_trees(vec![pause_entry.clone()], ScopeTier::Mod);
        let resolved = stack
            .tree(render::ui::demo::PAUSE_MENU_NAME)
            .expect("pauseMenu resolves through tiered registry");
        assert_eq!(
            button_action(&resolved.root, "pauseResume"),
            Some(render::ui::actions::CLOSE_DIALOG_ACTION),
            "the returned mod tree shadows the fallback marker",
        );
        assert!(
            !widget_contains_text(&resolved.root, "PRESS ESC OR B TO RESUME"),
            "shadowed mod tree does not expose the fallback marker",
        );

        stack.push_named(render::ui::demo::PAUSE_MENU_NAME, None);
        assert_eq!(stack.active_name(), Some(render::ui::demo::PAUSE_MENU_NAME));
        assert_eq!(
            stack.top_capture_mode(),
            render::ui::descriptor::CaptureMode::Capture
        );
        assert_eq!(
            stack.entries()[0].descriptor.initial_focus.as_deref(),
            Some("pauseResume"),
            "initial focus metadata reaches the modal-stack entry",
        );

        let mut keyboard_focus = UiFocusEngine::new();
        let initial = keyboard_focus.tick(
            Some(render::ui::demo::PAUSE_MENU_NAME),
            Some(&focus_rects),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(initial.focused.as_deref(), Some("pauseResume"));
        let keyboard_confirm = keyboard_focus.tick(
            Some(render::ui::demo::PAUSE_MENU_NAME),
            Some(&focus_rects),
            &[NavIntent::Confirm],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        let keyboard_action = focus_button_action(&focus_rects, &keyboard_confirm);

        let mut gamepad_focus = UiFocusEngine::new();
        gamepad_focus.tick(
            Some(render::ui::demo::PAUSE_MENU_NAME),
            Some(&focus_rects),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        let gamepad_confirm = gamepad_focus.tick(
            Some(render::ui::demo::PAUSE_MENU_NAME),
            Some(&focus_rects),
            &[NavIntent::Confirm],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        let gamepad_action = focus_button_action(&focus_rects, &gamepad_confirm);

        let click_pos = PointerPos {
            x: resume_rect.rect[0] as f64 + resume_rect.rect[2] as f64 * 0.5,
            y: resume_rect.rect[1] as f64 + resume_rect.rect[3] as f64 * 0.5,
        };
        let mut pointer_focus = UiFocusEngine::new();
        let pointer_click = pointer_focus.tick(
            Some(render::ui::demo::PAUSE_MENU_NAME),
            Some(&focus_rects),
            &[],
            None,
            &[click_pos],
            InputMode::Pointer,
            0.0,
        );
        let pointer_action = focus_button_action(&focus_rects, &pointer_click);

        assert_eq!(
            keyboard_action,
            render::ui::actions::CLOSE_DIALOG_ACTION,
            "keyboard confirm resolves the reserved Resume action",
        );
        assert_eq!(
            gamepad_action, keyboard_action,
            "gamepad confirm resolves the same Resume action",
        );
        assert_eq!(
            pointer_action, keyboard_action,
            "pointer click resolves the same Resume action",
        );

        let routed = route_ui_button_action(&keyboard_action, &mut stack);
        assert_eq!(routed, UiButtonAction::CloseDialog);
        assert!(
            stack.is_empty(),
            "ui.closeDialog pops the active pause menu before named-reaction dispatch",
        );

        stack.push_named(render::ui::demo::PAUSE_MENU_NAME, None);
        let ordinary = route_ui_button_action("resumePauseMenu", &mut stack);
        assert_eq!(
            ordinary,
            UiButtonAction::NamedReaction,
            "ordinary button action names retain named-reaction dispatch",
        );
        assert_eq!(
            stack.active_name(),
            Some(render::ui::demo::PAUSE_MENU_NAME),
            "ordinary names are not intercepted as reserved close actions",
        );

        stack.replace_script_tree_tier(Vec::<RegisteredUiTree>::new(), ScopeTier::Mod);
        assert_eq!(
            stack
                .tree(render::ui::demo::PAUSE_MENU_NAME)
                .and_then(|tree| button_action(&tree.root, "pauseResume")),
            None,
            "staged omission reveals the fallback in the registry",
        );
        assert!(
            widget_contains_text(&stack.entries()[0].descriptor.root, "PAUSED"),
            "already-open pause menu keeps its cloned descriptor",
        );
        assert_eq!(
            button_action(&stack.entries()[0].descriptor.root, "pauseResume"),
            Some(render::ui::actions::CLOSE_DIALOG_ACTION),
            "already-open menu remains stable until closed",
        );
        stack.pop();
        stack.push_named(render::ui::demo::PAUSE_MENU_NAME, None);
        assert!(
            widget_contains_text(
                &stack.entries()[0].descriptor.root,
                "PRESS ESC OR B TO RESUME"
            ),
            "reopening after staged omission resolves the engine fallback",
        );
        assert_eq!(
            button_action(&stack.entries()[0].descriptor.root, "pauseResume"),
            None,
            "fallback has no Resume button or reserved-action dependency",
        );
    }

    #[test]
    fn nav_menu_policy_opens_closes_pause_and_ignores_other_modals() {
        use crate::render::ui::descriptor::{
            Align, AnchoredTree, CaptureMode, ContainerWidget, SpacingValue, Widget,
        };
        use crate::render::ui::layout::Anchor;
        use crate::render::ui::modal_stack::{ModalStack, ScopeTier};

        fn capturing_tree() -> AnchoredTree {
            AnchoredTree {
                anchor: Anchor::Center,
                offset: [0.0, 0.0],
                root: Widget::VStack(ContainerWidget {
                    gap: SpacingValue::Literal(0.0),
                    padding: SpacingValue::Literal(0.0),
                    align: Align::Start,
                    fill: None,
                    border: None,
                    id: None,
                    focus_neighbors: Default::default(),
                    focus: None,
                    restore_on_return: false,
                    local_state: None,
                    visible_when: None,
                    role: None,
                    children: Vec::new(),
                }),
                capture_mode: CaptureMode::Capture,
                initial_focus: None,
                text_entry_target: None,
                accessible_name: None,
                role: None,
            }
        }

        let mut stack = ModalStack::new();
        stack.registry_mut().register(
            render::ui::demo::PAUSE_MENU_NAME,
            capturing_tree(),
            ScopeTier::Engine,
            false,
        );
        stack
            .registry_mut()
            .register("dialog", capturing_tree(), ScopeTier::Engine, false);

        apply_pause_menu_nav_policy(&mut stack);
        assert_eq!(
            stack.active_name(),
            Some(render::ui::demo::PAUSE_MENU_NAME),
            "nav.menu opens pauseMenu on an empty modal stack",
        );

        apply_pause_menu_nav_policy(&mut stack);
        assert!(
            stack.is_empty(),
            "nav.menu closes pauseMenu when it is the active modal",
        );

        stack.push_named("dialog", None);
        apply_pause_menu_nav_policy(&mut stack);
        assert_eq!(
            stack.active_name(),
            Some("dialog"),
            "nav.menu is ignored while another modal is active",
        );
        assert_eq!(stack.len(), 1);
    }

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
    fn dependency_reload_requests_rerun_mod_init() {
        // Dependency classification happens in ScriptRuntime; the frame loop
        // queues staged mod-init only for paths that matched that active set.
        assert!(reload_summary_requires_mod_init(ReloadSummary {
            mod_init: true,
        }));
        assert!(!reload_summary_requires_mod_init(ReloadSummary::default()));
    }

    fn staged_tree(name: &str) -> RegisteredUiTree {
        use crate::render::ui::descriptor::{
            Align, AnchoredTree, CaptureMode, ContainerWidget, SpacingValue, Widget,
        };
        use crate::render::ui::layout::Anchor;

        RegisteredUiTree {
            name: name.to_string(),
            tree: AnchoredTree {
                anchor: Anchor::TopLeft,
                offset: [0.0, 0.0],
                root: Widget::VStack(ContainerWidget {
                    gap: SpacingValue::Literal(0.0),
                    padding: SpacingValue::Literal(0.0),
                    align: Align::Start,
                    fill: None,
                    border: None,
                    id: None,
                    focus_neighbors: Default::default(),
                    focus: None,
                    restore_on_return: false,
                    local_state: None,
                    visible_when: None,
                    role: None,
                    children: Vec::new(),
                }),
                capture_mode: CaptureMode::Passthrough,
                initial_focus: None,
                text_entry_target: None,
                accessible_name: None,
                role: None,
            },
            always_on: true,
        }
    }

    fn staged_built_ui_result(generation: u64) -> StagedManifestBuildResult {
        use std::collections::HashMap;

        StagedManifestBuildResult {
            generation,
            mod_root: PathBuf::from("content/dev"),
            status: StagedManifestBuildStatus::Built(Box::new(
                postretro_scripting_core::staged_manifest::StagedManifest {
                    name: "UiCommit".to_string(),
                    entities: Vec::new(),
                    maps: Vec::new(),
                    reactions: Vec::new(),
                    crossings: Vec::new(),
                    ui_trees: vec![staged_tree("hud")],
                    theme: ModThemeTokens {
                        colors: HashMap::from([("critical".to_string(), [0.25, 0.5, 0.75, 1.0])]),
                        ..Default::default()
                    },
                    frontend: Some(Frontend {
                        menu_tree: "mainMenu".to_string(),
                        background_level: Some("backdrop".to_string()),
                        camera: MenuCamera {
                            position: [1.0, 2.0, 3.0],
                            yaw: 0.25,
                            pitch: -0.5,
                        },
                    }),
                    store_declarations: Default::default(),
                    dependency_paths: Vec::new(),
                },
            )),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn staged_ui_payload_exists_only_for_successful_current_commit() {
        let result = staged_built_ui_result(9);
        let committed = StagedManifestCommitOutcome::Committed {
            generation: 9,
            descriptor_count: 0,
            applied_actions: 0,
            dropped_missing_targets: 0,
        };
        let (trees, theme, frontend) = staged_ui_commit_payload(&result, &committed)
            .expect("successful current staged result commits UI/theme/frontend");
        assert_eq!(trees.len(), 1);
        assert_eq!(trees[0].name, "hud");
        assert_eq!(theme.colors["critical"], [0.25, 0.5, 0.75, 1.0]);
        assert_eq!(
            frontend
                .as_ref()
                .map(|frontend| frontend.menu_tree.as_str()),
            Some("mainMenu")
        );

        for outcome in [
            StagedManifestCommitOutcome::DiscardedStale {
                generation: 8,
                latest_requested: Some(9),
            },
            StagedManifestCommitOutcome::FailedBuild { generation: 9 },
            StagedManifestCommitOutcome::Rejected {
                generation: 9,
                reason: "schema rejected".to_string(),
            },
        ] {
            assert!(
                staged_ui_commit_payload(&result, &outcome).is_none(),
                "non-committed staged outcomes must preserve current UI/theme"
            );
        }
    }

    #[test]
    fn no_start_script_staged_commit_clears_mod_ui_and_theme_snapshot() {
        let result = StagedManifestBuildResult {
            generation: 10,
            mod_root: PathBuf::from("content/dev"),
            status: StagedManifestBuildStatus::NoStartScript,
            diagnostics: Vec::new(),
        };
        let outcome = StagedManifestCommitOutcome::Committed {
            generation: 10,
            descriptor_count: 0,
            applied_actions: 0,
            dropped_missing_targets: 0,
        };

        let (trees, theme, frontend) =
            staged_ui_commit_payload(&result, &outcome).expect("no-start commit is a snapshot");
        assert!(trees.is_empty());
        assert_eq!(theme, ModThemeTokens::default());
        assert_eq!(frontend, None);
    }

    // --- G1b drain-before-drop lifecycle invariant (Task 6) -----------------

    /// RAII temp mod root mirroring `runtime.rs`'s test helper: a fresh dir under
    /// `std::env::temp_dir()`, removed on drop so a panic leaks nothing.
    struct TempModRoot(std::path::PathBuf);
    impl std::ops::Deref for TempModRoot {
        type Target = std::path::Path;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl Drop for TempModRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn temp_mod_root(name: &str) -> TempModRoot {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "postretro_g1b_drain_test_{}_{}_{name}",
            std::process::id(),
            n,
        ));
        std::fs::create_dir_all(&p).unwrap();
        TempModRoot(p)
    }

    fn test_runtime() -> ScriptRuntime {
        let ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, ctx.clone());
        ScriptRuntime::new(&registry, &ScriptRuntimeConfig::default(), &ctx).unwrap()
    }

    #[test]
    fn ui_registrations_drain_after_mod_init_returns_with_no_vm_resident_then_render() {
        // Drain-before-drop, the assertable half: `run_mod_init` creates AND drops
        // the authoring VM context *within* the call (scripting.md §2/§11), then
        // stores the manifest as plain Rust. So when `run_mod_init` RETURNS, the VM
        // is already gone and the UI registrations survive as owned data on the
        // manifest. The App then drains that data into the registry (the ordering
        // `App` enforces at main.rs's mod-init handler) — provably after the VM
        // drop, because the VM cannot outlive `run_mod_init`. A frame then renders
        // the registered tree with no VM anywhere in scope.
        let dir = temp_mod_root("drain_order");
        std::fs::write(
            dir.join("start-script.js"),
            r#"
            globalThis.__postretroModManifest = {
                name: "DrainMod",
                uiTrees: [
                    { name: "banner", alwaysOn: true,
                      tree: { anchor: "top", offset: [0.0, 0.0],
                              root: { kind: "text", content: "REGISTERED", fontSize: 18.0, color: [1.0,1.0,1.0,1.0] } } },
                ],
            };
            "#,
        )
        .unwrap();

        let mut rt = test_runtime();
        rt.run_mod_init(&dir).expect("mod-init succeeds");

        // `run_mod_init` has returned: the VM it built is dropped. The manifest
        // carries the registrations as plain Rust (no live VM reference).
        let trees = {
            let manifest = rt.mod_manifest().expect("manifest present after mod-init");
            assert_eq!(manifest.ui_trees.len(), 1, "the UI tree survived as data");
            manifest.ui_trees.clone()
        };

        // Drain into the tiered registry AFTER the VM is gone — the exact ordering
        // the App's mod-init handler enforces (drain, then the VM has already
        // dropped inside run_mod_init).
        let mut stack = render::ui::modal_stack::ModalStack::new();
        stack.register_script_trees(trees, render::ui::modal_stack::ScopeTier::Mod);

        // A frame renders the registered tree with NO VM resident: resolve by name
        // and build draw data from the resolved descriptor alone.
        let resolved = stack
            .tree("banner")
            .expect("registered tree resolves by name");
        let theme = render::ui::theme::UiTheme::engine_default();
        let mut ui = render::ui::tree::UiTree::from_descriptor(resolved, &theme);
        let mut fs = render::ui::text::build_font_system();
        let data = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &render::ui::tree::ImageSizes::new(),
            &std::collections::HashMap::new(),
            &render::ui::tree::CellValues::new(),
            0.0,
        );
        assert!(
            data.texts.iter().any(|t| t.content == "REGISTERED"),
            "the registered UI renders from drained data with no VM resident",
        );
    }

    #[test]
    fn malformed_theme_token_is_skipped_and_mod_init_still_succeeds() {
        // A structurally-broken `theme` token (a color that is not a [r,g,b,a]
        // tuple) is logged and skipped per-token (`ui.md` §5) rather than
        // aborting the mod — consistent with the `uiTrees` per-entry skip and the
        // Luau theme twin. The mod still loads: its name and any valid sibling
        // token survive; only the malformed token is degraded out. Boot never
        // aborts and never panics.
        let dir = temp_mod_root("bad_theme");
        std::fs::write(
            dir.join("start-script.js"),
            r#"
            globalThis.__postretroModManifest = {
                name: "BadThemeMod",
                theme: { colors: { critical: "not-an-rgba-array", ok: [1, 0, 0, 1] } },
            };
            "#,
        )
        .unwrap();

        let mut rt = test_runtime();
        rt.run_mod_init(&dir)
            .expect("a wrong-type theme token is skipped, not a fatal error");
        let manifest = rt
            .mod_manifest()
            .expect("the manifest still drains despite the bad token");
        // The rest of the manifest still drains.
        assert_eq!(manifest.name, "BadThemeMod");
        // The malformed token is degraded out; the valid sibling token survives.
        assert!(
            !manifest.theme.colors.contains_key("critical"),
            "the malformed `critical` color token should be skipped",
        );
        assert!(
            manifest.theme.colors.contains_key("ok"),
            "the valid `ok` color token should still drain",
        );
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

        for code in [
            KeyCode::Backslash,
            KeyCode::Digit1,
            KeyCode::KeyV,
            KeyCode::KeyP,
            KeyCode::KeyN,
            KeyCode::KeyL,
            KeyCode::KeyG,
        ] {
            let blocked = consumed_gate(&mut diagnostics, code, true, false);
            assert_eq!(
                blocked, None,
                "consumed-event gate must suppress non-toggle diagnostic chord {code:?}",
            );
        }

        assert_eq!(
            consumed_gate(&mut diagnostics, KeyCode::Backquote, true, false),
            Some(DiagnosticAction::ToggleDebugPanel),
            "consumed-event gate must allow ToggleDebugPanel through",
        );
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn drawable_visible_cell_mask_derives_only_from_drawable_visible_cells() {
        assert_eq!(
            drawable_visible_cell_mask(4, &VisibleCells::Culled(vec![1, 3, 99])),
            Some(vec![false, true, false, true]),
        );
        // DrawAll is an all-visible sentinel. The BVH overlay interprets the
        // absent mask as unfiltered/all-visible when visible-cells-only is on.
        assert_eq!(drawable_visible_cell_mask(4, &VisibleCells::DrawAll), None);
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

    fn spawn_mesh_entity(registry: &mut postretro_entities::EntityRegistry, model: &str) {
        use postretro_entities::Transform;
        use postretro_entities::components::mesh::MeshComponent;

        let id = registry.spawn(Transform::default());
        registry
            .set_component(id, MeshComponent::stateless(model.to_string()))
            .expect("freshly spawned id is live");
    }

    #[test]
    fn distinct_mesh_models_dedups_repeated_handles() {
        use postretro_entities::EntityRegistry;

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
        use postretro_entities::EntityRegistry;

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
        use postretro_entities::EntityRegistry;

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
        use postretro_entities::components::mesh::{
            AnimationState, DEFAULT_CROSSFADE_MS, InterruptPolicy, MeshAnimation, MeshComponent,
        };
        use postretro_entities::{EntityRegistry, Transform};
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
                    origin_offset: glam::Vec3::ZERO,
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
    fn resolve_after_remote_enemy_materialization_uses_declared_default_clip_not_first_clip() {
        use postretro_entities::components::mesh::{
            AnimationState, DEFAULT_CROSSFADE_MS, InterruptPolicy, MeshComponent,
        };
        use postretro_entities::{EntityRegistry, Transform};
        use postretro_entities::{EntityTypeDescriptor, MeshDescriptor};
        use std::collections::HashMap;

        let unresolved = |clip: &str, looping| AnimationState {
            clip: clip.into(),
            looping,
            crossfade_ms: DEFAULT_CROSSFADE_MS,
            interrupt: InterruptPolicy::Smooth,
            clip_index: None,
        };
        let mut states = HashMap::new();
        states.insert("idle".to_string(), unresolved("Idle", true));
        states.insert("attack".to_string(), unresolved("Attack", false));

        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("remote_enemy".to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            mesh: Some(MeshDescriptor {
                model: "models/remote_enemy/scene.gltf".to_string(),
                animations: states,
                default_state: Some("idle".to_string()),
            }),
            health: None,
            ai: None,
        }];

        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        crate::scripting::builtins::net_descriptor::materialize_net_remote_enemy_presentation(
            "remote_enemy",
            &descriptors,
            &mut registry,
            id,
            None,
        );

        let mut tables = scripting_systems::mesh_anim::MeshClipTables::new();
        let meta = vec![
            crate::render::mesh_pass::ClipMetadata {
                name: "Attack".to_string(),
                duration: 0.8,
            },
            crate::render::mesh_pass::ClipMetadata {
                name: "Idle".to_string(),
                duration: 2.0,
            },
        ];
        tables.insert(
            crate::model::ModelHandle::from("models/remote_enemy/scene.gltf"),
            &meta,
        );

        resolve_mesh_entity_clips(&mut registry, &tables);

        let component = registry
            .get_component::<MeshComponent>(id)
            .expect("remote presentation mesh attached");
        let anim = component
            .animation
            .as_ref()
            .expect("animation block present");
        assert_eq!(anim.current_state, "idle");
        assert_eq!(anim.states.get("idle").unwrap().clip_index, Some(1));
        assert_eq!(anim.states.get("attack").unwrap().clip_index, Some(0));
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
        use postretro_entities::SlotValue;

        // The default table carries engine `player.*` slots with `None` values
        // plus two value-bearing engine surfaces: `screen.flash` (resting
        // transparent) and `input.mode` (defaults to `focus`). Setting one of the
        // value-less slots asserts the boundary contract: the snapshot clones
        // value-bearing slots and omits value-less ones.
        let mut table = postretro_entities::SlotTable::new();
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
        // `screen.vignette`/`screen.shake` default to zeroed arrays, so they are
        // value-bearing and present (the screen-effects resolve reads them).
        assert_eq!(
            snapshot.get("screen.vignette"),
            Some(&SlotValue::Array(vec![0.0, 0.0, 0.0, 0.0])),
            "engine-owned screen.vignette defaults to zeroed rgba and is cloned",
        );
        assert_eq!(
            snapshot.get("screen.shake"),
            Some(&SlotValue::Array(vec![0.0, 0.0])),
            "engine-owned screen.shake defaults to zero offset and is cloned",
        );
        assert!(
            !snapshot.contains_key("player.maxHealth"),
            "value-less slots are skipped",
        );
        assert_eq!(
            snapshot.len(),
            6,
            "only the set player.health and the default-valued screen.flash + screen.vignette + screen.shake + input.mode + ui.textEntry appear",
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

    // --- CellWrite dispatch: presentation cell written, slot table untouched ---
    //
    // G1b AC #6: a `localState` `.set()` (the `CellWrite` system-reaction command,
    // drained by `App::dispatch_system_commands`) must write into the
    // `PresentationCellStore` but leave the authoritative slot table (`SlotTable`)
    // completely untouched.
    //
    // `App` cannot be constructed headlessly (it needs a window and GPU; see
    // context/lib/testing_guide.md §3). The test therefore exercises the
    // two-component seam that the `CellWrite` arm of `dispatch_system_commands`
    // exercises directly:
    //   1. `scripting_systems::presentation_cells::json_to_cell_value` — coerces
    //      the raw JSON value (identical to how the drain does it).
    //   2. `PresentationCellStore::write` — the only mutation the drain performs.
    //   3. `SlotTable` — checked for the absence of any matching entry, proving the
    //      drain never touches the authoritative store.
    // This mirrors the production `CellWrite` arm exactly: that arm calls nothing
    // else.

    #[test]
    fn cell_write_dispatch_writes_presentation_cell_and_leaves_slot_table_untouched() {
        use postretro_entities::SlotTable;
        use scripting_systems::presentation_cells::{PresentationCellStore, json_to_cell_value};

        let scope = "counter".to_string();
        let cell = "count".to_string();
        // The raw JSON value as it arrives from the `CellWrite` command.
        let raw_value = serde_json::Value::Number(serde_json::Number::from(42));

        // --- Drain path: mirror `App::dispatch_system_commands` CellWrite arm ---
        let mut presentation_cells = PresentationCellStore::new();
        let slot_table = SlotTable::new();

        let cell_value = json_to_cell_value(&raw_value)
            .expect("a numeric JSON value must coerce to a SlotValue");
        presentation_cells.write(scope.clone(), cell.clone(), cell_value);

        // --- AC assertion 1: presentation cell now holds the written value ---
        let snapshot = presentation_cells.snapshot();
        assert_eq!(
            snapshot.get(&(scope.clone(), cell.clone())),
            Some(&postretro_entities::SlotValue::Number(42.0)),
            "CellWrite must land in the presentation cell store",
        );

        // --- AC assertion 2: authoritative slot table has NO corresponding entry ---
        // The slot table is keyed by dotted `namespace.slot` names (never by
        // `(scope, cell)` pairs). We verify that no slot with a name that could
        // encode the written cell exists beyond the built-in engine-declared slots —
        // and that the built-in engine slots carry no value for `counter.count`.
        assert!(
            slot_table.get("counter.count").is_none(),
            "CellWrite must NOT create a slot-table entry for the written cell",
        );
        // Cross-check: the default slot table carries its built-in engine slots
        // (player.*, screen.*, input.*, ui.*) but nothing under the `counter`
        // namespace written above.
        assert!(
            slot_table
                .iter()
                .all(|(name, _)| !name.starts_with("counter.")),
            "slot table must have no entries under the `counter` namespace after a CellWrite",
        );
    }
}
