// Session-lifetime boot construction: argument parsing, content-root selection,
// the `SessionServices` build (script runtime/registries, options I/O, input
// seeding, UI registration), `App` assembly, and the `PendingSessionInit` owner
// that finishes net-endpoint setup after the first visible logo frame.
// See: context/lib/boot_sequence.md §1 (Boot Order, stages 1-4)

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use glam::Vec3;
use winit::event_loop::EventLoop;

use crate::camera::Camera;
use crate::frame_timing::{FrameRateMeter, FrameTiming, InterpolableState};
use crate::input;
use crate::scripting::builtins::{
    ClassnameDispatch, register_builtins as register_builtin_classnames,
};
use crate::scripting::ctx::ScriptCtx;
use crate::scripting::data_descriptors::ModThemeTokens;
use crate::scripting::primitives::light::register_sequenced_light_primitives;
use crate::scripting::primitives::register_all;
use crate::scripting::primitives_registry::PrimitiveRegistry;
use crate::scripting::reaction_dispatch::ProgressTracker;
use crate::scripting::reactions::registry::{
    ReactionPrimitiveRegistry, register_emitter_reaction_primitives,
    register_fog_reaction_primitives, register_sequenced_fog_primitives,
};
use crate::scripting::reactions::system_commands::{
    SystemReactionRegistry, register_system_reaction_primitives,
};
use crate::scripting::runtime::{ScriptRuntime, ScriptRuntimeConfig};
use crate::scripting::sequence::SequencedPrimitiveRegistry;
use crate::scripting::state_crossings::CrossingDetector;
use crate::scripting::state_persistence::StateStoreLifecycle;
use crate::startup::StartupTimings;
use crate::{App, collision, netcode, options, scripting_systems, view_feel};

/// Dev-default boot map when no content root or map argument is supplied. Used by
/// `content_root_from_map` to derive the default `content/dev` root.
const DEFAULT_MAP_PATH: &str = "content/dev/maps/campaign-test.prl";

/// Built session: the winit event loop plus the constructed `App`, handed back to
/// `main` so it can drive the loop and return the app's exit result.
pub(crate) struct BootSession {
    pub(crate) event_loop: EventLoop<()>,
    pub(crate) app: App,
}

/// Deferred-startup owner. Carries the raw inputs needed to finish session
/// startup AFTER the first visible logo frame paints. Two construction sites read
/// from it until Task 3 collapses them: the migrated input/UI/modal group is built
/// here via [`Session::build`] (using `input_seed`), and the net endpoint is set
/// up (parse the raw net args, build the transport, fall back to single-player on
/// parse/setup failure). It is taken and consumed exactly once by
/// `App::install_pending_session`; suspend/resume keeps it unconsumed until the
/// install commits, so a resume that re-enters the splash loop never runs deferred
/// init twice. See: context/lib/boot_sequence.md §1, §9.
pub(crate) struct PendingSessionInit {
    /// Full `argv` (including `argv[0]`). Net args are parsed here, after first
    /// pixels — never before the event loop. See: context/lib/networking.md.
    raw_args: Vec<String>,

    /// Look-preference seed for the migrated `Session`'s `InputSystem`, captured
    /// pre-window from the loaded `PlayerOptions` so the post-first-pixel build
    /// needs no `player_options` borrow (which is not in the migrated group).
    /// See: context/lib/player_options.md §3.
    input_seed: crate::session::InputSeed,
}

