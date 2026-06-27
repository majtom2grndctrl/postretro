// Live session-lifetime runtime container, built after the first visible frame.
// Owns EVERY session-lifetime field: the input/UI/modal group, the scripting core
// (script context + runtime, registries, and every system that captures a
// `ScriptCtx` clone or a registry reference), the player options + settings path,
// the committed frontend declaration, the net endpoint, the audio subsystem, and
// (dev-tools) the debug-UI state. Migrated off the `App` god-struct so boot code
// cannot name a session field before install. Building the script tranche here
// moves the heaviest startup init (`ScriptCtx::new` / `register_all` /
// `ScriptRuntime::new`) behind first pixels. After Task 3, `Session::build` is the
// sole session construction site and `App` holds only boot-lifetime fields plus
// `session: Option<Session>`.
// See: context/lib/boot_sequence.md §1 (Deferred-session boundary and single commit)

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::input;
use crate::render;
use crate::scripting::runtime::Frontend;
use crate::{audio, netcode, options};
use crate::scripting::builtins::{
    ClassnameDispatch, register_builtins as register_builtin_classnames,
};
use crate::scripting::ctx::ScriptCtx;
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
use crate::scripting_systems;
use crate::startup::StartupTimings;

/// Live session-lifetime container, held on `App` as `Option<Session>` and built
/// once after first pixels by [`Session::build`]. Owns EVERY session-lifetime
/// field; none can be named while `App.session` is `None` (boot phase). After
/// Task 3 there is no transient pre-window construction bundle — `Session::build`
/// is the sole session construction site, and `App` holds only boot-lifetime
/// fields plus `session: Option<Session>`.
/// See: context/lib/boot_sequence.md §1.
pub(crate) struct Session {
    /// Keyboard/mouse/gamepad action state. Seeded at build with the loaded
    /// look preferences. See: context/lib/input.md
    pub(crate) input_system: input::InputSystem,

    /// Per-tick gameplay-input latch; neutralized while a modal captures input.
    pub(crate) gameplay_input_latch: input::GameplayInputLatch,

    /// Input-stage UI-dispatch seam (capture vs. passthrough).
    /// See: context/lib/input.md
    pub(crate) ui_dispatch: input::UiDispatch,

    /// Gamepad subsystem. Inner `Option` encodes runtime absence (no pad / gilrs
    /// init failure) — distinct from "session not yet installed."
    pub(crate) gamepad_system: Option<input::gamepad::GamepadSystem>,

    /// Coarse keyboard/mouse focus owner. Drives pointer-lock acquire/release.
    /// See: context/lib/input.md
    pub(crate) input_focus: input::InputFocus,

    /// UI focus engine: moves focus through the top stack tree, runs the
    /// hold-to-repeat clock, yields the focused node id. See: context/lib/ui.md §4.
    pub(crate) ui_focus: input::UiFocusEngine,

    /// The focus rect list the renderer exported for the top tree LAST frame.
    /// Inner `Option` encodes "not exported yet," not "session not installed."
    /// See: context/lib/ui.md §4.
    pub(crate) ui_focus_rects: Option<render::ui::tree::FocusRectList>,

    /// Pointer-vs-focus interaction mode (hover moves focus only in `Pointer`).
    /// See: context/lib/input.md §7.
    pub(crate) ui_input_mode: input::InputMode,

    /// Gameplay-UI modal stack + named-tree registry. Built-in trees register at
    /// build. See: context/lib/ui.md §1.
    pub(crate) modal_stack: render::ui::modal_stack::ModalStack,

    // --- Scripting core (Task 2). The script runtime, the context handle every
    // primitive closure captures, the Rust-side registries, and every system
    // that holds a cloned `ScriptCtx` or a registry reference. The whole tranche
    // is one indivisible group: `ScriptCtx` is `Clone` (`Rc`-backed) and is
    // cloned into eight construction sites. See: context/lib/scripting.md. ---
    /// The script VM runtime. Constructed once here (post-first-pixel); never
    /// recreated. See: context/lib/scripting.md.
    pub(crate) script_runtime: ScriptRuntime,

    /// Holds the entity registry shared by the light bridge and the script
    /// runtime. Outlives the renderer so device resets preserve scripted light
    /// state. See: context/lib/scripting.md.
    pub(crate) script_ctx: ScriptCtx,

    /// Publishes live pawn HP and max HP into the player HUD slots each frame.
    /// See: context/lib/scripting.md §5 for the store contract.
    pub(crate) player_hud_state: scripting_systems::ui_proxy::PlayerHudStatePublisher,

