// Session-lifetime boot construction: argument parsing, content-root selection,
// script-runtime/registry build, options I/O, input seeding, and `App` assembly.
// See: context/lib/boot_sequence.md §1 (Boot Order, stages 1-4)

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use glam::Vec3;
use winit::event_loop::EventLoop;

use crate::camera::Camera;
use crate::frame_timing::{FrameRateMeter, FrameTiming, InterpolableState};
use crate::input::{self, InputFocus};
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
use crate::{App, collision, netcode, options, render, scripting_systems, view_feel};

/// Dev-default boot map when no content root or map argument is supplied. Used by
/// `content_root_from_map` to derive the default `content/dev` root.
const DEFAULT_MAP_PATH: &str = "content/dev/maps/campaign-test.prl";

/// Built session: the winit event loop plus the constructed `App`, handed back to
/// `main` so it can drive the loop and return the app's exit result.
pub(crate) struct BootSession {
    pub(crate) event_loop: EventLoop<()>,
    pub(crate) app: App,
}

/// Build all session-lifetime boot state (stages 1-4 of the boot order) and the
/// winit event loop, returning the constructed `App` in the `Booting` state. Mod
/// init, the hot-reload watcher, and the level-load worker spawn are deliberately
/// NOT done here — they run on the splash frame loop so the first splash frame
/// paints before any mod-supplied work. See: context/lib/boot_sequence.md §1.
pub(crate) fn build_session() -> Result<BootSession> {
    // Timing starts at session construction so the first stage captures the
    // args_parsed → wgpu_init gap. See `StartupTimings` doc comment for
    // the per-line stage layout.
    let mut boot_timings = StartupTimings::new();

    let args: Vec<String> = std::env::args().collect();

    let map_path = resolve_map_path(&args);
    let content_root = resolve_content_root(&args, map_path.as_deref());
    log::info!("[Engine] Content root: {}", content_root.display());

    // M15 Phase 1 net role (default single-player). A malformed flag or a failed
    // transport construction degrades to single-player (net inert) rather than
    // blocking boot — the engine is playable without networking.
    let net_role = match netcode::parse_net_config(&args) {
        Ok(config) => config.role,
        Err(err) => {
            log::error!("[Net] CLI parse failed ({err}); starting single-player");
            netcode::NetRole::SinglePlayer
        }
    };
    let net_endpoint = match netcode::NetEndpoint::from_role(&net_role) {
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
    // See: context/lib/boot_sequence.md §1 (Splash state machine)

    let mut input_system = input::InputSystem::new(input::default_bindings());
    input_system.set_mouse_sensitivity(player_options.mouse_sensitivity);
    input_system.set_invert_y(player_options.invert_y);

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
        input_system,
        gameplay_input_latch: input::GameplayInputLatch::new(),
        crouch_toggle_active: false,
        ai_warned: std::collections::HashSet::new(),
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
        player_hud_state: scripting_systems::ui_proxy::PlayerHudStatePublisher::new(
            script_ctx.clone(),
        ),
        flash_decay: scripting_systems::flash_decay::FlashDecay::new(script_ctx.clone()),
        vignette_decay: scripting_systems::vignette_decay::VignetteDecay::new(script_ctx.clone()),
        shake_decay: scripting_systems::shake_decay::ShakeDecay::new(script_ctx.clone()),
        presentation_cells: scripting_systems::presentation_cells::PresentationCellStore::new(),
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
            // The HUD is always-on: it composes as the bottom base layer every
            // gameplay frame (resolved through the always-on read seam). The pause
            // menu and keyboard are pushed-only modals — not always-on.
            render::ui::tree_asset::register_tree_from_disk(
                registry,
                render::ui::tree_asset::HUD_NAME,
                "hud.json",
                true,
            );
            render::ui::tree_asset::register_tree_from_disk(
                registry,
                render::ui::demo::PAUSE_MENU_NAME,
                "pauseMenu.json",
                false,
            );
            render::ui::tree_asset::register_tree_from_disk(
                registry,
                render::ui::demo::FRONTEND_MENU_NAME,
                "frontendMenu.json",
                false,
            );
            render::ui::tree_asset::register_tree_from_disk(
                registry,
                render::ui::keyboard_asset::KEYBOARD_TREE_NAME,
                "keyboard.json",
                false,
            );
            stack
        },
        mod_theme_override: ModThemeTokens::default(),
        frontend: None,
        ui_focus: input::UiFocusEngine::new(),
        ui_focus_rects: None,
        ui_input_mode: input::InputMode::default(),
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
        net_endpoint,
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
