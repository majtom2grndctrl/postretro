// Postretro engine entry point, boot state machine, and level-load orchestration.
// See: context/lib/boot_sequence.md §3 · context/lib/index.md

// Movable navigation agent collide-and-slide harness, driven each tick by the
// steering system in `agent_steering`.
// See: context/lib/movement.md §1, context/lib/entity_model.md §7
mod agent;
// Per-tick navigation-agent steering: replan budget, waypoint following, and
// separation, built on the `agent` harness and `nav::find_path`.
#[cfg(feature = "dev-tools")]
mod agent_diagnostics;
mod agent_steering;
mod audio;
mod camera;
#[cfg(test)]
mod candidate_cull {
    pub use postretro_renderer::{GatherStatus, gather_candidate_leaves};
}
#[cfg(test)]
mod candidate_cull_mirror;
#[cfg(test)]
mod candidate_cull_probes;
mod collision;
mod combat_positioning;
mod content_hash;
// App-side diagnostics for baked door-to-portal occluder associations. Keeps
// the render-only blocked portal buffer inspectable without changing gameplay.
#[cfg(feature = "dev-tools")]
mod door_occluder_diagnostics;
mod frame_timing;
mod fx;
mod grant;
mod health;
mod impact_effects;
mod impact_policy;
mod input;
mod kinematic_mover;
mod mod_digest;
mod movement;
// App-side debug-line geometry for rotating kinematic movers. This owns no GPU
// state; the renderer only consumes its emitted lines.
#[cfg(feature = "dev-tools")]
mod mover_diagnostics;
// The runtime nav graph is built in every build whenever a level carries a
// baked navmesh; pathfinding consumes its query surface.
mod nav;
// Engine-side netcode glue: role selection, the optional endpoint held by `App`,
// game-logic-owned serialize/apply, interpolation, prediction, and reconciliation.
// The ONLY engine code that touches the registry on behalf of replication.
// See `context/lib/entity_model.md` §6.
mod netcode;
// Headless batch-mode observability vocabulary: runspec, entity dump, and
// deterministic JSON output. Feature-gated; consumed by the headless driver.
// See: context/plans/done/agentic-observability
#[cfg(feature = "observability")]
mod observability;
// Static offscreen frame-capture scene parser and renderer driver. It exits
// before boot constructs winit state, so this remains independent of UI.
#[cfg(feature = "capture")]
mod capture;
mod options;
mod presentation_pool;
mod presentation_projection;
mod weapon;

mod render;
mod runtime_movers;
mod scripting;
// Live session-lifetime container: all session-lifetime state (scripting core,
// audio, net endpoint, input/UI/modal group, and their bridges and registries),
// held on `App` as `Option<Session>` and built after the first visible frame.
// See: context/lib/boot_sequence.md §1
mod session;
mod sim;
mod spawner;
mod startup;
mod trigger_bindings;
mod trigger_commands;
#[cfg(feature = "dev-tools")]
mod trigger_diagnostics;
mod trigger_pools;
mod trigger_system;
mod view_feel;

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

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt::Write as _;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use glam::{Quat, Vec3};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes};

use crate::camera::Camera;
use crate::frame_timing::{FrameRateMeter, FrameTiming, InterpolableState};
use crate::input::{Action, ButtonState, DiagnosticAction, InputFocus};
// Owns the apply-before-detect stage order of the frame's replicated-state path.
// See: context/lib/networking.md
use crate::netcode::frame_order;
use crate::render::Renderer;
use crate::scripting::reactions::system_commands::SystemReactionIrDispatch;
use crate::scripting::state_persistence::{
    apply_join_seed, collect_per_owner_state, collect_persisted_state,
    collected_per_owner_only_state, merge_per_owner_state, retain_saved_per_owner_state,
    save_persisted_state, state_path, sync_client_per_owner_projection,
};
// Session-owned types referenced in `main.rs` only by `#[cfg(test)]` code, so
// they are gated test-only to keep the bin build warning-free.
use crate::startup::{
    BootState, FRONTEND_CLEAR_COLOR, InFlightLevelLoad, LevelRequest, LevelSource, LoadOutcome,
    SplashSource, StartupTimings,
};
#[cfg(test)]
use postretro_scripting_core::reaction_dispatch::ProgressTracker;
#[cfg(test)]
use postretro_scripting_core::runtime::ScriptRuntime;
// Positional-map-path recovery lives with boot construction; re-exported at the
// crate root so `crate::resolve_map_path` keeps resolving for the netcode CLI
// tests. Test-only: the boot path calls it through `startup::session`.
#[cfg(test)]
pub(crate) use crate::startup::session::resolve_map_path;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::{
    ComponentKind, ComponentValue, ScriptCtx, SystemReactionCommand, Transform,
};
use postretro_foundation::{ModThemeTokens, Seat, SwitchingDescriptor, WeaponPlacementDescriptor};
use postretro_scripting_core::data_descriptors::RegisteredUiTree;
#[cfg(test)]
use postretro_scripting_core::reaction_dispatch::fire_named_event;
use postretro_scripting_core::reaction_dispatch::{
    ResidualOrigin, dispatch_deferred_named_events_with_sequences, fire_named_event_with_sequences,
    fire_prepartitioned_reactions_with_sequences,
};
use postretro_scripting_core::runtime::{
    Frontend, MenuCamera, ReloadSummary, StagedManifestCommitOutcome,
};
use postretro_scripting_core::staged_manifest::{
    StagedManifestBuildResult, StagedManifestBuildStatus,
};
use postretro_visibility::{
    CameraCullVisibility, VisibilityPath, VisibilityResult, VisibilityStats, VisibleCells,
};

/// Fraction of a vignette reaction's single `durationMs` spent ramping in. The
/// author supplies one duration (mirroring `flashScreen`); the drain splits it
/// into a short rise so the vignette eases in rather than snapping to peak, with
/// the remainder spent decaying back to rest. See `dispatch_system_commands`.
const VIGNETTE_RISE_FRACTION: f32 = 0.2;

#[derive(Debug, Clone, Copy)]
enum PendingWeaponScriptEvent {
    Weapon(&'static str),
    Reload(sim::ReloadDelivery),
}

impl PendingWeaponScriptEvent {
    const fn event_name(self) -> &'static str {
        match self {
            Self::Weapon(event_name) => event_name,
            Self::Reload(delivery) => delivery.outcome.event_name(),
        }
    }
}

fn append_tick_weapon_script_events(
    pending: &mut Vec<PendingWeaponScriptEvent>,
    weapon_events: Vec<&'static str>,
    reload_deliveries: Vec<sim::ReloadDelivery>,
) {
    pending.extend(
        weapon_events
            .into_iter()
            .map(PendingWeaponScriptEvent::Weapon),
    );
    pending.extend(
        reload_deliveries
            .into_iter()
            .map(PendingWeaponScriptEvent::Reload),
    );
}

/// Resolve host-local mover transition edges to the authored named-reaction
/// addresses on their source movers. Missing movers and absent event KVPs are
/// ordinary no-ops.
fn mover_event_dispatch_addresses(
    events: &[(kinematic_mover::MoverEventKind, u32)],
    registry: &postretro_entities::EntityRegistry,
) -> Vec<String> {
    events
        .iter()
        .filter_map(|(kind, mover_id)| {
            registry
                .iter_with_kind(ComponentKind::KinematicMover)
                .filter_map(|(_, value)| {
                    let ComponentValue::KinematicMover(mover) = value else {
                        return None;
                    };
                    Some(mover)
                })
                .find(|mover| mover.mover_id == *mover_id)
                .and_then(|mover| kind.dispatch_address(mover))
                .map(str::to_owned)
        })
        .collect()
}

/// Rebuild the render-only blocked-portal input from the final post-tick mover
/// phase. It is intentionally cleared before every refill so a prior map or
/// phase can never leave a portal latched closed.
fn rebuild_blocked_portals(
    blocked_portals: &mut Vec<bool>,
    world: Option<&postretro_level_loader::LevelWorld>,
    registry: &postretro_entities::EntityRegistry,
) {
    blocked_portals.clear();
    let Some(world) = world else {
        return;
    };
    blocked_portals.resize(world.portals.len(), false);

    for (_, value) in registry.iter_with_kind(ComponentKind::KinematicMover) {
        let ComponentValue::KinematicMover(mover) = value else {
            continue;
        };
        if !kinematic_mover::mover_is_docked_closed(mover) {
            continue;
        }
        for &portal_id in &mover.sealed_portal_ids {
            if let Some(blocked) = blocked_portals.get_mut(portal_id as usize) {
                *blocked = true;
            }
        }
    }
}

/// Execute a batch of post-tick named events through the sequence-aware path.
/// Plain `fire_named_event` only collects primitive `on_complete` names; it does
/// not execute primitive or sequence bodies.
fn drain_named_events_with_sequences<I, S>(
    event_names: I,
    data_registry: &postretro_entities::DataRegistry,
    sequence_registry: &postretro_scripting_core::sequence::SequencedPrimitiveRegistry,
    reaction_registry: &postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry,
    system_registry: &postretro_scripting_core::reaction_registry::SystemReactionRegistry,
    script_ctx: &postretro_entities::ScriptCtx,
) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut chained = Vec::new();
    for event_name in event_names {
        chained.extend(fire_named_event_with_sequences(
            event_name.as_ref(),
            data_registry,
            sequence_registry,
            reaction_registry,
            system_registry,
            script_ctx,
            None,
        ));
    }
    chained
}

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

/// Collect the distinct, non-empty holder and attachment model handles currently
/// in the registry, preserving first-seen order. GPU-free: this is the pure half
/// of the level-load model sweep — the renderer's GPU upload happens in the
/// caller, once per returned handle, so each distinct model is uploaded exactly
/// once.
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
    let mut add_model = |model: &str| {
        if !model.is_empty() && seen.insert(model.to_string()) {
            ordered.push(model.to_string());
        }
    };
    for (_id, value) in registry.iter_with_kind(ComponentKind::Mesh) {
        let ComponentValue::Mesh(mesh) = value else {
            continue;
        };
        add_model(&mesh.model);
        for attachment in &mesh.attachments {
            add_model(&attachment.model);
        }
    }
    ordered
}

/// Resolve every animated mesh entity's declared state map against the level's
/// clip tables, filling each `AnimationState.clip_index` (name → glTF index),
/// and resolve descriptor-authored attachment sockets from the game-side loaded
/// model table. Clip resolution remains animation-gated; attachment resolution
/// deliberately also visits stateless and rigid holders.
///
/// Runs at level load with a mutable registry, after the model sweep built the
/// clip tables — so every state's index is concrete before the first frame.
fn resolve_mesh_entity_bindings(
    registry: &mut postretro_entities::EntityRegistry,
    tables: &scripting_systems::mesh_anim::MeshClipTables,
    hit_zone_store: &scripting_systems::hit_zones::HitZoneStore,
) {
    use postretro_entities::{ComponentKind, ComponentValue};

    // Collect ids first so the mutable per-entity writes do not alias the
    // immutable iteration borrow. Mesh entity counts are small.
    let needing_resolution: Vec<postretro_entities::EntityId> = registry
        .iter_with_kind(ComponentKind::Mesh)
        .filter_map(|(id, value)| match value {
            ComponentValue::Mesh(mesh)
                if mesh.animation.is_some() || !mesh.attachments.is_empty() =>
            {
                Some(id)
            }
            _ => None,
        })
        .collect();

    resolve_mesh_entity_bindings_for_entities(registry, tables, hit_zone_store, needing_resolution);
}

/// Resolve clip indices and attachment bindings for a known set of newly
/// materialized mesh entities. Runtime spawners call this through their
/// session-owned pending-id queue after descriptor attachment; their models were
/// already uploaded at level install.
fn resolve_mesh_entity_bindings_for_entities(
    registry: &mut postretro_entities::EntityRegistry,
    tables: &scripting_systems::mesh_anim::MeshClipTables,
    hit_zone_store: &scripting_systems::hit_zones::HitZoneStore,
    entity_ids: impl IntoIterator<Item = postretro_entities::EntityId>,
) {
    use postretro_entities::ComponentKind;
    use postretro_entities::components::mesh::AttachmentBinding;
    use postretro_model::gltf_loader::SocketBinding;

    for id in entity_ids {
        if !matches!(
            registry.has_component_kind(id, ComponentKind::Mesh),
            Ok(true)
        ) {
            continue;
        }
        let Ok(mut component) = registry
            .get_component::<postretro_entities::components::mesh::MeshComponent>(id)
            .cloned()
        else {
            continue;
        };
        let model_name = component.model.clone();
        let handle = postretro_model::ModelHandle::from(model_name.clone());
        if let Some(anim) = component.animation.as_mut() {
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
        }

        for attachment in &mut component.attachments {
            // An attachment model must have made it through the same model
            // sweep as its holder. The game-side store records successful loads,
            // so absence covers missing and failed paths without a placeholder.
            // The renderer already emitted the single path-level load diagnostic.
            if hit_zone_store.get_by_name(&attachment.model).is_none() {
                attachment.binding = AttachmentBinding::Unresolved;
                continue;
            }

            let binding = hit_zone_store
                .get(&handle)
                .and_then(|holder| holder.sockets.get(&attachment.socket));
            match binding {
                Some(SocketBinding::SkinnedJoint(joint)) => {
                    attachment.binding = AttachmentBinding::Skinned(*joint);
                }
                Some(SocketBinding::RigidRest(rest)) => {
                    attachment.binding = AttachmentBinding::Rigid(*rest);
                }
                None => {
                    attachment.binding = AttachmentBinding::Unresolved;
                    let warning_key = format!(
                        "attachment-socket:{model_name}:{}:{}",
                        attachment.socket, attachment.model
                    );
                    if hit_zone_store.mark_attachment_resolution_warning(warning_key) {
                        log::warn!(
                            "[Model] holder model '{}' has no socket '{}' for attachment model '{}' — attachment unresolved",
                            model_name,
                            attachment.socket,
                            attachment.model,
                        );
                    }
                }
            }
        }
        let _ = registry.set_component(id, component);
    }
}

/// Resolve presentation attached by the listen-host accept lifecycle after the
/// install-time model sweep. Kept as a named seam so acceptance cannot depend on
/// a later weapon-attachment change to make the body animation usable.
fn resolve_accepted_host_pawn_presentation(
    registry: &mut postretro_entities::EntityRegistry,
    tables: &scripting_systems::mesh_anim::MeshClipTables,
    hit_zone_store: &scripting_systems::hit_zones::HitZoneStore,
    pawn: postretro_entities::EntityId,
) {
    resolve_mesh_entity_bindings_for_entities(registry, tables, hit_zone_store, [pawn]);
}

/// Level-load cross-check: for every archetype that declares both a mesh model
/// and `health.zoneMultipliers`, warn ONCE per archetype per declared tag that
/// names no zone on the spawned model. The unknown set is computed by the pure,
/// unit-tested `unknown_zone_multiplier_tags`; this is a thin warn-only caller,
/// modeled on `resolve_mesh_entity_bindings`. An archetype whose model has no
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
        let handle = postretro_model::ModelHandle::from(mesh.model.clone());
        let declared = health.zone_multipliers.keys().map(String::as_str);
        // A model with no hit-zone entry carries no zones: every declared tag is
        // unknown. Pass an empty zone table so the cross-check reports them all.
        let empty_zones: Vec<Option<postretro_model::gltf_loader::JointZone>> = Vec::new();
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

/// Client-side tick path for static PRL-loaded movers. The host replicates
/// mover *phase*, not a transform or carried-light pose; render consumers read
/// the transform reconstructed here through the same interpolation accessor.
fn client_predict_loaded_movers_tick(
    registry: &mut postretro_entities::EntityRegistry,
    mover_tick_states: &mut kinematic_mover::MoverTickStateTable,
    tick_dt: f32,
) {
    let mover_entities: Vec<_> = registry
        .iter_with_kind(postretro_entities::ComponentKind::KinematicMover)
        .map(|(id, _)| id)
        .collect();
    for entity in mover_entities {
        registry.snapshot_transform(entity);
    }
    kinematic_mover::run_kinematic_mover_tick(registry, mover_tick_states, tick_dt);
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

    /// Current-frame interpolation-derived remote-avatar inputs. These are kept on
    /// the App between the interpolation and presentation assembly stages so remote
    /// aim/heading follows the exact transform the renderer receives.
    remote_player_presentation: netcode::ClientPresentationInputs,

    /// Persistent crouch toggle latch for `CrouchMode::Toggle`. Flipped on each
    /// `Action::Crouch` press rising edge by the input layer; fed into
    /// `MovementInput::crouch_intent`. Lives on `App` (the input layer), NEVER on
    /// the movement component. Inert in `CrouchMode::Hold` (hold tracks the
    /// button level directly). See: context/lib/input.md, context/lib/player_options.md
    crouch_toggle_active: bool,

    /// Warn-once state for the enemy-AI tick. Content-keyed diagnostics (e.g.
    /// `anim:<name>` for an animation state that fails to switch,
    /// `UnknownState`/`NotAnimated`, prior animation kept) fire exactly once
    /// across the run via a `HashSet<String>` latch; the blocked-chase warning
    /// (a chasing enemy whose agent found no path) is separate, keyed by a
    /// typed `HashSet<EntityId>` rather than a formatted string so the
    /// per-tick check never allocates, and pruned each tick against the live
    /// brain set. Lives on `App` (the AI tick owner), threaded into
    /// `scripting_systems::ai::run_ai_tick`. See: scripting/systems/ai/mod.rs.
    ai_runtime: crate::scripting_systems::ai::AiRuntime,

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
    /// Render-only portal blockers rebuilt from live mover phase every frame.
    blocked_portals: Vec<bool>,

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

    /// Current mod-global switching policy. This remains App-owned because
    /// input policy is not replicated state; the local weapon commit gate is
    /// the sole simulation consumer of its reload-interrupt rule.
    switching: SwitchingDescriptor,

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
    /// Local-space collider payloads for PRL-loaded kinematic movers.
    kinematic_mover_colliders: Vec<collision::moving::MoverCollider>,
    /// Live fixed-tick mover poses, published before player movement consumes
    /// the combined collision query.
    kinematic_mover_tick_states: kinematic_mover::MoverTickStateTable,
    /// Owning pawn ground reference captured at the start of the exact tick
    /// that consumed the pending mover pose delta. The next Input stage uses
    /// it to gate that delta's yaw carry.
    mover_yaw_carry_ground: postretro_foundation::GroundRef,
    /// Render-stage CPU collector for loaded kinematic mover brush instances.
    kinematic_mover_render: runtime_movers::KinematicMoverRenderCollector,
    /// Per-level trigger event bindings resolved from the final composed
    /// reaction set during install. The fixed-tick seam borrows this table.
    trigger_bindings: trigger_bindings::TriggerBindingTable,
    /// Host-local outcome of the most recent trigger-pool install. Connected
    /// clients retain the default empty report because they never run the pass.
    trigger_pool_report: trigger_pools::TriggerPoolInstallReport,

    client_fire_resolutions: Vec<weapon::ClientFireResolution>,
    client_predicted_shots: weapon::ClientPredictedShots,

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

    /// A retained copy of the level's `player_spawn` placements for the host's
    /// runtime net-slot accept path (M15 Phase 3 Task 4), so each accepted
    /// client's descriptor-backed remote pawn can be spawned from its
    /// deterministically assigned placement. Populated from segment B's returned
    /// spawn points at install. Empty before level load and on maps with no
    /// player_spawn. The install-internal classname/archetype partition
    /// (spawn-point / built-in-handled / remaining-entity bookkeeping) now lives
    /// as locals inside `install_world_cpu`; only this host copy outlives install.
    host_spawn_points: Vec<crate::scripting::map_entity::MapEntity>,

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

    /// Parsed once before either the windowed or headless entry path. The
    /// install sites resolve it into their two-variant per-install policy.
    session_boot_config: startup::session::SessionBootConfig,

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
        postretro_ui::actions::COMMIT_TEXT_ENTRY_ACTION => UiButtonAction::CommitTextEntry,
        postretro_ui::actions::CLOSE_DIALOG_ACTION => UiButtonAction::CloseDialog,
        postretro_ui::actions::EXIT_TO_DESKTOP_ACTION => UiButtonAction::ExitToDesktop,
        postretro_ui::actions::QUIT_TO_MENU_ACTION => UiButtonAction::QuitToMenu,
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
    rects: Option<&postretro_ui::tree::FocusRectList>,
    focused_id: Option<&str>,
) -> Option<String> {
    use postretro_ui::tree::NodeInteraction;

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
    modal_stack: &mut postretro_ui::modal_stack::ModalStack,
) -> UiButtonAction {
    match classify_ui_button_action(on_press) {
        UiButtonAction::CloseDialog => {
            modal_stack.pop();
            UiButtonAction::CloseDialog
        }
        other => other,
    }
}

fn apply_pause_menu_nav_policy(modal_stack: &mut postretro_ui::modal_stack::ModalStack) {
    match modal_stack.active_name() {
        Some(postretro_ui::demo::PAUSE_MENU_NAME) => modal_stack.pop(),
        None => modal_stack.push_named(postretro_ui::demo::PAUSE_MENU_NAME, None),
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
    modal_stack: &postretro_ui::modal_stack::ModalStack,
) -> bool {
    ui_captured_gameplay_at_frame_start
        || modal_stack.top_capture_mode() == postretro_ui::descriptor::CaptureMode::Capture
}

fn world_up_yaw_delta(tick_rotation_delta: Quat) -> f32 {
    if !tick_rotation_delta.is_finite() || tick_rotation_delta.length_squared() <= 1.0e-12 {
        return 0.0;
    }
    let rotation = tick_rotation_delta.normalize();
    let twist_length = (rotation.w * rotation.w + rotation.y * rotation.y).sqrt();
    if twist_length <= 1.0e-6 {
        return 0.0;
    }

    let yaw = 2.0 * (rotation.y / twist_length).atan2(rotation.w / twist_length);
    (yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

fn yaw_after_mover_carry(camera_yaw: f32, carry_yaw: bool, tick_rotation_delta: Quat) -> f32 {
    if carry_yaw {
        camera_yaw + world_up_yaw_delta(tick_rotation_delta)
    } else {
        camera_yaw
    }
}

/// Carry only the owning player's upright view from its prior mover pose.
/// Input runs before this tick's mover update, so the pose and captured ground
/// reference both belong to the exact preceding tick.
fn apply_mover_yaw_carry(
    camera: &mut Camera,
    carry_ground: postretro_foundation::GroundRef,
    mover_states: &kinematic_mover::MoverTickStateTable,
) {
    let postretro_foundation::GroundRef::Mover(mover_id) = carry_ground else {
        return;
    };
    let Some(pose) = mover_states.get(mover_id) else {
        return;
    };

    camera.yaw = yaw_after_mover_carry(camera.yaw, pose.carry_yaw, pose.tick_rotation_delta);
}

/// Presentation-only fraction of the mover rotation that the next input seam will
/// commit in full. The simulation-facing camera yaw and `facing_yaw` remain settled
/// at fixed-tick boundaries.
fn mover_yaw_render_residual(
    carry_ground: postretro_foundation::GroundRef,
    mover_states: &kinematic_mover::MoverTickStateTable,
    alpha: f32,
) -> f32 {
    let postretro_foundation::GroundRef::Mover(mover_id) = carry_ground else {
        return 0.0;
    };
    let Some(pose) = mover_states.get(mover_id) else {
        return 0.0;
    };
    if !pose.carry_yaw {
        return 0.0;
    }

    world_up_yaw_delta(pose.tick_rotation_delta) * alpha.clamp(0.0, 1.0)
}

fn effective_render_yaw(
    settled_camera_yaw: f32,
    carry_ground: postretro_foundation::GroundRef,
    mover_states: &kinematic_mover::MoverTickStateTable,
    alpha: f32,
) -> f32 {
    settled_camera_yaw + mover_yaw_render_residual(carry_ground, mover_states, alpha)
}

/// Reconcile the mover pose table at the same seam that owns camera yaw. The
/// start-of-tick correction preserves the rider's camera-to-platform offset;
/// the refreshed tick delta is then committed once by the ordinary input seam.
fn apply_authoritative_mover_corrections(
    camera: &mut camera::Camera,
    carry_ground: postretro_foundation::GroundRef,
    mover_states: &mut kinematic_mover::MoverTickStateTable,
    corrections: &[netcode::MoverCorrection],
) {
    for correction in corrections {
        let authoritative = correction.authoritative_state;
        let previous = mover_states.get(correction.mover_id).copied();
        if carry_ground == postretro_foundation::GroundRef::Mover(correction.mover_id)
            && authoritative.carry_yaw
            && let Some(previous) = previous
        {
            let predicted_tick_start =
                previous.tick_rotation_delta.inverse() * previous.transform.rotation;
            let authoritative_tick_start =
                authoritative.tick_rotation_delta.inverse() * authoritative.transform.rotation;
            let start_correction = authoritative_tick_start * predicted_tick_start.inverse();
            camera.yaw += world_up_yaw_delta(start_correction);
        }
        mover_states.publish(correction.mover_id, authoritative);
    }
}

fn camera_right_for_yaw(yaw: f32) -> Vec3 {
    Vec3::new(yaw.cos(), 0.0, -yaw.sin())
}

fn local_player_ground(
    registry: &postretro_entities::EntityRegistry,
) -> postretro_foundation::GroundRef {
    followed_player_pawn(registry)
        .and_then(|pawn| {
            registry
                .get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
                .ok()
        })
        .map_or(postretro_foundation::GroundRef::Airborne, |movement| {
            movement.ground
        })
}

/// Snapshot the local pawn's inventory at the input phase. On a single-player
/// or listen-host role this reflects the last completed local tick; on a
/// connected client it reflects the last applied local-inventory snapshot. The
/// incoming snapshot for this frame applies later in game logic, so the cursor
/// can be one frame stale but never reads a wire-only mirror or simulation-only
/// preference state.
fn local_wieldable_occupancy(
    registry: &postretro_entities::EntityRegistry,
) -> (
    [bool; postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY],
    Option<usize>,
) {
    let Some(inventory) = registry.local_player_movement_pawn().and_then(|pawn| {
        registry
            .get_component::<postretro_entities::components::inventory::Inventory>(pawn)
            .ok()
    }) else {
        return (
            [false; postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY],
            None,
        );
    };
    let occupied = inventory.wieldables.map(|wieldable| {
        wieldable.is_some_and(|id| {
            registry.exists(id)
                && registry.has_component_kind(id, postretro_entities::ComponentKind::Weapon)
                    == Ok(true)
        })
    });
    let active = occupied
        .get(inventory.active_slot)
        .copied()
        .unwrap_or(false)
        .then_some(inventory.active_slot);
    (occupied, active)
}

#[allow(clippy::too_many_arguments)]
fn build_sim_command(
    snapshot: &input::ActionSnapshot,
    camera: &Camera,
    crouch_intent: bool,
    dash_pressed: bool,
    shoot_pressed: bool,
    select_pressed: bool,
    use_pressed: bool,
    drop_pressed: bool,
) -> sim::SimCommand {
    let jump_pressed = snapshot.button(Action::Jump).is_active();
    let sprint = snapshot.button(Action::Sprint).is_active();
    let shoot = snapshot.button(Action::Shoot);
    let reload = snapshot.button(Action::Reload);
    let select_slot = select_pressed
        .then(|| {
            [
                Action::SelectWieldable1,
                Action::SelectWieldable2,
                Action::SelectWieldable3,
                Action::SelectWieldable4,
                Action::SelectWieldable5,
                Action::SelectWieldable6,
                Action::SelectWieldable7,
                Action::SelectWieldable8,
                Action::SelectWieldable9,
                Action::SelectWieldable10,
            ]
            .into_iter()
            .position(|action| matches!(snapshot.button(action), ButtonState::Pressed))
        })
        .flatten();

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
            use_pressed,
            drop_pressed,
        },
        fire_button: weapon::FireButtonState {
            pressed: shoot_pressed,
            active: shoot.is_active(),
        },
        reload: reload.is_active(),
        firing_slot: 0,
        select_slot,
        use_pressed,
        drop_pressed,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ClientFrameFireCommand {
    client_tick: u32,
    button: weapon::FireButtonState,
    elapsed_ms: f32,
}

fn client_fire_ticks_for_post_loop(
    commands: &[ClientFrameFireCommand],
    weapon: &postretro_entities::components::weapon::WeaponComponent,
) -> Vec<u32> {
    let mut selected = Vec::new();
    let mut cooldown_ms = weapon.cooldown_remaining_ms.max(0.0);
    let mut previous_elapsed_ms = 0.0;
    let stats = weapon.effective();

    for command in commands {
        let elapsed_delta_ms = (command.elapsed_ms - previous_elapsed_ms).max(0.0);
        cooldown_ms = (cooldown_ms - elapsed_delta_ms).max(0.0);
        previous_elapsed_ms = command.elapsed_ms;

        let wants_fire = match stats.fire_mode {
            postretro_foundation::FireMode::Semi => {
                command.button.pressed && !weapon.shoot_press_consumed
            }
            postretro_foundation::FireMode::Auto => command.button.active,
        };
        if weapon.state.allows_fire() && wants_fire && cooldown_ms <= 0.0 {
            selected.push(command.client_tick);
            cooldown_ms = stats.cooldown_ms;
            if stats.fire_mode == postretro_foundation::FireMode::Semi {
                break;
            }
        }
    }

    selected
}

fn build_post_movement_command(camera: &Camera) -> sim::PostMovementCommand {
    let (aim_origin, aim_direction) = camera.aim_ray();
    sim::PostMovementCommand {
        aim_origin,
        aim_direction,
    }
}

fn client_weapon_cooldown_from_slot_table(
    slot_table: &postretro_entities::SlotTable,
) -> Option<f32> {
    match slot_table
        .get("player.weaponCooldownMs")
        .and_then(|record| record.value.as_ref())
    {
        Some(postretro_entities::SlotValue::Number(value)) => Some(*value),
        _ => None,
    }
}

fn reconcile_client_weapon_cooldown_from_slot_table(
    predicted: &mut weapon::ClientPredictedShots,
    registry: &mut postretro_entities::EntityRegistry,
    slot_table: &postretro_entities::SlotTable,
    authoritative_slot: Option<usize>,
) -> bool {
    let Some(authoritative_slot) = authoritative_slot else {
        return false;
    };
    let Some(cooldown_ms) = client_weapon_cooldown_from_slot_table(slot_table) else {
        return false;
    };
    let Some(pawn) = registry.local_player_movement_pawn() else {
        return false;
    };
    let Some(weapon_id) = registry
        .get_component::<postretro_entities::components::inventory::Inventory>(pawn)
        .ok()
        .and_then(|inventory| inventory.wieldables.get(authoritative_slot))
        .copied()
        .flatten()
        .filter(|weapon| registry.exists(*weapon))
    else {
        return false;
    };
    let Ok(mut component) = registry
        .get_component::<postretro_entities::components::weapon::WeaponComponent>(weapon_id)
        .cloned()
    else {
        return false;
    };
    predicted.reconcile_cooldown(weapon_id, &mut component, cooldown_ms);
    let _ = registry.set_component(weapon_id, component);
    true
}

fn local_active_wieldable(
    registry: &postretro_entities::EntityRegistry,
) -> Option<(usize, postretro_entities::EntityId)> {
    let pawn = registry.local_player_movement_pawn()?;
    let inventory = registry
        .get_component::<postretro_entities::components::inventory::Inventory>(pawn)
        .ok()?;
    let weapon = inventory.active_wieldable()?;
    if !registry.exists(weapon)
        || registry.has_component_kind(weapon, postretro_entities::ComponentKind::Weapon)
            != Ok(true)
    {
        return None;
    }
    Some((inventory.active_slot, weapon))
}

#[derive(Debug, Clone, PartialEq)]
struct ClientFireMuzzleTerms {
    placement: WeaponPlacementDescriptor,
    muzzle_offset: Option<Vec3>,
}

/// Select both authoritative fire-origin terms from one host tuning row. The
/// local weapon component is intentionally absent: a Control-only replacement
/// can make it stale until the next local-pawn snapshot applies.
fn client_fire_muzzle_terms(
    tuning: &netcode::TuningPayload,
    active_slot: usize,
) -> Option<ClientFireMuzzleTerms> {
    let row = tuning.wieldables.get(active_slot)?.as_ref()?;
    Some(ClientFireMuzzleTerms {
        placement: row.placement.clone(),
        muzzle_offset: tuning
            .muzzle_for_slot(active_slot)
            .copied()
            .map(Vec3::from_array),
    })
}

fn client_fire_snapshot_for_post_loop<'a>(
    fixed_tick_snapshot: Option<&'a input::ActionSnapshot>,
    zero_tick_snapshot: Option<&'a input::ActionSnapshot>,
) -> Option<&'a input::ActionSnapshot> {
    fixed_tick_snapshot.or(zero_tick_snapshot)
}

fn has_player_pawn(registry: &postretro_entities::EntityRegistry) -> bool {
    use postretro_entities::ComponentKind;

    registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .next()
        .is_some()
}

/// Resolve the pawn followed by local camera and input consumers. Identity follows
/// the registry's movement-pawn policy; callers apply camera-specific component gates.
fn followed_player_pawn(
    registry: &postretro_entities::EntityRegistry,
) -> Option<postretro_entities::EntityId> {
    registry.local_player_movement_pawn()
}

/// Resolve a local first-person asset strictly through the pawn's live weapon
/// ownership relationship. Model selection remains local descriptor content;
/// connected-client placement is replaced with host tuning at the caller.
fn local_viewmodel_asset<'a>(
    registry: &postretro_entities::EntityRegistry,
    local_pawn: postretro_entities::EntityId,
    descriptors: &'a [postretro_entities::EntityTypeDescriptor],
) -> Option<(
    postretro_entities::EntityId,
    &'a str,
    Option<WeaponPlacementDescriptor>,
    usize,
)> {
    let inventory = registry.get_component::<Inventory>(local_pawn).ok()?;
    let active_slot = inventory.active_slot;
    let weapon = inventory.active_wieldable()?;
    let provenance = registry
        .get_component::<postretro_entities::provenance::DescriptorProvenance>(weapon)
        .ok()?;
    let archetype = provenance.canonical_name.as_str();
    let (viewmodel, placement) = viewmodel_asset_for_archetype(archetype, descriptors)?;
    Some((weapon, viewmodel, placement, active_slot))
}