    /// App-side flash-decay state for the engine-owned `screen.flash` surface.
    /// See: context/lib/ui.md §3.
    pub(crate) flash_decay: scripting_systems::flash_decay::FlashDecay,

    /// App-side vignette-decay state for the engine-owned `screen.vignette`
    /// surface. See: context/lib/ui.md §3.
    pub(crate) vignette_decay: scripting_systems::vignette_decay::VignetteDecay,

    /// App-side screen-shake state for the engine-owned `screen.shake` surface.
    /// See: context/lib/ui.md §3.
    pub(crate) shake_decay: scripting_systems::shake_decay::ShakeDecay,

    /// Presentation-cell store for `ui.createLocalState()`. Presentation-only —
    /// NEVER the authoritative store. See: context/lib/ui.md §3/§6.
    pub(crate) presentation_cells: scripting_systems::presentation_cells::PresentationCellStore,

    /// App-side input-mode tracker: observes mode signals, debounces them, writes
    /// the engine-owned `input.mode` slot, drives `ui_input_mode`.
    /// See: context/lib/input.md §7.
    pub(crate) input_mode_tracker: scripting_systems::input_mode::InputModeTracker,

    /// Gates the one-time persistence overlay and clean-exit save.
    pub(crate) state_store_lifecycle: StateStoreLifecycle,

    /// Consulted by `fire_named_event_with_sequences` for `Sequence` steps.
    /// See: context/lib/scripting.md §2.
    pub(crate) sequence_registry: SequencedPrimitiveRegistry,

    /// Resolved by name when a `Primitive` reaction fires.
    /// See: context/lib/scripting.md §2.
    pub(crate) reaction_registry: ReactionPrimitiveRegistry,

    /// Resolved by name when a `Primitive` reaction with no `tag` fires — the
    /// system-reaction arm. See: context/lib/scripting.md §10.4.
    pub(crate) system_registry: SystemReactionRegistry,

    /// Per-tag kill-count subscriptions. See: context/lib/scripting.md §2.
    pub(crate) progress_tracker: ProgressTracker,

    /// State-crossing watchers (M13 HUD dynamics). See: context/lib/scripting.md §10.4.
    pub(crate) crossing_detector: CrossingDetector,

    /// Maps `classname` strings to engine spawn handlers. Survives level unload.
    /// See: context/lib/scripting.md.
    pub(crate) classname_dispatch: ClassnameDispatch,

    /// Repacks `GpuLight` bytes when any `LightComponent` is dirty.
    /// See: context/lib/scripting.md.
    pub(crate) light_bridge: scripting_systems::light_bridge::LightBridge,

    /// Per-level fog-volume registry side-table; packs `FogVolume` GPU bytes.
    /// See: context/lib/rendering_pipeline.md §7.5.
    pub(crate) fog_volume_bridge: scripting_systems::fog_volume_bridge::FogVolumeBridge,

    /// Walks every `BillboardEmitterComponent` after game logic and before
    /// particle sim. See: context/lib/scripting.md.
    pub(crate) emitter_bridge: scripting_systems::emitter_bridge::EmitterBridge,

    /// Packs `SpriteInstance` bytes per collection in the Render stage; never
    /// touches wgpu directly. See: context/lib/scripting.md.
    pub(crate) particle_render: scripting_systems::particle_render::ParticleRenderCollector,

    /// Packs per-instance skinned-mesh world matrices in the Render stage; never
    /// touches wgpu. See: context/lib/scripting.md.
    pub(crate) mesh_render: scripting_systems::mesh_render::MeshRenderCollector,

    /// Game-side per-model animation clip tables (name → glTF index + per-index
    /// duration). Cleared on level unload. See: context/lib/scripting.md §10.3.
    pub(crate) mesh_clip_tables: scripting_systems::mesh_anim::MeshClipTables,

    /// Game-side skeletal hit-zone store: per model TYPE, the CPU skeleton,
    /// clips, authored joint-zone table, and a derived broad-phase bound.
    /// CPU-only — no wgpu. See: context/lib/entity_model.md §7.
    pub(crate) hit_zone_store: scripting_systems::hit_zones::HitZoneStore,

    // --- Remaining session state (Task 3). The last fields that lived directly
    // on `App`; their migration here completes the boot/session split. ---
    /// Per-human runtime preferences loaded at build. Seeds input look
    /// preferences; `crouch_mode` is read each input tick by
    /// `resolve_crouch_intent`; `view_feel_scale` feeds `view_feel::evaluate`.
    /// Owned (not `Option`) — defaults stand in when no settings file exists.
    /// See: context/lib/player_options.md
    pub(crate) player_options: options::PlayerOptions,