impl PendingSessionInit {
    /// Finish session startup after the first logo frame. Builds the migrated
    /// input/UI/modal `Session` group (whole-or-nothing) and installs it into
    /// `app.session`, then parses the raw net args and installs the endpoint into
    /// `app.net_endpoint`, degrading to single-player (net inert) on parse or
    /// transport-setup failure. Records the `net_endpoint_complete` and
    /// `session_init_complete` boot-timing marks.
    ///
    /// Returns `Err` if the `Session` build fails; the caller stores it in
    /// `exit_result`, logs, exits the event loop, and early-returns from the
    /// install frame so no later step runs against a `None` session.
    ///
    /// Caller guards single-commit via `Option::take` on `app.pending_session`,
    /// so this never runs twice across a suspend/resume.
    pub(crate) fn install(self, app: &mut App) -> Result<()> {
        // Migrated input/UI/modal group: built post-first-pixel, synchronously,
        // whole-or-nothing. A hard failure propagates so the caller exits boot.
        app.session =
            Some(crate::session::Session::build(&self.input_seed).context("failed to build session")?);

        // M15 Phase 1 net role (default single-player). A malformed flag or a
        // failed transport construction degrades to single-player (net inert)
        // rather than blocking boot — the engine is playable without networking.
        let net_role = match netcode::parse_net_config(&self.raw_args) {
            Ok(config) => config.role,
            Err(err) => {
                log::error!("[Net] CLI parse failed ({err}); starting single-player");
                netcode::NetRole::SinglePlayer
            }
        };
        app.net_endpoint = match netcode::NetEndpoint::from_role(&net_role) {
            Ok(endpoint) => {
                match &net_role {
                    netcode::NetRole::SinglePlayer => {}
                    netcode::NetRole::Host { port } => {
                        log::info!("[Net] hosting (listen server) on port {port}");
                    }
                    netcode::NetRole::Connect { addr } => {
                        log::info!("[Net] connecting to {addr}");
                    }
                }
                endpoint
            }
            Err(err) => {
                log::error!("[Net] endpoint setup failed ({err}); starting single-player");
                None
            }
        };
        app.boot_timings.record("net_endpoint_complete");
        app.boot_timings.record("session_init_complete");
        Ok(())
    }
}

/// Session-lifetime service bundle, built once after `EventLoop::new` and folded
/// into the `App` literal. Groups the session-dependent systems the spec calls
/// out: the script context/runtime, the Rust-only handler registries, player
/// options + settings path, the input system, the modal stack with its built-in
/// UI registrations, and the script-derived per-frame systems. The bundle is the
/// construction grouping ("build before any path can use them"); the `App` owns
/// the fields after assembly. See: context/lib/boot_sequence.md §1.
struct SessionServices {
    script_ctx: ScriptCtx,
    script_runtime: ScriptRuntime,
    sequence_registry: SequencedPrimitiveRegistry,
    reaction_registry: ReactionPrimitiveRegistry,
    system_registry: SystemReactionRegistry,
    classname_dispatch: ClassnameDispatch,
    player_options: options::PlayerOptions,
    settings_path: Option<PathBuf>,
}

impl SessionServices {
    /// Build the session-lifetime systems. Runs AFTER `EventLoop::new` so the
    /// scripting bootstrap, registries, options I/O, input seeding, and UI
    /// registration all follow event-loop creation. Net setup is NOT here — it
    /// defers past first pixels through `PendingSessionInit`.
    fn build() -> Result<Self> {
        // Scripting bootstrap: primitive registry, runtime construction, and SDK
        // type emission. See: context/lib/scripting.md
        let script_ctx = ScriptCtx::new();
        let mut script_registry = PrimitiveRegistry::new();
        register_all(&mut script_registry, script_ctx.clone());
        let script_runtime = ScriptRuntime::new(
            &script_registry,
            &ScriptRuntimeConfig::default(),
            &script_ctx,
        )
        .context("failed to construct script runtime")?;
        // See `ScriptRuntime::new` for why this runs here rather than in the
        // constructor. See: context/lib/scripting.md §7.
        crate::scripting::typedef::emit_sdk_types_in_debug(&script_registry);

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
        // `script_ctx.system_commands`, drained once per frame after the
        // post-tick event drains. See: context/lib/scripting.md §10.4.
        let mut system_registry = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system_registry);

        // Built-in classname dispatch — survives level unload because handlers
        // describe engine types, not per-level state. See: context/lib/scripting.md
        let mut classname_dispatch = ClassnameDispatch::new();
        register_builtin_classnames(&mut classname_dispatch);