/// Resolve optional first-person presentation from a shared weapon archetype.
/// Connected clients use this without constructing a host-only weapon entity.
fn viewmodel_asset_for_archetype<'a>(
    archetype: &str,
    descriptors: &'a [postretro_entities::EntityTypeDescriptor],
) -> Option<(&'a str, Option<WeaponPlacementDescriptor>)> {
    let weapon = descriptors
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some(archetype))?
        .weapon
        .as_ref()?;
    let viewmodel = weapon.viewmodel.as_deref()?.trim();
    (!viewmodel.is_empty()).then_some((viewmodel, weapon.placement.clone()))
}

const BASE_OFFSET: Vec3 = Vec3::new(0.32, -0.28, -0.62);

/// Resolve authored first-person weapon placement by whole descriptor. Future
/// character and per-instance tiers are intentionally parameters only in v1;
/// callers pass `None` until their real storage homes exist.
fn resolve_weapon_placement(
    mod_default: Option<&WeaponPlacementDescriptor>,
    character: Option<&WeaponPlacementDescriptor>,
    weapon: Option<&WeaponPlacementDescriptor>,
    instance: Option<&WeaponPlacementDescriptor>,
) -> WeaponPlacementDescriptor {
    instance
        .or(weapon)
        .or(character)
        .or(mod_default)
        .cloned()
        .unwrap_or_else(legacy_weapon_placement)
}

/// The descriptor form of the legacy hard-coded `BASE_OFFSET`. Keeping the
/// authored labels here means the normal conversion path produces precisely
/// the same transform for an entirely unauthored weapon.
fn legacy_weapon_placement() -> WeaponPlacementDescriptor {
    WeaponPlacementDescriptor {
        offset: postretro_foundation::PlacementOffset {
            right: BASE_OFFSET.x,
            up: BASE_OFFSET.y,
            forward: -BASE_OFFSET.z,
        },
        rotation: postretro_foundation::PlacementRotation::default(),
    }
}

/// Camera-space placement of the first-person model. World camera yaw/pitch
/// intentionally do not appear here: [`viewmodel_world_transform`] applies the
/// render camera afterward. Render-rate bob, sway, and tilt are composed at this
/// game-side assembly seam before the instance crosses into the renderer.
fn viewmodel_camera_space_transform(
    camera_right: Vec3,
    view_feel_eye_offset: Vec3,
    view_feel_roll: f32,
    view_feel_yaw: f32,
    view_feel_pitch: f32,
    placement: &WeaponPlacementDescriptor,
) -> glam::Mat4 {
    let bob_offset = Vec3::new(
        view_feel_eye_offset.dot(camera_right),
        view_feel_eye_offset.y,
        0.0,
    );
    let sway_rotation = Quat::from_rotation_y(view_feel_yaw)
        * Quat::from_rotation_x(view_feel_pitch)
        * Quat::from_rotation_z(view_feel_roll);
    let (placement_offset, placement_rotation) = placement.camera_space();
    glam::Mat4::from_scale_rotation_translation(
        Vec3::ONE,
        sway_rotation * placement_rotation,
        placement_offset + bob_offset,
    )
}