    /// Resolved `settings.toml` path. Inner `Option` is genuine runtime absence:
    /// `None` when the platform exposes no config directory (the engine then runs
    /// on in-memory defaults without persistence). Held for the future M13
    /// settings menu's save path; no reader yet.
    /// See: context/lib/player_options.md
    #[allow(dead_code)]
    pub(crate) settings_path: Option<PathBuf>,

    /// Currently committed mod frontend declaration. Successful staged mod-init
    /// commits replace this snapshot. Inner `Option` is genuine runtime absence:
    /// `None` falls back to the engine/default frontend behavior.
    /// See: context/lib/boot_sequence.md §4.
    pub(crate) frontend: Option<Frontend>,

    /// Network endpoint (M15 Phase 1). Inner `Option` is genuine runtime absence:
    /// `None` for single-player (net inert); `Host`/`Client` once a
    /// `--host`/`--connect` role's transport is constructed. A malformed net flag
    /// or a failed transport construction degrades to single-player rather than
    /// blocking boot. The net subsystem never touches the registry —
    /// `crate::netcode` owns that seam. See: context/lib/networking.md.
    pub(crate) net_endpoint: Option<netcode::NetEndpoint>,

    /// Audio subsystem. Inner `Option` is genuine runtime absence: `None` if kira
    /// init fails — the game then runs silent, never a crash.
    /// See: context/lib/audio.md §1.
    pub(crate) audio: Option<audio::Audio>,

    /// CPU-side egui debug-UI state (dev-tools only). Inner `Option` is a genuine
    /// runtime/lazy state, NOT "session not yet installed": the constructor needs
    /// the boot-ready renderer's `max_texture_dimension_2d` limit and the window,
    /// neither available at `Session::build` time, so it is lazy-initialized by
    /// `App::ensure_debug_ui` after install and reset to `None` on suspend (the
    /// window-derived state is rebuilt on resume). The GPU half lives on
    /// `Renderer` as `debug_ui_gpu`. See: context/lib/boot_sequence.md §1, §9.
    #[cfg(feature = "dev-tools")]
    pub(crate) debug_ui: Option<render::debug_ui::DebugUi>,
}

impl Session {
    /// Build ALL session-lifetime state AFTER the first visible frame,
    /// synchronously and whole-or-nothing. Runs entirely within the single
    /// install redraw — no `await`, no yield. This is the sole session
    /// construction site (Task 3 collapsed the residual pre-window build). It
    /// builds, in boot-order:
    /// 1. player options I/O (load + first-run default write), seeding input;
    /// 2. the fault-tolerant audio subsystem (silent on kira failure);
    /// 3. the scripting bootstrap (`ScriptCtx::new` / `register_all` /
    ///    `ScriptRuntime::new` / SDK-type emission), the Rust-side registries, the
    ///    eight `ScriptCtx`-clone systems, and the input/UI/modal group;
    /// 4. the net endpoint (parse net args, build transport, degrade to
    ///    single-player on failure).
    ///
    /// `frontend` starts `None` (mod-init commits it later this frame) and
    /// `debug_ui` starts `None` (lazy-initialized by `App::ensure_debug_ui` once
    /// the renderer/window are available — see its field doc).
    ///
    /// `boot_timings` is threaded in so the deferred-session marks
    /// (`audio_init_complete`, `script_runtime_ctor`, `net_endpoint_complete`)
    /// record where the work now runs (post-first-pixel). `audio_init_complete`
    /// precedes `script_runtime_ctor`, mirroring the prior boot order.
    ///
    /// The only fallible step is the script-runtime construction (a hard boot
    /// failure); audio, net, and the built-in UI tree disk loads all degrade in
    /// place. `Session::build` returns `Err` on a runtime-construction failure;
    /// the install path stores it in `exit_result` and exits boot.
    /// See: context/lib/boot_sequence.md §1.
    pub(crate) fn build(raw_args: &[String], boot_timings: &mut StartupTimings) -> Result<Self> {
        // 1. Player options load first so the loaded look preferences seed the
        //    `InputSystem` constructed below. On first boot (no file present),
        //    write defaults so the human gets an editable starting file — the only
        //    `save` call until the M13 settings menu lands. A missing config dir
        //    or a save failure is logged, not fatal: boot proceeds on in-memory
        //    defaults. See: context/lib/player_options.md §3.
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

        // 2. Audio: fault-tolerant. A kira/device failure logs and runs silent
        //    (`audio` stays `None`) — never a crash. `audio_init_complete` is
        //    recorded before the scripting bootstrap so the boot order keeps
        //    audio ahead of `script_runtime_ctor`. See: context/lib/audio.md §1.
        let audio = match audio::Audio::new() {
            Ok(audio) => {
                log::info!("[Audio] Initialized");
                Some(audio)
            }
            Err(err) => {
                log::error!("[Audio] Init failed, running silent: {err}");
                None
            }
        };
        boot_timings.record("audio_init_complete");

        // 3. Scripting bootstrap: primitive registry, runtime construction, and
        //    SDK type emission. Runs behind first pixels.
        //    See: context/lib/scripting.md.
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
        // The runtime is now constructed behind first pixels; record the mark
        // where it actually fires so the boot order line proves first pixels
        // precede `script_runtime_ctor`. See: context/lib/boot_sequence.md §1.
        boot_timings.record("script_runtime_ctor");

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
        // `script_ctx.system_commands`. See: context/lib/scripting.md §10.4.
        let mut system_registry = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system_registry);