        // Player options load before `InputSystem` is constructed so the loaded
        // look preferences seed input at startup. On first boot (no file
        // present), write defaults so the human gets an editable starting file —
        // the only `save` call until the M13 settings menu lands. Missing config
        // dir or a save failure is logged, not fatal: boot proceeds on in-memory
        // defaults. See: context/lib/player_options.md §3
        let settings_path = options::settings_path();
        let player_options = match &settings_path {
            Some(path) => {
                // `load` returns defaults for both missing and malformed files,
                // so detect first-run by probing existence before loading. A
                // malformed file exists, so it is never overwritten here.
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

        // Input seeding and built-in UI tree registration moved into
        // `Session::build`, which runs post-first-pixel. `player_options` stays
        // here (still an `App` field); the two look-preference scalars ride
        // `PendingSessionInit::input_seed` to the migrated `InputSystem` build so
        // no construction dependency crosses the dual-construction boundary.

        Ok(Self {
            script_ctx,
            script_runtime,
            sequence_registry,
            reaction_registry,
            system_registry,
            classname_dispatch,
            player_options,
            settings_path,
        })
    }
}

/// Build all session-lifetime boot state (stages 1-4 of the boot order) and the
/// winit event loop, returning the constructed `App` in the `Booting` state.
///
/// Ordering: minimal pre-event-loop work (logging, boot-timing setup, raw arg
/// collection, content-root / boot-map selection) runs first, THEN
/// `EventLoop::new`, THEN the `SessionServices` build (scripting bootstrap incl.
/// the script primitive registry, the Rust-only registries, options I/O, input
/// seeding, and built-in UI registration). Net-role parsing and
/// `NetEndpoint::from_role` are NOT done here — they defer past the first logo
/// frame through `PendingSessionInit`. Mod init, the hot-reload watcher, and the
/// level-load worker spawn likewise run on the splash frame loop so the first
/// splash frame paints before any of that. See: context/lib/boot_sequence.md §1.
pub(crate) fn build_session() -> Result<BootSession> {
    // Timing starts at session construction so the first stage captures the
    // args_parsed → wgpu_init gap. See `StartupTimings` doc comment for
    // the per-line stage layout.
    let mut boot_timings = StartupTimings::new();

    // Minimal pre-event-loop work: raw args plus just enough parsing to identify
    // the content root and optional boot map. Net role is intentionally NOT
    // parsed here — it defers into `PendingSessionInit`.
    let args: Vec<String> = std::env::args().collect();
    let map_path = resolve_map_path(&args);
    let content_root = resolve_content_root(&args, map_path.as_deref());
    log::info!("[Engine] Content root: {}", content_root.display());
    boot_timings.record("args_parsed");

    // Event loop is created AHEAD of the scripting bootstrap and net-endpoint
    // setup so the window can come up as early as practical.
    let event_loop = EventLoop::new().context("failed to create event loop")?;
    boot_timings.record("event_loop_created");

    let session = SessionServices::build()?;
    boot_timings.record("script_runtime_ctor");

    // Camera starts at a placeholder; `install_level_payload` repositions it
    // to the first `player_spawn` or the level geometry center
    // (`spawn_position()`) when no player start exists.
    let initial_camera_pos = Vec3::new(0.0, 200.0, 500.0);
    let initial_state = InterpolableState::new(initial_camera_pos);

    let SessionServices {
        script_ctx,
        script_runtime,
        sequence_registry,
        reaction_registry,
        system_registry,
        classname_dispatch,
        player_options,
        settings_path,
    } = session;

    // Capture the migrated `InputSystem`'s look-preference seed before
    // `player_options` moves into the `App` literal. `player_options` stays on
    // `App` (not in the Task-1 group); the two scalars ride
    // `PendingSessionInit::input_seed` so the post-first-pixel `Session::build`
    // never borrows the not-yet-migrated field. See: boot_sequence §1.
    let input_seed = crate::session::InputSeed {
        mouse_sensitivity: player_options.mouse_sensitivity,
        invert_y: player_options.invert_y,
    };

    let app = App {
        renderer: None,
        audio: None,
        window_state: None,
        level: None,
        nav_graph: None,
        map_path: map_path.map(PathBuf::from),
        content_root,
        exit_result: Ok(()),
        camera: Camera::new(initial_camera_pos, 0.0, 0.0),
        // Session-lifetime input/UI/modal group is built post-first-pixel by
        // `PendingSessionInit::install`; `None` through the boot phase.
        session: None,
        crouch_toggle_active: false,
        ai_warned: std::collections::HashSet::new(),
        player_options,
        settings_path,
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
        player_hud_state: scripting_systems::ui_proxy::PlayerHudStatePublisher::new(
            script_ctx.clone(),
        ),
        flash_decay: scripting_systems::flash_decay::FlashDecay::new(script_ctx.clone()),
        vignette_decay: scripting_systems::vignette_decay::VignetteDecay::new(script_ctx.clone()),
        shake_decay: scripting_systems::shake_decay::ShakeDecay::new(script_ctx.clone()),
        presentation_cells: scripting_systems::presentation_cells::PresentationCellStore::new(),
        mod_theme_override: ModThemeTokens::default(),
        frontend: None,
        input_mode_tracker: scripting_systems::input_mode::InputModeTracker::new(
            script_ctx.clone(),
        ),
        pending_mode_signal: None,
        pending_menu_toggle: false,
        pending_exit_to_desktop: false,
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
        host_spawn_points: Vec::new(),
        pending_map_entities: None,
        script_time: 0.0,
        anim_time: 0.0,
        anim_time_scale: 1.0,
        boot_state: App::initial_boot_state(),
        splash_frame: 0,
        pending_level_log: false,
        pending_splash_override: None,
        boot_timings,
        mod_timings: StartupTimings::new(),
        level_timings: StartupTimings::new(),
        active_level_tags: Vec::new(),
        active_level_source: None,
        level_load: None,
        level_rx: None,
        level_worker: None,
        level_requests: VecDeque::new(),
        boot_load: false,
        // Net endpoint is built after first pixels by `PendingSessionInit`; until
        // then the engine is single-player inert (`None`).
        net_endpoint: None,
        pending_session: Some(PendingSessionInit {
            raw_args: args,
            input_seed,
        }),
        #[cfg(feature = "dev-tools")]
        debug_ui: None,
        #[cfg(feature = "dev-tools")]
        debug_chase_agent: None,
    };

    Ok(BootSession { event_loop, app })
}

/// Recover the positional map-path argument (the raw-path dev bypass), skipping
/// the values consumed by `--content-root`/`--mod` and any other flags.
pub(crate) fn resolve_map_path(args: &[String]) -> Option<String> {
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        if arg == "--content-root" || arg == "--mod" {
            if iter.peek().is_some_and(|value| !value.starts_with("--")) {
                let _ = iter.next();
            }
            continue;
        }
        if arg.starts_with("--content-root=") || arg.starts_with("--mod=") || arg.starts_with("--")
        {
            continue;
        }
        return Some(arg.clone());
    }
    None
}