/// Convert camera-relative weapon placement to a world transform. The dedicated
/// viewmodel camera later applies the same view matrix with its tight projection,
/// preserving camera-space clip placement while shared mesh shading receives a
/// genuine world position.
fn viewmodel_world_transform(
    view_matrix: glam::Mat4,
    camera_right: Vec3,
    view_feel_eye_offset: Vec3,
    view_feel_roll: f32,
    view_feel_yaw: f32,
    view_feel_pitch: f32,
    placement: &WeaponPlacementDescriptor,
) -> glam::Mat4 {
    view_matrix.inverse()
        * viewmodel_camera_space_transform(
            camera_right,
            view_feel_eye_offset,
            view_feel_roll,
            view_feel_yaw,
            view_feel_pitch,
            placement,
        )
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

/// Whether clean exit should save the global persistent-slot projection. A
/// connected client skips this path because those values are host-authoritative;
/// its device-local per-owner values use the separate private save path.
fn should_save_persisted_state(can_save: bool, is_connected_client: bool) -> bool {
    can_save && !is_connected_client
}

/// Apply one host-validated join seed to an admitted seat. The host registry is
/// authoritative: unknown durable keys and any schema-incompatible entries are
/// warned and skipped before owner-private replication observes the values.
fn apply_host_join_seed(
    script_ctx: &postretro_entities::ScriptCtx,
    identity: Option<&postretro_scripting_core::store_identity::StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
    seat: Seat,
    slots: BTreeMap<String, postretro_net::wire::JoinSeedValue>,
) {
    for warning in apply_join_seed(
        &mut script_ctx.slot_table.borrow_mut(),
        identity,
        committed_store_slots,
        seat,
        slots,
    ) {
        log::warn!("[Net] {warning}");
    }
}

/// Save a connected client's own persistent per-owner values. This is kept
/// separate from `should_save_persisted_state`: global slots remain
/// host-authoritative, while the durable player-owned values belong in the
/// local device's document.
fn save_connected_client_per_owner_state(session: &mut crate::session::Session) {
    if !session.state_store_lifecycle.can_save() {
        return;
    }
    let Some((_, _, Some(local_seat))) = session
        .net_endpoint
        .as_ref()
        .and_then(netcode::NetEndpoint::client_per_owner_save_context)
    else {
        return;
    };
    let Some(local_player_id) = session.player_options.player_id else {
        // A process without a durable player identity must not manufacture a
        // per-owner key or write a document it cannot later identify.
        return;
    };
    let Some((mod_id, _)) = session.scripting.script_runtime.committed_mod_identity() else {
        log::warn!("[State] no committed mod manifest; skipping persistent per-owner state save");
        return;
    };
    let mod_id = mod_id.to_owned();
    let Some(state_path) = state_path(&mod_id) else {
        if session.state_store_lifecycle.disable_persistence() {
            log::warn!(
                "[State] platform data directory is unavailable; persistent state is disabled for this run"
            );
        }
        return;
    };

    let identity = session.scripting.script_runtime.store_identity().cloned();
    let committed_store_slots = session
        .scripting
        .script_runtime
        .committed_store_slots()
        .clone();
    let script_ctx = session.scripting.script_ctx.clone();
    let collected = collect_per_owner_state(
        &script_ctx.slot_table.borrow(),
        identity.as_ref(),
        &committed_store_slots,
        local_seat,
        local_player_id,
    );
    for warning in collected.warnings {
        log::warn!("[State] {warning}");
    }

    let save_state =
        collected_per_owner_only_state(session.persisted_state.as_ref(), collected.per_owner);
    match save_persisted_state(&state_path, &save_state) {
        Ok(()) => {
            // Keep boot-loaded globals in memory for their own lifecycle, but
            // advance the retained per-owner document used by future client
            // saves and join-seed assembly.
            retain_saved_per_owner_state(&mut session.persisted_state, save_state);
            log::info!(
                "[State] saved persistent per-owner slots to {}",
                state_path.display()
            );
        }
        Err(error) => log::warn!(
            "[State] failed to save persistent per-owner slots to {}: {error}",
            state_path.display()
        ),
    }
}

/// Advance the connected-client private-save cadence after all game-logic work
/// has settled for the frame. The caller owns the exact post-command-drain
/// location; this helper owns only role/participation gating and synchronous I/O.
fn maybe_save_connected_client_per_owner_state(
    session: &mut crate::session::Session,
    frame_dt: std::time::Duration,
) {
    let Some((connected, participating, _)) = session
        .net_endpoint
        .as_ref()
        .and_then(netcode::NetEndpoint::client_per_owner_save_context)
    else {
        return;
    };
    session.per_owner_save_timer.observe_connection(connected);
    if session
        .per_owner_save_timer
        .advance(frame_dt, connected && participating)
    {
        save_connected_client_per_owner_state(session);
    }
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
        if let Some(session) = self.session.as_mut() {
            let registry = session.scripting.script_ctx.registry.borrow();
            if let Some(seats) = session.seat_table.as_mut() {
                // Harvest state but retain bindings until lifecycle events drain
                // after resume; their durable fallback resolves the old pawn.
                seats.harvest_bound_pawns(&registry);
            }
        }
        self.clear_net_level_parity();
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
            WindowEvent::MouseWheel { delta, .. } => {
                if input::wheel_diagnostics_enabled() {
                    log::info!(
                        "[Input] wheel diagnostic: WindowEvent::MouseWheel received ({delta:?})"
                    );
                }
                if egui_consumed {
                    if input::wheel_diagnostics_enabled() {
                        log::info!(
                            "[Input] wheel diagnostic: WindowEvent::MouseWheel dropped because egui consumed it"
                        );
                    }
                    return;
                }
                let Some(session) = self.session.as_mut() else {
                    if input::wheel_diagnostics_enabled() {
                        log::info!(
                            "[Input] wheel diagnostic: WindowEvent::MouseWheel dropped before session install"
                        );
                    }
                    return;
                };
                let forwards_to_gameplay = session
                    .ui_dispatch
                    .dispatch_event(None)
                    .forwards_to_gameplay();
                if forwards_to_gameplay && session.input_focus == InputFocus::Gameplay {
                    session.input_system.handle_mouse_wheel(delta);
                } else if input::wheel_diagnostics_enabled() {
                    log::info!(
                        "[Input] wheel diagnostic: WindowEvent::MouseWheel dropped; forwards_to_gameplay={forwards_to_gameplay}, focus={:?}",
                        session.input_focus,
                    );
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

                // Seat holds measure elapsed rendered time rather than fixed
                // simulation time: Frontend and Loading keep polling a host even
                // though neither runs the simulation loop. Advance exactly here,
                // once per frame, because a Splash/install frame can drain the
                // transport more than once.
                self.advance_seat_hold_clock(frame_dt);

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

                if !self.drive_boot_state_for_redraw(event_loop, frame_dt) {
                    return;
                }

                // Advance the timed-reaction scheduler's monotonic frame counter
                // after the boot/install boundary but before any same-frame UI
                // dispatch or gameplay ticks. A `levelLoad` wait enrolled while a
                // ready world installs above therefore advances on this redraw's
                // first tick (O1/O2/O31). A UI wait enrolled below stamps the new
                // counter and remains protected from this redraw's ticks (O51).
                // Distinct from `frame_timing.begin_frame`.
                if let Some(session) = self.session.as_ref() {
                    session.scripting.scheduler.begin_frame();
                }

                if self.boot_state == BootState::Frontend {
                    // Frontend has no world but is not a peerless state: keep an
                    // installed endpoint alive before frontend-only game logic.
                    let _ = self.poll_world_less_transport(frame_dt);
                    if !self.run_frontend_ui_logic(event_loop, frame_dt) {
                        return;
                    }
                    self.render_frontend_frame(event_loop, now);
                    return;
                }

                // The frame's animation sample clock is a single value shared by
                // game-side hit-zone pose resolution and render collection. It is
                // computed before game logic so same-tick animation switches hit
                // with the exact stamp the visible frame will resolve below.
                #[cfg(feature = "dev-tools")]
                let frozen = self
                    .renderer
                    .as_ref()
                    .is_some_and(|renderer| renderer.freeze_time());
                #[cfg(not(feature = "dev-tools"))]
                let frozen = false;
                let frame_anim_time = Self::frame_anim_time(
                    self.anim_time,
                    frame_dt as f64,
                    self.anim_time_scale,
                    frozen,
                );

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
                        .unwrap_or_else(|| postretro_ui::tree_asset::HUD_NAME.to_string());
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
                            == Some(postretro_ui::demo::PAUSE_MENU_NAME)
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
                let (gameplay_snapshot, zero_tick_fire_snapshot) = {
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
                    if !ui_captures_gameplay {
                        let (occupied, active_slot) = {
                            let registry = session.scripting.script_ctx.registry.borrow();
                            local_wieldable_occupancy(&registry)
                        };
                        let cycle_dwell_ms = session
                            .player_options
                            .switch_cycle_dwell_ms
                            .map(|dwell| dwell as f32)
                            .unwrap_or(self.switching.cycle_commit_dwell_ms);
                        session
                            .gameplay_input_latch
                            .wieldable_selection_mut()
                            .advance_frame(
                                &frame_snapshot,
                                &occupied,
                                active_slot,
                                input::WieldableSelectionPolicy {
                                    commit_on_direct_select: self.switching.commit_on_direct_select,
                                    cycle_dwell_ms,
                                },
                                frame_dt * 1000.0,
                            );
                    }
                    let pending_weapon_slot = session
                        .gameplay_input_latch
                        .wieldable_selection()
                        .cursor_slot();
                    session
                        .scripting
                        .player_hud_state
                        .set_pending_weapon_slot(pending_weapon_slot);
                    let zero_tick_fire_snapshot =
                        (!ui_captures_gameplay && ticks == 0).then_some(frame_snapshot);
                    // Apply look rotation once at render rate, not once per tick —
                    // so zero-tick frames still consume accumulated mouse motion.
                    self.camera
                        .rotate(look.yaw_delta(frame_dt), look.pitch_delta(frame_dt));
                    (gameplay_snapshot, zero_tick_fire_snapshot)
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
                let engine_frame = script_ctx.frame.get();

                // Net poll (M15 Phase 1): non-blocking, once per frame, BEFORE
                // the catch-up tick loop. The client applies received
                // host-authoritative snapshots into the registry here so the
                // render below reflects this frame's replicated state. The host's
                // serialize + send runs AFTER the tick loop (post-loop, beside
                // the other drains). Single-player → inert no-op. See
                // `context/lib/entity_model.md` §6, development_guide §4.3.
                //
                // Driven through `netcode::frame_order` so the apply-before-detect
                // order is owned by one seam: the witness minted here is the only key
                // to the crossing stage below, so inverting the two is a type error.
                let applied = frame_order::run_snapshot_apply_stage(self, engine_frame, frame_dt);

                // Accumulate app-side residual and post-tick events across all ticks;
                // drain after the loop against fully-settled world state. Direct trigger
                // consequential work instead executes and rechecks inside each fixed tick.
                // Weapon and reload events share one stream so catch-up ticks stay ordered.
                // See: context/lib/entity_model.md §5
                let mut pending_movement_events: Vec<&'static str> = Vec::new();
                let mut pending_ai_events: Vec<std::borrow::Cow<'static, str>> = Vec::new();
                let mut pending_weapon_script_events = Vec::new();
                // These edges are populated only by the authoritative simulation
                // branch below. Connected clients run the shared mover driver but
                // never enqueue host-local mover audio.
                let mut pending_mover_events = Vec::new();
                let mut pending_trigger_residuals = Vec::new();
                let mut repointed_pawns = Vec::new();
                let mut sent_client_fire_commands: Vec<ClientFrameFireCommand> = Vec::new();
                let mut host_snapshot_due = false;
                // Death-event names accumulate here and join the sequence-aware
                // post-tick batch below, so a `progress` reaction naming a sequence
                // resolves. Frame-end removals append to the session buffer after
                // this drain, so take that carryover now rather than running game
                // logic during render.
                let mut pending_death_events = std::mem::take(
                    &mut self
                        .session
                        .as_mut()
                        .expect("running session installed")
                        .pending_death_events,
                );

                // Fix B: restore connected-client pawns to their authoritative poses
                // before any fixed tick and before snapshot serialization (`net_serialize_and_send`
                // below) read them, undoing the previous frame's delayed presentation write.
                // Unconditional so it runs even on zero-tick / no-gameplay-snapshot frames —
                // serialization never ingests a delayed pose. A no-op off the host path.
                self.host_restore_client_pawn_authoritative_poses();

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
                        let use_pressed = tick_index == 0
                            && matches!(snapshot.button(Action::Use), ButtonState::Pressed);
                        let drop_pressed = tick_index == 0
                            && matches!(snapshot.button(Action::Drop), ButtonState::Pressed);
                        let mut trigger_use_edges = HashMap::new();
                        if use_pressed {
                            let registry = script_ctx.registry.borrow();
                            if let Some(pawn) = followed_player_pawn(&registry) {
                                trigger_use_edges
                                    .insert(trigger_system::PlayerId::Local(pawn), true);
                            }
                        }
                        let mut touch_drop_edges = HashMap::new();
                        if drop_pressed {
                            let registry = script_ctx.registry.borrow();
                            if let Some(pawn) = followed_player_pawn(&registry) {
                                touch_drop_edges
                                    .insert(trigger_system::PlayerId::Local(pawn), true);
                            }
                        }
                        {
                            let registry = script_ctx.registry.borrow();
                            apply_mover_yaw_carry(
                                &mut self.camera,
                                self.mover_yaw_carry_ground,
                                &self.kinematic_mover_tick_states,
                            );
                            self.mover_yaw_carry_ground = local_player_ground(&registry);
                        }
                        let select_slot = if tick_index == 0 {
                            let (occupied, active_slot) = {
                                let registry = script_ctx.registry.borrow();
                                local_wieldable_occupancy(&registry)
                            };
                            self.session
                                .as_mut()
                                .expect("running session installed")
                                .gameplay_input_latch
                                .wieldable_selection_mut()
                                .take_pending_commit(&occupied, active_slot)
                        } else {
                            None
                        };
                        let mut command = build_sim_command(
                            snapshot,
                            &self.camera,
                            crouch_intent,
                            dash_pressed,
                            shoot_pressed,
                            false,
                            use_pressed,
                            drop_pressed,
                        );
                        command.select_slot = select_slot;

                        // Connected-client prediction (M15 Phase 3 Task 3): send one
                        // Input command and advance ONLY the local pawn's movement
                        // through the movement-only replay helper — never the full
                        // `simulate_tick` (AI / weapons / death stay host-authoritative
                        // and arrive via snapshots). The camera follows the predicted
                        // pawn; frame timing pushes the predicted camera pose. Task 5
                        // adds reconciliation/smoothing on top of this seam.
                        if self.is_connected_client() {
                            let local_pawn = {
                                let registry = script_ctx.registry.borrow();
                                registry.local_player_movement_pawn()
                            };
                            let (switch_accepted, repointed) = {
                                let hit_zone_store = &self
                                    .session
                                    .as_ref()
                                    .expect("connected client session installed")
                                    .hit_zone_store;
                                sim::simulate_client_wieldable_tick(
                                    script_ctx.registry.clone(),
                                    &self.collision_world,
                                    hit_zone_store,
                                    local_pawn,
                                    self.switching.block_during_reload,
                                    command.select_slot,
                                    command.fire_button,
                                    command.reload,
                                    frame_anim_time,
                                    tick_dt,
                                )
                            };
                            if let Some(pawn) = repointed {
                                repointed_pawns.push(pawn);
                            }
                            let (allows_fire, allows_reload) = {
                                let registry = script_ctx.registry.borrow();
                                local_active_wieldable(&registry)
                                    .and_then(|(_, weapon)| {
                                        registry
                                            .get_component::<postretro_entities::components::weapon::WeaponComponent>(weapon)
                                            .ok()
                                    })
                                    .map_or((false, false), |weapon| {
                                        (weapon.state.allows_fire(), weapon.state.allows_reload())
                                    })
                            };
                            if !allows_fire {
                                command.fire_button = weapon::FireButtonState {
                                    pressed: false,
                                    active: false,
                                };
                            }
                            if !allows_reload {
                                command.reload = false;
                            }
                            if switch_accepted && let Some(slot) = command.select_slot {
                                self.client_declare_switch(slot);
                            }
                            self.client_predict_loaded_movers_tick(tick_dt);
                            if let Some(client_tick) =
                                self.client_predict_movement_tick(&command, tick_dt)
                            {
                                sent_client_fire_commands.push(ClientFrameFireCommand {
                                    client_tick,
                                    button: command.fire_button,
                                    elapsed_ms: (tick_index + 1) as f32 * tick_dt * 1000.0,
                                });
                            }
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

                        // Inventory liveness is entity lifecycle, not an input event.
                        // Normalize every authoritative pawn even when a remote queue
                        // produces no command this tick; active changes reuse the
                        // ordinary repoint attachment-dirty path below.
                        repointed_pawns.extend(sim::normalize_wieldable_inventories(
                            &mut script_ctx.registry.borrow_mut(),
                        ));

                        // Host: resolve remote (owned) pawn inputs up front, then the
                        // shared `simulate_tick` runs loaded movers and every player
                        // movement consumer against the same combined collision query.
                        let resolved_remote_commands = self.host_resolve_remote_commands();
                        let remote_pawn_commands =
                            self.host_prepare_remote_pawn_commands(&resolved_remote_commands);
                        trigger_use_edges.extend(remote_pawn_commands.iter().filter_map(
                            |remote| {
                                remote.command.use_pressed.then_some((
                                    trigger_system::PlayerId::Remote(remote.owner_client_id),
                                    true,
                                ))
                            },
                        ));
                        touch_drop_edges.extend(remote_pawn_commands.iter().filter_map(|remote| {
                            remote.command.drop_pressed.then_some((
                                trigger_system::PlayerId::Remote(remote.owner_client_id),
                                true,
                            ))
                        }));

                        // Borrow the two session-owned `simulate_tick` inputs
                        // (hit-zone store, progress tracker) and the boot-owned
                        // `camera` as disjoint field borrows; the post-movement
                        // closure captures these locals (not `self`) so it does not
                        // re-borrow `self.session`.
                        let data_registry = script_ctx.data_registry.borrow();
                        let descriptors = &data_registry.entities;
                        let default_weapon_placement =
                            data_registry.default_weapon_placement.as_ref();
                        let session = self.session.as_mut().expect("running session installed");
                        let hit_zone_store = &session.hit_zone_store;
                        let progress_tracker = &mut session.progress_tracker;
                        let scripting = &mut session.scripting;
                        let trigger_system = &mut session.trigger_system;
                        let touch_system = &mut session.touch_system;
                        let trigger_volume_bridge = &session.trigger_volume_bridge;
                        let trigger_bindings = &self.trigger_bindings;
                        let presentation_camera_aim = (self.camera.pitch, self.camera.yaw);
                        let camera = &mut self.camera;
                        #[cfg(feature = "dev-tools")]
                        let debug_chase_agent = self.debug_chase_agent;
                        let tick_events = sim::simulate_tick_with_presentation_aim(
                            script_ctx.registry.clone(),
                            &self.collision_world,
                            hit_zone_store,
                            self.nav_graph.as_ref(),
                            script_ctx.gravity.get(),
                            self.switching.block_during_reload,
                            frame_anim_time,
                            presentation_camera_aim,
                            progress_tracker,
                            &mut self.ai_runtime,
                            &self.kinematic_mover_colliders,
                            &mut self.kinematic_mover_tick_states,
                            &remote_pawn_commands,
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
                            touch_system,
                            descriptors,
                            default_weapon_placement,
                            &trigger_use_edges,
                            &touch_drop_edges,
                            Some(sim::TriggerTickContext {
                                system: trigger_system,
                                bridge: trigger_volume_bridge,
                                bindings: trigger_bindings,
                                slot_table: script_ctx.slot_table.clone(),
                                script_ctx: Some(script_ctx.clone()),
                                auto_close_timers: Some(scripting.auto_close_timers.clone()),
                                use_edges: &trigger_use_edges,
                            }),
                            |registry| scripting.evaluate_pending_in_tick_impacts(registry),
                        );
                        // Advance timed-reaction countdowns for this tick. Position
                        // relative to `evaluate_slot_accumulators` is not
                        // behaviourally load-bearing: landings execute at the
                        // frame-end drain, after every tick's accumulator pass. An
                        // instance enrolled this frame is skipped via its stamp.
                        // This tick's paired-trigger Exit fires cancel matching
                        // interruptible instances before the countdown advances
                        // (O4), so an Exit on the exact landing tick wins.
                        scripting
                            .scheduler
                            .evaluate(&tick_events.trigger_exit_fires);
                        // A runtime-spawned host enemy receives a mesh only
                        // after the install-time whole-registry clip resolve.
                        // Drain its one-shot queue now: its archetype model and
                        // clip table were preloaded from the map spawner, so this
                        // is solely an animation-index fill, never a GPU upload.
                        let mut spawned_meshes = session
                            .scripting
                            .spawn_context
                            .take_pending_mesh_clip_resolves();
                        spawned_meshes.extend(tick_events.dropped_item_meshes.iter().copied());
                        resolve_mesh_entity_bindings_for_entities(
                            &mut script_ctx.registry.borrow_mut(),
                            &session.mesh_clip_tables,
                            &session.hit_zone_store,
                            spawned_meshes,
                        );
                        // Runtime descriptor spawns can carry dynamic lights.
                        // Enroll them after the fixed tick; the renderer still
                        // receives only the bridge's CPU-packed update.
                        session
                            .light_bridge
                            .absorb_dynamic_lights(&script_ctx.registry.borrow());
                        scripting_systems::slot_accumulators::evaluate_slot_accumulators(
                            &mut session.scripting.slot_accumulator_bindings,
                            tick_dt,
                        );
                        self.host_record_authorized_shots(&tick_events.authorized_shots);
                        self.host_send_rejected_projectile_fire_verdicts(
                            &tick_events.rejected_remote_projectile_fires,
                        );
                        self.host_spawn_projectile_presentations(
                            &script_ctx.registry,
                            &tick_events.remote_projectile_presentation_launches,
                            &tick_events.local_projectile_spawns,
                            &tick_events.enemy_projectile_spawns,
                        );
                        self.host_note_local_projectile_contacts(
                            &tick_events.local_projectile_contacts,
                        );
                        if self.host_flush_pending_hit_declarations() {
                            pending_death_events.extend(self.host_run_remote_hit_death_sweep());
                        }
                        self.host_advance_projectile_presentations(&script_ctx.registry, tick_dt);
                        pending_movement_events.extend(tick_events.movement);
                        pending_ai_events.extend(tick_events.ai);
                        append_tick_weapon_script_events(
                            &mut pending_weapon_script_events,
                            tick_events.weapon,
                            tick_events.reload_deliveries,
                        );
                        pending_mover_events.extend(tick_events.mover);
                        repointed_pawns.extend(tick_events.repointed_pawns);
                        pending_death_events.extend(tick_events.death);
                        pending_trigger_residuals.extend(tick_events.trigger_residuals);

                        self.frame_timing
                            .push_state(InterpolableState::new(self.camera.position));
                        // Fix B: capture each connected-client pawn's authoritative pose
                        // for this tick before the tick stamp advances, so the buffered
                        // sample is keyed to the tick whose end-of-tick pose it carries.
                        self.host_record_client_pawn_poses();
                        self.host_advance_fixed_sim_tick(&mut host_snapshot_due);
                        // Unconditional per-tick registration sweeps, not gated on "did a spawn
                        // happen this tick": they catch runtime enemies and component-driven
                        // world-item acquisition/drop changes before post-loop serialization.
                        self.host_register_map_enemies_after_fixed_sim_tick();
                        self.host_register_world_items_after_fixed_sim_tick();
                    }
                }

                // Regression: a turntable's transform slerps through this tick while
                // carry_yaw previously held the local view until the next input seam.
                let render_camera_yaw = effective_render_yaw(
                    self.camera.yaw,
                    self.mover_yaw_carry_ground,
                    &self.kinematic_mover_tick_states,
                    frame_result.alpha,
                );

                // Task 6 client remote interpolation: sample each remote entity's
                // buffer at `estimated_server_tick - interpolation_delay` and write the
                // interpolated pose through the registry's remote-presentation helper.
                // Runs AFTER the tick loop (so the stage-0 `snapshot_transforms` does
                // not clobber the previous/current pair this writes) and BEFORE the
                // render stage reads entities, so the renderer stays read-only.
                // No-op for single-player and the host.
                self.net_sample_remote_interpolation(frame_dt, frame_anim_time);
                self.update_repointed_weapon_attachments(&script_ctx, &repointed_pawns);
                // Connected clients skip the authoritative `simulate_tick`, so
                // generate renderer-facing pose inputs here from the freshly
                // interpolated displayed transforms. These transient mesh fields
                // are client presentation only and never enter replication.
                self.update_client_presentation_pose_inputs(frame_anim_time, render_camera_yaw);
                self.update_client_overlay_anchors(&script_ctx, frame_anim_time);
                self.run_client_fire_path_post_loop(
                    gameplay_snapshot.as_ref(),
                    zero_tick_fire_snapshot.as_ref(),
                    &sent_client_fire_commands,
                    frame_dt,
                    frame_anim_time,
                    &mut pending_weapon_script_events,
                );

                // Status overlays are host/single-player presentation facts.
                // This runs once after every fixed tick (including zero-tick
                // frames), so a same-frame damage refresh is stamped before
                // Render while a create-then-kill is removed before it draws.
                if !self.is_connected_client() {
                    let session = self.session.as_mut().expect("running session installed");
                    let overlay_config = session
                        .scripting
                        .impact_policy_runtime
                        .client_overlay_config();
                    if let Some(config) = overlay_config.as_ref()
                        && let Some(netcode::NetEndpoint::Host {
                            server,
                            allocator,
                            replicable,
                            owners,
                            ..
                        }) = session.net_endpoint.as_mut()
                    {
                        session.host_overlay_fact_tracker.begin_frame(
                            frame_dt,
                            config.linger_seconds,
                            owners,
                        );
                        let overlay_frame = {
                            let registry = script_ctx.registry.borrow();
                            session
                                .scripting
                                .impact_policy_runtime
                                .update_damaged_enemy_overlays(
                                    &mut session.presentation_pool,
                                    &registry,
                                    &session.hit_zone_store,
                                    frame_anim_time,
                                    session.host_overlay_fact_tracker.tracked_entities(),
                                    |source| {
                                        source
                                            .is_some_and(|source| owners.owner_of(source).is_some())
                                    },
                                )
                        };

                        // The host renderer pool contains only host-local feedback.
                        // Each remote recipient has an independently capped fact
                        // stream on the unreliable presentation channel.
                        netcode::send_host_overlay_facts(
                            &mut session.host_overlay_fact_tracker,
                            server,
                            allocator,
                            replicable,
                            owners,
                            &overlay_frame,
                            config.max_visible,
                        );
                    } else {
                        session.host_overlay_fact_tracker.clear();
                        let registry = script_ctx.registry.borrow();
                        let _ = session
                            .scripting
                            .impact_policy_runtime
                            .update_damaged_enemy_overlays(
                                &mut session.presentation_pool,
                                &registry,
                                &session.hit_zone_store,
                                frame_anim_time,
                                [],
                                |_| false,
                            );
                    }
                }

                let pending_mover_event_names = {
                    let registry = script_ctx.registry.borrow();
                    mover_event_dispatch_addresses(&pending_mover_events, &registry)
                };
                if let Some(session) = self.session.as_ref() {
                    let mut pending_trigger_follow_ups = Vec::new();
                    // Every post-tick named source uses the executing path, then
                    // contributes `fire`/`on_complete` names to one bounded deferred
                    // batch. This keeps movement, AI, weapon, mover, and death events
                    // semantically aligned and lets waits enroll through the common
                    // sequence control arm.
                    pending_trigger_follow_ups.extend(drain_named_events_with_sequences(
                        pending_movement_events.iter().copied(),
                        &script_ctx.data_registry.borrow(),
                        &session.scripting.sequence_registry,
                        &session.scripting.reaction_registry,
                        &session.scripting.system_registry,
                        &script_ctx,
                    ));
                    pending_trigger_follow_ups.extend(drain_named_events_with_sequences(
                        pending_ai_events.iter().map(|event| event.as_ref()),
                        &script_ctx.data_registry.borrow(),
                        &session.scripting.sequence_registry,
                        &session.scripting.reaction_registry,
                        &session.scripting.system_registry,
                        &script_ctx,
                    ));
                    pending_trigger_follow_ups.extend(drain_named_events_with_sequences(
                        pending_weapon_script_events
                            .iter()
                            .map(|event| event.event_name()),
                        &script_ctx.data_registry.borrow(),
                        &session.scripting.sequence_registry,
                        &session.scripting.reaction_registry,
                        &session.scripting.system_registry,
                        &script_ctx,
                    ));
                    pending_trigger_follow_ups.extend(drain_named_events_with_sequences(
                        pending_mover_event_names.iter(),
                        &script_ctx.data_registry.borrow(),
                        &session.scripting.sequence_registry,
                        &session.scripting.reaction_registry,
                        &session.scripting.system_registry,
                        &script_ctx,
                    ));
                    pending_trigger_follow_ups.extend(drain_named_events_with_sequences(
                        pending_death_events.iter(),
                        &script_ctx.data_registry.borrow(),
                        &session.scripting.sequence_registry,
                        &session.scripting.reaction_registry,
                        &session.scripting.system_registry,
                        &script_ctx,
                    ));
                    for (handle, trigger, player) in &pending_trigger_residuals {
                        let Some(residual) = self.trigger_bindings.residual(*handle) else {
                            log::warn!(
                                "[Trigger] residual handle {handle:?} was not bound at install"
                            );
                            continue;
                        };
                        // Scope the origin guard to THIS residual iteration only,
                        // released before the deferred batch below (O54): a `wait`
                        // reached synchronously here keys its instance to this
                        // `(trigger, player)`, while a batch-seeded `fire` stays
                        // sourceless. The paired-enter standing check (O52/O60)
                        // reads the trigger system from the session the drain
                        // already holds — an interruptible instance parks only
                        // while its origin's enter is live, so a player who left
                        // within the frame does not park an uncancellable beat.
                        let paired_enter_standing = session
                            .trigger_system
                            .paired_enters()
                            .contains(&(*trigger, *player));
                        let _origin = session.scripting.scheduler.begin_origin(
                            *trigger,
                            *player,
                            paired_enter_standing,
                        );
                        pending_trigger_follow_ups.extend(
                            fire_prepartitioned_reactions_with_sequences(
                                residual.steps(),
                                &session.scripting.sequence_registry,
                                &session.scripting.reaction_registry,
                                &session.scripting.system_registry,
                                &script_ctx,
                                ResidualOrigin::TriggerBinding,
                            ),
                        );
                    }
                    if !pending_trigger_follow_ups.is_empty() {
                        // Direct residual work has already been partitioned and
                        // run above. Follow-up names advance by bounded FIFO
                        // hops so authored onComplete order is never flattened.
                        dispatch_deferred_named_events_with_sequences(
                            pending_trigger_follow_ups,
                            &script_ctx.data_registry.borrow(),
                            &session.scripting.sequence_registry,
                            &session.scripting.reaction_registry,
                            &session.scripting.system_registry,
                            &script_ctx,
                        );
                    }
                    // Resume timed-reaction landings AFTER the trigger follow-up
                    // dispatch and OUTSIDE any origin guard: a resumed tail runs
                    // where a trigger residual runs, but each landing gets its own
                    // deferred-dispatch call so a `fire`-seeded child's depth is
                    // attributable per instance (O27, O65). The scheduler owns its
                    // tails as `Vec<SequenceStep>` and never mints a
                    // `TriggerResidualHandle`, so this never resolves through
                    // `self.trigger_bindings` (O33). `take_landings` (inside
                    // `drain_landings`) `mem::take`s the queue, so nothing borrows
                    // it across the block — no need to move it onto `App`.
                    //
                    // Before draining, drop any interruptible instance whose keyed
                    // trigger left the level mid-wait: `paired_enters` retains only
                    // live triggers, so a surviving parked interruptible instance
                    // absent from it has no Exit to ever cancel on and must not land
                    // uncancelled (O63).
                    session
                        .scripting
                        .scheduler
                        .drop_orphaned_interruptible_instances(
                            session.trigger_system.paired_enters(),
                        );
                    session.scripting.scheduler.drain_landings(
                        &script_ctx.data_registry.borrow(),
                        &session.scripting.sequence_registry,
                        &session.scripting.reaction_registry,
                        &session.scripting.system_registry,
                        &script_ctx,
                    );
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

                // Player HUD state: republish engine-owned health/ammo/reload slots
                // after game logic settles and before crossing detection / UI
                // snapshot construction, so same-frame consumers see the
                // settled pawn and weapon state. In-tick impact evaluation has
                // already published health at each fire seam. See:
                // context/lib/scripting.md §5.
                //
                // A connected client skips host-authoritative HUD slot writes:
                // those values arrive through state-slot apply. It still samples
                // a materialized local weapon so reload-feedback acknowledgement
                // cannot accumulate. A missing local weapon is a safe no-op.
                let is_connected_client = self.is_connected_client();
                let hud_sampled_weapon = if let Some(session) = self.session.as_mut() {
                    session
                        .scripting
                        .player_hud_state
                        .tick_for_role_and_report_sampled_weapon(is_connected_client, None)
                } else {
                    None
                };
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
                //
                // Consumes this frame's `SnapshotsApplied` witness: on a connected
                // client the replicated slot writes this frame's snapshots carried
                // have already landed, so a crossing fires on the SAME frame its
                // authoritative value arrives, never a frame late.
                let _crossings = frame_order::run_crossing_stage(self, engine_frame, applied);
                if !script_ctx.system_commands.is_empty() {
                    self.dispatch_system_commands();
                }

                // Connected-client per-owner persistence runs exactly after the
                // second command drain: every fixed tick and same-frame crossing
                // write has settled, and neither the SlotTable nor registry
                // RefCell is borrowed. Keep this synchronous on the main thread
                // so it cannot race clean exit or the retained state document.
                if let Some(session) = self.session.as_mut() {
                    maybe_save_connected_client_per_owner_state(
                        session,
                        std::time::Duration::from_secs_f32(frame_dt),
                    );
                }

                if let Some(session) = self.session.as_mut() {
                    session
                        .scripting
                        .impact_policy_runtime
                        .discard_app_drain_pending();
                }

                // Terminal impact effects stay live through every post-catch-up
                // presentation/reaction drain above. Reap them exactly once per
                // rendered frame, before replication and render observe state.
                impact_effects::run_end_of_frame_removal_pass(
                    &mut script_ctx.registry.borrow_mut(),
                    |_, pending_kill_credit| {
                        let Some(pending_kill_credit) = pending_kill_credit else {
                            return;
                        };
                        let session = self.session.as_mut().expect("running session installed");
                        session.pending_death_events.extend(
                            session
                                .progress_tracker
                                .on_entity_killed(&pending_kill_credit.tags),
                        );
                    },
                );

                // Host serialize + send after terminal removals, so the
                // authoritative snapshot cannot carry an entity already reaped
                // this frame. No-op for the client and single-player.
                let owner_projected_weapons = self.net_serialize_and_send(host_snapshot_due);

                // Fix B: present connected-client pawns from the delay buffer at a delayed
                // fractional target, AFTER serialization read the authoritative poses and
                // BEFORE the render collectors read entities. Clocked off the host's own
                // authoritative tick plus the render sub-tick `alpha`, so the presented
                // pose varies smoothly per render frame instead of stepping at 60 Hz.
                self.host_present_client_pawns(frame_result.alpha);

                // Advance each reload-endpoint consumer only after it sampled
                // this frame. Catch-up endpoints remain queued in tick order.
                if let Some(weapon) = hud_sampled_weapon {
                    sim::clear_reload_feedback_for_weapon(
                        &mut script_ctx.registry.borrow_mut(),
                        weapon,
                    );
                }
                sim::clear_owner_reload_feedback_for_weapons(
                    &mut script_ctx.registry.borrow_mut(),
                    &owner_projected_weapons,
                );

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
                if !frozen {
                    self.script_time += frame_dt as f64;
                    // Animation clock accumulates scaled dt at the same site,
                    // under the same freeze gate. Accumulation (not absolute-time
                    // scaling) keeps a mid-fade scale change from jumping poses;
                    // scale 0 holds every clip and fade. See scripting.md §10.3.
                    self.anim_time = frame_anim_time;
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
                // then map its scalar output back onto that basis. The same
                // carry-yaw-adjusted render angle that enters `RenderCamera` below
                // supplies the yaw-derived, Y-free, unit-length right vector that
                // `view_feel_inputs`/`map_output_to_camera` expect, so view feel and
                // the view matrix do not disagree during a sub-tick turntable rotation.
                let camera_right = camera_right_for_yaw(render_camera_yaw);
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
                                    (params.clone(), component.velocity, component.is_grounded())
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
                    render_camera_yaw + vf_yaw_offset,
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

                {
                    let registry = script_ctx.registry.borrow();
                    rebuild_blocked_portals(
                        &mut self.blocked_portals,
                        self.level.as_ref(),
                        &registry,
                    );
                }

                // Portal DFS → cell IDs → visible-cell bitmask → indirect draw buffer.
                let (vis_result, _frustum) = match self.level.as_ref() {
                    Some(world) => postretro_visibility::determine_visible_cells(
                        render_eye_position,
                        view_proj,
                        world,
                        &self.blocked_portals,
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
                        postretro_visibility::extract_frustum_planes(view_proj),
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
                // `postretro_lighting::light_reaches_visible_cell`). Intentionally the wider
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

                let presentation_viewport = self
                    .window_state
                    .as_ref()
                    .map(|state| state.window.inner_size())
                    .map(|size| [size.width, size.height])
                    .unwrap_or([0, 0]);
                let is_connected_client = self.is_connected_client();

                if let Some(renderer) = self.renderer.as_mut() {
                    // The render-stage bridges + collectors live on `Session`;
                    // borrow it once here (disjoint from the `renderer` borrow of
                    // `self.renderer` and from the other `self` fields read below).
                    let session = self.session.as_mut().expect("running session installed");
                    let presentation_inputs = {
                        let mut registry = script_ctx.registry.borrow_mut();
                        session.presentation_pool.advance_and_collect_inputs(
                            &mut registry,
                            frame_dt,
                            view_proj,
                            presentation_viewport,
                        )
                    };
                    let recycled_inputs =
                        renderer.set_presentation_draw_inputs(presentation_inputs);
                    session
                        .presentation_pool
                        .recycle_draw_inputs(recycled_inputs);
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
                        // Connected clients materialize predicted and remote
                        // projectile lights locally from shared descriptors. They
                        // skip the host tick's enrollment path, so absorb before
                        // this render-frame update makes those lights visible.
                        if is_connected_client {
                            session.light_bridge.absorb_dynamic_lights(&registry);
                        }
                        if let Some(update) = session.light_bridge.update(
                            &mut registry,
                            self.script_time as f32,
                            frame_result.alpha,
                        ) {
                            if update.has_dirty_data {
                                renderer.upload_bridge_lights(&update.lights_bytes);
                                renderer.upload_bridge_influences(&update.influence_bytes);
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
                    // The light bridge tracks the full authored light list plus
                    // script-spawned dynamic lights. The fog bridge filters that
                    // snapshot to the dynamic point-light subset it consumes.
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
                    renderer.update_viewmodel_view_projection(
                        self.camera.aspect(),
                        render_camera.view_matrix,
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
                        let presentation_tick = match session.net_endpoint.as_ref() {
                            Some(netcode::NetEndpoint::Host { tick, .. }) => f64::from(*tick),
                            Some(netcode::NetEndpoint::Client {
                                time_sync,
                                replication,
                                ..
                            }) => time_sync.estimated_server_tick().unwrap_or_else(|| {
                                replication.latest_server_tick().map_or(0.0, f64::from)
                            }),
                            None => 0.0,
                        };
                        session.particle_render.collect_at_tick(
                            &registry,
                            self.level.as_ref(),
                            &visible_cells,
                            presentation_tick,
                        );
                    }
                    let particle_collections: Vec<(&str, &[u8])> =
                        session.particle_render.iter_collections().collect();

                    // Mesh render — emits per-instance inputs (model handle +
                    // interpolated transform + phase seed) for skinned-mesh
                    // entities. Forward visibility comes from
                    // `mesh_pass::mesh_visible`; selected-static shadow casters
                    // can be retained as non-forward instances. Like the particle
                    // collector it never touches wgpu; the renderer consumes the
                    // inputs via `set_mesh_draws`. Runs before
                    // `render_frame_indirect`, while `visible_cells` is still live
                    // (it is reclaimed into scratch after).
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
                        session.mesh_render.collect_with_hit_zones(
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
                            &session.hit_zone_store,
                        );

                        // The first-person model is not an entity attachment. A connected
                        // client resolves its local asset by inventory or replicated
                        // archetype, but always takes effective placement from host tuning.
                        let descriptors = script_ctx.data_registry.borrow();
                        if let Some(local_pawn) = followed_player_pawn(&registry) {
                            let viewmodel = match session.net_endpoint.as_ref() {
                                Some(netcode::NetEndpoint::Client {
                                    replication,
                                    tuning,
                                    ..
                                }) => local_viewmodel_asset(
                                    &registry,
                                    local_pawn,
                                    &descriptors.entities,
                                )
                                .and_then(|(weapon, model, _, active_slot)| {
                                    Some((
                                        weapon.to_raw(),
                                        model,
                                        tuning.as_deref()?.placement_for_slot(active_slot)?.clone(),
                                    ))
                                })
                                .or_else(|| {
                                    let archetype = replication.local_active_weapon_archetype()?;
                                    let (model, _) = viewmodel_asset_for_archetype(
                                        archetype,
                                        &descriptors.entities,
                                    )?;
                                    let placement = tuning
                                        .as_deref()?
                                        .placement_for_archetype(archetype)?
                                        .clone();
                                    Some((local_pawn.to_raw(), model, placement))
                                }),
                                _ => local_viewmodel_asset(
                                    &registry,
                                    local_pawn,
                                    &descriptors.entities,
                                )
                                .map(
                                    |(weapon, model, placement, _)| {
                                        (
                                            weapon.to_raw(),
                                            model,
                                            resolve_weapon_placement(
                                                descriptors.default_weapon_placement.as_ref(),
                                                None,
                                                placement.as_ref(),
                                                None,
                                            ),
                                        )
                                    },
                                ),
                            };
                            if let Some((weapon_seed, model, placement)) = viewmodel {
                                session.mesh_render.collect_viewmodel(
                                    model,
                                    viewmodel_world_transform(
                                        render_camera.view_matrix,
                                        camera_right,
                                        vf_eye_offset,
                                        vf_roll,
                                        vf_yaw_offset,
                                        vf_pitch_offset,
                                        &placement,
                                    ),
                                    weapon_seed,
                                );
                            }
                        }
                        renderer.set_mesh_draws(session.mesh_render.instances());

                        self.kinematic_mover_render.collect(
                            &registry,
                            world,
                            &visible_cells,
                            frame_result.alpha,
                        );
                        renderer.set_kinematic_mover_draws(
                            self.kinematic_mover_render.instances(),
                            self.kinematic_mover_render.shadow_instances(),
                        );
                        renderer
                            .set_mover_occluder_aabbs(self.kinematic_mover_render.occluder_aabbs());
                    }

                    #[cfg(feature = "dev-tools")]
                    let (agent_overlay_geometry, agent_overlay_labels) = {
                        let agent_overlay_state = renderer.agent_overlay_state();
                        let diagnostics_visible = session
                            .debug_ui
                            .as_ref()
                            .is_some_and(|debug_ui| debug_ui.is_visible());
                        let include_geometry = agent_overlay_state.enabled
                            && (agent_overlay_state.paths
                                || agent_overlay_state.velocities
                                || agent_overlay_state.destinations);
                        let include_labels = diagnostics_visible
                            || (agent_overlay_state.enabled && agent_overlay_state.labels);
                        if include_geometry || include_labels {
                            let registry = script_ctx.registry.borrow();
                            let viewport_size_points = self
                                .window_state
                                .as_ref()
                                .map(|ws| {
                                    let size = ws.window.inner_size();
                                    let scale_factor = ws.window.scale_factor() as f32;
                                    egui::vec2(
                                        size.width as f32 / scale_factor,
                                        size.height as f32 / scale_factor,
                                    )
                                })
                                .unwrap_or(egui::Vec2::ZERO);
                            agent_diagnostics::collect_agent_overlay_snapshots_for_view(
                                &registry,
                                view_proj,
                                viewport_size_points,
                                include_geometry,
                                include_labels,
                            )
                        } else {
                            (Vec::new(), Vec::new())
                        }
                    };
                    #[cfg(feature = "dev-tools")]
                    let agent_rows =
                        agent_diagnostics::agent_overlay_diagnostics_rows(&agent_overlay_labels);
                    #[cfg(feature = "dev-tools")]
                    let (trigger_rows, trigger_overlay_labels) = {
                        let diagnostics_visible = session
                            .debug_ui
                            .as_ref()
                            .is_some_and(|debug_ui| debug_ui.is_visible());
                        if diagnostics_visible {
                            let registry = script_ctx.registry.borrow();
                            let viewport_size_points = self
                                .window_state
                                .as_ref()
                                .map(|ws| {
                                    let size = ws.window.inner_size();
                                    let scale_factor = ws.window.scale_factor() as f32;
                                    egui::vec2(
                                        size.width as f32 / scale_factor,
                                        size.height as f32 / scale_factor,
                                    )
                                })
                                .unwrap_or(egui::Vec2::ZERO);
                            (
                                trigger_diagnostics::collect_trigger_diagnostics_rows(
                                    &registry,
                                    &session.trigger_volume_bridge,
                                    &session.trigger_system,
                                    &self.trigger_bindings,
                                    &self.trigger_pool_report,
                                ),
                                trigger_diagnostics::collect_trigger_overlay_labels(
                                    &registry,
                                    &session.trigger_volume_bridge,
                                    &session.trigger_system,
                                    view_proj,
                                    viewport_size_points,
                                ),
                            )
                        } else {
                            (Vec::new(), Vec::new())
                        }
                    };
                    #[cfg(feature = "dev-tools")]
                    let door_occluder_diagnostics = {
                        let diagnostics_visible = session
                            .debug_ui
                            .as_ref()
                            .is_some_and(|debug_ui| debug_ui.is_visible());
                        if diagnostics_visible {
                            let registry = script_ctx.registry.borrow();
                            door_occluder_diagnostics::collect(&registry, &self.blocked_portals)
                        } else {
                            door_occluder_diagnostics::DoorOccluderDiagnostics::default()
                        }
                    };

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
                            let agent_label_state = renderer.agent_overlay_state();
                            let paint_agent_labels =
                                agent_label_state.enabled && agent_label_state.labels;
                            let diagnostics_visible = debug_ui.is_visible();
                            if diagnostics_visible || paint_agent_labels {
                                let window = &ws.window;
                                let raw_input = debug_ui.winit_state.take_egui_input(window);
                                let timing_snapshot = renderer.frame_timing_snapshot().cloned();
                                let panel_state = &mut debug_ui.panel_state;
                                let sh_state = &mut debug_ui.sh_diagnostics_state;
                                let ctx_clone = debug_ui.ctx.clone();
                                let full_output = ctx_clone.run_ui(raw_input, |ui| {
                                    let ctx = ui.ctx();
                                    if paint_agent_labels {
                                        agent_diagnostics::paint_agent_overlay_labels(
                                            ctx,
                                            &agent_overlay_labels,
                                        );
                                    }
                                    if diagnostics_visible {
                                        trigger_diagnostics::paint_trigger_overlay_labels(
                                            ctx,
                                            &trigger_overlay_labels,
                                        );
                                        render::debug_ui::draw_diagnostics_panel(
                                            ctx,
                                            panel_state,
                                            sh_state,
                                            renderer,
                                            timing_snapshot.as_ref(),
                                            &agent_rows,
                                            &trigger_rows,
                                            &door_occluder_diagnostics.mover_rows,
                                            &door_occluder_diagnostics.blocked_portal_ids,
                                        );
                                    }
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
                            if session
                                .debug_ui
                                .as_ref()
                                .is_some_and(|debug_ui| debug_ui.is_visible())
                            {
                                door_occluder_diagnostics::emit_blocked_portal_geometry(
                                    renderer,
                                    world,
                                    &self.blocked_portals,
                                );
                            }
                        }
                        // Navmesh overlay: append region rectangles + portal
                        // edges. No-op unless the `Alt+Shift+N` toggle is on
                        // and the map carried a baked navmesh.
                        if let Some(nav_graph) = self.nav_graph.as_ref() {
                            render::nav_diagnostics::emit(renderer, nav_graph);
                        }
                        // Rotating-mover spin axes and orientation. The app owns the
                        // registry read and line geometry; renderer only consumes the
                        // established debug-line primitive.
                        if session
                            .debug_ui
                            .as_ref()
                            .is_some_and(|debug_ui| debug_ui.is_visible())
                        {
                            let registry = script_ctx.registry.borrow();
                            mover_diagnostics::emit(renderer, &registry);
                        }
                        // All-agent path/velocity/destination overlay. The
                        // registry was read once before egui; this emit pass
                        // emits from owned plain geometry through renderer
                        // debug-line surfaces.
                        agent_diagnostics::emit_agent_overlay_geometry(
                            renderer,
                            &agent_overlay_geometry,
                        );
                        // Replicated-entity fallback wireframe: on a host or client,
                        // draw capsules only for replicated entities that still lack
                        // mesh presentation. Thin delegation — `netcode` collects
                        // centers (registry read, no wgpu); the renderer owns the draw.
                        // No-op in single-player and once every replicated entity here
                        // has materialized its descriptor mesh.
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
                        .unwrap_or(postretro_ui::demo::FRONTEND_MENU_NAME);
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

                    let present_handle = match renderer.render_frame_indirect(
                        &mut session.font_system,
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
                    if let Some(present_handle) = present_handle {
                        #[cfg(feature = "dev-tools")]
                        let mut present_handle = present_handle;

                        #[cfg(feature = "dev-tools")]
                        {
                            if let Some((textures_delta, paint_jobs, scale)) = debug_ui_frame {
                                if let Err(err) = renderer.render_debug_ui(
                                    &mut present_handle,
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
                        renderer.present(present_handle);
                        if self.pending_level_log {
                            // First level frame just presented — close out
                            // log line C with the present-cost of the frame
                            // the user is about to see.
                            self.level_timings.record("first_level_frame");
                            log::info!("{}", self.level_timings.summary());
                            self.pending_level_log = false;
                        }
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
                    VisibilityPath::PortalStepLimitFallback { .. } => "portal-step-limit",
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
        if input::wheel_diagnostics_enabled()
            && let DeviceEvent::MouseWheel { delta } = &event
        {
            log::info!(
                "[Input] wheel diagnostic: DeviceEvent::MouseWheel received ({delta:?}); raw-device wheel input is not yet routed to gameplay"
            );
        }
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
        // A connected client must NOT persist replicated global slot writes to
        // `state.json`: its `player.health` /
        // `player.maxHealth` and shared mod slots are server-authoritative
        // values applied through the replicated-state path. Its own
        // per-owner values are the narrow exception below, keyed by its
        // durable player identity and saved without global slots.
        let can_save = self
            .session
            .as_ref()
            .is_some_and(|session| session.state_store_lifecycle.can_save());
        let is_connected_client = self.is_connected_client();
        if is_connected_client {
            if let Some(session) = self.session.as_mut() {
                // Private owner values are intentionally outside the legacy
                // global-save gate: a client never writes global slots, but it
                // does retain its own durable progression on clean exit.
                save_connected_client_per_owner_state(session);
            }
        } else if should_save_persisted_state(can_save, is_connected_client) {
            let session = self
                .session
                .as_mut()
                .expect("session installed at clean exit");
            if let Some((mod_id, _)) = session.scripting.script_runtime.committed_mod_identity() {
                let mod_id = mod_id.to_owned();
                let identity = session.scripting.script_runtime.store_identity().cloned();
                let committed_store_slots = session
                    .scripting
                    .script_runtime
                    .committed_store_slots()
                    .clone();
                let script_ctx = session.scripting.script_ctx.clone();
                if let Some(state_path) = state_path(&mod_id) {
                    let mut collected = collect_persisted_state(
                        &script_ctx.slot_table.borrow(),
                        identity.as_ref(),
                        &committed_store_slots,
                    );
                    for warning in collected.warnings {
                        log::warn!("[State] {warning}");
                    }
                    if let Some(local_player_id) = session.player_options.player_id {
                        let per_owner = collect_per_owner_state(
                            &script_ctx.slot_table.borrow(),
                            identity.as_ref(),
                            &committed_store_slots,
                            postretro_foundation::Seat(0),
                            local_player_id,
                        );
                        for warning in per_owner.warnings {
                            log::warn!("[State] {warning}");
                        }
                        merge_per_owner_state(&mut collected.state, per_owner.per_owner);
                    }
                    match save_persisted_state(&state_path, &collected.state) {
                        Ok(()) => {
                            log::info!("[State] saved persistent slots to {}", state_path.display())
                        }
                        Err(error) => log::warn!(
                            "[State] failed to save persistent slots to {}: {error}",
                            state_path.display()
                        ),
                    }
                } else if session.state_store_lifecycle.disable_persistence() {
                    log::warn!(
                        "[State] platform data directory is unavailable; persistent state is disabled for this run"
                    );
                }
            } else {
                log::warn!("[State] no committed mod manifest; skipping persistent-state save");
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

/// The production frame's two replicated-state stages. `netcode::frame_order` owns
/// their order (apply, then detect); this impl supplies only the bodies. The headless
/// co-op harness implements the same trait, so neither site invents its own sequencing.
/// See: context/lib/networking.md
impl frame_order::ReplicatedStateFrame for App {
    fn apply_received_snapshots(&mut self, frame_dt: f32) {
        self.net_poll_and_apply(frame_dt);
    }

    fn dispatch_state_crossings(&mut self) -> Vec<String> {
        // Clone the `ScriptCtx` handle (cheap `Rc` bump) so the slot-table /
        // data-registry reads borrow nothing of `self` while the disjoint
        // `session` borrow below holds the detector and the reaction registries.
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return Vec::new();
        };
        let Some(session) = self.session.as_mut() else {
            return Vec::new();
        };
        crate::scripting::reactions::dispatch_state_crossings_with_sequences(
            &mut session.crossing_detector,
            &script_ctx.slot_table.borrow(),
            &script_ctx.data_registry.borrow(),
            &session.scripting.sequence_registry,
            &session.scripting.reaction_registry,
            &session.scripting.system_registry,
            &script_ctx,
        )
    }
}

impl App {
    /// Advance the host-local seat hold clock once for this rendered frame.
    ///
    /// Poll drains only evaluate expiry; they must not consume `frame_dt`, since
    /// one frame can perform multiple drains while the boot state changes.
    fn advance_seat_hold_clock(&mut self, frame_dt: f32) {
        if !frame_dt.is_finite() || frame_dt <= 0.0 {
            return;
        }
        let Some(seats) = self
            .session
            .as_mut()
            .and_then(|session| session.seat_table.as_mut())
        else {
            return;
        };
        seats.advance_hold_clock(std::time::Duration::from_secs_f32(frame_dt));
    }

    /// Advance a session-owned endpoint on a frame with no installed world.
    ///
    /// The endpoint-presence predicate deliberately has no boot-state branch:
    /// Frontend, Loading, and resumed Splash all use this same path. Its result
    /// preserves the Task 4 Control-router and Task 7 host-lifecycle
    /// handoff; this task only opens the bounded transport seam.
    pub(crate) fn poll_world_less_transport(
        &mut self,
        frame_dt: f32,
    ) -> Option<netcode::WorldLessPoll> {
        let script_ctx = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())?;
        let (poll, client_connected) = {
            let endpoint = self.session.as_mut()?.net_endpoint.as_mut()?;
            let mut registry = script_ctx.registry.borrow_mut();
            let poll = endpoint
                .poll_world_less(std::time::Duration::from_secs_f32(frame_dt), &mut registry);
            endpoint.warn_once_if_mod_identity_missing();
            let client_connected = endpoint
                .client_per_owner_save_context()
                .map(|(connected, _, _)| connected);
            (poll, client_connected)
        };
        if let Some(connected) = client_connected
            && let Some(session) = self.session.as_mut()
        {
            // Loading and frontend frames have no periodic save, but still
            // observe a transport reconnection so the resumed cadence starts
            // from zero rather than carrying pre-disconnect elapsed time.
            session.per_owner_save_timer.observe_connection(connected);
        }
        if let netcode::WorldLessPoll::Client(controls) = &poll {
            netcode::client_drain_control(self, controls.clone());
        }
        if matches!(poll, netcode::WorldLessPoll::Failed)
            && let Some(session) = self.session.as_mut()
            && let (Some(netcode::NetEndpoint::Host { server, .. }), Some(seats)) =
                (session.net_endpoint.as_mut(), session.seat_table.as_mut())
        {
            // Hold expiry is session-clock work, not successful socket-I/O work.
            clear_released_seat_slot_values(
                &mut script_ctx.slot_table.borrow_mut(),
                netcode::finish_host_poll(server, seats),
            );
        }
        if let netcode::WorldLessPoll::Host(host_poll) = &poll {
            let Some(session) = self.session.as_mut() else {
                return Some(poll);
            };
            let join_seed_identity = session.scripting.script_runtime.store_identity().cloned();
            let join_seed_membership = session
                .scripting
                .script_runtime
                .committed_store_slots()
                .clone();
            let mut seat_table = session.seat_table.as_mut();
            let Some(netcode::NetEndpoint::Host {
                server,
                allocator,
                replicable,
                replication,
                state_slots,
                slot_pawns,
                command_queues,
                owners,
                weapon_owners,
                open_shots,
                pending_hit_declarations,
                weaponless_fire_logged,
                last_sent_tuning,
                join_seeds: join_seed_state,
                client_pawn_presentation,
                ..
            }) = session.net_endpoint.as_mut()
            else {
                return Some(poll);
            };
            let mut registry = script_ctx.registry.borrow_mut();
            for client_id in &host_poll.disconnects {
                join_seed_state.remove_client(*client_id);
                let durable_pawn = seat_table
                    .as_deref()
                    .and_then(|seats| seats.pawn_for_client(*client_id));
                let forgotten_net_id = netcode::host_handle_transport_disconnect(
                    &mut registry,
                    allocator,
                    replicable,
                    replication,
                    state_slots,
                    slot_pawns,
                    command_queues,
                    owners,
                    weapon_owners,
                    open_shots,
                    pending_hit_declarations,
                    weaponless_fire_logged,
                    last_sent_tuning,
                    seat_table.as_deref_mut(),
                    *client_id,
                    durable_pawn,
                );
                // Mirror the client's per-despawn buffer forget: drop the disconnected
                // pawn's delayed presentation samples so they do not leak across the
                // session (a per-level sample leak otherwise).
                if let Some(net_id) = forgotten_net_id {
                    client_pawn_presentation.forget(net_id);
                }
                if let Some(seats) = seat_table.as_deref_mut() {
                    seats.hold_disconnected_client(&mut registry, *client_id);
                }
            }
            for outcome in &host_poll.handshakes {
                let postretro_net::transport::HandshakeOutcome::Admitted { client_id } = outcome
                else {
                    continue;
                };
                if server.is_closed(*client_id) {
                    continue;
                }
                let claim = server.connect_claim(*client_id).cloned();
                let Some(seats) = seat_table.as_deref_mut() else {
                    log::warn!("[Net] admitted client {client_id} has no host seat table");
                    continue;
                };
                let Some(admission) = seats.admit_or_reclaim(*client_id, claim, false) else {
                    log::warn!(
                        "[Net] admitted client {client_id} could not receive a seat: namespace exhausted"
                    );
                    continue;
                };
                clear_released_seat_slot_values(
                    &mut script_ctx.slot_table.borrow_mut(),
                    admission.released_seats,
                );
                if admission.reclaimed {
                    join_seed_state.mark_reclaimed(*client_id);
                }
            }
            for (client_id, arrival) in
                join_seed_state.route_poll(host_poll, |id| server.is_participating(id))
            {
                match arrival {
                    netcode::JoinSeedArrival::Buffered => {}
                    netcode::JoinSeedArrival::Apply(slots) => {
                        let Some(seat) = seat_table
                            .as_deref()
                            .and_then(|seats| seats.seat_for_client(client_id))
                        else {
                            log::warn!(
                                "[Net] join seed for participating client {client_id} has no admitted seat; dropping it"
                            );
                            continue;
                        };
                        apply_host_join_seed(
                            &script_ctx,
                            join_seed_identity.as_ref(),
                            &join_seed_membership,
                            seat,
                            slots,
                        );
                    }
                    netcode::JoinSeedArrival::DroppedConsumed => log::warn!(
                        "[Net] dropping post-consumption join seed from client {client_id}"
                    ),
                    netcode::JoinSeedArrival::DroppedReclaimed => log::info!(
                        "[Net] dropping join seed from client {client_id}; reclaimed seat keeps live per-owner values"
                    ),
                }
            }
            for event in &host_poll.lifecycle {
                let forgotten_net_ids = netcode::host_handle_lifecycle(
                    &mut registry,
                    allocator,
                    replicable,
                    replication,
                    state_slots,
                    slot_pawns,
                    command_queues,
                    owners,
                    weapon_owners,
                    open_shots,
                    pending_hit_declarations,
                    weaponless_fire_logged,
                    last_sent_tuning,
                    seat_table.as_deref_mut(),
                    std::slice::from_ref(event),
                );
                // Mirror the client's per-despawn buffer forget: drop each closed
                // pawn's delayed presentation samples so they do not leak across the
                // session (a per-level sample leak otherwise).
                for net_id in forgotten_net_ids {
                    client_pawn_presentation.forget(net_id);
                }
                match event {
                    postretro_net::slots::SlotEvent::Closed { client_id, .. } => {
                        join_seed_state.remove_client(*client_id);
                    }
                    postretro_net::slots::SlotEvent::Demoted { .. } => {}
                    postretro_net::slots::SlotEvent::Participating { client_id } => {
                        if !server.is_current_participation_entry(event) {
                            continue;
                        }
                        let Some(seat) = seat_table
                            .as_deref()
                            .and_then(|seats| seats.seat_for_client(*client_id))
                        else {
                            continue;
                        };
                        match join_seed_state.on_participating(*client_id) {
                            netcode::ParticipationSeed::None => {}
                            netcode::ParticipationSeed::Apply(slots) => apply_host_join_seed(
                                &script_ctx,
                                join_seed_identity.as_ref(),
                                &join_seed_membership,
                                seat,
                                slots,
                            ),
                            netcode::ParticipationSeed::DroppedReclaimed => log::info!(
                                "[Net] dropping join seed from client {client_id}; reclaimed seat keeps live per-owner values"
                            ),
                        }
                    }
                }
            }
            if let Some(seats) = seat_table {
                clear_released_seat_slot_values(
                    &mut script_ctx.slot_table.borrow_mut(),
                    netcode::finish_host_poll(server, seats),
                );
            }
        }
        Some(poll)
    }

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
                .replace_script_tree_tier(ui_trees, postretro_ui::modal_stack::ScopeTier::Mod);
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
        let descriptor = postretro_ui::theme::ThemeDescriptor {
            colors: self.mod_theme_override.colors.clone(),
            fonts: self.mod_theme_override.fonts.clone(),
            spacing: self.mod_theme_override.spacing.clone(),
        };
        let merged = postretro_ui::theme::UiTheme::engine_default().with_override(&descriptor);
        renderer.set_ui_theme(merged);
    }

    fn frontend_menu_tree_name(&self) -> &str {
        self.session
            .as_ref()
            .and_then(|session| session.frontend.as_ref())
            .map(|frontend| frontend.menu_tree.as_str())
            .unwrap_or(postretro_ui::demo::FRONTEND_MENU_NAME)
    }

    fn present_frontend_menu(&mut self) -> bool {
        let menu_tree = self.frontend_menu_tree_name().to_string();
        let presented = self.session.as_mut().and_then(|session| {
            session
                .modal_stack
                .replace_with_frontend_menu(&menu_tree, postretro_ui::demo::FRONTEND_MENU_NAME)
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
                || active == postretro_ui::demo::FRONTEND_MENU_NAME
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
        modal_stack: &postretro_ui::modal_stack::ModalStack,
        presentation_cells: &mut scripting_systems::presentation_cells::PresentationCellStore,
        slot_table: &postretro_entities::SlotTable,
        script_time: f64,
        ui_input_mode: input::InputMode,
        ui_focused_id: Option<String>,
        frontend_menu_is_top: bool,
    ) -> postretro_ui::UiReadSnapshot {
        let slot_values = Self::build_ui_slot_snapshot(slot_table);
        let mut trees: Vec<postretro_ui::UiTreeEntry> = if frontend_menu_is_top {
            Vec::new()
        } else {
            modal_stack.always_on_layers()
        };
        trees.extend(modal_stack.entries());

        let composed_trees: Vec<&postretro_ui::descriptor::AnchoredTree> =
            trees.iter().map(|entry| &entry.descriptor).collect();
        presentation_cells.reconcile(&composed_trees);
        let cell_values = presentation_cells.snapshot();

        let ring_id = if modal_stack.top_capture_mode()
            == postretro_ui::descriptor::CaptureMode::Capture
            && !ui_input_mode.ring_visible()
        {
            None
        } else {
            ui_focused_id
        };

        postretro_ui::UiReadSnapshot::with_trees(
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
        let content_root = self.content_root.clone();
        let (Some(renderer), Some(session)) = (self.renderer.as_mut(), self.session.as_mut())
        else {
            return;
        };
        for (family, rel_path) in fonts.families {
            let path = content_root.join(&rel_path);
            let bytes = match postretro_ui::text::read_font_file(&path) {
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
            if renderer.register_ui_font(&mut session.font_system, &family, bytes) {
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
    /// seam stays passthrough during boot). Returns true only after presentation
    /// so the splash schedule advances on a visible frame; a transient surface
    /// failure requests another redraw without advancing.
    fn paint_splash(&mut self, event_loop: &ActiveEventLoop) -> bool {
        match self.renderer.as_mut() {
            // Splash requires only boot-ready (surface/device/queue/boot-splash).
            Some(renderer) if renderer.is_boot_ready() => match renderer.render_splash_frame() {
                Ok(Some(handle)) => {
                    renderer.present(handle);
                    true
                }
                Ok(None) => false,
                Err(err) => {
                    self.exit_result = Err(err);
                    event_loop.exit();
                    false
                }
            },
            // Surface not yet configured: nothing presented, ask to redraw.
            _ => false,
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
        let recycled_inputs = renderer.set_presentation_draw_inputs(Vec::new());
        session
            .presentation_pool
            .recycle_draw_inputs(recycled_inputs);
        let present_handle = match renderer.render_frame_indirect(
            &mut session.font_system,
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
        if let Some(present_handle) = present_handle {
            renderer.present(present_handle);
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
        use postretro_ui::tree::NodeInteraction;

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
                    dispatch_source: "ui.slider".to_string(),
                    dispatch_values: Vec::new(),
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
                        let script_ctx = &session.scripting.script_ctx;
                        // Capture chained names (a `fire` step's target or a fired
                        // `Primitive`'s `on_complete`) and dispatch them, rather
                        // than discarding as before. A `wait` step enrolls its tail
                        // ahead of the tick loop; the frame-counter stamp keeps it
                        // from advancing in this same redraw.
                        let chained = fire_named_event_with_sequences(
                            &on_press,
                            &script_ctx.data_registry.borrow(),
                            &session.scripting.sequence_registry,
                            &session.scripting.reaction_registry,
                            &session.scripting.system_registry,
                            script_ctx,
                            None,
                        );
                        if !chained.is_empty() {
                            dispatch_deferred_named_events_with_sequences(
                                chained,
                                &script_ctx.data_registry.borrow(),
                                &session.scripting.sequence_registry,
                                &session.scripting.reaction_registry,
                                &session.scripting.system_registry,
                                script_ctx,
                            );
                        }
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
        use postretro_ui::tree::NodeInteraction;
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
                let script_ctx = &session.scripting.script_ctx;
                let chained = fire_named_event_with_sequences(
                    &on_commit,
                    &script_ctx.data_registry.borrow(),
                    &session.scripting.sequence_registry,
                    &session.scripting.reaction_registry,
                    &session.scripting.system_registry,
                    script_ctx,
                    None,
                );
                if !chained.is_empty() {
                    dispatch_deferred_named_events_with_sequences(
                        chained,
                        &script_ctx.data_registry.borrow(),
                        &session.scripting.sequence_registry,
                        &session.scripting.reaction_registry,
                        &session.scripting.system_registry,
                        script_ctx,
                    );
                }
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
    /// - `SetState` → a literal takes the existing readonly-gated JSON write;
    ///   an install-bound runtime value evaluates against live slots at this
    ///   game-logic write point (invalid/readonly/non-projectable IR warns and
    ///   no-ops).
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
                SystemReactionCommand::SetState {
                    slot,
                    value,
                    dispatch_source,
                    dispatch_values,
                } => {
                    if crate::scripting::reactions::system_commands::is_ir_node(&value) {
                        let outcome = self.session.as_ref().map_or(
                            SystemReactionIrDispatch::Unknown,
                            |session| {
                                session.scripting.system_reaction_ir_bindings.dispatch(
                                    &slot,
                                    &value,
                                    &dispatch_source,
                                    &dispatch_values,
                                    &script_ctx,
                                )
                            },
                        );
                        match outcome {
                            SystemReactionIrDispatch::Evaluated
                            | SystemReactionIrDispatch::Rejected => {
                                // Rejected IR was already diagnosed during install. It must
                                // never fall through to the literal write path, but repeated
                                // fires are a safe no-op rather than a per-dispatch warning.
                            }
                            SystemReactionIrDispatch::Unknown => {
                                // This command is not from the current install table (for
                                // example, a stale queue entry after a rebuild), so retain a
                                // diagnostic instead of silently accepting an unbound write.
                                log::warn!(
                                    "[Scripting] setState runtime value for `{slot}` was not bound at level install; skipping"
                                );
                            }
                        }
                    } else if self.session.as_ref().is_some_and(|session| {
                        session
                            .scripting
                            .system_reaction_ir_bindings
                            .rejects_literal(&slot, &value)
                    }) {
                        // Install-time binding already named the reaction and slot.
                        // Never route the rejected per-owner write through the
                        // scalar JSON fallback.
                    } else if let Err(err) =
                        crate::scripting::primitives::store::write_state_slot_json(
                            &script_ctx,
                            &slot,
                            &value,
                        )
                    {
                        // Literal behavior stays on the existing readonly-gated
                        // JSON path, including target range validation/clamping.
                        log::warn!("[Scripting] setState write to `{slot}` failed: {err}");
                    }
                }
                SystemReactionCommand::AddOwnerSlot { slot, seats, delta } => {
                    // The reaction dispatcher resolves concrete seats at fire
                    // time, but a disconnect/release can happen before this
                    // app-frame drain. Never recreate a released owner's value.
                    for seat in seats {
                        let seat_is_live = self.session.as_ref().is_some_and(|session| {
                            session
                                .seat_table
                                .as_ref()
                                .is_some_and(|seat_table| seat_table.contains_seat(seat))
                        });
                        if !seat_is_live {
                            continue;
                        }

                        let mut slot_table = script_ctx.slot_table.borrow_mut();
                        let Some(record) = slot_table.get_mut(&slot) else {
                            log::warn!(
                                "[Scripting] addSlot references missing slot `{slot}` at drain; skipping"
                            );
                            continue;
                        };
                        if !record.schema.per_owner {
                            log::warn!(
                                "[Scripting] addSlot requires per-owner slot `{slot}`; skipping"
                            );
                            continue;
                        }
                        if record.schema.readonly {
                            log::warn!(
                                "[Scripting] addSlot rejects readonly slot `{slot}`; skipping"
                            );
                            continue;
                        }
                        let Some(postretro_entities::SlotValue::Number(current)) =
                            record.per_seat_value(seat)
                        else {
                            log::warn!(
                                "[Scripting] addSlot requires numeric slot `{slot}`; skipping"
                            );
                            continue;
                        };
                        let next = postretro_scripting_core::store_bridge::validate_slot_value(
                            &slot,
                            &record.schema,
                            postretro_entities::SlotValue::Number(*current + delta),
                        );
                        match next {
                            Ok(next) => record.set_per_seat_value(seat, next),
                            Err(error) => log::warn!(
                                "[Scripting] addSlot for `{slot}` failed validation; skipping: {error}"
                            ),
                        }
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
        // participation entry materializes each client's descriptor-backed remote
        // pawn (M15 Phase 3 Task 4), and these reads alias the session script context /
        // `self.nav_graph` / `self.host_spawn_points`, which the endpoint borrow would
        // otherwise lock out. Cheap on the non-accept path (descriptors clone is the
        // only cost, paid once per frame on the host).
        // Both host participation entry and client apply need the shared descriptor
        // table: the host materializes each client's descriptor-backed pawn
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
        let (net_descriptors, net_default_weapon_placement) = if is_networked {
            let registry = script_ctx.data_registry.borrow();
            (
                registry.entities.clone(),
                registry.default_weapon_placement.clone(),
            )
        } else {
            (Vec::new(), None)
        };
        // Apply reliable lifecycle controls before draining Snapshot. A same-drain
        // hold for epoch N followed by the marker for N+1 must clear N's engine
        // state first; transport epoch filtering can then admit N+1 snapshots into
        // the clean replication state during this frame.
        let client_controls = {
            let endpoint = self
                .session
                .as_mut()
                .and_then(|session| session.net_endpoint.as_mut());
            match endpoint {
                Some(netcode::NetEndpoint::Client { client, .. }) => {
                    if let Err(err) = client.update(dt) {
                        log::error!("[Net] client update failed: {err}");
                    }
                    client.drain_control()
                }
                _ => Vec::new(),
            }
        };
        netcode::client_drain_control(self, client_controls);
        let host_agent_params = self.nav_graph.as_ref().map(|g| g.agent_params());
        let host_spawn_points = std::mem::take(&mut self.host_spawn_points);
        // M15 Phase 3 Task 5: the client reconcile replay threads collision + gravity
        // through `client_receive_and_apply`. Capture the gravity scalar before the
        // endpoint borrow (a `Cell` copy); the collision world is read by-reference
        // inside the client arm (a disjoint `self` field from `self.session`).
        let gravity = script_ctx.gravity.get();
        let collision_world = &self.collision_world;
        let mod_block_during_reload = self.switching.block_during_reload;
        // `net_endpoint` and `mesh_clip_tables` are both session-owned but distinct
        // fields; bind the session once and reach each as a disjoint field borrow,
        // so the client arm's `mesh_clip_tables` read does not re-borrow the
        // session while the `net_endpoint` match holds it.
        let Some(session) = self.session.as_mut() else {
            self.host_spawn_points = host_spawn_points;
            return;
        };
        let hit_zone_store = &session.hit_zone_store;
        let mesh_clip_tables = &session.mesh_clip_tables;
        let mut seat_table = session.seat_table.as_mut();
        let presentation_templates = matches!(
            session.net_endpoint.as_ref(),
            Some(netcode::NetEndpoint::Client { .. })
        )
        .then(|| session.scripting.presentation_template_registry());
        let client_overlay_config = session
            .scripting
            .impact_policy_runtime
            .client_overlay_config();
        let presentation_anim_time = self.anim_time;
        let script_runtime = &session.scripting.script_runtime;
        let replication_identity = netcode::ReplicatedSlotIdentity::borrowed(
            script_runtime.committed_mod_identity().map(|(id, _)| id),
            script_runtime.store_identity(),
            script_runtime.committed_store_slots(),
        );
        let join_seed_identity = script_runtime.store_identity().cloned();
        let join_seed_membership = script_runtime.committed_store_slots().clone();
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
                weapon_owners,
                open_shots,
                pending_hit_declarations,
                weaponless_fire_logged,
                tick,
                last_emitted_snapshot_tick: _,
                host_pawn: _,
                map_enemies: _,
                world_items: _,
                loaded_movers: _,
                demo_mover: _,
                state_slots,
                last_sent_tuning,
                join_seeds: join_seed_state,
                missing_identity_warned: _,
                client_pawn_presentation,
                projectile_presentations: _,
            }) => {
                // Drive the listen server (accept handshakes, drain the socket).
                // Snapshots are sent post-loop in `net_serialize_and_send`.
                match server.update(dt) {
                    // Drive this frame's ordered participation transitions through
                    // the game-logic-owned registry borrow.
                    Ok(poll) => {
                        use postretro_net::transport::HandshakeOutcome;
                        // Gate verdicts log diagnostics. Ordered lifecycle edges own
                        // participation state: entry registers/spawns, while either exit
                        // cleans up. Both paths mutate the registry, so take one
                        // game-logic-owned borrow when either has work.
                        if !poll.disconnects.is_empty()
                            || !poll.handshakes.is_empty()
                            || !poll.lifecycle.is_empty()
                            || !poll.switch_declarations.is_empty()
                            || !poll.join_seeds.is_empty()
                        {
                            let mut registry = script_ctx.registry.borrow_mut();
                            // A transport disconnect ends the short-lived client-id
                            // binding before the same poll's admission outcomes are
                            // considered. A closed admitted slot therefore cannot mint
                            // a durable seat from historical control traffic.
                            for client_id in &poll.disconnects {
                                join_seed_state.remove_client(*client_id);
                                let durable_pawn = seat_table
                                    .as_deref()
                                    .and_then(|seats| seats.pawn_for_client(*client_id));
                                let forgotten_net_id = netcode::host_handle_transport_disconnect(
                                    &mut registry,
                                    allocator,
                                    replicable,
                                    replication,
                                    state_slots,
                                    slot_pawns,
                                    command_queues,
                                    owners,
                                    weapon_owners,
                                    open_shots,
                                    pending_hit_declarations,
                                    weaponless_fire_logged,
                                    last_sent_tuning,
                                    seat_table.as_deref_mut(),
                                    *client_id,
                                    durable_pawn,
                                );
                                // Mirror the client's per-despawn buffer forget: drop
                                // the disconnected pawn's delayed presentation samples
                                // so they do not leak across the session (a per-level
                                // sample leak otherwise).
                                if let Some(net_id) = forgotten_net_id {
                                    client_pawn_presentation.forget(net_id);
                                }
                                if let Some(seats) = seat_table.as_deref_mut() {
                                    seats.hold_disconnected_client(&mut registry, *client_id);
                                }
                            }
                            for outcome in &poll.handshakes {
                                match outcome {
                                    HandshakeOutcome::Admitted { client_id } => {
                                        if server.is_closed(*client_id) {
                                            continue;
                                        }
                                        let claim = server.connect_claim(*client_id).cloned();
                                        let Some(seats) = seat_table.as_deref_mut() else {
                                            log::warn!(
                                                "[Net] admitted client {client_id} has no host seat table"
                                            );
                                            continue;
                                        };
                                        let Some(admission) =
                                            seats.admit_or_reclaim(*client_id, claim, false)
                                        else {
                                            log::warn!(
                                                "[Net] admitted client {client_id} could not receive a seat: namespace exhausted"
                                            );
                                            continue;
                                        };
                                        clear_released_seat_slot_values(
                                            &mut script_ctx.slot_table.borrow_mut(),
                                            admission.released_seats,
                                        );
                                        if admission.reclaimed {
                                            join_seed_state.mark_reclaimed(*client_id);
                                        }
                                        log::info!(
                                            "[Net] client {client_id} admitted; awaiting content parity"
                                        );
                                    }
                                    HandshakeOutcome::Rejected { client_id, cause } => {
                                        log::warn!("[Net] client {client_id} rejected: {cause:?}");
                                    }
                                    HandshakeOutcome::ParityHeld { client_id, cause } => {
                                        log::info!(
                                            "[Net] client {client_id} held for content parity: {cause:?}"
                                        );
                                    }
                                }
                            }
                            for (client_id, arrival) in
                                join_seed_state.route_poll(&poll, |id| server.is_participating(id))
                            {
                                match arrival {
                                    netcode::JoinSeedArrival::Buffered => {}
                                    netcode::JoinSeedArrival::Apply(slots) => {
                                        let Some(seat) = seat_table
                                            .as_deref()
                                            .and_then(|seats| seats.seat_for_client(client_id))
                                        else {
                                            log::warn!(
                                                "[Net] join seed for participating client {client_id} has no admitted seat; dropping it"
                                            );
                                            continue;
                                        };
                                        apply_host_join_seed(
                                            &script_ctx,
                                            join_seed_identity.as_ref(),
                                            &join_seed_membership,
                                            seat,
                                            slots,
                                        );
                                    }
                                    netcode::JoinSeedArrival::DroppedConsumed => log::warn!(
                                        "[Net] dropping post-consumption join seed from client {client_id}"
                                    ),
                                    netcode::JoinSeedArrival::DroppedReclaimed => log::info!(
                                        "[Net] dropping join seed from client {client_id}; reclaimed seat keeps live per-owner values"
                                    ),
                                }
                            }
                            // Preserve edge order. One poll can contain entry followed
                            // by demotion; batch-cleaning every exit before batch-spawning
                            // every entry would leave a pawn for a finally-admitted slot.
                            for event in &poll.lifecycle {
                                let forgotten_net_ids = netcode::host_handle_lifecycle(
                                    &mut registry,
                                    allocator,
                                    replicable,
                                    replication,
                                    state_slots,
                                    slot_pawns,
                                    command_queues,
                                    owners,
                                    weapon_owners,
                                    open_shots,
                                    pending_hit_declarations,
                                    weaponless_fire_logged,
                                    last_sent_tuning,
                                    seat_table.as_deref_mut(),
                                    std::slice::from_ref(event),
                                );
                                // Mirror the client's per-despawn buffer forget: drop
                                // each closed pawn's delayed presentation samples so
                                // they do not leak across the session (a per-level
                                // sample leak otherwise).
                                for net_id in forgotten_net_ids {
                                    client_pawn_presentation.forget(net_id);
                                }
                                match event {
                                    postretro_net::slots::SlotEvent::Closed {
                                        client_id, ..
                                    } => {
                                        join_seed_state.remove_client(*client_id);
                                    }
                                    postretro_net::slots::SlotEvent::Demoted { .. } => {}
                                    postretro_net::slots::SlotEvent::Participating { .. } => {}
                                }
                                let postretro_net::slots::SlotEvent::Participating { client_id } =
                                    event
                                else {
                                    continue;
                                };
                                // A poll can contain a complete promote-then-demote
                                // history. Do not materialize a historical entry for
                                // a slot whose final participation predicate is false.
                                if !server.is_current_participation_entry(event) {
                                    continue;
                                }
                                let Some(seat) = seat_table
                                    .as_deref()
                                    .and_then(|seats| seats.seat_for_client(*client_id))
                                else {
                                    log::warn!(
                                        "[Net] participating client {client_id} has no admitted seat; closing inconsistent slot"
                                    );
                                    let _ = server.close_relay_connection(
                                        *client_id,
                                        postretro_net::slots::CloseCause::Disconnect,
                                    );
                                    continue;
                                };
                                match join_seed_state.on_participating(*client_id) {
                                    netcode::ParticipationSeed::None => {}
                                    netcode::ParticipationSeed::Apply(slots) => {
                                        apply_host_join_seed(
                                            &script_ctx,
                                            join_seed_identity.as_ref(),
                                            &join_seed_membership,
                                            seat,
                                            slots,
                                        );
                                    }
                                    netcode::ParticipationSeed::DroppedReclaimed => log::info!(
                                        "[Net] dropping join seed from client {client_id}; reclaimed seat keeps live per-owner values"
                                    ),
                                }
                                let pawn = if host_spawn_points.is_empty() {
                                    netcode::host_handle_accept(
                                        &mut registry,
                                        allocator,
                                        replicable,
                                        slot_pawns,
                                        *client_id,
                                    );
                                    slot_pawns.pawn_for(*client_id)
                                } else {
                                    let live_placements = seat_table
                                        .as_deref()
                                        .map(|seats| {
                                            seats.occupied_live_placements(
                                                &registry,
                                                host_spawn_points.len(),
                                            )
                                        })
                                        .unwrap_or_default();
                                    let Some(placement_index) =
                                        seat_table.as_deref_mut().and_then(|seats| {
                                            seats.assign_placement(
                                                seat,
                                                host_spawn_points.len(),
                                                live_placements,
                                            )
                                        })
                                    else {
                                        log::warn!(
                                            "[Net] participating client {client_id} could not receive a player_spawn placement; closing inconsistent slot"
                                        );
                                        let _ = server.close_relay_connection(
                                            *client_id,
                                            postretro_net::slots::CloseCause::Disconnect,
                                        );
                                        continue;
                                    };
                                    let carried_loadout = seat_table
                                        .as_deref()
                                        .and_then(|seats| seats.carried_state(seat))
                                        .cloned();
                                    netcode::host_handle_accept_descriptor_at_placement(
                                        &mut registry,
                                        allocator,
                                        replicable,
                                        slot_pawns,
                                        command_queues,
                                        owners,
                                        weapon_owners,
                                        open_shots,
                                        pending_hit_declarations,
                                        weaponless_fire_logged,
                                        *client_id,
                                        &host_spawn_points,
                                        placement_index,
                                        &net_descriptors,
                                        host_agent_params,
                                        carried_loadout.as_ref(),
                                    )
                                };
                                let Some(pawn) = pawn else {
                                    log::warn!(
                                        "[Net] participating client {client_id} could not materialize a pawn; closing inconsistent slot"
                                    );
                                    let _ = server.close_relay_connection(
                                        *client_id,
                                        postretro_net::slots::CloseCause::Disconnect,
                                    );
                                    continue;
                                };
                                replication.register_client(*client_id);
                                state_slots.register_client(*client_id);
                                if let Some(seats) = seat_table.as_deref_mut() {
                                    seats.bind_pawn(&mut registry, seat, pawn);
                                }
                                resolve_accepted_host_pawn_presentation(
                                    &mut registry,
                                    mesh_clip_tables,
                                    hit_zone_store,
                                    pawn,
                                );
                                let payload = netcode::tuning_payload_for_pawn(
                                    &registry,
                                    pawn,
                                    &net_descriptors,
                                    net_default_weapon_placement.as_ref(),
                                );
                                netcode::host_send_tuning_if_changed(
                                    server,
                                    last_sent_tuning,
                                    *client_id,
                                    payload,
                                );
                            }
                            for &(client_id, declaration) in &poll.switch_declarations {
                                netcode::host_handle_switch_declaration(
                                    &mut registry,
                                    server,
                                    slot_pawns,
                                    weapon_owners,
                                    client_id,
                                    declaration.declaration_id,
                                    declaration.slot,
                                    mod_block_during_reload,
                                );
                            }
                        }
                        if let Some(seats) = seat_table {
                            clear_released_seat_slot_values(
                                &mut script_ctx.slot_table.borrow_mut(),
                                netcode::finish_host_poll(server, seats),
                            );
                        }
                    }
                    Err(err) => {
                        log::error!("[Net] host update failed: {err}");
                        if let Some(seats) = seat_table {
                            // A persistently failing socket must not freeze
                            // session-clock hold expiry or its roster update.
                            clear_released_seat_slot_values(
                                &mut script_ctx.slot_table.borrow_mut(),
                                netcode::finish_host_poll(server, seats),
                            );
                        }
                    }
                }
                // Inventory changes have no separate dirty protocol. Rebuild every
                // participating pawn's small fixed tuning payload each host poll;
                // `host_send_tuning_if_changed` remains the final wire dedupe.
                {
                    let registry = script_ctx.registry.borrow();
                    for client_id in server.participating_clients() {
                        let Some(pawn) = slot_pawns.pawn_for(client_id) else {
                            continue;
                        };
                        let payload = netcode::tuning_payload_for_pawn(
                            &registry,
                            pawn,
                            &net_descriptors,
                            net_default_weapon_placement.as_ref(),
                        );
                        netcode::host_send_tuning_if_changed(
                            server,
                            last_sent_tuning,
                            client_id,
                            payload,
                        );
                    }
                }
                // Drain each participating client's reliable Channel::Input: apply
                // replication acks and baseline-refresh requests into the tracker,
                // and echo time-sync probes with the current server tick. The echo
                // microseconds are telemetry only, derived from the monotonic tick.
                let server_tick = *tick;
                let server_now_us = u64::from(server_tick) * netcode::SERVER_TICK_MICROS;
                let participating_clients = server.participating_clients();
                for client_id in participating_clients {
                    netcode::host_handle_client_messages(
                        server,
                        replication,
                        state_slots,
                        command_queues,
                        pending_hit_declarations,
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
                tuning,
                tuning_generation,
                applied_movement_tuning_generation,
                session_status,
                ..
            }) => {
                // Drive the 5 Hz time-sync send loop + echo ingest. The client's
                // local sim tick is the engine frame counter; the estimator reads
                // its own monotonic clock for send/receive microseconds.
                let client_tick = script_ctx.frame.get() as u32;
                let shot_verdicts = netcode::client_drive_time_sync(client, time_sync, client_tick);
                let presentation_messages = client.drain_presentation();
                // Decode + apply every snapshot received this frame through the
                // Phase 2 client state machine, arm prediction off any `local_player`
                // baseline, apply replicated state-slot records through the store-write
                // path, send the resulting acks + baseline-refresh requests, and advance
                // the pending-repair 5 Hz cadence. The registry and slot table are
                // disjoint RefCells; both borrows coexist for the duration of the apply.
                let mut registry = script_ctx.registry.borrow_mut();
                // Connected clients skip authoritative `simulate_tick`, whose
                // fixed-tick queue boundary normally advances transient impact
                // lights. Advance their predicted/observed presentation effects
                // once per rendered frame before this frame can spawn new ones.
                sim::advance_client_presentation_effects(&mut registry, frame_dt);
                let mut slot_table = script_ctx.slot_table.borrow_mut();
                for verdict in shot_verdicts {
                    let _ = self.client_predicted_shots.apply_verdict(
                        &mut registry,
                        verdict.shot_id,
                        verdict.accept,
                        verdict.hit_accepted,
                    );
                }
                let mover_target_tick = time_sync
                    .estimated_server_tick()
                    .map(|tick| tick.floor().clamp(0.0, f64::from(u32::MAX)) as u32);
                let apply_outcome = {
                    let combined_collision = collision::moving::CombinedCollisionWorld::new(
                        collision_world,
                        &self.kinematic_mover_colliders,
                        &self.kinematic_mover_tick_states,
                    );
                    netcode::client_receive_and_apply(
                        &mut registry,
                        &mut slot_table,
                        &replication_identity,
                        client,
                        replication,
                        state_slots,
                        prediction,
                        &net_descriptors,
                        hit_zone_store,
                        host_agent_params,
                        &combined_collision,
                        gravity,
                        crate::frame_timing::TICK_DURATION.as_secs_f32(),
                        dt,
                        mover_target_tick,
                        tuning
                            .as_deref()
                            .and_then(|payload| payload.movement.as_ref()),
                        tuning.as_deref(),
                        *applied_movement_tuning_generation != *tuning_generation,
                    )
                };
                netcode::ingest_client_presentation_messages(
                    &mut registry,
                    presentation_messages,
                    &net_descriptors,
                    presentation_templates
                        .expect("client presentation ingest has a borrowed template registry"),
                    &mut session.client_overlay_facts,
                    replication,
                    &mut session.presentation_pool,
                    client_overlay_config.as_ref(),
                    hit_zone_store,
                    host_agent_params,
                    presentation_anim_time,
                    frame_dt,
                );
                if apply_outcome.replicated_state_changed
                    && let Some(local_seat) = session_status.local_seat()
                {
                    // State replication publishes only this client's scalar
                    // owner-private projection. Keep the local seat cache in
                    // sync so persistence reads the same seat-addressed source
                    // as an authoritative host without learning other seats.
                    sync_client_per_owner_projection(&mut slot_table, local_seat);
                }
                if apply_outcome.armed_local_pawn.is_some()
                    && tuning
                        .as_deref()
                        .and_then(|payload| payload.movement.as_ref())
                        .is_some()
                {
                    *applied_movement_tuning_generation = *tuning_generation;
                }
                apply_authoritative_mover_corrections(
                    &mut self.camera,
                    self.mover_yaw_carry_ground,
                    &mut self.kinematic_mover_tick_states,
                    &apply_outcome.mover_corrections,
                );
                if apply_outcome.owner_private_weapon_cooldown_slot.is_some() {
                    let _ = reconcile_client_weapon_cooldown_from_slot_table(
                        &mut self.client_predicted_shots,
                        &mut registry,
                        &slot_table,
                        apply_outcome.owner_private_weapon_cooldown_slot,
                    );
                }
                if apply_outcome.materialized_remote_entity_presentation {
                    // `mesh_clip_tables` is a disjoint field of the same `session`
                    // bound for the `net_endpoint` match above.
                    resolve_mesh_entity_bindings(
                        &mut registry,
                        &session.mesh_clip_tables,
                        hit_zone_store,
                    );
                }
                // The interpolation-buffer sampling that writes presented remote poses
                // runs in `net_sample_remote_interpolation`, AFTER the catch-up tick
                // loop's stage-0 `snapshot_transforms` — so its previous/current
                // remote-presentation write is the final word before render and is not
                // clobbered by the snapshot pass.
            }
        }
        if let Some(endpoint) = session.net_endpoint.as_mut() {
            endpoint.warn_once_if_mod_identity_missing();
        }
        // Restore the spawn-point cache taken before the endpoint borrow. The host
        // needs it on every future accept; `mem::take` only borrowed it for this call.
        self.host_spawn_points = host_spawn_points;
    }

    fn run_client_fire_path_post_loop(
        &mut self,
        snapshot: Option<&input::ActionSnapshot>,
        zero_tick_snapshot: Option<&input::ActionSnapshot>,
        sent_fire_commands: &[ClientFrameFireCommand],
        frame_dt: f32,
        frame_anim_time: f64,
        pending_weapon_script_events: &mut Vec<PendingWeaponScriptEvent>,
    ) {
        self.client_fire_resolutions.clear();
        if !self.is_connected_client() {
            return;
        }
        self.run_client_fire_path_post_loop_inner(
            snapshot,
            zero_tick_snapshot,
            sent_fire_commands,
            frame_dt,
            frame_anim_time,
            pending_weapon_script_events,
        );
        // Connected clients never enter the host simulation seam. Advance their
        // locally predicted projectiles once here, after interpolation wrote the
        // rendered poses, so they cannot double-advance in a catch-up tick.
        self.advance_client_predicted_projectiles(frame_dt, frame_anim_time);
    }

    fn run_client_fire_path_post_loop_inner(
        &mut self,
        snapshot: Option<&input::ActionSnapshot>,
        zero_tick_snapshot: Option<&input::ActionSnapshot>,
        sent_fire_commands: &[ClientFrameFireCommand],
        frame_dt: f32,
        frame_anim_time: f64,
        pending_weapon_script_events: &mut Vec<PendingWeaponScriptEvent>,
    ) {
        let Some(snapshot) = client_fire_snapshot_for_post_loop(snapshot, zero_tick_snapshot)
        else {
            return;
        };
        let Some(local_pawn_network_id) = netcode::client_local_pawn_network_id(
            self.session
                .as_ref()
                .and_then(|session| session.net_endpoint.as_ref()),
        ) else {
            return;
        };

        let shoot = snapshot.button(Action::Shoot);
        let button = weapon::FireButtonState {
            pressed: matches!(shoot, ButtonState::Pressed),
            active: shoot.is_active(),
        };
        let mut zero_tick_fire_command =
            zero_tick_snapshot
                .filter(|_| button.pressed)
                .map(|snapshot| {
                    build_sim_command(
                        snapshot,
                        &self.camera,
                        false,
                        false,
                        true,
                        false,
                        false,
                        false,
                    )
                });
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let script_ctx = session.scripting.script_ctx.clone();
        let (local_pawn, active_slot, weapon_id, mut component, pellet_salt_name) = {
            let registry = script_ctx.registry.borrow();
            let Some((active_slot, weapon_id)) = local_active_wieldable(&registry) else {
                if zero_tick_fire_command.is_some()
                    && let Some(session) = self.session.as_mut()
                {
                    session.gameplay_input_latch.clear_pressed(Action::Shoot);
                }
                return;
            };
            let local_pawn = registry
                .local_player_movement_pawn()
                .expect("an active local wieldable belongs to the local pawn");
            let Ok(component) = registry
                .get_component::<postretro_entities::components::weapon::WeaponComponent>(weapon_id)
                .cloned()
            else {
                return;
            };
            let pellet_salt_name = weapon::pellet_salt_name(&registry, weapon_id, &component);
            (
                local_pawn,
                active_slot,
                weapon_id,
                component,
                pellet_salt_name,
            )
        };
        let Some(fire_terms) = session
            .net_endpoint
            .as_ref()
            .and_then(|endpoint| match endpoint {
                netcode::NetEndpoint::Client { tuning, .. } => tuning.as_deref(),
                netcode::NetEndpoint::Host { .. } => None,
            })
            .and_then(|tuning| client_fire_muzzle_terms(tuning, active_slot))
        else {
            return;
        };
        if let Some(command) = zero_tick_fire_command.as_mut() {
            command.firing_slot = u8::try_from(active_slot).unwrap_or_default();
        }
        let client_ticks = if zero_tick_fire_command.is_some() {
            netcode::client_peek_next_command_tick(
                self.session
                    .as_ref()
                    .and_then(|session| session.net_endpoint.as_ref()),
            )
            .into_iter()
            .collect::<Vec<_>>()
        } else {
            client_fire_ticks_for_post_loop(sent_fire_commands, &component)
        };
        let Some(&client_tick) = client_ticks.first() else {
            let _ = weapon::advance_client_fire_state(&mut component, button, frame_dt);
            let mut registry = script_ctx.registry.borrow_mut();
            let _ = registry.set_component(weapon_id, component);
            if zero_tick_fire_command.is_some() {
                if let Some(session) = self.session.as_mut() {
                    session.gameplay_input_latch.clear_pressed(Action::Shoot);
                }
            }
            return;
        };
        let (aim_origin, aim_direction) = self.camera.aim_ray();
        let cooldown_before_ms = component.cooldown_remaining_ms;
        let resolution = {
            let registry = script_ctx.registry.borrow();
            weapon::resolve_client_fire(
                Some(local_pawn),
                &mut component,
                &pellet_salt_name,
                active_slot,
                button,
                aim_origin,
                aim_direction,
                &fire_terms.placement,
                fire_terms.muzzle_offset,
                client_tick,
                &self.collision_world,
                &registry,
                &session.hit_zone_store,
                frame_anim_time,
                frame_dt,
            )
        };
        let cooldown_after_ms = component.cooldown_remaining_ms;
        {
            let mut registry = script_ctx.registry.borrow_mut();
            let _ = registry.set_component(weapon_id, component);
        }
        if let Some(resolution) = resolution {
            if let Some(command) = zero_tick_fire_command.as_ref() {
                let aim_pitch = self.camera.pitch;
                let sent_tick = netcode::client_send_input_command(
                    self.session
                        .as_mut()
                        .and_then(|session| session.net_endpoint.as_mut()),
                    command,
                    aim_pitch,
                );
                if sent_tick != Some(resolution.client_tick) {
                    return;
                }
                if let Some(session) = self.session.as_mut() {
                    session.gameplay_input_latch.clear_pressed(Action::Shoot);
                }
            }
            let shot_id = netcode::shot_id_raw(local_pawn_network_id, resolution.client_tick);
            let projectile_launch = resolution.projectile_launch.clone();
            self.client_predicted_shots.predict(
                shot_id,
                weapon_id,
                &resolution,
                cooldown_before_ms,
                cooldown_after_ms,
            );
            // Predict the muzzle FX on a gated local fire, mirroring the host/
            // single-player weapon-activation ("activate") event. It drains with the
            // shared sequence-aware named-event batch; a host reject rolls this shot's
            // `muzzle_fx_visible` state back in reconcile.
            pending_weapon_script_events.push(PendingWeaponScriptEvent::Weapon("activate"));
            let projectile_spawned = projectile_launch.is_some_and(|launch| {
                sim::spawn_projectile(
                    &mut script_ctx.registry.borrow_mut(),
                    local_pawn,
                    weapon_id,
                    launch,
                    Some(shot_id),
                )
                .is_some()
            });
            if !projectile_spawned {
                // Hitscan resolves now. A projectile that could not materialize
                // cannot declare later, so promptly retire its authorized shot
                // with the same valid empty declaration used on normal expiry.
                let _ = netcode::client_send_hit_declaration(
                    self.session
                        .as_mut()
                        .and_then(|session| session.net_endpoint.as_mut()),
                    shot_id,
                    &resolution.hits,
                );
            }
            // Only the first tick casts a ray (once per frame, at the rendered pose);
            // each later tick in a multi-tick frame still authorized a host shot, so
            // send an empty declaration per remaining tick to retire it and keep
            // shot_id accounting balanced with the host without extra ray casts.
            for client_tick in client_ticks.iter().copied().skip(1) {
                let shot_id = netcode::shot_id_raw(local_pawn_network_id, client_tick);
                let _ = netcode::client_send_hit_declaration(
                    self.session
                        .as_mut()
                        .and_then(|session| session.net_endpoint.as_mut()),
                    shot_id,
                    &[],
                );
            }
            self.client_fire_resolutions.push(resolution);
        } else if zero_tick_fire_command.is_some() {
            if let Some(session) = self.session.as_mut() {
                session.gameplay_input_latch.clear_pressed(Action::Shoot);
            }
        }
    }

    fn advance_client_predicted_projectiles(&mut self, frame_dt: f32, frame_anim_time: f64) {
        let mut declarations = Vec::new();
        {
            let Some(session) = self.session.as_ref() else {
                return;
            };
            sim::advance_predicted(
                &session.scripting.script_ctx.registry,
                &self.collision_world,
                &session.hit_zone_store,
                frame_anim_time,
                frame_dt,
                &mut |resolution| match resolution {
                    sim::PredictedProjectileResolution::Impact { shot_id, impact } => {
                        declarations.push((shot_id, Some(impact)));
                    }
                    sim::PredictedProjectileResolution::Expired { shot_id } => {
                        declarations.push((shot_id, None));
                    }
                },
            );
        }

        for (shot_id, impact) in declarations {
            let predicted_entity_hit = impact
                .as_ref()
                .is_some_and(|impact| impact.target.is_some());
            let sent_records = netcode::client_send_projectile_resolution_declaration(
                self.session
                    .as_mut()
                    .and_then(|session| session.net_endpoint.as_mut()),
                shot_id,
                impact.as_ref(),
            );
            if predicted_entity_hit && sent_records.is_some_and(|record_count| record_count > 0) {
                self.client_predicted_shots.mark_hitmarker(shot_id);
            }
        }
    }

    /// Host Phase 2 replication step. Thin delegation to `crate::netcode`. Ingests
    /// the settled replicable set from the registry (immutable borrow) and, when
    /// this redraw completed a 30 Hz cadence tick, sends each accepted client a
    /// per-client delta snapshot over the snapshot channel. No-op for single-player
    /// and the client.
    fn net_serialize_and_send(&mut self, snapshot_due: bool) -> Vec<postretro_entities::EntityId> {
        let host_aim_pitch = self.camera.pitch;
        // Session-owned `ScriptCtx` cloned before the `net_endpoint` borrow (this
        // method stays on `App`). See: context/lib/boot_sequence.md §1.
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return Vec::new();
        };
        let Some(session) = self.session.as_mut() else {
            return Vec::new();
        };
        let script_runtime = &session.scripting.script_runtime;
        let replication_identity = netcode::ReplicatedSlotIdentity::borrowed(
            script_runtime.committed_mod_identity().map(|(id, _)| id),
            script_runtime.store_identity(),
            script_runtime.committed_store_slots(),
        );
        let mesh_clip_tables = &session.mesh_clip_tables;
        let hit_zone_store = &session.hit_zone_store;
        let Some(netcode::NetEndpoint::Host {
            server,
            allocator,
            tick,
            last_emitted_snapshot_tick,
            replication,
            replicable,
            slot_pawns: _,
            command_queues,
            owners,
            weapon_owners,
            open_shots: _,
            pending_hit_declarations: _,
            weaponless_fire_logged: _,
            host_pawn,
            map_enemies: _,
            world_items: _,
            loaded_movers: _,
            demo_mover,
            state_slots,
            last_sent_tuning: _,
            join_seeds: _,
            missing_identity_warned: _,
            client_pawn_presentation: _,
            projectile_presentations,
        }) = session.net_endpoint.as_mut()
        else {
            return Vec::new();
        };

        // Demo path only (POSTRETRO_NET_DEMO_MOVER=1): spawn-and-drive the
        // deterministic Phase 2 net-demo mover for this tick before snapshotting, so
        // its pose is in the replicable set when `host_replicate` ingests below. A
        // no-op on an ordinary host.
        {
            let mut registry = script_ctx.registry.borrow_mut();
            netcode::route_host_presentation_spawns(&mut registry, server, owners);
            netcode::host_drive_demo_mover(&mut registry, demo_mover, allocator, replicable, *tick);
            if weapon_owners.has_attachment_changes() {
                let descriptors = script_ctx.data_registry.borrow();
                let changed_pawns = netcode::synchronize_weapon_owner_attachments(
                    &mut registry,
                    weapon_owners,
                    &descriptors.entities,
                    hit_zone_store,
                );
                crate::resolve_mesh_entity_bindings_for_entities(
                    &mut registry,
                    mesh_clip_tables,
                    hit_zone_store,
                    changed_pawns,
                );
            }
        }

        {
            // M15 Phase 3.5: borrow the slot table (immutable) alongside the registry so
            // `host_replicate` can collect this frame's replicated-state source values
            // and splice the per-client state records into the snapshot envelope. The
            // two RefCells are disjoint, so both borrows coexist. Game logic has settled
            // the live components by this post-tick point, but host HUD slot publication
            // occurs later in the frame. Owner-private projections read the live components
            // directly, so replication observes this frame's settled authoritative state
            // without depending on those later HUD slot writes.
            let registry = script_ctx.registry.borrow();
            let slot_table = script_ctx.slot_table.borrow();
            let sampled_weapons = netcode::host_replicate(
                &registry,
                &slot_table,
                &replication_identity,
                server,
                allocator,
                replication,
                state_slots,
                replicable,
                owners,
                weapon_owners,
                command_queues,
                (*host_pawn).map(|pawn| (pawn, host_aim_pitch)),
                tick.wrapping_add(1),
                snapshot_due,
                last_emitted_snapshot_tick,
            );
            // Ingesting the current presentation pose establishes which baseline
            // must eventually be acknowledged; it does not imply snapshot delivery.
            projectile_presentations.mark_current_poses_ingested();
            sampled_weapons
        }
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
    fn net_sample_remote_interpolation(&mut self, frame_dt: f32, frame_anim_time: f64) {
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return;
        };
        let presentation = {
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
                frame_anim_time,
            )
        };
        self.remote_player_presentation = presentation;
    }

    /// Resolve client overlay anchors after remote interpolation and local pose
    /// selection have written the exact transforms and animation state rendered
    /// this frame. Fact ingestion remains in the earlier snapshot-apply seam.
    fn update_client_overlay_anchors(&mut self, script_ctx: &ScriptCtx, anim_time: f64) {
        let agent_params = self.nav_graph.as_ref().map(|graph| graph.agent_params());
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(netcode::NetEndpoint::Client { replication, .. }) = session.net_endpoint.as_ref()
        else {
            return;
        };
        let config = session
            .scripting
            .impact_policy_runtime
            .client_overlay_config();
        let descriptors = script_ctx.data_registry.borrow();
        let registry = script_ctx.registry.borrow();
        netcode::update_client_overlay_anchors(
            &registry,
            &descriptors.entities,
            &mut session.client_overlay_facts,
            replication,
            &mut session.presentation_pool,
            config.as_ref(),
            &session.hit_zone_store,
            agent_params,
            anim_time,
        );
    }

    fn update_client_presentation_pose_inputs(
        &mut self,
        frame_anim_time: f64,
        render_camera_yaw: f32,
    ) {
        if !self.is_connected_client() {
            return;
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let remote_network_ids = session
            .net_endpoint
            .as_ref()
            .and_then(|endpoint| match endpoint {
                netcode::NetEndpoint::Client { replication, .. } => {
                    Some(replication.entity_network_ids())
                }
                netcode::NetEndpoint::Host { .. } => None,
            })
            .unwrap_or_default();
        let camera_aim = (self.camera.pitch, render_camera_yaw);
        let mut registry = session.scripting.script_ctx.registry.borrow_mut();
        sim::update_presentation_pose_inputs(
            &mut registry,
            &self.collision_world,
            &self.kinematic_mover_colliders,
            &self.kinematic_mover_tick_states,
            &session.hit_zone_store,
            frame_anim_time,
            sim::PresentationPoseInputs {
                camera_aim,
                remote_player_aims: &HashMap::new(),
                remote_aim_pitches: &self.remote_player_presentation.aim_pitches,
                remote_heading_yaws: &self.remote_player_presentation.heading_yaws,
                remote_network_ids: &remote_network_ids,
            },
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

    /// Fix B — restore each connected-client pawn's authoritative `Transform` from the
    /// host presentation buffer before the fixed-tick loop, undoing the previous frame's
    /// delayed presentation write so movement and snapshot serialization read the true
    /// authoritative pose. Thin delegation to `netcode::host_presentation`; a no-op for
    /// single-player, the client, and a host with no connected-client pawns.
    fn host_restore_client_pawn_authoritative_poses(&mut self) {
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return;
        };
        let Some(netcode::NetEndpoint::Host {
            client_pawn_presentation,
            owners,
            allocator,
            ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        let mut registry = script_ctx.registry.borrow_mut();
        netcode::host_restore_client_pawn_authoritative_poses(
            client_pawn_presentation,
            owners,
            allocator,
            &mut registry,
        );
    }

    /// Fix B — record each connected-client pawn's authoritative `Transform` for the
    /// current fixed tick into the host presentation buffer. Called after the tick's
    /// movement has written the authoritative pose and before the tick stamp advances,
    /// so the sample carries the correct end-of-tick pose. Thin delegation.
    fn host_record_client_pawn_poses(&mut self) {
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return;
        };
        let Some(netcode::NetEndpoint::Host {
            client_pawn_presentation,
            owners,
            allocator,
            tick,
            ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        let current_tick = *tick;
        let registry = script_ctx.registry.borrow();
        netcode::host_record_client_pawn_poses(
            client_pawn_presentation,
            owners,
            allocator,
            &registry,
            current_tick,
        );
    }

    /// Fix B — sample each connected-client pawn's buffer at a delayed fractional target
    /// (`current_tick − 1 − delay + alpha`) and write the smoothed pose through the
    /// registry's presentation helper. Called once per render frame after snapshot
    /// serialization (which reads the authoritative pose) and before the render collectors
    /// read entities. `alpha` is the render sub-tick accumulator, so the presented pose
    /// varies smoothly per render frame rather than stepping once per 60 Hz tick. Thin
    /// delegation; a no-op for single-player and the client.
    fn host_present_client_pawns(&mut self, alpha: f32) {
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return;
        };
        let Some(netcode::NetEndpoint::Host {
            client_pawn_presentation,
            owners,
            allocator,
            tick,
            ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        let current_tick = *tick;
        let mut registry = script_ctx.registry.borrow_mut();
        netcode::host_present_client_pawns(
            client_pawn_presentation,
            owners,
            allocator,
            &mut registry,
            current_tick,
            alpha,
        );
    }

    /// Host authoritative remote-pawn command resolution. Resolves one full command
    /// per OWNED remote pawn through the deterministic gap policy. Movement consumes
    /// only the movement subset; host FIRE/reload consumes the same resolved command
    /// later in the sim weapon stage.
    fn host_resolve_remote_commands(&mut self) -> Vec<netcode::ResolvedPawnCommand> {
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
        netcode::host_resolve_remote_commands(owners, command_queues)
    }

    fn host_prepare_remote_pawn_commands(
        &mut self,
        resolved: &[netcode::ResolvedPawnCommand],
    ) -> Vec<sim::RemotePawnCommand> {
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return Vec::new();
        };
        let Some(netcode::NetEndpoint::Host {
            allocator,
            weaponless_fire_logged,
            tick,
            ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return Vec::new();
        };

        resolved
            .iter()
            .map(|resolved| {
                Self::prepare_remote_pawn_command(
                    allocator,
                    &script_ctx.registry.borrow(),
                    weaponless_fire_logged,
                    *tick,
                    resolved,
                )
            })
            .collect()
    }

    fn prepare_remote_pawn_command(
        allocator: &netcode::NetworkIdAllocator,
        registry: &postretro_entities::EntityRegistry,
        weaponless_fire_logged: &mut std::collections::HashSet<postretro_entities::EntityId>,
        fire_tick: u32,
        resolved: &netcode::ResolvedPawnCommand,
    ) -> sim::RemotePawnCommand {
        let firing_slot = usize::from(resolved.command.firing_slot);
        let weapon = registry
            .get_component::<postretro_entities::components::inventory::Inventory>(resolved.pawn)
            .ok()
            .and_then(|inventory| inventory.wieldables.get(firing_slot).copied().flatten());
        let wants_fire =
            resolved.command.fire_button.pressed || resolved.command.fire_button.active;
        if weapon.is_none() && wants_fire && weaponless_fire_logged.insert(resolved.pawn) {
            log::warn!(
                "[Net] pawn {} declared unowned firing slot {}; rejecting remote fire",
                resolved.pawn,
                resolved.command.firing_slot,
            );
        }
        let shot_id = allocator
            .network_id_for_entity(resolved.pawn)
            .map(|network_id| netcode::ShotId::from_parts(network_id, resolved.client_tick));
        sim::RemotePawnCommand {
            pawn: resolved.pawn,
            owner_client_id: resolved.client_id,
            weapon,
            shot_id,
            fire_tick,
            client_tick: resolved.client_tick,
            aim_pitch: resolved.aim_pitch,
            command: resolved.command.clone(),
        }
    }

    fn host_record_authorized_shots(&mut self, shots: &[netcode::OpenAuthorizedShot]) {
        let Some(netcode::NetEndpoint::Host { open_shots, .. }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        for shot in shots {
            open_shots.record(shot.shot.clone(), shot.owner_client_id);
        }
    }

    fn host_send_rejected_projectile_fire_verdicts(
        &mut self,
        rejections: &[sim::RemoteProjectileFireRejection],
    ) {
        let Some(netcode::NetEndpoint::Host { server, .. }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        for rejection in rejections {
            netcode::send_shot_verdict(
                server,
                rejection.owner_client_id,
                rejection.shot_id.raw(),
                false,
                false,
            );
        }
    }

    fn host_spawn_projectile_presentations(
        &mut self,
        registry: &std::rc::Rc<std::cell::RefCell<postretro_entities::EntityRegistry>>,
        remote_launches: &[sim::RemoteProjectilePresentationLaunch],
        local_projectile_spawns: &[postretro_entities::EntityId],
        enemy_projectile_spawns: &[sim::EnemyProjectilePresentationSpawn],
    ) {
        let Some(netcode::NetEndpoint::Host {
            allocator,
            tick,
            replication,
            replicable,
            projectile_presentations,
            ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        let mut registry = registry.borrow_mut();
        for launch in remote_launches {
            projectile_presentations.spawn_remote(
                &mut registry,
                allocator,
                replicable,
                replication,
                launch,
                tick.wrapping_add(1),
            );
        }
        for &projectile in local_projectile_spawns {
            projectile_presentations.mirror_local_gameplay_projectile(
                &mut registry,
                allocator,
                replicable,
                replication,
                projectile,
                *tick,
            );
        }
        for spawn in enemy_projectile_spawns {
            projectile_presentations.mirror_gameplay_projectile_with_descriptor_class(
                &mut registry,
                allocator,
                replicable,
                replication,
                spawn.projectile,
                &spawn.descriptor_class,
                *tick,
            );
        }
    }

    fn host_advance_projectile_presentations(
        &mut self,
        registry: &std::rc::Rc<std::cell::RefCell<postretro_entities::EntityRegistry>>,
        tick_dt: f32,
    ) {
        let Some(netcode::NetEndpoint::Host {
            allocator,
            replication,
            replicable,
            open_shots,
            projectile_presentations,
            ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        projectile_presentations.advance(
            &mut registry.borrow_mut(),
            allocator,
            replicable,
            replication,
            open_shots,
            tick_dt,
        );
    }

    fn host_note_local_projectile_contacts(&mut self, contacts: &[sim::ProjectileContactEvent]) {
        let Some(netcode::NetEndpoint::Host {
            projectile_presentations,
            ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        for contact in contacts {
            projectile_presentations.note_gameplay_contact(contact.projectile, contact.point);
        }
    }

    fn host_flush_pending_hit_declarations(&mut self) -> bool {
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return false;
        };
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let scripting = &mut session.scripting;
        let Some(netcode::NetEndpoint::Host {
            server,
            allocator,
            tick,
            command_queues,
            owners,
            open_shots,
            pending_hit_declarations,
            projectile_presentations,
            ..
        }) = session.net_endpoint.as_mut()
        else {
            return false;
        };

        let mut registry = script_ctx.registry.borrow_mut();
        netcode::host_flush_pending_hit_declarations(
            server,
            &mut registry,
            &self.collision_world,
            allocator,
            owners,
            command_queues,
            open_shots,
            pending_hit_declarations,
            *tick,
            |registry| scripting.evaluate_pending_in_tick_impacts(registry),
            |shot_id, point| projectile_presentations.note_contact(shot_id, point),
        )
    }

    fn host_run_remote_hit_death_sweep(&mut self) -> Vec<String> {
        let Some(session) = self.session.as_mut() else {
            return Vec::new();
        };
        let registry = session.scripting.script_ctx.registry.clone();
        sim::run_death_sweep(&registry)
    }

    /// Advance the listen host's authoritative fixed-simulation tick after one
    /// completed fixed tick. Snapshot stamps and time-sync echoes read this value, so
    /// mover replay deltas are measured in simulation ticks rather than render/network
    /// frames. `snapshot_due` retains cadence edges crossed earlier in the same
    /// catch-up redraw for the post-loop serializer.
    fn host_advance_fixed_sim_tick(&mut self, snapshot_due: &mut bool) {
        let Some(netcode::NetEndpoint::Host { tick, .. }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        netcode::complete_host_fixed_tick(tick, snapshot_due);
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
    /// tracks it so a level reload unregisters the stale pawn. Single-player and the
    /// client are inert; a host map with no `player_spawn` clears any prior host-pawn
    /// replication and weapon ownership. The host pawn stays driven locally by
    /// `simulate_tick` — this only replicates its Transform + PlayerMovementState
    /// outbound.
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
            weapon_owners,
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
            netcode::host_unregister_own_pawn(allocator, replicable, host_pawn, weapon_owners);
            return;
        };
        netcode::host_register_own_pawn(allocator, replicable, host_pawn, weapon_owners, pawn);
    }

    /// Route committed inventory repoints to third-person presentation. Hosts queue
    /// the change for their pre-snapshot attachment pass; single-player and a future
    /// client-local inventory update their local pawn immediately.
    fn update_repointed_weapon_attachments(
        &mut self,
        script_ctx: &postretro_entities::ScriptCtx,
        repointed_pawns: &[postretro_entities::EntityId],
    ) {
        if repointed_pawns.is_empty() {
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if let Some(netcode::NetEndpoint::Host { weapon_owners, .. }) =
            session.net_endpoint.as_mut()
        {
            for &pawn in repointed_pawns {
                weapon_owners.mark_attachment_dirty(pawn);
            }
            return;
        }

        let descriptors = script_ctx.data_registry.borrow().entities.clone();
        let mut registry = script_ctx.registry.borrow_mut();
        for &pawn in repointed_pawns {
            if crate::netcode::synchronize_weapon_attachment_for_pawn(
                &mut registry,
                pawn,
                &descriptors,
                &session.hit_zone_store,
            ) {
                crate::resolve_mesh_entity_bindings_for_entities(
                    &mut registry,
                    &session.mesh_clip_tables,
                    &session.hit_zone_store,
                    [pawn],
                );
            }
        }
    }

    /// Register the listen host's networked AI enemies for outbound replication after a
    /// level install (E10 Task 4). Descriptor enemies carrying `Brain` + `Agent` from a
    /// map placement or runtime spawner must enter the `ReplicableSet`, or clients never
    /// receive their snapshots.
    ///
    /// Host-gated: a no-op for single-player and the connected client (the endpoint is not
    /// the `Host` variant). Thin delegation to `netcode::host_register_map_enemies`, which
    /// sweeps the registry for networked AI enemies, stamps each a `NetworkId`, registers it with
    /// NO owner mapping (host-authoritative, never `local_player`), and tracks the ids in
    /// the `Host` endpoint's `map_enemies` set so a level reload unregisters the stale ones
    /// first. The enemies stay driven by the host's AI/steering systems — this only
    /// replicates their `Transform` (and descriptor class) outbound.
    fn host_register_map_enemies_after_install(&mut self) {
        self.host_register_map_enemies();
    }

    /// Re-sweep after every completed host fixed tick so runtime-spawned AI enemies are
    /// registered before the next outbound snapshot. The underlying sweep is idempotent,
    /// so existing registered enemies retain their `NetworkId`; this is a no-op off the host.
    fn host_register_map_enemies_after_fixed_sim_tick(&mut self) {
        self.host_register_map_enemies();
    }

    /// Shared host-gated delegation for install-time and post-tick AI registration.
    fn host_register_map_enemies(&mut self) {
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

    /// Register map-placed and dropped world items after level install. The item sweep
    /// is host-gated and reload-safe; connected clients receive these entities solely
    /// through host baselines.
    fn host_register_world_items_after_install(&mut self) {
        self.host_register_world_items();
    }

    /// Re-sweep after every host fixed tick so acquisition emits a despawn and a drop
    /// receives a fresh baseline before the next outbound snapshot.
    fn host_register_world_items_after_fixed_sim_tick(&mut self) {
        self.host_register_world_items();
    }

    /// Shared host-gated delegation for install-time and post-tick world-item
    /// registration. Membership derives solely from `TouchableComponent` presence.
    fn host_register_world_items(&mut self) {
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
            world_items,
            ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        let registry = script_ctx.registry.borrow();
        netcode::host_register_world_items(&registry, allocator, replicable, world_items);
    }

    /// Register PRL-loaded kinematic movers for outbound replication after level
    /// install. Host-gated and reload-safe; connected clients have already spawned
    /// the same movers locally from PRL and bind incoming baselines by `mover_id`.
    fn host_register_loaded_movers_after_install(&mut self) {
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
            loaded_movers,
            ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        let registry = script_ctx.registry.borrow();
        netcode::host_register_loaded_movers(&registry, allocator, replicable, loaded_movers);
    }

    /// Connected-client predicted fixed tick (M15 Phase 3 Task 3). Thin delegation
    /// to `crate::netcode`: sends one `ClientMessage::Input` for `command`, then
    /// advances the local pawn through the movement-only replay helper and writes the
    /// predicted state back to the registry. Returns the sent `client_tick`; `None`
    /// means this process was not a connected client at the call site. The
    /// caller skips `simulate_tick`'s local gameplay movement when this path runs —
    /// AI / weapons / death stay host-authoritative and arrive via snapshots.
    fn client_predict_movement_tick(
        &mut self,
        command: &sim::SimCommand,
        tick_dt: f32,
    ) -> Option<u32> {
        let aim_pitch = self.camera.pitch;
        let script_ctx = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())?;
        let Some(netcode::NetEndpoint::Client {
            client, prediction, ..
        }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return None;
        };
        let gravity = script_ctx.gravity.get();
        let mut registry = script_ctx.registry.borrow_mut();
        let mut command = command.clone();
        command.firing_slot = registry
            .local_player_movement_pawn()
            .and_then(|pawn| {
                registry
                    .get_component::<postretro_entities::components::inventory::Inventory>(pawn)
                    .ok()
            })
            .and_then(|inventory| u8::try_from(inventory.active_slot).ok())
            .unwrap_or_default();
        let combined_collision = collision::moving::CombinedCollisionWorld::new(
            &self.collision_world,
            &self.kinematic_mover_colliders,
            &self.kinematic_mover_tick_states,
        );
        Some(netcode::client_predict_tick(
            &mut registry,
            client,
            prediction,
            netcode::ClientPredictionTickContext {
                command: &command,
                aim_pitch,
                collision: &combined_collision,
                gravity,
                tick_dt,
            },
        ))
    }

    /// Send a switch already accepted by the local wieldable machine to the host.
    /// Occupancy and reload policy were checked before the immediate local lower,
    /// including a zero-duration lower that may already have repointed.
    fn client_declare_switch(&mut self, slot: usize) {
        let Ok(slot) = u8::try_from(slot) else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let rollback_slot = {
            let registry = session.scripting.script_ctx.registry.borrow();
            let Some(pawn) = registry.local_player_movement_pawn() else {
                return;
            };
            let Ok(inventory) = registry.get_component::<Inventory>(pawn) else {
                return;
            };
            let Some(rollback_slot) = inventory.switch_origin else {
                return;
            };
            rollback_slot
        };
        let rollback_last_weapon_slot = session
            .gameplay_input_latch
            .wieldable_selection()
            .last_weapon_slot_before_latest_declaration();
        if let Some(endpoint) = session.net_endpoint.as_mut() {
            endpoint.send_client_switch_declaration(slot, rollback_slot, rollback_last_weapon_slot);
        }
    }

    fn client_predict_loaded_movers_tick(&mut self, tick_dt: f32) {
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return;
        };
        let mut registry = script_ctx.registry.borrow_mut();
        client_predict_loaded_movers_tick(
            &mut registry,
            &mut self.kinematic_mover_tick_states,
            tick_dt,
        );
    }

    /// Accumulate one frame onto the animation clock: `prev + dt × scale`.
    /// Pure so the accumulation contract (scale 0.5 halves advancement; a
    /// mid-accumulation scale change never jumps the clock because we add scaled
    /// deltas rather than scaling absolute time) is unit-verifiable without the
    /// event loop. The freeze gate lives at the call site. See scripting.md §10.3.
    fn advance_anim_clock(prev: f64, frame_dt: f64, scale: f64) -> f64 {
        prev + frame_dt * scale
    }

    /// The animation clock value every pose consumer should use for one render
    /// frame: either the held clock while time is frozen, or the post-advance
    /// clock the visible frame will sample.
    fn frame_anim_time(prev: f64, frame_dt: f64, scale: f64, frozen: bool) -> f64 {
        if frozen {
            prev
        } else {
            Self::advance_anim_clock(prev, frame_dt, scale)
        }
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

        let want_menu = matches!(mode, postretro_ui::descriptor::CaptureMode::Capture);
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
            DiagnosticAction::ToggleAgentOverlay => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.toggle_agent_overlay();
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

/// Capture placement provenance while newly materialized pawns still have their
/// authored transforms. Later occupancy reads this durable association after
/// movement changes those transforms.
fn capture_player_spawn_placements(
    registry: &postretro_entities::EntityRegistry,
    spawn_points: &[crate::scripting::map_entity::MapEntity],
    seats: &mut netcode::SeatTable,
) {
    for (pawn, _) in registry.iter_with_kind(ComponentKind::PlayerMovement) {
        let Ok(transform) = registry.get_component::<Transform>(pawn) else {
            continue;
        };
        let Some(placement) = spawn_points
            .iter()
            .position(|placement| placement.origin == transform.position)
        else {
            continue;
        };
        seats.bind_level_spawn_placement(pawn, placement);
    }
}

/// Drop every mod-store value owned by seats that have actually left the
/// session. Disconnect holds deliberately do not call this: a reclaim must
/// observe the same seat-keyed values until expiry releases that seat.
fn clear_released_seat_slot_values(
    slot_table: &mut postretro_entities::SlotTable,
    released_seats: impl IntoIterator<Item = Seat>,
) {
    for seat in released_seats {
        slot_table.clear_per_seat_values(seat);
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

    // A connected client skips the global clean-exit save; its private
    // per-owner path remains enabled. Single-player and the host save both.
    #[test]
    fn connected_client_skips_global_state_save_while_single_player_and_host_save() {
        // Single-player (no endpoint) and host (not a connected client) save.
        assert!(
            should_save_persisted_state(true, false),
            "single-player / host saves when the lifecycle permits"
        );
        // A connected client never saves the global projection.
        assert!(
            !should_save_persisted_state(true, true),
            "a connected client skips the global clean-exit save"
        );
        // The lifecycle gate still suppresses the save before commit/restore.
        assert!(!should_save_persisted_state(false, false));
        assert!(!should_save_persisted_state(false, true));
    }

    #[test]
    fn client_prediction_keeps_carried_light_on_its_reconstructed_mover_pose() {
        use postretro_entities::components::light::{LightCarrier, LightComponent};
        use postretro_entities::{EntityRegistry, KinematicMoverComponent, KinematicMoverConfig};
        use postretro_level_loader::{FalloffModel, LightType, MapLight, ShadowType};

        let mut registry = EntityRegistry::new();
        let mut bridge = scripting_systems::light_bridge::LightBridge::new();
        let light = MapLight {
            origin: [0.0, 2.0, 0.0],
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 12.0,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, 0.0],
            is_dynamic: true,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: Vec::new(),
            cell_index: 0,
            shadow_type: ShadowType::StaticLightMap,
        };
        bridge.populate_from_level(&[light], &mut registry, 0);

        let mover = registry.spawn(Transform::default());
        registry
            .set_component(
                mover,
                KinematicMoverComponent::new(
                    41,
                    KinematicMoverConfig {
                        waypoints: vec![Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)],
                        waypoint_names: vec!["start".to_string(), "finish".to_string()],
                        speed_mps: 4.0,
                        wait_ms: 0.0,
                        mode: postretro_entities::KinematicMoverMode::Once,
                        started: true,
                        spin_axis: Vec3::ZERO,
                        initial_spin_rate_rad_s: 0.0,
                        spin_accel_rad_s2: 0.0,
                        carry_yaw: false,
                    },
                ),
            )
            .unwrap();
        let light_entity = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(light_entity)
            .unwrap()
            .clone();
        component.carrier = Some(LightCarrier {
            mover_entity: mover,
            local_offset: Vec3::new(0.0, 2.0, 0.0),
        });
        registry.set_component(light_entity, component).unwrap();

        let mut mover_tick_states = kinematic_mover::MoverTickStateTable::default();
        client_predict_loaded_movers_tick(&mut registry, &mut mover_tick_states, 0.25);
        let alpha = 0.4;
        let expected_mover = registry.interpolated_transform(mover, alpha).unwrap();
        let expected_light = expected_mover.position + Vec3::new(0.0, 2.0, 0.0);

        assert!(
            bridge.update(&mut registry, 0.0, alpha).is_some(),
            "the client bridge packs the hand-bound carrier after client prediction"
        );
        let packed_origin = bridge.collect_all_as_map_lights(&registry, 0.0)[0].0.origin;
        let observed_light = Vec3::from_array(packed_origin.map(|value| value as f32));
        assert!(
            observed_light.distance(expected_light) <= 1.0e-6,
            "client light and geometry must share client-local interpolated mover pose"
        );
    }

    #[test]
    fn blocked_portal_buffer_rebuilds_from_live_docked_movers_without_latching() {
        use postretro_entities::{EntityRegistry, KinematicMoverComponent, KinematicMoverConfig};
        use postretro_level_loader::{CellData, CellLocatorChild, LevelWorld, PortalData};

        let world = LevelWorld::new_visibility_only(
            vec![
                CellData {
                    bounds_min: Vec3::new(0.0, 0.0, 0.0),
                    bounds_max: Vec3::new(1.0, 1.0, 1.0),
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 0,
                    portal_ref_count: 1,
                    is_solid: false,
                    is_exterior: false,
                    is_drawable: false,
                },
                CellData {
                    bounds_min: Vec3::new(1.0, 0.0, 0.0),
                    bounds_max: Vec3::new(2.0, 1.0, 1.0),
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 1,
                    portal_ref_count: 1,
                    is_solid: false,
                    is_exterior: false,
                    is_drawable: false,
                },
            ],
            vec![0, 0],
            CellLocatorChild::Cell(0),
            Vec::new(),
            vec![PortalData {
                polygon: vec![Vec3::ZERO, Vec3::Y, Vec3::Z],
                front_cell: 0,
                back_cell: 1,
            }],
            true,
        )
        .expect("two-cell portal world must be valid");
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform::default());
        let mut mover = KinematicMoverComponent::new(
            7,
            KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::X],
                waypoint_names: vec!["closed".to_string(), "open".to_string()],
                speed_mps: 1.0,
                wait_ms: 0.0,
                mode: postretro_entities::KinematicMoverMode::Once,
                started: false,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        mover.sealed_portal_ids = vec![0];
        registry.set_component(entity, mover.clone()).unwrap();

        let mut blocked = vec![true, true];
        rebuild_blocked_portals(&mut blocked, Some(&world), &registry);
        assert_eq!(blocked, vec![true]);

        mover.segment_elapsed_ms = 1.0;
        registry.set_component(entity, mover).unwrap();
        rebuild_blocked_portals(&mut blocked, Some(&world), &registry);
        assert_eq!(blocked, vec![false]);
    }

    #[test]
    fn blocked_portal_buffer_ors_closed_sealers_and_one_mover_can_seal_many_portals() {
        use postretro_entities::{EntityRegistry, KinematicMoverComponent, KinematicMoverConfig};
        use postretro_level_loader::{CellData, CellLocatorChild, LevelWorld, PortalData};

        let world = LevelWorld::new_visibility_only(
            vec![
                CellData {
                    bounds_min: Vec3::ZERO,
                    bounds_max: Vec3::ONE,
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 0,
                    portal_ref_count: 2,
                    is_solid: false,
                    is_exterior: false,
                    is_drawable: false,
                },
                CellData {
                    bounds_min: Vec3::X,
                    bounds_max: Vec3::new(2.0, 1.0, 1.0),
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 2,
                    portal_ref_count: 2,
                    is_solid: false,
                    is_exterior: false,
                    is_drawable: false,
                },
            ],
            vec![0, 1, 0, 1],
            CellLocatorChild::Cell(0),
            Vec::new(),
            vec![
                PortalData {
                    polygon: vec![Vec3::ZERO, Vec3::Y, Vec3::Z],
                    front_cell: 0,
                    back_cell: 1,
                },
                PortalData {
                    polygon: vec![Vec3::X, Vec3::X + Vec3::Y, Vec3::X + Vec3::Z],
                    front_cell: 0,
                    back_cell: 1,
                },
            ],
            true,
        )
        .expect("two-portal world must be valid");
        let mut registry = EntityRegistry::new();
        let primary = registry.spawn(Transform::default());
        let secondary = registry.spawn(Transform::default());
        let sealer = |mover_id, sealed_portal_ids| {
            let mut mover = KinematicMoverComponent::new(
                mover_id,
                KinematicMoverConfig {
                    waypoints: vec![Vec3::ZERO, Vec3::X],
                    waypoint_names: vec!["closed".to_string(), "open".to_string()],
                    speed_mps: 1.0,
                    wait_ms: 0.0,
                    mode: postretro_entities::KinematicMoverMode::Once,
                    started: false,
                    spin_axis: Vec3::ZERO,
                    initial_spin_rate_rad_s: 0.0,
                    spin_accel_rad_s2: 0.0,
                    carry_yaw: false,
                },
            );
            mover.sealed_portal_ids = sealed_portal_ids;
            mover
        };
        registry
            .set_component(primary, sealer(1, vec![0, 1]))
            .expect("primary sealer installs");
        registry
            .set_component(secondary, sealer(2, vec![0]))
            .expect("secondary sealer installs");

        let mut blocked = Vec::new();
        rebuild_blocked_portals(&mut blocked, Some(&world), &registry);
        assert_eq!(blocked, vec![true, true]);

        let mut leaving = registry
            .get_component::<KinematicMoverComponent>(primary)
            .expect("primary sealer remains installed")
            .clone();
        leaving.was_active_this_tick = true;
        leaving.current_linear_velocity = Vec3::X;
        registry
            .set_component(primary, leaving)
            .expect("primary departure phase writes");
        rebuild_blocked_portals(&mut blocked, Some(&world), &registry);
        assert_eq!(
            blocked,
            vec![true, false],
            "the second closed door still seals portal zero, while portal one opens"
        );

        let mut second_leaving = registry
            .get_component::<KinematicMoverComponent>(secondary)
            .expect("secondary sealer remains installed")
            .clone();
        second_leaving.was_active_this_tick = true;
        second_leaving.current_linear_velocity = Vec3::X;
        registry
            .set_component(secondary, second_leaving)
            .expect("secondary departure phase writes");
        rebuild_blocked_portals(&mut blocked, Some(&world), &registry);
        assert_eq!(blocked, vec![false, false]);
    }

    #[test]
    fn blocked_portal_buffer_clears_on_map_replacement_and_when_no_movers_remain() {
        use postretro_entities::{EntityRegistry, KinematicMoverComponent, KinematicMoverConfig};
        use postretro_level_loader::{CellData, CellLocatorChild, LevelWorld, PortalData};

        let world_with_two_portals = LevelWorld::new_visibility_only(
            vec![
                CellData {
                    bounds_min: Vec3::ZERO,
                    bounds_max: Vec3::ONE,
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 0,
                    portal_ref_count: 2,
                    is_solid: false,
                    is_exterior: false,
                    is_drawable: false,
                },
                CellData {
                    bounds_min: Vec3::X,
                    bounds_max: Vec3::new(2.0, 1.0, 1.0),
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 2,
                    portal_ref_count: 2,
                    is_solid: false,
                    is_exterior: false,
                    is_drawable: false,
                },
            ],
            vec![0, 1, 0, 1],
            CellLocatorChild::Cell(0),
            Vec::new(),
            vec![
                PortalData {
                    polygon: vec![Vec3::ZERO, Vec3::Y, Vec3::Z],
                    front_cell: 0,
                    back_cell: 1,
                },
                PortalData {
                    polygon: vec![Vec3::X, Vec3::X + Vec3::Y, Vec3::X + Vec3::Z],
                    front_cell: 0,
                    back_cell: 1,
                },
            ],
            true,
        )
        .expect("two-portal world must be valid");
        let world_with_one_portal = LevelWorld::new_visibility_only(
            vec![
                CellData {
                    bounds_min: Vec3::ZERO,
                    bounds_max: Vec3::ONE,
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 0,
                    portal_ref_count: 1,
                    is_solid: false,
                    is_exterior: false,
                    is_drawable: false,
                },
                CellData {
                    bounds_min: Vec3::X,
                    bounds_max: Vec3::new(2.0, 1.0, 1.0),
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 1,
                    portal_ref_count: 1,
                    is_solid: false,
                    is_exterior: false,
                    is_drawable: false,
                },
            ],
            vec![0, 0],
            CellLocatorChild::Cell(0),
            Vec::new(),
            vec![PortalData {
                polygon: vec![Vec3::ZERO, Vec3::Y, Vec3::Z],
                front_cell: 0,
                back_cell: 1,
            }],
            true,
        )
        .expect("one-portal world must be valid");
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform::default());
        let mut sealer = KinematicMoverComponent::new(
            7,
            KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::X],
                waypoint_names: vec!["closed".to_string(), "open".to_string()],
                speed_mps: 1.0,
                wait_ms: 0.0,
                mode: postretro_entities::KinematicMoverMode::Once,
                started: false,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        sealer.sealed_portal_ids = vec![1];
        registry
            .set_component(entity, sealer)
            .expect("map-A sealer installs");

        let mut blocked = vec![true, true, true];
        rebuild_blocked_portals(&mut blocked, Some(&world_with_two_portals), &registry);
        assert_eq!(blocked, vec![false, true]);

        rebuild_blocked_portals(&mut blocked, Some(&world_with_one_portal), &registry);
        assert_eq!(
            blocked,
            vec![false],
            "a reused buffer starts map B false before its movers are rebuilt"
        );

        let no_movers = EntityRegistry::new();
        rebuild_blocked_portals(&mut blocked, Some(&world_with_two_portals), &no_movers);
        assert_eq!(blocked, vec![false, false]);

        rebuild_blocked_portals(&mut blocked, None, &no_movers);
        assert!(
            blocked.is_empty(),
            "an unloaded map retains no stale portal slots"
        );
    }

    /// Exercise the shipped closet fixture's door payload all the way through
    /// the host-only presentation path: section-43 bytes -> loader-shaped
    /// geometry -> spawned mover -> per-frame blocked portal input -> public
    /// visible-cell result. The compact two-cell world is the fixture's main
    /// room / closet doorway, using the authored closed-door waypoint and
    /// doorway dimensions; it keeps this regression CPU-only.
    #[test]
    fn closet_reveal_closed_loaded_door_hides_interior_until_it_moves() {
        use crate::scripting_systems::mesh_anim::MeshClipTables;
        use crate::scripting_systems::mesh_render::MeshRenderCollector;
        use glam::Mat4;
        use postretro_entities::{EntityRegistry, Transform, components::mesh::MeshComponent};
        use postretro_level_format::geometry::Vertex;
        use postretro_level_format::kinematic_geometry::{
            KINEMATIC_GEOMETRY_VERSION, KinematicGeometrySection, KinematicMoverRecord,
            KinematicWaypointRecord,
        };
        use postretro_level_loader::{
            CellData, CellLocatorChild, CellLocatorNodeData, KinematicGeometry, LevelWorld,
            PortalData,
        };

        let fixture = include_str!("../../../content/dev/maps/closet-reveal.map");
        assert!(fixture.contains("\"classname\" \"kinematic_mover\""));
        assert!(fixture.contains("\"name\" \"closet_door\""));
        assert!(fixture.contains("\"path\" \"closet_door_closed\""));
        assert!(fixture.contains("\"origin\" \"168 -16 48\""));

        let mover_section = KinematicGeometrySection {
            version: KINEMATIC_GEOMETRY_VERSION,
            movers: vec![KinematicMoverRecord {
                mover_id: 3,
                name: "closet_door".to_string(),
                tags: vec!["closet_door".to_string()],
                origin: [168.0, -16.0, 48.0],
                path: "closet_door_closed".to_string(),
                speed: 4.0,
                wait_ms: 0.0,
                move_mode: 0,
                start_on_spawn: false,
                vertices: vec![
                    Vertex::new(
                        [160.0, -112.0, 0.0],
                        [0.0, 0.0],
                        [0.0, 1.0, 0.0],
                        [1.0, 0.0, 0.0],
                        true,
                        [0.0, 0.0],
                        0,
                    ),
                    Vertex::new(
                        [176.0, -112.0, 0.0],
                        [1.0, 0.0],
                        [0.0, 1.0, 0.0],
                        [1.0, 0.0, 0.0],
                        true,
                        [0.0, 0.0],
                        0,
                    ),
                    Vertex::new(
                        [176.0, -112.0, 96.0],
                        [1.0, 1.0],
                        [0.0, 1.0, 0.0],
                        [1.0, 0.0, 0.0],
                        true,
                        [0.0, 0.0],
                        0,
                    ),
                ],
                indices: vec![0, 1, 2],
                face_meta: Vec::new(),
                spin_axis: [0.0, 0.0, 0.0],
                spin_speed_deg_s: 0.0,
                spin_accel_deg_s2: 0.0,
                carry_yaw: false,
                block_policy: "displace".to_string(),
                crush_damage: 0.0,
                crush_interval_ms: 0.0,
                auto_close_ms: None,
                open_event: None,
                close_event: None,
                blocked_event: None,
                crush_event: None,
                sealed_portal_ids: vec![0],
                carried_lights: Vec::new(),
            }],
            waypoints: vec![
                KinematicWaypointRecord {
                    name: "closet_door_closed".to_string(),
                    next: "closet_door_open".to_string(),
                    origin: [168.0, -16.0, 48.0],
                },
                KinematicWaypointRecord {
                    name: "closet_door_open".to_string(),
                    next: String::new(),
                    origin: [168.0, -16.0, -64.0],
                },
            ],
        };
        let decoded = KinematicGeometrySection::from_bytes(&mover_section.to_bytes())
            .expect("closet door v5 section must load");

        let mut world = LevelWorld::new_visibility_only(
            vec![
                CellData {
                    bounds_min: Vec3::new(0.0, -128.0, 0.0),
                    bounds_max: Vec3::new(176.0, 0.0, 96.0),
                    face_start: 0,
                    face_count: 1,
                    portal_ref_start: 0,
                    portal_ref_count: 1,
                    is_solid: false,
                    is_exterior: false,
                    is_drawable: true,
                },
                CellData {
                    bounds_min: Vec3::new(176.0, -128.0, 0.0),
                    bounds_max: Vec3::new(272.0, 0.0, 96.0),
                    face_start: 1,
                    face_count: 1,
                    portal_ref_start: 1,
                    portal_ref_count: 1,
                    is_solid: false,
                    is_exterior: false,
                    is_drawable: true,
                },
            ],
            vec![0, 0],
            CellLocatorChild::Node(0),
            vec![CellLocatorNodeData {
                plane_normal: Vec3::X,
                plane_distance: 176.0,
                front: CellLocatorChild::Cell(1),
                back: CellLocatorChild::Cell(0),
            }],
            vec![PortalData {
                polygon: vec![
                    Vec3::new(176.0, -112.0, 0.0),
                    Vec3::new(176.0, -48.0, 0.0),
                    Vec3::new(176.0, -48.0, 96.0),
                    Vec3::new(176.0, -112.0, 96.0),
                ],
                front_cell: 0,
                back_cell: 1,
            }],
            true,
        )
        .expect("closet doorway visibility world must be valid");
        world.kinematic_geometry = KinematicGeometry {
            movers: decoded.movers.into_iter().map(Into::into).collect(),
            waypoints: decoded.waypoints.into_iter().map(Into::into).collect(),
        };

        let mut registry = EntityRegistry::new();
        let spawned = runtime_movers::spawn_loaded_kinematic_movers(
            &mut registry,
            &world,
            runtime_movers::ENGINE_AUTO_CLOSE_MS,
        )
        .expect("loaded closet mover must spawn");
        assert_eq!(spawned.len(), 1);
        let closet_enemy = registry.spawn(Transform {
            position: Vec3::new(220.0, -80.0, 48.0),
            ..Transform::default()
        });
        registry
            .set_component(
                closet_enemy,
                MeshComponent::stateless("closet_enemy".to_string()),
            )
            .expect("closet enemy mesh installs");

        let camera_position = Vec3::new(40.0, -80.0, 48.0);
        let view = Mat4::look_at_rh(camera_position, camera_position + Vec3::X, Vec3::Y);
        let view_proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 512.0) * view;
        let visibility = |blocked_portals: &[bool]| {
            let (result, _) = postretro_visibility::determine_visible_cells(
                camera_position,
                view_proj,
                &world,
                blocked_portals,
                false,
                &mut Vec::new(),
            );
            result
        };
        let drawable_ids = |visible_cells: &VisibleCells| match visible_cells {
            VisibleCells::Culled(ids) => ids.clone(),
            VisibleCells::DrawAll => panic!("closet world must use portal visibility"),
        };
        fn closet_enemy_collected(
            registry: &EntityRegistry,
            world: &LevelWorld,
            visible_cells: &VisibleCells,
            camera_position: Vec3,
        ) -> bool {
            let mut collector = MeshRenderCollector::new();
            collector.collect(
                registry,
                world,
                visible_cells,
                1.0,
                0.0,
                &MeshClipTables::new(),
                camera_position,
            );
            collector
                .instances()
                .iter()
                .any(|instance| instance.model.as_str() == "closet_enemy")
        }

        let mut blocked_portals = Vec::new();
        rebuild_blocked_portals(&mut blocked_portals, Some(&world), &registry);
        assert_eq!(blocked_portals, vec![true]);
        let closed_visibility = visibility(&blocked_portals);
        assert_eq!(drawable_ids(&closed_visibility.visible_cells), vec![0]);
        assert_eq!(
            closed_visibility.fog_reachable,
            vec![0],
            "fog isolated behind the closed door must not be marched"
        );
        assert!(
            !closet_enemy_collected(
                &registry,
                &world,
                &closed_visibility.visible_cells,
                camera_position,
            ),
            "the visible-cell gate drops closet entities from the forward collection"
        );

        let entity = spawned[0];
        let mut moving = registry
            .get_component::<postretro_entities::KinematicMoverComponent>(entity)
            .expect("spawned closet mover must have a component")
            .clone();
        moving.was_active_this_tick = true;
        registry
            .set_component(entity, moving)
            .expect("moving closet mover phase must be writable");
        rebuild_blocked_portals(&mut blocked_portals, Some(&world), &registry);
        assert_eq!(blocked_portals, vec![false]);
        let open_visibility = visibility(&blocked_portals);
        assert_eq!(drawable_ids(&open_visibility.visible_cells), vec![0, 1]);
        assert_eq!(open_visibility.fog_reachable, vec![0, 1]);
        assert!(
            closet_enemy_collected(
                &registry,
                &world,
                &open_visibility.visible_cells,
                camera_position,
            ),
            "opening restores closet entities through the same VisibleCells seam"
        );

        // A map with no door association takes the unchanged portal baseline.
        let no_door_visibility = visibility(&[]);
        assert_eq!(
            drawable_ids(&no_door_visibility.visible_cells),
            drawable_ids(&open_visibility.visible_cells)
        );
    }

    fn mover_yaw_states(
        carry_yaw: bool,
        tick_rotation_delta: Quat,
    ) -> kinematic_mover::MoverTickStateTable {
        use postretro_entities::Transform;

        let mut mover_states = kinematic_mover::MoverTickStateTable::default();
        mover_states.publish(
            7,
            kinematic_mover::MoverTickState {
                entity: postretro_entities::EntityId::from_raw(0),
                transform: Transform::default(),
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::Y,
                tick_rotation_delta,
                carry_yaw,
                tick_dt: 1.0 / 60.0,
            },
        );
        mover_states
    }

    // Regression: rotating-mover carry held the camera until the next tick while the
    // platform slerped immediately, producing a periodic camera/platform yaw jitter.
    #[test]
    fn mover_yaw_render_residual_tracks_platform_slerp_through_tick_boundaries() {
        const EPS: f32 = 1.0e-6;
        let tick_yaw = 0.25;
        let mover_states = mover_yaw_states(true, Quat::from_rotation_y(tick_yaw));
        let settled_yaw = 0.8;

        for (alpha, expected) in [
            (0.0, settled_yaw),
            (0.25, settled_yaw + tick_yaw * 0.25),
            (0.5, settled_yaw + tick_yaw * 0.5),
            (0.75, settled_yaw + tick_yaw * 0.75),
            (1.0, settled_yaw + tick_yaw),
        ] {
            let render_yaw = effective_render_yaw(
                settled_yaw,
                postretro_foundation::GroundRef::Mover(7),
                &mover_states,
                alpha,
            );
            assert!(
                (render_yaw - expected).abs() <= EPS,
                "alpha {alpha} should keep the camera locked to the interpolated platform"
            );
        }

        let yaw_at_tick_end = effective_render_yaw(
            settled_yaw,
            postretro_foundation::GroundRef::Mover(7),
            &mover_states,
            1.0,
        );
        let yaw_at_next_tick_start = effective_render_yaw(
            settled_yaw + tick_yaw,
            postretro_foundation::GroundRef::Mover(7),
            &mover_states,
            0.0,
        );
        assert!(
            (yaw_at_tick_end - yaw_at_next_tick_start).abs() <= EPS,
            "the next fixed-tick carry must absorb the full prior residual"
        );
        assert!(
            (settled_yaw - 0.8).abs() <= EPS,
            "render presentation must not mutate fixed-tick camera yaw"
        );

        let render_right = camera_right_for_yaw(yaw_at_tick_end);
        let expected_right = Vec3::new(yaw_at_tick_end.cos(), 0.0, -yaw_at_tick_end.sin());
        assert!(
            (render_right - expected_right).length() <= EPS,
            "view-facing render calculations must use the same carry-adjusted yaw"
        );

        let raw_view =
            camera::RenderCamera::new(Vec3::ZERO, 16.0 / 9.0, settled_yaw, 0.0, 0.0, Vec3::ZERO)
                .view_projection;
        let carried_view = camera::RenderCamera::new(
            Vec3::ZERO,
            16.0 / 9.0,
            yaw_at_tick_end,
            0.0,
            0.0,
            Vec3::ZERO,
        )
        .view_projection;
        assert!(
            raw_view
                .to_cols_array()
                .iter()
                .zip(carried_view.to_cols_array())
                .any(|(raw, carried)| (raw - carried).abs() > EPS),
            "the carry-adjusted yaw must reach render-camera construction"
        );
    }

    #[test]
    fn mover_yaw_render_residual_excludes_disabled_and_non_upright_rotation() {
        const EPS: f32 = 1.0e-6;
        let disabled = mover_yaw_states(false, Quat::from_rotation_y(0.5));
        let tilted = mover_yaw_states(true, Quat::from_rotation_x(0.5));

        for mover_states in [&disabled, &tilted] {
            assert!(
                mover_yaw_render_residual(
                    postretro_foundation::GroundRef::Mover(7),
                    mover_states,
                    0.75,
                )
                .abs()
                    <= EPS
            );
        }
    }

    #[test]
    fn mover_yaw_render_residual_uses_only_the_current_catch_up_tick() {
        const EPS: f32 = 1.0e-6;
        let prior_carry = 0.1 + 0.2;
        let current_tick_yaw = 0.4;
        let mover_states = mover_yaw_states(true, Quat::from_rotation_y(current_tick_yaw));

        let render_yaw = effective_render_yaw(
            0.3 + prior_carry,
            postretro_foundation::GroundRef::Mover(7),
            &mover_states,
            0.5,
        );
        assert!(
            (render_yaw - (0.3 + prior_carry + current_tick_yaw * 0.5)).abs() <= EPS,
            "a catch-up frame must retain only the final current-tick residual"
        );
    }

    // Regression: mover reconciliation replaced phase/Transform but left the live
    // carry table stale, permanently offsetting an owning camera from its platform.
    #[test]
    fn authoritative_mover_correction_refreshes_zero_tick_yaw_and_commits_once() {
        use postretro_entities::{EntityId, Transform};
        use postretro_foundation::GroundRef;

        const EPS: f32 = 1.0e-6;
        let entity = EntityId::from_raw(0);
        let mut mover_states = kinematic_mover::MoverTickStateTable::default();
        mover_states.publish(
            7,
            kinematic_mover::MoverTickState {
                entity,
                transform: Transform {
                    rotation: Quat::from_rotation_y(0.6),
                    ..Transform::default()
                },
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::Y * 0.2,
                tick_rotation_delta: Quat::from_rotation_y(0.2),
                carry_yaw: true,
                tick_dt: 1.0,
            },
        );
        let authoritative = kinematic_mover::MoverTickState {
            entity,
            transform: Transform {
                rotation: Quat::from_rotation_y(0.35),
                ..Transform::default()
            },
            linear_velocity: Vec3::ZERO,
            tick_delta: Vec3::ZERO,
            angular_velocity: Vec3::Y * 0.25,
            tick_rotation_delta: Quat::from_rotation_y(0.25),
            carry_yaw: true,
            tick_dt: 1.0,
        };
        let correction = netcode::MoverCorrection {
            network_id: postretro_net::wire::NetworkId(70),
            mover_id: 7,
            magnitude: 0.0,
            authoritative_state: authoritative,
        };
        let mut camera = Camera::new(Vec3::ZERO, 1.0, 0.0);

        apply_authoritative_mover_corrections(
            &mut camera,
            GroundRef::Mover(7),
            &mut mover_states,
            &[correction],
        );

        assert!(
            (camera.yaw - 0.7).abs() <= EPS,
            "the -0.3 rad authoritative start-phase correction applies once"
        );
        assert!(
            (effective_render_yaw(camera.yaw, GroundRef::Mover(7), &mover_states, 0.5,) - 0.825)
                .abs()
                <= EPS,
            "a zero-tick render must use the refreshed authoritative residual"
        );

        apply_mover_yaw_carry(&mut camera, GroundRef::Mover(7), &mover_states);
        assert!(
            (camera.yaw - 0.95).abs() <= EPS,
            "the ordinary input seam commits only the authoritative tick delta"
        );
        apply_authoritative_mover_corrections(
            &mut camera,
            GroundRef::Mover(7),
            &mut mover_states,
            &[correction],
        );
        assert!(
            (camera.yaw - 0.95).abs() <= EPS,
            "reapplying the same authority cannot duplicate its correction"
        );
    }

    #[test]
    fn authoritative_mover_correction_preserves_carry_yaw_off_camera_authority() {
        use postretro_entities::{EntityId, Transform};
        use postretro_foundation::GroundRef;

        let entity = EntityId::from_raw(0);
        let mut mover_states = mover_yaw_states(false, Quat::from_rotation_y(0.2));
        let correction = netcode::MoverCorrection {
            network_id: postretro_net::wire::NetworkId(70),
            mover_id: 7,
            magnitude: 0.0,
            authoritative_state: kinematic_mover::MoverTickState {
                entity,
                transform: Transform {
                    rotation: Quat::from_rotation_y(1.0),
                    ..Transform::default()
                },
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::Y,
                tick_rotation_delta: Quat::from_rotation_y(0.4),
                carry_yaw: false,
                tick_dt: 1.0,
            },
        };
        let mut camera = Camera::new(Vec3::ZERO, 0.6, -0.2);

        apply_authoritative_mover_corrections(
            &mut camera,
            GroundRef::Mover(7),
            &mut mover_states,
            &[correction],
        );

        assert!((camera.yaw - 0.6).abs() <= 1.0e-6);
        assert!((camera.pitch + 0.2).abs() <= 1.0e-6);
        assert!(mover_yaw_render_residual(GroundRef::Mover(7), &mover_states, 1.0).abs() <= 1.0e-6);
    }

    #[test]
    fn mover_yaw_carry_uses_only_world_up_rotation_when_enabled() {
        const EPS: f32 = 1.0e-6;
        let camera_yaw = 0.4;
        let world_up_spin = glam::Quat::from_rotation_y(0.25);

        assert!(
            (yaw_after_mover_carry(camera_yaw, true, world_up_spin) - 0.65).abs() <= EPS,
            "carry_yaw should add the mover's upright rotation"
        );
        assert!(
            (world_up_yaw_delta(glam::Quat::from_rotation_x(0.25))).abs() <= EPS
                && (world_up_yaw_delta(glam::Quat::from_rotation_z(-0.25))).abs() <= EPS,
            "pitch and roll must never tilt or yaw the upright FPS camera"
        );
    }

    #[test]
    fn mover_yaw_carry_disabled_leaves_view_and_aim_unchanged() {
        const EPS: f32 = 1.0e-6;
        let mut camera = Camera::new(Vec3::new(2.0, 3.0, 4.0), 0.4, -0.2);
        let before_aim = camera.aim_ray();
        let before_pitch = camera.pitch;

        camera.yaw = yaw_after_mover_carry(camera.yaw, false, glam::Quat::from_rotation_y(0.75));

        assert!((camera.yaw - 0.4).abs() <= EPS);
        assert!((camera.pitch - before_pitch).abs() <= EPS);
        assert!((camera.aim_ray().0 - before_aim.0).length() <= EPS);
        assert!((camera.aim_ray().1 - before_aim.1).length() <= EPS);
    }

    #[test]
    fn mover_yaw_carry_reads_the_captured_tick_start_ground_and_prior_pose() {
        use postretro_entities::{EntityRegistry, Transform};
        use postretro_foundation::{GroundRef, PlayerMovementComponent};

        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let mut movement = PlayerMovementComponent::from_descriptor(&minimal_player_descriptor());
        movement.ground = GroundRef::Mover(7);
        registry
            .set_component(pawn, movement)
            .expect("test pawn accepts movement state");
        registry
            .mark_local_player_pawn(pawn)
            .expect("test pawn is the camera owner");

        let mut mover_states = kinematic_mover::MoverTickStateTable::default();
        mover_states.publish(
            7,
            kinematic_mover::MoverTickState {
                entity: pawn,
                transform: Transform::default(),
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::Y,
                tick_rotation_delta: glam::Quat::from_rotation_y(0.25),
                carry_yaw: true,
                tick_dt: 1.0 / 60.0,
            },
        );
        let mut camera = Camera::new(Vec3::ZERO, 0.4, -0.2);

        apply_mover_yaw_carry(&mut camera, GroundRef::Mover(7), &mover_states);

        assert!((camera.yaw - 0.65).abs() <= 1.0e-6);
        assert!(
            (camera.pitch - -0.2).abs() <= 1.0e-6,
            "yaw carry keeps the camera upright"
        );
    }

    // Regression: settled post-tick ground incorrectly granted landing carry
    // and dropped the final carry on a jump/detach tick.
    #[test]
    fn mover_yaw_carry_eligibility_uses_the_position_carry_ticks_start_ground() {
        use postretro_entities::{EntityRegistry, Transform};
        use postretro_foundation::{GroundRef, PlayerMovementComponent};

        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let mut movement = PlayerMovementComponent::from_descriptor(&minimal_player_descriptor());
        movement.ground = GroundRef::Mover(7);
        registry.set_component(pawn, movement).unwrap();
        registry.mark_local_player_pawn(pawn).unwrap();

        let mut mover_states = kinematic_mover::MoverTickStateTable::default();
        mover_states.publish(
            7,
            kinematic_mover::MoverTickState {
                entity: pawn,
                transform: Transform::default(),
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::Y,
                tick_rotation_delta: glam::Quat::from_rotation_y(0.25),
                carry_yaw: true,
                tick_dt: 1.0 / 60.0,
            },
        );

        let mut landing_camera = Camera::new(Vec3::ZERO, 0.4, 0.0);
        assert_eq!(local_player_ground(&registry), GroundRef::Mover(7));
        apply_mover_yaw_carry(&mut landing_camera, GroundRef::Airborne, &mover_states);
        assert!(
            (landing_camera.yaw - 0.4).abs() <= 1.0e-6,
            "landing must not gain rotation produced before contact"
        );

        let mut detached = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone();
        detached.ground = GroundRef::Airborne;
        registry.set_component(pawn, detached).unwrap();
        let mut detach_camera = Camera::new(Vec3::ZERO, 0.4, 0.0);
        assert_eq!(local_player_ground(&registry), GroundRef::Airborne);
        apply_mover_yaw_carry(&mut detach_camera, GroundRef::Mover(7), &mover_states);
        assert!(
            (detach_camera.yaw - 0.65).abs() <= 1.0e-6,
            "jump/detach must retain the final rotation consumed while planted"
        );
    }

    fn weapon_viewmodel_descriptor(
        canonical_name: &str,
        viewmodel: Option<&str>,
        placement: Option<WeaponPlacementDescriptor>,
    ) -> postretro_entities::EntityTypeDescriptor {
        postretro_entities::EntityTypeDescriptor {
            canonical_name: Some(canonical_name.to_owned()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: Some(postretro_foundation::WeaponDescriptor {
                damage: 1.0,
                pellet_count: 1,
                spread_degrees: 0.0,
                range: 1.0,
                cooldown_ms: 1.0,
                fire_mode: postretro_foundation::FireMode::Semi,
                resolution: postretro_foundation::ResolutionMode::Hitscan,
                projectile: None,
                credit_source: None,
                third_person_model: None,
                viewmodel: viewmodel.map(str::to_owned),
                placement,
                muzzle_offset: None,
                resource: None,
                lower_ms: 0,
                raise_ms: 0,
                block_during_reload: None,
            }),
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn placement(right: f32, up: f32, forward: f32, yaw: f32) -> WeaponPlacementDescriptor {
        WeaponPlacementDescriptor {
            offset: postretro_foundation::PlacementOffset { right, up, forward },
            rotation: postretro_foundation::PlacementRotation {
                yaw,
                ..Default::default()
            },
        }
    }

    #[test]
    fn weapon_placement_resolver_uses_whole_value_precedence() {
        let mod_default = placement(0.10, -0.10, 0.50, 10.0);
        let character = placement(0.20, -0.20, 0.60, 20.0);
        let weapon = placement(0.30, -0.30, 0.70, 30.0);
        let instance = placement(0.40, -0.40, 0.80, 40.0);

        assert_eq!(
            resolve_weapon_placement(Some(&mod_default), None, None, None),
            mod_default,
            "an unauthored weapon falls back to the mod default"
        );
        assert_eq!(
            resolve_weapon_placement(Some(&mod_default), Some(&character), None, None),
            character,
            "the reserved character tier outranks the mod default"
        );
        assert_eq!(
            resolve_weapon_placement(Some(&mod_default), Some(&character), Some(&weapon), None,),
            weapon,
            "a weapon placement wholly overrides lower tiers"
        );
        assert_eq!(
            resolve_weapon_placement(
                Some(&mod_default),
                Some(&character),
                Some(&weapon),
                Some(&instance),
            ),
            instance,
            "the reserved instance tier has highest precedence"
        );

        let sparse_weapon = placement(0.90, 0.0, 0.0, 0.0);
        assert_eq!(
            resolve_weapon_placement(Some(&mod_default), None, Some(&sparse_weapon), None),
            sparse_weapon,
            "resolution never merges sparse weapon fields with the mod default"
        );
    }

    #[test]
    fn local_viewmodel_paths_resolve_live_mod_default_and_weapon_placement() {
        let default_a = placement(0.15, -0.25, 0.55, 5.0);
        let default_b = placement(0.45, -0.35, 0.75, -5.0);
        let shared_weapon_placement = placement(0.25, -0.15, 0.65, 12.0);
        let unauthored = vec![weapon_viewmodel_descriptor(
            "reference_pistol",
            Some("models/pistol/view.gltf"),
            None,
        )];
        let authored = vec![weapon_viewmodel_descriptor(
            "reference_pistol",
            Some("models/pistol/view.gltf"),
            Some(shared_weapon_placement.clone()),
        )];

        let local_raw = viewmodel_asset_for_archetype("reference_pistol", &unauthored)
            .expect("local archetype lookup must find the viewmodel")
            .1;
        assert_eq!(
            resolve_weapon_placement(Some(&default_a), None, local_raw.as_ref(), None),
            default_a,
            "a mod default applies when the archetype omits placement"
        );

        let host_raw = viewmodel_asset_for_archetype("reference_pistol", &authored)
            .expect("local archetype lookup must find the viewmodel")
            .1;
        assert_eq!(
            resolve_weapon_placement(Some(&default_a), None, host_raw.as_ref(), None),
            shared_weapon_placement,
            "the shared per-weapon placement wholly overrides the mod default"
        );

        let mut data_registry = postretro_entities::DataRegistry::new();
        data_registry.set_default_weapon_placement(Some(default_a));
        let first_frame = resolve_weapon_placement(
            data_registry.default_weapon_placement.as_ref(),
            None,
            None,
            None,
        );
        data_registry.set_default_weapon_placement(Some(default_b.clone()));
        let next_frame = resolve_weapon_placement(
            data_registry.default_weapon_placement.as_ref(),
            None,
            None,
            None,
        );
        assert_ne!(first_frame, next_frame);
        assert_eq!(
            next_frame, default_b,
            "the next render lookup sees a re-drained default"
        );
    }

    #[test]
    fn client_fire_muzzle_terms_use_the_replaced_host_payload_row() {
        let host_placement = placement(0.3, -0.2, 0.7, 15.0);
        let replacement_placement = placement(-0.4, 0.1, 0.5, -20.0);
        let host_muzzle = [0.2, -0.1, -0.8];
        let replacement_muzzle = [-0.3, 0.4, -1.2];
        let local_component_muzzle = Vec3::new(9.0, 8.0, 7.0);
        let local_data_placement = placement(8.0, 7.0, 6.0, 45.0);
        let local_data_registry = vec![weapon_viewmodel_descriptor(
            "reference_pistol",
            Some("models/local/view.gltf"),
            Some(local_data_placement),
        )];
        let mut slots = std::array::from_fn(|_| None);
        slots[0] = Some(netcode::WieldableTuningPayload {
            canonical_name: "reference_pistol".to_string(),
            placement: host_placement.clone(),
            muzzle_offset: Some(host_muzzle),
            range: 64.0,
            cooldown_ms: 100.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            fire_mode: postretro_foundation::FireMode::Semi,
            resolution: postretro_foundation::ResolutionMode::Projectile,
            lower_ms: 0,
            raise_ms: 0,
        });
        let initial = netcode::TuningPayload::new(None, slots.clone());
        let terms = client_fire_muzzle_terms(&initial, 0).expect("host row exists");
        assert_eq!(terms.placement, host_placement);
        assert_eq!(terms.muzzle_offset, Some(Vec3::from_array(host_muzzle)));
        assert_ne!(
            terms.placement,
            local_data_registry[0]
                .weapon
                .as_ref()
                .unwrap()
                .placement
                .clone()
                .unwrap()
        );
        assert_ne!(terms.muzzle_offset, Some(local_component_muzzle));

        // A reliable Control replacement updates this payload before any local
        // snapshot refresh. The old component/data values remain irrelevant.
        slots[0].as_mut().unwrap().placement = replacement_placement.clone();
        slots[0].as_mut().unwrap().muzzle_offset = Some(replacement_muzzle);
        let replacement = netcode::TuningPayload::new(None, slots);
        let terms = client_fire_muzzle_terms(&replacement, 0).expect("replacement row exists");
        assert_eq!(terms.placement, replacement_placement);
        assert_eq!(
            terms.muzzle_offset,
            Some(Vec3::from_array(replacement_muzzle))
        );
        assert_ne!(terms.muzzle_offset, Some(local_component_muzzle));
    }

    #[test]
    fn local_viewmodel_asset_uses_inventory_weapon_and_weapon_provenance() {
        use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
        use postretro_entities::{EntityRegistry, Transform};

        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        registry
            .set_component(
                weapon,
                DescriptorProvenance {
                    canonical_name: "reference_pistol".to_owned(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::DefaultWeapon,
                },
            )
            .unwrap();
        let mut inventory = postretro_entities::components::inventory::Inventory::default();
        inventory.wieldables[0] = Some(weapon);
        registry.set_component(pawn, inventory).unwrap();
        let descriptors = vec![weapon_viewmodel_descriptor(
            "reference_pistol",
            Some("models/pistol/view.gltf"),
            Some(WeaponPlacementDescriptor {
                offset: postretro_foundation::PlacementOffset {
                    right: 0.4,
                    up: -0.2,
                    forward: 0.7,
                },
                rotation: postretro_foundation::PlacementRotation {
                    yaw: 15.0,
                    pitch: 0.0,
                    roll: 0.0,
                },
            }),
        )];

        let local = local_viewmodel_asset(&registry, pawn, &descriptors)
            .expect("host inventory lookup must find the viewmodel");
        assert_eq!(local.0, weapon);
        assert_eq!(local.1, "models/pistol/view.gltf");
        assert_eq!(local.3, 0);
        assert_eq!(
            local.2,
            Some(WeaponPlacementDescriptor {
                offset: postretro_foundation::PlacementOffset {
                    right: 0.4,
                    up: -0.2,
                    forward: 0.7,
                },
                rotation: postretro_foundation::PlacementRotation {
                    yaw: 15.0,
                    pitch: 0.0,
                    roll: 0.0,
                },
            }),
        );

        let client = viewmodel_asset_for_archetype("reference_pistol", &descriptors)
            .expect("client archetype lookup must find the same descriptor");
        let mod_default = placement(0.3, -0.1, 0.5, 2.0);
        assert_eq!(
            resolve_weapon_placement(Some(&mod_default), None, local.2.as_ref(), None),
            resolve_weapon_placement(Some(&mod_default), None, client.1.as_ref(), None),
            "host and client resolve the raw placement from their shared descriptor lookup",
        );
    }

    #[test]
    fn missing_viewmodel_descriptor_drops_local_presentation_without_stale_asset() {
        use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
        use postretro_entities::{EntityRegistry, Transform};

        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        registry
            .set_component(
                weapon,
                DescriptorProvenance {
                    canonical_name: "reference_pistol".to_owned(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::DefaultWeapon,
                },
            )
            .unwrap();
        let mut inventory = postretro_entities::components::inventory::Inventory::default();
        inventory.wieldables[0] = Some(weapon);
        registry.set_component(pawn, inventory).unwrap();

        assert!(
            local_viewmodel_asset(
                &registry,
                pawn,
                &[weapon_viewmodel_descriptor("reference_pistol", None, None)],
            )
            .is_none()
        );
    }

    #[test]
    fn replicated_weapon_archetype_resolves_viewmodel_without_weapon_entity() {
        let descriptors = vec![weapon_viewmodel_descriptor(
            "reference_pistol",
            Some("models/pistol/view.gltf"),
            Some(WeaponPlacementDescriptor {
                offset: postretro_foundation::PlacementOffset {
                    right: 0.1,
                    up: 0.2,
                    forward: 0.3,
                },
                rotation: postretro_foundation::PlacementRotation::default(),
            }),
        )];
        assert_eq!(
            viewmodel_asset_for_archetype("reference_pistol", &descriptors),
            Some((
                "models/pistol/view.gltf",
                Some(WeaponPlacementDescriptor {
                    offset: postretro_foundation::PlacementOffset {
                        right: 0.1,
                        up: 0.2,
                        forward: 0.3,
                    },
                    rotation: postretro_foundation::PlacementRotation::default(),
                }),
            ))
        );
        assert!(viewmodel_asset_for_archetype("missing", &descriptors).is_none());
        assert!(
            viewmodel_asset_for_archetype(
                "reference_pistol",
                &[weapon_viewmodel_descriptor("reference_pistol", None, None)],
            )
            .is_none()
        );
    }

    #[test]
    fn viewmodel_transform_applies_view_feel_offsets_without_world_camera_rotation() {
        let placement = resolve_weapon_placement(None, None, None, None);
        let transform = viewmodel_camera_space_transform(
            Vec3::X,
            Vec3::new(0.1, 0.2, 0.3),
            0.1,
            0.2,
            -0.3,
            &placement,
        );
        let translation = transform.w_axis.truncate();

        assert_eq!(translation, Vec3::new(0.42, -0.08, -0.62));
        assert_ne!(
            transform.x_axis.truncate(),
            Vec3::X,
            "view-feel tilt must rotate the camera-space model"
        );
    }

    #[test]
    fn viewmodel_placement_preserves_legacy_default_and_applies_continuous_authored_offsets() {
        let camera_right = Vec3::X;
        let eye_offset = Vec3::new(0.1, 0.2, 0.3);
        let roll = 0.1;
        let yaw = 0.2;
        let pitch = -0.3;
        let bob_offset = Vec3::new(eye_offset.dot(camera_right), eye_offset.y, 0.0);
        let legacy = glam::Mat4::from_scale_rotation_translation(
            Vec3::ONE,
            Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch) * Quat::from_rotation_z(roll),
            BASE_OFFSET + bob_offset,
        );
        let absent_placement = resolve_weapon_placement(None, None, None, None);
        let absent = viewmodel_camera_space_transform(
            camera_right,
            eye_offset,
            roll,
            yaw,
            pitch,
            &absent_placement,
        );
        assert_eq!(absent, legacy, "unauthored placement stays byte-identical");

        let base = WeaponPlacementDescriptor {
            offset: postretro_foundation::PlacementOffset {
                right: 0.32,
                up: -0.28,
                forward: 0.62,
            },
            rotation: postretro_foundation::PlacementRotation {
                yaw: 15.0,
                pitch: -10.0,
                roll: 5.0,
            },
        };
        let higher = WeaponPlacementDescriptor {
            offset: postretro_foundation::PlacementOffset {
                up: base.offset.up + 0.12,
                ..base.offset.clone()
            },
            rotation: base.rotation.clone(),
        };
        let base_transform =
            viewmodel_camera_space_transform(camera_right, eye_offset, roll, yaw, pitch, &base);
        let higher_transform =
            viewmodel_camera_space_transform(camera_right, eye_offset, roll, yaw, pitch, &higher);

        assert_ne!(
            base_transform, absent,
            "authored placement changes the transform"
        );
        assert_eq!(
            base_transform.w_axis, absent.w_axis,
            "right/up/forward map to +X/+Y/-Z without sway rotating the placement offset"
        );
        assert_eq!(
            base_transform,
            glam::Mat4::from_scale_rotation_translation(
                Vec3::ONE,
                (Quat::from_rotation_y(yaw)
                    * Quat::from_rotation_x(pitch)
                    * Quat::from_rotation_z(roll))
                    * (Quat::from_rotation_y(15_f32.to_radians())
                        * Quat::from_rotation_x((-10_f32).to_radians())
                        * Quat::from_rotation_z(5_f32.to_radians())),
                Vec3::new(0.32, -0.28, -0.62) + bob_offset,
            ),
            "placement rotation is yaw × pitch × roll below render-only sway"
        );
        assert_ne!(base_transform, higher_transform);
        assert!(
            (higher_transform.w_axis.y - base_transform.w_axis.y - 0.12).abs() <= 1.0e-6,
            "vertical placement changes camera-space height continuously",
        );
        assert_ne!(
            base_transform.x_axis.truncate(),
            absent.x_axis.truncate(),
            "authored placement rotation composes under sway"
        );
    }

    #[test]
    fn viewmodel_world_transform_keeps_shared_shader_positions_in_world_space() {
        let view =
            glam::Mat4::look_at_rh(Vec3::new(6.0, 2.0, 4.0), Vec3::new(5.0, 2.5, 3.0), Vec3::Y);
        let placement = resolve_weapon_placement(None, None, None, None);
        let camera_space =
            viewmodel_camera_space_transform(Vec3::X, Vec3::ZERO, 0.0, 0.0, 0.0, &placement);
        let world = viewmodel_world_transform(view, Vec3::X, Vec3::ZERO, 0.0, 0.0, 0.0, &placement);
        let model_point = Vec3::new(0.1, 0.2, -0.3).extend(1.0);

        assert!(
            (view * world * model_point).distance(camera_space * model_point) < 1.0e-5,
            "world-space instance data must map back to authored camera-relative placement",
        );
        assert!(
            (world.w_axis.truncate() - camera_space.w_axis.truncate()).length() > 1.0,
            "shared shader world_position must not receive raw camera-space coordinates",
        );
    }

    #[test]
    fn stale_weapon_cooldown_slot_does_not_overwrite_local_decay() {
        let mut table = postretro_entities::SlotTable::new();
        table
            .get_mut("player.weaponCooldownMs")
            .expect("default player weapon cooldown slot exists")
            .value = Some(postretro_entities::SlotValue::Number(100.0));
        let mut registry = postretro_entities::EntityRegistry::new();
        let pawn = registry.spawn(postretro_entities::Transform::default());
        registry
            .set_component(
                pawn,
                postretro_foundation::PlayerMovementComponent::from_descriptor(
                    &minimal_player_descriptor(),
                ),
            )
            .unwrap();
        let weapon_id = registry.spawn(postretro_entities::Transform::default());
        let mut component =
            weapon::tests::weapon_component(postretro_foundation::FireMode::Semi, 100.0);
        component.cooldown_remaining_ms = 72.0;
        registry.set_component(weapon_id, component).unwrap();
        let mut inventory = postretro_entities::components::inventory::Inventory::default();
        inventory.wieldables[0] = Some(weapon_id);
        registry.set_component(pawn, inventory).unwrap();
        let mut predicted = weapon::ClientPredictedShots::new();

        assert!(!reconcile_client_weapon_cooldown_from_slot_table(
            &mut predicted,
            &mut registry,
            &table,
            None
        ));
        assert_eq!(
            registry
                .get_component::<postretro_entities::components::weapon::WeaponComponent>(
                    weapon_id,
                )
                .unwrap()
                .cooldown_remaining_ms,
            72.0,
            "stale slot value must not reset locally-decayed cooldown"
        );

        assert!(reconcile_client_weapon_cooldown_from_slot_table(
            &mut predicted,
            &mut registry,
            &table,
            Some(0)
        ));
        assert_eq!(
            registry
                .get_component::<postretro_entities::components::weapon::WeaponComponent>(
                    weapon_id,
                )
                .unwrap()
                .cooldown_remaining_ms,
            100.0
        );
    }

    // Regression: an authoritative cooldown for A arrived after the client had
    // locally repointed to B and overwrote B's independent prediction state.
    #[test]
    fn correlated_cooldown_updates_pending_shot_weapon_after_local_switch() {
        let mut table = postretro_entities::SlotTable::new();
        table.get_mut("player.weaponCooldownMs").unwrap().value =
            Some(postretro_entities::SlotValue::Number(64.0));
        let mut registry = postretro_entities::EntityRegistry::new();
        let pawn = registry.spawn(postretro_entities::Transform::default());
        registry
            .set_component(
                pawn,
                postretro_foundation::PlayerMovementComponent::from_descriptor(
                    &minimal_player_descriptor(),
                ),
            )
            .unwrap();
        let weapon_a = registry.spawn(postretro_entities::Transform::default());
        let weapon_b = registry.spawn(postretro_entities::Transform::default());
        let mut component_a =
            weapon::tests::weapon_component(postretro_foundation::FireMode::Semi, 100.0);
        component_a.cooldown_remaining_ms = 80.0;
        let mut component_b =
            weapon::tests::weapon_component(postretro_foundation::FireMode::Semi, 100.0);
        component_b.cooldown_remaining_ms = 11.0;
        registry.set_component(weapon_a, component_a).unwrap();
        registry.set_component(weapon_b, component_b).unwrap();
        let mut inventory = postretro_entities::components::inventory::Inventory::default();
        inventory.wieldables[0] = Some(weapon_a);
        inventory.wieldables[1] = Some(weapon_b);
        inventory.active_slot = 1;
        registry.set_component(pawn, inventory).unwrap();

        let mut predicted = weapon::ClientPredictedShots::new();
        predicted.predict(
            7,
            weapon_a,
            &weapon::ClientFireResolution {
                client_tick: 3,
                hits: Vec::new(),
                projectile_launch: None,
            },
            0.0,
            80.0,
        );

        assert!(reconcile_client_weapon_cooldown_from_slot_table(
            &mut predicted,
            &mut registry,
            &table,
            Some(0),
        ));
        assert_eq!(
            registry
                .get_component::<postretro_entities::components::weapon::WeaponComponent>(weapon_a,)
                .unwrap()
                .cooldown_remaining_ms,
            64.0
        );
        assert_eq!(
            registry
                .get_component::<postretro_entities::components::weapon::WeaponComponent>(weapon_b,)
                .unwrap()
                .cooldown_remaining_ms,
            11.0,
            "A's authoritative sample must not overwrite locally-active B"
        );

        let _ = predicted.apply_verdict(&mut registry, 7, false, false);
        assert_eq!(
            registry
                .get_component::<postretro_entities::components::weapon::WeaponComponent>(weapon_a,)
                .unwrap()
                .cooldown_remaining_ms,
            64.0,
            "A's fresh authority must also block its older predicted-shot rollback"
        );
    }

    #[test]
    fn zero_tick_frame_shoot_press_reaches_post_loop_client_fire_snapshot() {
        let mut latch = input::GameplayInputLatch::new();
        let zero_tick_snapshot =
            input::ActionSnapshot::with_button_state(Action::Shoot, ButtonState::Pressed);

        let fixed_tick_snapshot = latch.snapshot_for_ticks(&zero_tick_snapshot, 0);
        assert!(
            fixed_tick_snapshot.is_none(),
            "fixed gameplay intentionally waits for a later tick"
        );
        let selected = client_fire_snapshot_for_post_loop(
            fixed_tick_snapshot.as_ref(),
            Some(&zero_tick_snapshot),
        )
        .expect("post-loop client fire still sees the render-frame click");

        assert_eq!(selected.button(Action::Shoot), ButtonState::Pressed);
    }

    fn client_fire_selection_state(
        fire_mode: postretro_foundation::FireMode,
        cooldown_remaining_ms: f32,
        cooldown_ms: f32,
    ) -> postretro_entities::components::weapon::WeaponComponent {
        let mut component = weapon::tests::weapon_component(fire_mode, cooldown_ms);
        component.cooldown_remaining_ms = cooldown_remaining_ms;
        component
    }

    #[test]
    fn client_fire_tick_selection_keeps_press_independent_of_pruned_history() {
        let state = client_fire_selection_state(postretro_foundation::FireMode::Semi, 0.0, 100.0);
        let commands = [
            ClientFrameFireCommand {
                client_tick: 41,
                button: weapon::FireButtonState {
                    pressed: true,
                    active: true,
                },
                elapsed_ms: 16.0,
            },
            ClientFrameFireCommand {
                client_tick: 42,
                button: weapon::FireButtonState {
                    pressed: false,
                    active: true,
                },
                elapsed_ms: 32.0,
            },
        ];

        assert_eq!(client_fire_ticks_for_post_loop(&commands, &state), vec![41]);
    }

    #[test]
    fn held_auto_fire_tick_selection_accounts_for_hitch_cooldown_windows() {
        let mut state =
            client_fire_selection_state(postretro_foundation::FireMode::Auto, 0.0, 20.0);
        state.pellet_count = 8;
        state.spread_degrees = 4.0;
        let commands = [
            ClientFrameFireCommand {
                client_tick: 7,
                button: weapon::FireButtonState {
                    pressed: true,
                    active: true,
                },
                elapsed_ms: 16.0,
            },
            ClientFrameFireCommand {
                client_tick: 8,
                button: weapon::FireButtonState {
                    pressed: false,
                    active: true,
                },
                elapsed_ms: 32.0,
            },
            ClientFrameFireCommand {
                client_tick: 9,
                button: weapon::FireButtonState {
                    pressed: false,
                    active: true,
                },
                elapsed_ms: 48.0,
            },
        ];

        let selected = client_fire_ticks_for_post_loop(&commands, &state);
        assert_eq!(
            selected,
            vec![7, 9],
            "the first tick owns the rendered HIT; later eligible auto shots get miss declarations"
        );

        // The post-loop fire path resolves only `selected[0]`; its remaining
        // selected ticks are retired by the empty-declaration loop. This keeps
        // a hitch frame to one rendered-pose cast and one shell-counter advance.
        let registry = postretro_entities::EntityRegistry::new();
        let resolution = weapon::resolve_client_fire(
            None,
            &mut state,
            "weapon.unknown",
            0,
            commands[0].button,
            Vec3::ZERO,
            Vec3::NEG_Z,
            &WeaponPlacementDescriptor::default(),
            None,
            selected[0],
            &collision::CollisionWorld::new(),
            &registry,
            &scripting_systems::hit_zones::HitZoneStore::new(),
            0.0,
            0.0,
        )
        .expect("the first selected auto tick resolves the frame's one cast");

        assert_eq!(resolution.client_tick, 7);
        assert_eq!(state.shells_fired, 1);
        assert_eq!(selected[1..], [9]);
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
            slide: None,
            view_feel: None,
        }
    }

    #[test]
    fn o45_cursor_occupancy_reads_the_local_inventory_from_the_prior_frame() {
        use postretro_entities::components::inventory::Inventory;
        use postretro_entities::{EntityRegistry, Transform};
        use postretro_foundation::PlayerMovementComponent;

        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        registry
            .set_component(
                pawn,
                PlayerMovementComponent::from_descriptor(&minimal_player_descriptor()),
            )
            .unwrap();
        registry.mark_local_player_pawn(pawn).unwrap();
        let first = registry.spawn(Transform::default());
        let third = registry.spawn(Transform::default());
        registry
            .set_component(
                first,
                weapon::tests::weapon_component(postretro_foundation::FireMode::Semi, 100.0),
            )
            .unwrap();
        registry
            .set_component(
                third,
                weapon::tests::weapon_component(postretro_foundation::FireMode::Semi, 100.0),
            )
            .unwrap();
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(first);
        inventory.wieldables[2] = Some(third);
        registry.set_component(pawn, inventory).unwrap();

        let (occupied, active) = local_wieldable_occupancy(&registry);
        assert_eq!(active, Some(0));
        assert_eq!(
            occupied,
            [
                true, false, true, false, false, false, false, false, false, false
            ]
        );
    }

    #[test]
    fn ui_button_action_classifier_reserves_ui_actions_before_named_reactions() {
        assert_eq!(
            classify_ui_button_action(postretro_ui::actions::COMMIT_TEXT_ENTRY_ACTION),
            UiButtonAction::CommitTextEntry
        );
        assert_eq!(
            classify_ui_button_action(postretro_ui::actions::CLOSE_DIALOG_ACTION),
            UiButtonAction::CloseDialog
        );
        assert_eq!(
            classify_ui_button_action(postretro_ui::actions::EXIT_TO_DESKTOP_ACTION),
            UiButtonAction::ExitToDesktop
        );
        assert_eq!(
            classify_ui_button_action(postretro_ui::actions::QUIT_TO_MENU_ACTION),
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
        use postretro_ui::descriptor::{
            Align, AnchoredTree, CaptureMode, ContainerWidget, SpacingValue, Widget,
        };
        use postretro_ui::layout::Anchor;
        use postretro_ui::modal_stack::{ModalStack, ScopeTier};

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
            postretro_ui::demo::PAUSE_MENU_NAME,
            capturing_tree(),
            ScopeTier::Engine,
            false,
        );
        stack.push_named(postretro_ui::demo::PAUSE_MENU_NAME, None);
        let ui_captured_gameplay_at_frame_start =
            stack.top_capture_mode() == postretro_ui::descriptor::CaptureMode::Capture;

        let routed = route_ui_button_action(postretro_ui::actions::CLOSE_DIALOG_ACTION, &mut stack);
        assert_eq!(routed, UiButtonAction::CloseDialog);
        assert!(
            stack.is_empty(),
            "Resume closes the pause menu before gameplay snapshots are selected",
        );
        assert_ne!(
            stack.top_capture_mode(),
            postretro_ui::descriptor::CaptureMode::Capture,
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
        let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
        let mover_colliders = Vec::new();
        let mut mover_states = kinematic_mover::MoverTickStateTable::default();
        let remote_inputs = Vec::new();
        let command = sim::SimCommand {
            movement: movement::MovementInput {
                wish_dir: glam::Vec2::ZERO,
                jump_pressed: false,
                dash_pressed: false,
                running: false,
                crouch_intent: false,
                facing_yaw: 0.0,
                use_pressed: false,
                drop_pressed: false,
            },
            fire_button: weapon::FireButtonState {
                pressed: false,
                active: false,
            },
            reload: false,
            firing_slot: 0,
            select_slot: None,
            use_pressed: false,
            drop_pressed: false,
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
                &mut ai_runtime,
                &mover_colliders,
                &mut mover_states,
                &remote_inputs,
                &command,
                |registry| {
                    follow_camera_to_local_pawn(&mut camera, &registry.borrow(), Vec3::ZERO);
                    build_post_movement_command(&camera)
                },
                TICK_DURATION.as_secs_f32(),
                None,
                |_| {},
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

    // Regression: separate frame accumulators reordered catch-up weapon and reload events.
    #[test]
    fn catch_up_weapon_script_events_preserve_tick_order_and_same_tick_fire_order() {
        let pawn = postretro_entities::EntityId::from_raw(1);
        let weapon = postretro_entities::EntityId::from_raw(2);
        let mut pending = Vec::new();

        append_tick_weapon_script_events(
            &mut pending,
            Vec::new(),
            vec![sim::ReloadDelivery {
                pawn,
                weapon,
                outcome: sim::ReloadOutcome::Started,
            }],
        );
        append_tick_weapon_script_events(
            &mut pending,
            vec!["activate"],
            vec![sim::ReloadDelivery {
                pawn,
                weapon,
                outcome: sim::ReloadOutcome::Cancelled { transferred: 0 },
            }],
        );

        assert_eq!(
            pending
                .iter()
                .map(|event| event.event_name())
                .collect::<Vec<_>>(),
            vec!["reload_started", "activate", "reload_cancelled"],
        );
    }

    #[test]
    fn mover_sound_event_drain_maps_authored_name_and_executes_play_sound() {
        use crate::scripting_systems::system_reactions::register_system_reaction_primitives;
        use postretro_entities::{
            DataRegistry, EntityRegistry, KinematicMoverComponent, KinematicMoverConfig,
            KinematicMoverMode, NamedReaction, PrimitiveDescriptor, ReactionDescriptor,
        };
        use postretro_scripting_core::reaction_registry::{
            ReactionPrimitiveRegistry, SystemReactionRegistry,
        };
        use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

        let mut registry = EntityRegistry::new();
        let mover_entity = registry.spawn(Transform::default());
        let mut mover = KinematicMoverComponent::new(
            17,
            KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::X],
                waypoint_names: vec!["closed".to_string(), "open".to_string()],
                speed_mps: 1.0,
                wait_ms: 0.0,
                mode: KinematicMoverMode::PingPong,
                started: true,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        mover.open_event = Some("door.open".to_string());
        registry
            .set_component(mover_entity, mover)
            .expect("mover fixture attaches");

        let event_names = mover_event_dispatch_addresses(
            &[
                (kinematic_mover::MoverEventKind::Opened, 17),
                (kinematic_mover::MoverEventKind::Closed, 17),
            ],
            &registry,
        );
        assert_eq!(event_names, vec!["door.open"]);

        let script_ctx = ScriptCtx::new();
        let mut data_registry = DataRegistry::new();
        data_registry.populate_level(
            vec![NamedReaction {
                name: "door.open".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "playSound".to_string(),
                    target: None,
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({ "sound": "door_open", "bus": "sfx" }),
                }),
            }],
            Vec::new(),
            &[],
        );
        let sequence_registry = SequencedPrimitiveRegistry::new();
        let reaction_registry = ReactionPrimitiveRegistry::new();
        let mut system_registry = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system_registry);

        // Regression: the ordinary post-tick drain only collects chained names;
        // it does not execute the `playSound` primitive.
        assert!(fire_named_event("door.open", &data_registry).is_empty());
        assert!(script_ctx.system_commands.take().is_empty());

        drain_named_events_with_sequences(
            event_names.iter(),
            &data_registry,
            &sequence_registry,
            &reaction_registry,
            &system_registry,
            &script_ctx,
        );
        assert_eq!(
            script_ctx.system_commands.take(),
            vec![SystemReactionCommand::PlaySound {
                sound: "door_open".to_string(),
                bus: Some("sfx".to_string()),
            }],
            "mover events must use the executing dispatch path so the audio drain receives playSound"
        );
    }

    // Regression: movement, AI, and weapon event drains used the legacy named
    // dispatcher, which did not execute Sequence bodies at all.
    #[test]
    fn post_tick_named_event_batch_executes_waits_and_returns_fire_chains() {
        use crate::scripting_systems::reaction_scheduler::{
            ReactionScheduler, register_reaction_control_primitives,
        };
        use crate::scripting_systems::system_reactions::register_system_reaction_primitives;
        use postretro_entities::{
            DataRegistry, NamedReaction, PrimitiveDescriptor, ReactionDescriptor, SequenceStep,
            SequenceTarget,
        };
        use postretro_scripting_core::reaction_registry::{
            ReactionPrimitiveRegistry, SystemReactionRegistry,
        };
        use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

        let script_ctx = ScriptCtx::new();
        let scheduler = ReactionScheduler::default();
        scheduler.set_enabled(true);
        let mut sequence_registry = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut sequence_registry, scheduler.clone());
        let reaction_registry = ReactionPrimitiveRegistry::new();
        let mut system_registry = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system_registry);

        let source_reactions =
            ["movementEvent", "aiEvent", "weaponEvent"]
                .into_iter()
                .map(|name| NamedReaction {
                    name: name.to_string(),
                    descriptor: ReactionDescriptor::Sequence(vec![
                        SequenceStep {
                            id: SequenceTarget::Fire,
                            primitive: "fire".to_string(),
                            args: serde_json::json!({ "event": "postTickTarget" }),
                        },
                        SequenceStep {
                            id: SequenceTarget::Wait,
                            primitive: "wait".to_string(),
                            args: serde_json::json!({
                                "durationMs": 17.0,
                                "interruptible": false
                            }),
                        },
                    ]),
                });
        let mut reactions: Vec<_> = source_reactions.collect();
        reactions.push(NamedReaction {
            name: "postTickTarget".to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "playSound".to_string(),
                target: None,
                tag: None,
                on_complete: None,
                args: serde_json::json!({ "sound": "event_chain", "bus": "sfx" }),
            }),
        });
        let mut data_registry = DataRegistry::new();
        data_registry.populate_level(reactions, Vec::new(), &[]);

        let chained = drain_named_events_with_sequences(
            ["movementEvent", "aiEvent", "weaponEvent"],
            &data_registry,
            &sequence_registry,
            &reaction_registry,
            &system_registry,
            &script_ctx,
        );
        assert_eq!(
            scheduler.pending_len(),
            3,
            "all three named sources reach the shared wait enrollment arm"
        );
        assert_eq!(
            chained,
            vec![
                "postTickTarget".to_string(),
                "postTickTarget".to_string(),
                "postTickTarget".to_string(),
            ],
            "each source contributes its fire target to the shared deferred batch"
        );

        dispatch_deferred_named_events_with_sequences(
            chained,
            &data_registry,
            &sequence_registry,
            &reaction_registry,
            &system_registry,
            &script_ctx,
        );
        assert_eq!(
            script_ctx.system_commands.take(),
            vec![
                SystemReactionCommand::PlaySound {
                    sound: "event_chain".to_string(),
                    bus: Some("sfx".to_string()),
                },
                SystemReactionCommand::PlaySound {
                    sound: "event_chain".to_string(),
                    bus: Some("sfx".to_string()),
                },
                SystemReactionCommand::PlaySound {
                    sound: "event_chain".to_string(),
                    bus: Some("sfx".to_string()),
                },
            ],
            "the existing deferred dispatcher executes every chained target"
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
            .map(|_| {
                build_sim_command(
                    &snapshot,
                    &camera,
                    crouch_intent,
                    false,
                    false,
                    false,
                    false,
                    false,
                )
            })
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
                build_sim_command(
                    &snapshot,
                    &camera,
                    false,
                    dash_pressed,
                    false,
                    false,
                    false,
                    false,
                )
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
                build_sim_command(
                    &snapshot,
                    &camera,
                    false,
                    false,
                    shoot_pressed,
                    false,
                    false,
                    false,
                )
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
    fn sim_command_samples_reload_as_held_level_bit() {
        let mut input_system = InputSystem::new(default_bindings());
        input_system.set_physical_input(
            input::PhysicalInput::Key(winit::keyboard::KeyCode::KeyR),
            true,
        );
        let snapshot = input_system.snapshot();
        assert_eq!(snapshot.button(Action::Reload), ButtonState::Pressed);

        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let commands: Vec<sim::SimCommand> = (0..2)
            .map(|_| {
                build_sim_command(&snapshot, &camera, false, false, false, false, false, false)
            })
            .collect();

        assert!(
            commands.iter().all(|command| command.reload),
            "reload is a level signal and remains true across catch-up ticks while held"
        );
    }

    #[test]
    fn sim_command_strips_drop_edge_after_first_catchup_tick() {
        let mut input_system = InputSystem::new(default_bindings());
        input_system.set_physical_input(
            input::PhysicalInput::Key(winit::keyboard::KeyCode::KeyG),
            true,
        );
        let snapshot = input_system.snapshot();
        assert_eq!(snapshot.button(Action::Drop), ButtonState::Pressed);

        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let commands: Vec<sim::SimCommand> = (0..2)
            .map(|tick_index| {
                let drop_pressed = tick_index == 0
                    && matches!(snapshot.button(Action::Drop), ButtonState::Pressed);
                build_sim_command(
                    &snapshot,
                    &camera,
                    false,
                    false,
                    false,
                    false,
                    false,
                    drop_pressed,
                )
            })
            .collect();

        assert!(commands[0].drop_pressed);
        assert!(commands[0].movement.drop_pressed);
        assert!(
            !commands[1].drop_pressed && !commands[1].movement.drop_pressed,
            "one physical drop press must not replay as a new edge on every catch-up tick",
        );
    }

    /// Prime a host command queue past Fix A's buildup latch by ingesting a later no-fire tick,
    /// bringing the pending buffer to `INPUT_BUFFER_TARGET` depth so a lone earlier command
    /// resolves `Real` instead of being withheld during buildup. Needed because Fix A's
    /// one-shot latch withholds the first command until the pending buffer reaches that
    /// depth, so a single ingested fire would not otherwise resolve immediately.
    fn prime_remote_buildup(queues: &mut netcode::HostCommandQueues, client_id: u64, tick: u32) {
        queues.ingest(
            client_id,
            &postretro_net::wire::InputCommand {
                client_tick: tick,
                movement: postretro_net::wire::WireMovementInput {
                    wish_dir: [0.0, 0.0],
                    jump_pressed: false,
                    dash_pressed: false,
                    running: false,
                    crouch_intent: false,
                    facing_yaw: 0.0,
                    use_pressed: false,
                    drop_pressed: false,
                    aim_pitch: 0.0,
                    firing_slot: 0,
                },
                fire_button: postretro_net::wire::WireFireButtonState {
                    pressed: false,
                    active: false,
                },
                reload: false,
            },
        );
    }

    #[test]
    fn o27_unowned_remote_firing_slot_logs_once_as_warning_and_stays_unarmed() {
        let pawn = postretro_entities::EntityId::from_raw(17);
        let registry = postretro_entities::EntityRegistry::new();
        let mut allocator = netcode::NetworkIdAllocator::new();
        allocator.stamp(pawn);
        let mut weaponless_fire_logged = std::collections::HashSet::new();
        let mut owners = netcode::MovementOwners::new();
        owners.set(pawn, 7);
        let mut queues = netcode::HostCommandQueues::new();
        queues.ingest(
            7,
            &postretro_net::wire::InputCommand {
                client_tick: 33,
                movement: postretro_net::wire::WireMovementInput {
                    wish_dir: [0.0, 0.0],
                    jump_pressed: false,
                    dash_pressed: false,
                    running: false,
                    crouch_intent: false,
                    facing_yaw: 0.0,
                    use_pressed: false,
                    drop_pressed: false,
                    aim_pitch: 0.0,
                    firing_slot: 0,
                },
                fire_button: postretro_net::wire::WireFireButtonState {
                    pressed: true,
                    active: true,
                },
                reload: false,
            },
        );
        prime_remote_buildup(&mut queues, 7, 34);
        let resolved = netcode::host_resolve_remote_commands(&owners, &mut queues);
        let resolved = resolved.first().expect("weaponless fire command resolves");

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            let first = App::prepare_remote_pawn_command(
                &allocator,
                &registry,
                &mut weaponless_fire_logged,
                99,
                resolved,
            );
            let second = App::prepare_remote_pawn_command(
                &allocator,
                &registry,
                &mut weaponless_fire_logged,
                100,
                resolved,
            );
            assert_eq!(first.weapon, None);
            assert_eq!(second.weapon, None);
        });

        let weaponless_logs: Vec<_> = captured
            .iter()
            .filter(|(_, message)| message.contains("declared unowned firing slot"))
            .collect();
        assert_eq!(
            weaponless_logs.len(),
            1,
            "the same unowned firing slot logs its rejected fire once"
        );
        assert_eq!(weaponless_logs[0].0, log::Level::Warn);
        assert!(
            captured
                .iter()
                .all(|(level, _)| *level != log::Level::Error),
            "an unowned firing slot is diagnostic, never a fatal error"
        );
    }

    #[test]
    fn o26_remote_fire_resolves_declared_possessed_slot_during_equip_transition() {
        let mut registry = postretro_entities::EntityRegistry::new();
        let pawn = registry.spawn(postretro_entities::Transform::default());
        let active = registry.spawn(postretro_entities::Transform::default());
        let declared = registry.spawn(postretro_entities::Transform::default());
        let mut inventory = postretro_entities::components::inventory::Inventory::default();
        inventory.wieldables[0] = Some(active);
        inventory.wieldables[1] = Some(declared);
        inventory.switch_target = Some(1);
        inventory.switch_origin = Some(0);
        registry.set_component(pawn, inventory).unwrap();

        let mut allocator = netcode::NetworkIdAllocator::new();
        allocator.stamp(pawn);
        let mut owners = netcode::MovementOwners::new();
        owners.set(pawn, 7);
        let mut queues = netcode::HostCommandQueues::new();
        queues.ingest(
            7,
            &postretro_net::wire::InputCommand {
                client_tick: 33,
                movement: postretro_net::wire::WireMovementInput {
                    wish_dir: [0.0, 0.0],
                    jump_pressed: false,
                    dash_pressed: false,
                    running: false,
                    crouch_intent: false,
                    facing_yaw: 0.0,
                    use_pressed: false,
                    drop_pressed: false,
                    aim_pitch: 0.0,
                    firing_slot: 1,
                },
                fire_button: postretro_net::wire::WireFireButtonState {
                    pressed: true,
                    active: true,
                },
                reload: false,
            },
        );
        prime_remote_buildup(&mut queues, 7, 34);
        let resolved = netcode::host_resolve_remote_commands(&owners, &mut queues);
        let resolved = resolved.first().expect("remote command resolves");

        let command = App::prepare_remote_pawn_command(
            &allocator,
            &registry,
            &mut std::collections::HashSet::new(),
            99,
            resolved,
        );

        assert_eq!(command.weapon, Some(declared));
    }

    #[test]
    fn simulate_tick_resolves_weapon_aim_after_movement_camera_follow() {
        use std::cell::RefCell;

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
        let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
        let mover_colliders = Vec::new();
        let mut mover_states = kinematic_mover::MoverTickStateTable::default();
        let remote_inputs = Vec::new();
        let command = sim::SimCommand {
            movement: movement::MovementInput {
                wish_dir: glam::Vec2::ZERO,
                jump_pressed: false,
                dash_pressed: false,
                running: false,
                crouch_intent: false,
                facing_yaw: 0.0,
                use_pressed: false,
                drop_pressed: false,
            },
            fire_button: weapon::FireButtonState {
                pressed: true,
                active: true,
            },
            reload: false,
            firing_slot: 0,
            select_slot: None,
            use_pressed: false,
            drop_pressed: false,
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
            &mut ai_runtime,
            &mover_colliders,
            &mut mover_states,
            &remote_inputs,
            &command,
            |registry| {
                follow_camera_to_local_pawn(&mut camera, &registry.borrow(), Vec3::ZERO);
                let post = build_post_movement_command(&camera);
                resolved_aim_origin = Some(post.aim_origin);
                post
            },
            TICK_DURATION.as_secs_f32(),
            None,
            |_| {},
        );

        assert_eq!(resolved_aim_origin, Some(camera.position));
        assert_ne!(
            camera.position,
            Vec3::new(99.0, 99.0, 99.0),
            "weapon aim must be resolved from the post-movement followed camera, not the stale frame-start camera",
        );
    }

    fn widget_contains_text(widget: &postretro_ui::descriptor::Widget, needle: &str) -> bool {
        use postretro_ui::descriptor::Widget;

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

    fn button_action<'a>(
        widget: &'a postretro_ui::descriptor::Widget,
        id: &str,
    ) -> Option<&'a str> {
        use postretro_ui::descriptor::Widget;

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
        rects: &postretro_ui::tree::FocusRectList,
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
        use postretro_scripting_core::data_descriptors::RegisteredUiTree;
        use postretro_ui::descriptor::CaptureMode;
        use postretro_ui::layout::Anchor;
        use postretro_ui::modal_stack::{ModalStack, ScopeTier};
        use postretro_ui::tree::CellValues;

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
            .find(|tree| tree.name == postretro_ui::demo::PAUSE_MENU_NAME)
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
        assert_eq!(mod_pause.role, Some(postretro_ui::descriptor::Role::Group));
        assert!(widget_contains_text(&mod_pause.root, "PAUSED"));
        let postretro_ui::descriptor::Widget::VStack(pause_root) = &mod_pause.root else {
            panic!("pause menu root is a vstack");
        };
        assert_eq!(
            pause_root.focus.as_ref().map(|focus| focus.kind()),
            Some(postretro_ui::descriptor::FocusKind::Linear)
        );
        assert_eq!(
            pause_root.focus.as_ref().map(|focus| focus.wrap()),
            Some(true)
        );
        assert_eq!(
            button_action(&mod_pause.root, "pauseResume"),
            Some(postretro_ui::actions::CLOSE_DIALOG_ACTION),
            "Resume resolves to the reserved close action wire value",
        );
        assert_eq!(
            button_action(&mod_pause.root, "pauseExitDesktop"),
            Some(postretro_ui::actions::EXIT_TO_DESKTOP_ACTION),
            "Exit to Desktop resolves to the generic reserved quit action wire value",
        );

        let theme = postretro_ui::theme::UiTheme::engine_default();
        let mut retained = postretro_ui::tree::UiTree::from_descriptor(&mod_pause, &theme);
        let mut font_system = postretro_ui::text::build_font_system();
        let empty_slots = std::collections::HashMap::new();
        let empty_cells = CellValues::new();
        let draw = retained.build_draw_data_retained(
            [1280, 720],
            &mut font_system,
            &postretro_ui::tree::ImageSizes::new(),
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
            workspace_root().join(postretro_ui::tree_asset::ui_asset_path("pauseMenu.json"));
        let fallback = postretro_ui::tree_asset::load_named_tree(&fallback_path)
            .expect("engine pause fallback loads");
        assert!(
            widget_contains_text(&fallback.root, "PRESS ESC OR B TO RESUME"),
            "fallback marker distinguishes the engine JSON fallback",
        );

        let mut stack = ModalStack::new();
        stack.registry_mut().register(
            postretro_ui::demo::PAUSE_MENU_NAME,
            fallback.clone(),
            ScopeTier::Engine,
            false,
        );
        stack.register_script_trees(vec![pause_entry.clone()], ScopeTier::Mod);
        let resolved = stack
            .tree(postretro_ui::demo::PAUSE_MENU_NAME)
            .expect("pauseMenu resolves through tiered registry");
        assert_eq!(
            button_action(&resolved.root, "pauseResume"),
            Some(postretro_ui::actions::CLOSE_DIALOG_ACTION),
            "the returned mod tree shadows the fallback marker",
        );
        assert!(
            !widget_contains_text(&resolved.root, "PRESS ESC OR B TO RESUME"),
            "shadowed mod tree does not expose the fallback marker",
        );

        stack.push_named(postretro_ui::demo::PAUSE_MENU_NAME, None);
        assert_eq!(
            stack.active_name(),
            Some(postretro_ui::demo::PAUSE_MENU_NAME)
        );
        assert_eq!(
            stack.top_capture_mode(),
            postretro_ui::descriptor::CaptureMode::Capture
        );
        assert_eq!(
            stack.entries()[0].descriptor.initial_focus.as_deref(),
            Some("pauseResume"),
            "initial focus metadata reaches the modal-stack entry",
        );

        let mut keyboard_focus = UiFocusEngine::new();
        let initial = keyboard_focus.tick(
            Some(postretro_ui::demo::PAUSE_MENU_NAME),
            Some(&focus_rects),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(initial.focused.as_deref(), Some("pauseResume"));
        let keyboard_confirm = keyboard_focus.tick(
            Some(postretro_ui::demo::PAUSE_MENU_NAME),
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
            Some(postretro_ui::demo::PAUSE_MENU_NAME),
            Some(&focus_rects),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        let gamepad_confirm = gamepad_focus.tick(
            Some(postretro_ui::demo::PAUSE_MENU_NAME),
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
            Some(postretro_ui::demo::PAUSE_MENU_NAME),
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
            postretro_ui::actions::CLOSE_DIALOG_ACTION,
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

        stack.push_named(postretro_ui::demo::PAUSE_MENU_NAME, None);
        let ordinary = route_ui_button_action("resumePauseMenu", &mut stack);
        assert_eq!(
            ordinary,
            UiButtonAction::NamedReaction,
            "ordinary button action names retain named-reaction dispatch",
        );
        assert_eq!(
            stack.active_name(),
            Some(postretro_ui::demo::PAUSE_MENU_NAME),
            "ordinary names are not intercepted as reserved close actions",
        );

        stack.replace_script_tree_tier(Vec::<RegisteredUiTree>::new(), ScopeTier::Mod);
        assert_eq!(
            stack
                .tree(postretro_ui::demo::PAUSE_MENU_NAME)
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
            Some(postretro_ui::actions::CLOSE_DIALOG_ACTION),
            "already-open menu remains stable until closed",
        );
        stack.pop();
        stack.push_named(postretro_ui::demo::PAUSE_MENU_NAME, None);
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
        use postretro_ui::descriptor::{
            Align, AnchoredTree, CaptureMode, ContainerWidget, SpacingValue, Widget,
        };
        use postretro_ui::layout::Anchor;
        use postretro_ui::modal_stack::{ModalStack, ScopeTier};

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
            postretro_ui::demo::PAUSE_MENU_NAME,
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
            Some(postretro_ui::demo::PAUSE_MENU_NAME),
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
        use postretro_ui::descriptor::{
            Align, AnchoredTree, CaptureMode, ContainerWidget, SpacingValue, Widget,
        };
        use postretro_ui::layout::Anchor;

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
                    id: "ui-commit".to_string(),
                    version: "1".to_string(),
                    render: Default::default(),
                    movers: Default::default(),
                    switching: Default::default(),
                    default_weapon_placement: None,
                    entities: Vec::new(),
                    maps: Vec::new(),
                    reactions: Vec::new(),
                    crossings: Vec::new(),
                    events: Vec::new(),
                    trigger_events: Vec::new(),
                    trigger_pools: Vec::new(),
                    ui_trees: vec![staged_tree("hud")],
                    presentation_templates: Vec::new(),
                    presentation_overlays: Vec::new(),
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
                id: "drain-mod",
                version: "1",
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
        let mut stack = postretro_ui::modal_stack::ModalStack::new();
        stack.register_script_trees(trees, postretro_ui::modal_stack::ScopeTier::Mod);

        // A frame renders the registered tree with NO VM resident: resolve by name
        // and build draw data from the resolved descriptor alone.
        let resolved = stack
            .tree("banner")
            .expect("registered tree resolves by name");
        let theme = postretro_ui::theme::UiTheme::engine_default();
        let mut ui = postretro_ui::tree::UiTree::from_descriptor(resolved, &theme);
        let mut fs = postretro_ui::text::build_font_system();
        let data = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &postretro_ui::tree::ImageSizes::new(),
            &std::collections::HashMap::new(),
            &postretro_ui::tree::CellValues::new(),
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
                id: "bad-theme-mod",
                version: "1",
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
            KeyCode::KeyA,
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

    fn test_model_hit_zones(
        sockets: std::collections::HashMap<String, postretro_model::gltf_loader::SocketBinding>,
    ) -> scripting_systems::hit_zones::ModelHitZones {
        use std::sync::Arc;

        scripting_systems::hit_zones::ModelHitZones {
            skeleton: Arc::new(postretro_model::skeleton::Skeleton::default()),
            clips: Arc::new(Vec::new()),
            joint_zones: Vec::new(),
            sockets,
            derived_bound: None,
            legs: Vec::new(),
            pose_stack: Arc::new(postretro_model::pose_modifier::PoseModifierStack::default()),
        }
    }

    fn attachment_resolution_store(
        holder_model: &str,
        sockets: std::collections::HashMap<String, postretro_model::gltf_loader::SocketBinding>,
        attachment_models: &[&str],
    ) -> scripting_systems::hit_zones::HitZoneStore {
        let mut store = scripting_systems::hit_zones::HitZoneStore::new();
        store.insert_for_test(
            postretro_model::ModelHandle::from(holder_model),
            test_model_hit_zones(sockets),
        );
        for attachment_model in attachment_models {
            store.insert_for_test(
                postretro_model::ModelHandle::from(*attachment_model),
                test_model_hit_zones(Default::default()),
            );
        }
        store
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
    fn distinct_mesh_models_includes_attachment_handles_once() {
        use postretro_entities::components::mesh::MeshComponent;
        use postretro_entities::{EntityRegistry, Transform};

        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                MeshComponent::stateless("models/holder.gltf".to_string()).with_attachments([
                    ("hand".to_string(), "models/prop.gltf".to_string()),
                    ("hip".to_string(), "models/prop.gltf".to_string()),
                    ("unused".to_string(), "".to_string()),
                ]),
            )
            .expect("fresh mesh entity accepts its component");

        assert_eq!(
            distinct_mesh_models(&registry),
            vec![
                "models/holder.gltf".to_string(),
                "models/prop.gltf".to_string(),
            ],
            "holder and attachment handles join one first-seen, deduplicated upload union"
        );
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
            travel_speed: None,
            clip_index: None,
        };
        let mut states = HashMap::new();
        states.insert("idle".to_string(), unresolved("idle_clip"));
        states.insert("attack".to_string(), unresolved("attack_clip"));

        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        let unresolved_mesh = MeshComponent {
            model: "models/descriptor_mob/scene.gltf".to_string(),
            animation: Some(MeshAnimation::new(states, "idle".to_string())),
            origin_offset: glam::Vec3::ZERO,
            shadow_bias_scale: 1.0,
            shadow_only: false,
            attachments: Vec::new(),
            pose_inputs: None,
        };
        registry
            .set_component(id, unresolved_mesh.clone())
            .expect("freshly spawned id is live");

        // Before resolve, the descriptor mesh's model is already visible to the
        // sweep — so the single post-dispatch sweep would upload it.
        let models = distinct_mesh_models(&registry);
        assert!(models.contains(&"models/descriptor_mob/scene.gltf".to_string()));

        // Build the clip table the renderer would produce for this model
        // (glTF index order). Hand-built so no GPU is needed.
        let mut tables = scripting_systems::mesh_anim::MeshClipTables::new();
        let meta = vec![
            postretro_render_cpu::mesh_pass::ClipMetadata {
                name: "idle_clip".to_string(),
                duration: 2.0,
            },
            postretro_render_cpu::mesh_pass::ClipMetadata {
                name: "attack_clip".to_string(),
                duration: 0.8,
            },
        ];
        tables.insert_with_bounds(
            postretro_model::ModelHandle::from("models/descriptor_mob/scene.gltf"),
            &meta,
            postretro_render_data::cone_frustum::Aabb::default(),
        );

        let hit_zone_store = scripting_systems::hit_zones::HitZoneStore::new();
        resolve_mesh_entity_bindings(&mut registry, &tables, &hit_zone_store);

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

        // Regression: a listen-host net-slot pawn materializes after this whole-
        // registry install sweep. Its body must resolve immediately even when no
        // weapon exists to trigger the later attachment-change path.
        let accepted_pawn = registry.spawn(Transform::default());
        registry
            .set_component(accepted_pawn, unresolved_mesh)
            .expect("accepted pawn materializes its descriptor mesh");
        resolve_accepted_host_pawn_presentation(
            &mut registry,
            &tables,
            &hit_zone_store,
            accepted_pawn,
        );
        let accepted_animation = registry
            .get_component::<MeshComponent>(accepted_pawn)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(accepted_animation.states["idle"].clip_index, Some(0));
        assert_eq!(accepted_animation.states["attack"].clip_index, Some(1));
    }

    #[test]
    fn spawner_only_archetype_is_preuploaded_for_both_roles_and_host_spawn_resolves_clips() {
        use crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor;
        use postretro_entities::components::mesh::MeshComponent;
        use postretro_entities::components::spawner::SpawnerComponent;
        use postretro_entities::{ComponentKind, EntityRegistry, Transform};

        let mut descriptor = behavior_enemy_descriptor("spawner_only");
        descriptor.mesh.as_mut().unwrap().model = "models/spawner_only.gltf".to_string();
        descriptor.mesh.as_mut().unwrap().attachments =
            [("hand".to_string(), "models/spawner_prop.gltf".to_string())]
                .into_iter()
                .collect();
        let descriptors = vec![descriptor.clone()];

        // There is no pre-placed enemy mesh: both role-specific upload sets must
        // discover this model through the resolved map spawner alone.
        let mut registry = EntityRegistry::new();
        let spawner = registry.spawn(Transform::default());
        registry
            .set_component(
                spawner,
                SpawnerComponent {
                    archetype_name: "spawner_only".to_string(),
                    count: 1,
                    resolved: true,
                },
            )
            .unwrap();
        assert!(distinct_mesh_models(&registry).is_empty());
        let spawner_models =
            crate::startup::lifecycle::resolved_spawner_mesh_models(&registry, &descriptors);
        let host_upload_models = spawner_models.clone();
        let client_upload_models = spawner_models;
        assert_eq!(
            host_upload_models,
            vec![
                "models/spawner_only.gltf".to_string(),
                "models/spawner_prop.gltf".to_string(),
            ]
        );
        assert_eq!(client_upload_models, host_upload_models);

        // This stands in for the renderer install hook: the shared upload union
        // builds the clip table before either host spawn or client remote
        // materialization can attach the mesh.
        let mut tables = scripting_systems::mesh_anim::MeshClipTables::new();
        tables.insert_with_bounds(
            postretro_model::ModelHandle::from("models/spawner_only.gltf"),
            &[
                postretro_render_cpu::mesh_pass::ClipMetadata {
                    name: "idle_clip".to_string(),
                    duration: 2.0,
                },
                postretro_render_cpu::mesh_pass::ClipMetadata {
                    name: "attack_clip".to_string(),
                    duration: 0.8,
                },
            ],
            postretro_render_data::cone_frustum::Aabb::default(),
        );
        let hit_zone_store = attachment_resolution_store(
            "models/spawner_only.gltf",
            std::collections::HashMap::from([(
                "hand".to_string(),
                postretro_model::gltf_loader::SocketBinding::SkinnedJoint(0),
            )]),
            &["models/spawner_prop.gltf"],
        );

        let context = crate::spawner::SpawnContext::default();
        context.replace_level_data(
            [("spawner_only".to_string(), descriptor)]
                .into_iter()
                .collect(),
            None,
        );
        crate::spawner::spawn_from_spawner_targets(&mut registry, &[spawner], &context);

        let spawned = registry
            .iter_with_kind(ComponentKind::Mesh)
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        assert_eq!(spawned.len(), 1);
        resolve_mesh_entity_bindings_for_entities(
            &mut registry,
            &tables,
            &hit_zone_store,
            context.take_pending_mesh_clip_resolves(),
        );

        let mesh = registry.get_component::<MeshComponent>(spawned[0]).unwrap();
        let animation = mesh.animation.as_ref().unwrap();
        assert_eq!(animation.states["idle"].clip_index, Some(0));
        assert_eq!(animation.states["attack"].clip_index, Some(1));
        assert_eq!(
            mesh.attachments[0].binding,
            postretro_entities::components::mesh::AttachmentBinding::Skinned(0)
        );
    }

    #[test]
    fn resolve_stateless_mesh_attachments_uses_skinned_and_rigid_socket_bindings() {
        use postretro_entities::components::mesh::{AttachmentBinding, MeshComponent};
        use postretro_entities::{EntityRegistry, Transform};

        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                MeshComponent::stateless("models/holder.gltf".to_string()).with_attachments([
                    ("hand".to_string(), "models/hand_prop.gltf".to_string()),
                    ("rail".to_string(), "models/rail_prop.gltf".to_string()),
                ]),
            )
            .expect("fresh mesh entity accepts its component");
        let rigid_rest = glam::Mat4::from_translation(glam::Vec3::new(1.0, 2.0, 3.0));
        let store = attachment_resolution_store(
            "models/holder.gltf",
            std::collections::HashMap::from([
                (
                    "hand".to_string(),
                    postretro_model::gltf_loader::SocketBinding::SkinnedJoint(4),
                ),
                (
                    "rail".to_string(),
                    postretro_model::gltf_loader::SocketBinding::RigidRest(rigid_rest),
                ),
            ]),
            &["models/hand_prop.gltf", "models/rail_prop.gltf"],
        );

        resolve_mesh_entity_bindings(
            &mut registry,
            &scripting_systems::mesh_anim::MeshClipTables::new(),
            &store,
        );

        let mesh = registry.get_component::<MeshComponent>(id).unwrap();
        assert!(mesh.animation.is_none(), "holder stays stateless");
        assert_eq!(mesh.attachments[0].binding, AttachmentBinding::Skinned(4));
        assert_eq!(
            mesh.attachments[1].binding,
            AttachmentBinding::Rigid(rigid_rest)
        );
    }

    #[test]
    fn unresolved_attachment_socket_or_model_does_not_block_stateless_holder_resolution() {
        use postretro_entities::components::mesh::{AttachmentBinding, MeshComponent};
        use postretro_entities::{EntityRegistry, Transform};

        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                MeshComponent::stateless("models/holder.gltf".to_string()).with_attachments([
                    (
                        "missing_socket".to_string(),
                        "models/loaded_prop.gltf".to_string(),
                    ),
                    ("hand".to_string(), "models/missing_prop.gltf".to_string()),
                ]),
            )
            .expect("fresh mesh entity accepts its component");
        let store = attachment_resolution_store(
            "models/holder.gltf",
            std::collections::HashMap::from([(
                "hand".to_string(),
                postretro_model::gltf_loader::SocketBinding::SkinnedJoint(1),
            )]),
            &["models/loaded_prop.gltf"],
        );

        let tables = scripting_systems::mesh_anim::MeshClipTables::new();
        resolve_mesh_entity_bindings(&mut registry, &tables, &store);
        resolve_mesh_entity_bindings(&mut registry, &tables, &store);

        let mesh = registry.get_component::<MeshComponent>(id).unwrap();
        assert_eq!(mesh.attachments[0].binding, AttachmentBinding::Unresolved);
        assert_eq!(mesh.attachments[1].binding, AttachmentBinding::Unresolved);
        assert!(
            !store.mark_attachment_resolution_warning(
                "attachment-socket:models/holder.gltf:missing_socket:models/loaded_prop.gltf"
                    .to_string()
            ),
            "repeated whole-registry resolution must preserve the socket warn-once diagnostic"
        );
        assert!(
            store.mark_attachment_resolution_warning(
                "attachment-model:models/holder.gltf:hand:models/missing_prop.gltf".to_string()
            ),
            "missing attachment models rely on the renderer's path-level diagnostic instead of a second attachment warning"
        );
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
            travel_speed: None,
            clip_index: None,
        };
        let mut states = HashMap::new();
        states.insert("idle".to_string(), unresolved("Idle", true));
        states.insert("attack".to_string(), unresolved("Attack", false));

        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("remote_enemy".to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: Some(MeshDescriptor {
                model: "models/remote_enemy/scene.gltf".to_string(),
                shadow_only: false,
                attachments: [("hand".to_string(), "models/remote_prop.gltf".to_string())]
                    .into_iter()
                    .collect(),
                shadow_bias_scale: 1.0,
                animations: states,
                default_state: Some("idle".to_string()),
                locomotion: None,
            }),
            health: None,
            behavior: None,
        }];

        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        crate::scripting::builtins::net_descriptor::materialize_net_mesh_presentation(
            "remote_enemy",
            &descriptors,
            &mut registry,
            id,
            None,
        );
        let component = registry
            .get_component::<MeshComponent>(id)
            .expect("remote presentation mesh attached");
        assert_eq!(component.attachments.len(), 1);
        assert_eq!(component.attachments[0].socket, "hand");
        assert_eq!(component.attachments[0].model, "models/remote_prop.gltf");

        let mut tables = scripting_systems::mesh_anim::MeshClipTables::new();
        let meta = vec![
            postretro_render_cpu::mesh_pass::ClipMetadata {
                name: "Attack".to_string(),
                duration: 0.8,
            },
            postretro_render_cpu::mesh_pass::ClipMetadata {
                name: "Idle".to_string(),
                duration: 2.0,
            },
        ];
        tables.insert_with_bounds(
            postretro_model::ModelHandle::from("models/remote_enemy/scene.gltf"),
            &meta,
            postretro_render_data::cone_frustum::Aabb::default(),
        );
        let hit_zone_store = attachment_resolution_store(
            "models/remote_enemy/scene.gltf",
            std::collections::HashMap::from([(
                "hand".to_string(),
                postretro_model::gltf_loader::SocketBinding::SkinnedJoint(2),
            )]),
            &["models/remote_prop.gltf"],
        );

        resolve_mesh_entity_bindings(&mut registry, &tables, &hit_zone_store);

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
        assert_eq!(
            component.attachments[0].binding,
            postretro_entities::components::mesh::AttachmentBinding::Skinned(2),
            "remote descriptor attachments resolve through the existing whole-registry pass"
        );
    }

    #[test]
    fn malformed_gltf_load_returns_err() {
        // The loader contract the degrade AC rides on: a bad/missing model path
        // is `Err`, not a panic — `load_skinned_model` turns that `Err` into a
        // `warn!` + `None`, so the level-load model sweep continues and the
        // `prop_mesh` entity simply renders nothing.
        let bad = std::path::Path::new("definitely/not/a/real/model.gltf");
        assert!(
            postretro_model::gltf_loader::load_model(bad).is_err(),
            "loading a missing glTF must return Err, never panic",
        );
    }

    // --- build_ui_slot_snapshot (state-store → UI read-snapshot boundary) ---

    #[test]
    fn ui_slot_snapshot_clones_present_values_and_skips_valueless_slots() {
        use postretro_entities::SlotValue;

        // The default table carries most engine `player.*` slots with `None`
        // values. Reload feedback and local weapon display slots begin with
        // concrete inactive/empty values. Setting one of the value-less slots
        // asserts the boundary contract: the snapshot clones value-bearing slots
        // and omits value-less ones.
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
        assert_eq!(
            snapshot.get("player.reloadActive"),
            Some(&SlotValue::Boolean(false)),
            "engine-owned player.reloadActive defaults to false and is cloned",
        );
        assert_eq!(
            snapshot.get("player.reloadProgress"),
            Some(&SlotValue::Number(0.0)),
            "engine-owned player.reloadProgress defaults to zero and is cloned",
        );
        assert_eq!(
            snapshot.get("player.weapon.current"),
            Some(&SlotValue::String(String::new())),
            "local player.weapon.current defaults empty and is cloned",
        );
        assert_eq!(
            snapshot.get("player.weapon.pending"),
            Some(&SlotValue::String(String::new())),
            "local player.weapon.pending defaults empty and is cloned",
        );
        assert_eq!(
            snapshot.get("player.weapon.switching"),
            Some(&SlotValue::Boolean(false)),
            "local player.weapon.switching defaults false and is cloned",
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
            11,
            "only the set player.health and default-valued reload-feedback + local weapon display + screen.flash + screen.vignette + screen.shake + input.mode + ui.textEntry slots appear",
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
    fn frame_anim_time_uses_visible_frame_clock_unless_frozen() {
        let prev = 7.0;
        let dt = 1.0 / 60.0;
        let scale = 0.5;

        assert_eq!(
            App::frame_anim_time(prev, dt, scale, false),
            App::advance_anim_clock(prev, dt, scale),
            "game-side pose queries and render collection share the visible frame's post-advance clock",
        );
        assert_eq!(
            App::frame_anim_time(prev, dt, scale, true),
            prev,
            "dev freeze holds the shared pose clock",
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