        // Built-in classname dispatch — survives level unload because handlers
        // describe engine types, not per-level state. See: context/lib/scripting.md.
        let mut classname_dispatch = ClassnameDispatch::new();
        register_builtin_classnames(&mut classname_dispatch);

        // The five subsystems that each capture a `ScriptCtx` clone, previously
        // built inline in the `App` literal. They join the script tranche here.
        let player_hud_state =
            scripting_systems::ui_proxy::PlayerHudStatePublisher::new(script_ctx.clone());
        let flash_decay = scripting_systems::flash_decay::FlashDecay::new(script_ctx.clone());
        let vignette_decay =
            scripting_systems::vignette_decay::VignetteDecay::new(script_ctx.clone());
        let shake_decay = scripting_systems::shake_decay::ShakeDecay::new(script_ctx.clone());
        let input_mode_tracker =
            scripting_systems::input_mode::InputModeTracker::new(script_ctx.clone());

        let mut input_system = input::InputSystem::new(input::default_bindings());
        input_system.set_mouse_sensitivity(player_options.mouse_sensitivity);
        input_system.set_invert_y(player_options.invert_y);

        // Register engine built-in trees through the one shared load-and-register
        // path (`tree_asset::register_tree_from_disk`): each built-in screen's
        // `AnchoredTree` is authored in `content/base/ui/<file>.json` and loaded
        // from disk so a layout edit + reload changes it with no Rust change. A
        // missing/malformed asset warns once and skips the registration — that
        // screen is unavailable, the engine still runs.
        //
        // The HUD registers under `HUD_NAME` and resolves as the always-on bottom
        // passthrough layer each frame. The pause menu, frontend menu, and
        // keyboard register as pushed-only modals.
        let mut modal_stack = render::ui::modal_stack::ModalStack::new();
        {
            let registry = modal_stack.registry_mut();
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
        }

        // 4. Net endpoint (M15 Phase 1, default single-player). A malformed flag
        //    or a failed transport construction degrades to single-player (net
        //    inert) rather than blocking boot — the engine is playable without
        //    networking. The net subsystem never touches the registry.
        //    See: context/lib/networking.md.
        let net_role = match netcode::parse_net_config(raw_args) {
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
        boot_timings.record("net_endpoint_complete");

        Ok(Self {
            input_system,
            gameplay_input_latch: input::GameplayInputLatch::new(),
            ui_dispatch: input::UiDispatch::new(),
            gamepad_system: input::gamepad::GamepadSystem::new(),
            input_focus: input::InputFocus::Gameplay,
            ui_focus: input::UiFocusEngine::new(),
            ui_focus_rects: None,
            ui_input_mode: input::InputMode::default(),
            modal_stack,
            script_runtime,
            script_ctx,
            player_hud_state,
            flash_decay,
            vignette_decay,
            shake_decay,
            presentation_cells:
                scripting_systems::presentation_cells::PresentationCellStore::new(),
            input_mode_tracker,
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
            particle_render: scripting_systems::particle_render::ParticleRenderCollector::new(),
            mesh_render: scripting_systems::mesh_render::MeshRenderCollector::new(),
            mesh_clip_tables: scripting_systems::mesh_anim::MeshClipTables::new(),
            hit_zone_store: scripting_systems::hit_zones::HitZoneStore::new(),
            player_options,
            settings_path,
            // Committed by mod-init later this same install frame; engine/default
            // frontend until then.
            frontend: None,
            net_endpoint,
            audio,
            // Lazy: built by `App::ensure_debug_ui` once the renderer/window are
            // available, reset on suspend. See the field doc.
            #[cfg(feature = "dev-tools")]
            debug_ui: None,
        })
    }
}