fn mod_arg(args: &[String]) -> Option<PathBuf> {
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        if arg == "--content-root" {
            if iter.peek().is_some_and(|value| !value.starts_with("--")) {
                let _ = iter.next();
            }
            continue;
        }
        if arg == "--mod" {
            return iter
                .next_if(|value| !value.is_empty() && !value.starts_with("--"))
                .map(PathBuf::from);
        }
        if let Some(value) = arg.strip_prefix("--mod=") {
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        }
    }
    None
}

fn content_root_arg(args: &[String]) -> Option<PathBuf> {
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        if arg == "--mod" {
            if iter.peek().is_some_and(|value| !value.starts_with("--")) {
                let _ = iter.next();
            }
            continue;
        }
        if arg == "--content-root" {
            return iter
                .next_if(|value| !value.is_empty() && !value.starts_with("--"))
                .map(PathBuf::from);
        }
        if let Some(value) = arg.strip_prefix("--content-root=") {
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        }
    }
    None
}

fn resolve_content_root(args: &[String], map_path: Option<&str>) -> PathBuf {
    mod_arg(args)
        .or_else(|| content_root_arg(args))
        .unwrap_or_else(|| content_root_from_map(map_path))
}

fn content_root_from_map(map_path: Option<&str>) -> PathBuf {
    // `Path::new("maps/test.prl").parent()` returns `Some("maps")`, and
    // `"maps".parent()` returns `Some("")` — an empty path, not `None`. Filter
    // out the empty case so the `unwrap_or` fallback to `"."` actually fires.
    let map_path = map_path.unwrap_or(DEFAULT_MAP_PATH);
    Path::new(map_path)
        .parent()
        .and_then(|maps_dir| maps_dir.parent())
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_root_from_map_returns_grandparent_for_standard_path() {
        assert_eq!(
            content_root_from_map(Some("content/dev/maps/campaign-test.prl")),
            PathBuf::from("content/dev"),
        );
    }

    #[test]
    fn content_root_from_map_returns_grandparent_for_mod_path() {
        assert_eq!(
            content_root_from_map(Some("content/base/maps/e1m1.prl")),
            PathBuf::from("content/base"),
        );
    }

    // Regression: `Path::new("maps/test.prl").parent().and_then(parent)` returns
    // `Some("")` (an empty path), not `None`, so the prior `unwrap_or` fallback
    // was bypassed and the function returned `""` instead of `"."`.
    #[test]
    fn content_root_from_map_returns_dot_for_single_segment_parent() {
        assert_eq!(
            content_root_from_map(Some("maps/test.prl")),
            PathBuf::from(".")
        );
    }

    #[test]
    fn content_root_from_map_returns_dot_for_bare_filename() {
        assert_eq!(content_root_from_map(Some("test.prl")), PathBuf::from("."));
    }

    #[test]
    fn resolve_map_path_returns_none_when_no_positional_map_is_supplied() {
        let args = vec!["postretro".to_string()];
        assert_eq!(resolve_map_path(&args), None);
    }

    #[test]
    fn content_root_from_map_uses_default_dev_root_without_map() {
        assert_eq!(content_root_from_map(None), PathBuf::from("content/dev"));
    }

    #[test]
    fn content_root_arg_overrides_default_root() {
        let args = vec![
            "postretro".to_string(),
            "--content-root".to_string(),
            "content/base".to_string(),
        ];
        assert_eq!(content_root_arg(&args), Some(PathBuf::from("content/base")));
    }

    #[test]
    fn mod_arg_selects_content_root() {
        let args = vec![
            "postretro".to_string(),
            "--mod".to_string(),
            "content/mods/my-campaign".to_string(),
        ];
        assert_eq!(
            mod_arg(&args),
            Some(PathBuf::from("content/mods/my-campaign")),
        );
        assert_eq!(
            resolve_content_root(&args, None),
            PathBuf::from("content/mods/my-campaign"),
        );
    }

    #[test]
    fn mod_arg_accepts_equals_form_without_creating_a_map_arg() {
        let args = vec![
            "postretro".to_string(),
            "--mod=content/mods/my-campaign".to_string(),
        ];
        assert_eq!(
            mod_arg(&args),
            Some(PathBuf::from("content/mods/my-campaign")),
        );
        assert_eq!(resolve_map_path(&args), None);
        assert_eq!(
            resolve_content_root(&args, None),
            PathBuf::from("content/mods/my-campaign"),
        );
    }

    #[test]
    fn mod_arg_missing_value_does_not_consume_next_flag_or_corrupt_map_arg() {
        let args = vec![
            "postretro".to_string(),
            "--mod".to_string(),
            "--content-root".to_string(),
            "content/base".to_string(),
            "maps/dev.prl".to_string(),
        ];

        assert_eq!(mod_arg(&args), None);
        assert_eq!(content_root_arg(&args), Some(PathBuf::from("content/base")));
        assert_eq!(resolve_map_path(&args).as_deref(), Some("maps/dev.prl"));
    }

    #[test]
    fn mod_arg_empty_equals_value_is_ignored() {
        let args = vec![
            "postretro".to_string(),
            "--mod=".to_string(),
            "maps/dev.prl".to_string(),
        ];

        assert_eq!(mod_arg(&args), None);
        assert_eq!(resolve_map_path(&args).as_deref(), Some("maps/dev.prl"));
    }

    #[test]
    fn resolve_map_path_skips_mod_value() {
        let args = vec![
            "postretro".to_string(),
            "--mod".to_string(),
            "content/mods/my-campaign".to_string(),
        ];
        assert_eq!(resolve_map_path(&args), None);
    }

    #[test]
    fn resolve_map_path_returns_bare_map_after_selected_mod() {
        let args = vec![
            "postretro".to_string(),
            "--mod".to_string(),
            "content/mods/my-campaign".to_string(),
            "maps/dev-bypass.prl".to_string(),
        ];
        let map_path = resolve_map_path(&args);
        assert_eq!(map_path, Some("maps/dev-bypass.prl".to_string()));
        assert_eq!(
            resolve_content_root(&args, map_path.as_deref()),
            PathBuf::from("content/mods/my-campaign"),
        );
    }

    #[test]
    fn resolve_map_path_skips_content_root_value() {
        let args = vec![
            "postretro".to_string(),
            "--content-root=content/base".to_string(),
            "content/base/maps/e1m1.prl".to_string(),
        ];
        assert_eq!(
            resolve_map_path(&args),
            Some("content/base/maps/e1m1.prl".to_string()),
        );
    }
}
