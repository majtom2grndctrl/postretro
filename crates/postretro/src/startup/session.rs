// Session-lifetime boot construction: argument parsing, content-root selection,
// `App` assembly, and the `PendingSessionInit` owner that constructs the entire
// `Session` after the first visible logo frame. `Session::build` is the sole
// session construction site; `App` holds only boot-lifetime fields plus
// `session: Option<Session>`.
// See: context/lib/boot_sequence.md §1 (Boot Order, stages 1-4)

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use glam::Vec3;
use winit::event_loop::EventLoop;

use crate::camera::Camera;
use crate::frame_timing::{FrameRateMeter, FrameTiming, InterpolableState};
use crate::input;
use crate::startup::{LevelSource, StartupTimings};
use crate::trigger_pools::{TriggerPoolSeedPolicy, entropy_seed};
use crate::{App, collision, kinematic_mover, runtime_movers, view_feel};
use postretro_foundation::{ModThemeTokens, SwitchingDescriptor};

/// Dev-default boot map when no content root or map argument is supplied. Used by
/// `content_root_from_map` to derive the default `content/dev` root.
const DEFAULT_MAP_PATH: &str = "content/dev/maps/campaign-test.prl";

/// The one directory every mod lives directly under. `--mod <name>` selects
/// `content/<name>`; the two-component shape is what the runtime's `baked/`
/// grandparent derivation needs (`build_pipeline.md` §Baked texture mips), so
/// fixing the container here is what keeps a mod name from being a path.
const CONTENT_DIR: &str = "content";

/// Built session: the winit event loop plus the constructed `App`, handed back to
/// `main` so it can drive the loop and return the app's exit result.
pub(crate) struct BootSession {
    pub(crate) event_loop: EventLoop<()>,
    pub(crate) app: App,
}

/// Arguments parsed before either entry path branches. The explicit override is
/// retained rather than a precomputed entropy seed so a windowed restart gets a
/// fresh roll while a pinned `--pool-seed` stays reproducible.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SessionBootConfig {
    pool_seed_override: Option<u64>,
}

impl SessionBootConfig {
    fn from_args(args: &[String]) -> Self {
        let pool_seed_override = match pool_seed_arg(args) {
            PoolSeedArg::Absent => None,
            PoolSeedArg::Valid(seed) => Some(seed),
            PoolSeedArg::Invalid(value) => {
                log::warn!(
                    "[TriggerPools] invalid --pool-seed value {value:?}; using the default policy"
                );
                None
            }
        };
        Self { pool_seed_override }
    }

    /// Windowed installs always roll. Without an explicit seed, resolve fresh
    /// entropy at each install so restarting a level is a new run.
    pub(crate) fn windowed_trigger_pool_policy(self) -> TriggerPoolSeedPolicy {
        TriggerPoolSeedPolicy::Seeded(self.pool_seed_override.unwrap_or_else(entropy_seed))
    }

    /// Headless runs deliberately avoid an unpinned random subset, but a pinned
    /// CLI seed restores the same seeded install path used by windowed sessions.
    #[cfg_attr(not(feature = "observability"), allow(dead_code))]
    pub(crate) fn headless_trigger_pool_policy(self) -> TriggerPoolSeedPolicy {
        self.pool_seed_override
            .map(TriggerPoolSeedPolicy::Seeded)
            .unwrap_or(TriggerPoolSeedPolicy::ArmAll)
    }
}

enum PoolSeedArg {
    Absent,
    Valid(u64),
    Invalid(String),
}

/// Deferred-startup owner. Carries the raw inputs needed to construct the entire
/// `Session` after the first visible logo frame paints. It hands its raw argv to
/// [`Session::build`], the sole session construction site, which builds every
/// session-lifetime field (options I/O, audio, the scripting core, input/UI/modal
/// group, and the net endpoint). It is taken and consumed exactly once by
/// `App::install_pending_session`; suspend/resume keeps it unconsumed until the
/// install commits, so a resume that re-enters the splash loop never runs deferred
/// init twice. See: context/lib/boot_sequence.md §1, §5.
pub(crate) struct PendingSessionInit {
    /// Full `argv` (including `argv[0]`). Net args are parsed inside
    /// `Session::build` — never before the first visible frame.
    /// See: context/lib/networking.md.
    raw_args: Vec<String>,
}

impl PendingSessionInit {
    /// Construct the whole `Session` after the first logo frame and install it
    /// into `app.session`. `Session::build` runs synchronously, whole-or-nothing:
    /// it builds options I/O, the fault-tolerant audio subsystem, the scripting
    /// core + input/UI/modal group, and the net endpoint (degrading to
    /// single-player on parse/transport failure). It records the
    /// `audio_init_complete`, `script_runtime_ctor`, and `net_endpoint_complete`
    /// boot-timing marks; this method records the trailing `session_init_complete`
    /// mark once the session is installed.
    ///
    /// Returns `Err` if the `Session` build fails; the caller stores it in
    /// `exit_result`, logs, exits the event loop, and early-returns from the
    /// install frame so no later step runs against a `None` session.
    ///
    /// Caller guards single-commit via `Option::take` on `app.pending_session`,
    /// so this never runs twice across a suspend/resume.
    pub(crate) fn install(self, app: &mut App) -> Result<()> {
        // The sole session construction site. A hard failure (script-runtime
        // construction) propagates so the caller exits boot; audio and net
        // degrade in place inside `build`. `boot_timings` is threaded in so the
        // deferred-session marks record behind first pixels.
        // See: context/lib/boot_sequence.md §1.
        let session =
            crate::session::Session::build(&self.raw_args, &app.core_root, &mut app.boot_timings)
                .context("failed to build session")?;
        app.session = Some(session);
        app.boot_timings.record("session_init_complete");
        Ok(())
    }
}

/// Build the boot-lifetime `App` state (stages 1-3 of the boot order; stage 4 —
/// window + boot-ready renderer — fires later in `resumed()`) and the winit
/// event loop, returning the constructed `App` in the `Booting` state.
///
/// Ordering: minimal pre-event-loop work (logging, boot-timing setup, raw arg
/// collection, content-root / boot-map selection) runs first, THEN
/// `EventLoop::new`. The entire `Session` (options I/O, audio, the scripting
/// bootstrap, the input/UI/modal group, and the net endpoint) is constructed
/// post-first-pixel by `Session::build` through `PendingSessionInit`. Mod init,
/// the hot-reload watcher, debug-UI lazy-init, and the level-load worker spawn
/// likewise run on the splash frame loop so the first splash frame paints before
/// any of that.
/// See: context/lib/boot_sequence.md §1.
pub(crate) fn build_session() -> Result<BootSession> {
    // Timing starts at session construction so the first stage captures the
    // args_parsed → wgpu_init gap. See `StartupTimings` doc comment for
    // the per-line stage layout.
    let mut boot_timings = StartupTimings::new();

    // Minimal pre-event-loop work: raw args plus just enough parsing to identify
    // the content root and optional boot map. Net role is intentionally NOT
    // parsed here — it defers into `PendingSessionInit`.
    let args: Vec<String> = std::env::args().collect();
    // Parse once before either windowed or headless execution branches. The
    // resolved policy itself is chosen at each install so windowed restarts get
    // a new entropy seed while headless defaults to arm-all.
    let session_boot_config = SessionBootConfig::from_args(&args);
    let headless = headless_arg(&args);
    #[cfg(feature = "observe-live")]
    let observe_live_port = observe_live_port_arg(&args);

    // Static frame capture terminates the process instead of returning a
    // `BootSession`, so no event loop, window, or session is ever created. As
    // with `--headless`, detection remains outside its feature gate so a stock
    // binary can explain how to launch the mode.
    if let Some(scene_arg) = capture_arg(&args) {
        #[cfg(feature = "capture")]
        crate::capture::run_capture(scene_arg);
        #[cfg(not(feature = "capture"))]
        {
            let _ = scene_arg;
            eprintln!(
                "[Capture] `--capture` requires the `capture` feature; \\
                 launch via `cargo run -p xtask -- capture <scene.json>`"
            );
            std::process::exit(1);
        }
    }

    // Headless observability batch mode terminates the process (exit code)
    // instead of returning a `BootSession`, so `main` never drives a windowless
    // event loop. Arg detection sits OUTSIDE the feature gate: the driver body is
    // feature-gated, but the diagnostic for a build without the feature must be
    // reachable so `--headless` on a stock binary fails loudly.
    if let Some(runspec_arg) = headless {
        #[cfg(feature = "observability")]
        crate::observability::run_headless(
            runspec_arg,
            session_boot_config.headless_trigger_pool_policy(),
        );
        #[cfg(not(feature = "observability"))]
        {
            let _ = runspec_arg;
            eprintln!(
                "[Headless] `--headless` requires the `observability` feature; \
                 launch via `cargo run -p xtask -- observe <runspec>`"
            );
            std::process::exit(1);
        }
    }

    let map_arg = resolve_map_path(&args);
    let content_root = resolve_content_root(&args, map_arg.as_deref())?;
    let map_path = boot_map_path(&args, map_arg.as_deref(), &content_root)?;
    // Logged because a mismatch between this and the directory `prl-build` wrote
    // into surfaces only as per-texture placeholder warnings; the two paths in
    // the log are what makes that diagnosable.
    let baked_root = baked_root_arg(&args);
    // Logged for the same reason: a `core/` the engine cannot find costs three
    // screens and a splash, and every one of those degrades to a warning. The
    // resolved root in the log is what makes the absence attributable.
    let core_root = postretro_ui::CoreRoot::from_flag(core_root_arg(&args));
    log::info!("[Engine] Content root: {}", content_root.display());
    log::info!("[Engine] Core root: {}", core_root.path().display());
    if let Some(baked_root) = baked_root.as_ref() {
        log::info!(
            "[Engine] Baked root: {} (materials at {})",
            baked_root.display(),
            baked_root.join("materials").display(),
        );
    }
    boot_timings.record("args_parsed");

    // Event loop is created AHEAD of the whole session build (options I/O, audio,
    // the scripting bootstrap, and net-endpoint setup) so the window can come up
    // as early as practical. The entire `Session` is built post-first-pixel by
    // `PendingSessionInit::install`. See: context/lib/boot_sequence.md §1.
    let event_loop = EventLoop::new().context("failed to create event loop")?;
    boot_timings.record("event_loop_created");

    // Camera starts at a placeholder; `install_level_payload` repositions it
    // to the first `player_spawn` or the level geometry center
    // (`spawn_position()`) when no player start exists.
    let initial_camera_pos = Vec3::new(0.0, 200.0, 500.0);
    let initial_state = InterpolableState::new(initial_camera_pos);
    let active_level_source: Option<LevelSource> = None;

    #[cfg(feature = "observe-live")]
    let observe_live = observe_live_port.map(|port| {
        let map = active_level_source
            .as_ref()
            .map(|source| crate::startup::lifecycle::level_identity(source, &content_root))
            .unwrap_or_default();
        crate::observe_live::spawn_observe_live_transport(
            port,
            crate::observe_live::ServerHello {
                protocol: crate::observe_live::OBSERVE_LIVE_PROTOCOL,
                map,
                engine: env!("CARGO_PKG_VERSION").to_string(),
            },
        )
    });

    let app = App {
        renderer: None,
        window_state: None,
        level: None,
        nav_graph: None,
        map_path,
        content_root,
        baked_root,
        core_root,
        exit_result: Ok(()),
        camera: Camera::new(initial_camera_pos, 0.0, 0.0),
        // The entire `Session` (options, audio, scripting core, input/UI/modal
        // group, net endpoint) is built post-first-pixel by
        // `PendingSessionInit::install`; `None` through the boot phase.
        session: None,
        #[cfg(feature = "observe-live")]
        observe_live,
        remote_player_presentation: crate::netcode::ClientPresentationInputs::default(),
        crouch_toggle_active: false,
        ai_runtime: postretro_ai::AiRuntime::new(),
        cursor_pos: None,
        nav_stick_tracker: input::StickNavTracker::new(),
        frame_timing: FrameTiming::new(initial_state),
        view_feel_state: view_feel::ViewFeelState::default(),
        view_feel_followed_pawn: None,
        view_feel_descriptor: None,
        diagnostic_inputs: input::DiagnosticInputs::new(input::default_diagnostic_chords()),
        capture_portal_walk_next_frame: false,
        scratch_cells: Vec::new(),
        blocked_portals: Vec::new(),
        frame_rate_meter: FrameRateMeter::new(),
        title_buffer: String::with_capacity(256),
        last_title_update: Instant::now(),
        // Every session-lifetime field (scripting core, options, frontend, net
        // endpoint, audio, debug UI) is owned by `Session`, built post-first-pixel
        // by `PendingSessionInit::install`. See: context/lib/boot_sequence.md §1.
        mod_theme_override: ModThemeTokens::default(),
        switching: SwitchingDescriptor::default(),
        pending_mode_signal: None,
        pending_menu_toggle: false,
        pending_exit_to_desktop: false,
        ui_focused_id: None,
        particle_live_counts: std::collections::HashMap::new(),
        collision_world: collision::CollisionWorld::new(),
        kinematic_mover_colliders: Vec::new(),
        kinematic_mover_tick_states: kinematic_mover::MoverTickStateTable::default(),
        mover_yaw_carry_ground: postretro_foundation::GroundRef::Airborne,
        kinematic_mover_render: runtime_movers::KinematicMoverRenderCollector::new(),
        trigger_bindings: crate::trigger_bindings::TriggerBindingTable::default(),
        trigger_pool_report: crate::trigger_pools::TriggerPoolInstallReport::default(),
        client_fire_resolutions: Vec::new(),
        client_predicted_shots: crate::weapon::ClientPredictedShots::new(),
        host_spawn_points: Vec::new(),
        script_time: 0.0,
        anim_time: 0.0,
        anim_time_scale: 1.0,
        boot_state: App::initial_boot_state(),
        splash_frame: 0,
        pending_level_log: false,
        pending_splash_override: None,
        boot_timings,
        session_boot_config,
        mod_timings: StartupTimings::new(),
        level_timings: StartupTimings::new(),
        active_level_tags: Vec::new(),
        active_level_source,
        level_load: None,
        level_rx: None,
        level_worker: None,
        level_requests: VecDeque::new(),
        boot_load: false,
        pending_session: Some(PendingSessionInit { raw_args: args }),
        #[cfg(feature = "dev-tools")]
        debug_chase_agent: None,
    };

    Ok(BootSession { event_loop, app })
}

/// Every flag naming a directory — `--mod` names one by its name under
/// `content/` — in one list, read by the scanners that extract a value and by
/// the positional-map scan that must step over one.
///
/// Keeping it in one place is what holds the invariant. A flag added to only
/// half of them leaves its *value* exposed to `resolve_map_path`, which then
/// loads a directory as the level — the defect `--baked-root` hit and
/// `--core-root` would hit next.
const PATH_FLAGS: [&str; 3] = ["--mod", "--baked-root", "--core-root"];

/// Recover the positional map-path argument (the raw-path dev bypass), skipping
/// the values consumed by [`PATH_FLAGS`], `--pool-seed`, `--observe-live`, and
/// the netcode role flags `--host`/`--connect`. A value-taking flag missing
/// from this scan has its value mistaken for the map path, so every such flag
/// belongs here.
///
/// `--host`/`--connect` step over a following token under the same predicate
/// `postretro_netcode::parse_net_config` uses (non-empty, not `--`-prefixed),
/// so the two scans always agree on where a net value ends. That includes
/// `--host`'s optional port: a map placed directly after a bare `--host` is
/// taken as the port by both, and the net parser rejects it.
pub(crate) fn resolve_map_path(args: &[String]) -> Option<String> {
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        if PATH_FLAGS.contains(&arg.as_str()) || arg == "--pool-seed" || arg == "--observe-live" {
            if iter.peek().is_some_and(|value| !value.starts_with("--")) {
                let _ = iter.next();
            }
            continue;
        }
        if arg == "--host" || arg == "--connect" {
            if iter
                .peek()
                .is_some_and(|value| !value.is_empty() && !value.starts_with("--"))
            {
                let _ = iter.next();
            }
            continue;
        }
        // A `--flag=value` form carries its value inside one token, so no
        // separate per-flag list is needed: the general flag test covers them.
        if arg.starts_with("--") {
            continue;
        }
        return Some(arg.clone());
    }
    None
}

/// Read the value of one directory-naming flag, in `--flag <dir>` or
/// `--flag=<dir>` form.
///
/// Shared by every flag in [`PATH_FLAGS`] so they cannot drift apart. Absent, or
/// present with no value, yields `None` — a bare flag never silently resolves to
/// the current directory, and an empty value is treated as absence. The scan
/// steps over the other path flags' values so one flag never swallows another's.
fn path_flag_value(args: &[String], flag: &str) -> Option<PathBuf> {
    debug_assert!(
        PATH_FLAGS.contains(&flag),
        "every directory-naming flag belongs in PATH_FLAGS",
    );
    let equals_form = format!("{flag}=");
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter
                .next_if(|value| !value.is_empty() && !value.starts_with("--"))
                .map(PathBuf::from);
        }
        if let Some(value) = arg.strip_prefix(equals_form.as_str()) {
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
            continue;
        }
        if PATH_FLAGS.contains(&arg.as_str())
            && iter.peek().is_some_and(|value| !value.starts_with("--"))
        {
            let _ = iter.next();
        }
    }
    None
}

/// Parse the live-channel port without ever accepting a bindable address.
#[cfg(feature = "observe-live")]
fn observe_live_port_arg(args: &[String]) -> Option<u16> {
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        let value = if arg == "--observe-live" {
            iter.next_if(|value| !value.starts_with("--"))
                .map(String::as_str)
        } else if let Some(value) = arg.strip_prefix("--observe-live=") {
            Some(value)
        } else {
            continue;
        };

        let Some(value) = value.filter(|value| !value.is_empty()) else {
            log::warn!(
                "[Observe live] --observe-live requires a localhost TCP port; live introspection disabled"
            );
            return None;
        };
        return match value.parse::<u16>() {
            Ok(port) if port != 0 => Some(port),
            _ => {
                log::warn!(
                    "[Observe live] invalid --observe-live port {value:?}; live introspection disabled"
                );
                None
            }
        };
    }
    None
}

/// Detect the optional trigger-pool seed. This intentionally keeps manual
/// argument scanning beside `--headless`/`--mod`: no CLI parser owns
/// boot arguments yet, and a malformed seed degrades to the mode default.
fn pool_seed_arg(args: &[String]) -> PoolSeedArg {
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        let value = if arg == "--pool-seed" {
            match iter.next_if(|value| !value.starts_with("--")) {
                Some(value) => value.as_str(),
                None => return PoolSeedArg::Invalid("<missing>".to_string()),
            }
        } else if let Some(value) = arg.strip_prefix("--pool-seed=") {
            value
        } else {
            continue;
        };

        return match value.parse::<u64>() {
            Ok(seed) => PoolSeedArg::Valid(seed),
            Err(_) => PoolSeedArg::Invalid(value.to_string()),
        };
    }
    PoolSeedArg::Absent
}

/// Detect the headless observability batch-mode flag. Returns `Some(path)` when
/// `--headless <runspec>` / `--headless=<runspec>` is present (inner `None` when
/// the flag appears with no following value — the driver reports that as an
/// error), or `None` when the flag is absent. Kept beside the other arg helpers
/// and OUTSIDE the `observability` feature gate so a stock binary can still detect
/// the flag and emit the "rebuild with the feature" diagnostic.
fn headless_arg(args: &[String]) -> Option<Option<&str>> {
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        if arg == "--headless" {
            // A following token that is not itself a flag is the runspec path.
            let value = iter
                .next_if(|value| !value.starts_with("--"))
                .map(String::as_str);
            return Some(value);
        }
        if let Some(value) = arg.strip_prefix("--headless=") {
            return Some((!value.is_empty()).then_some(value));
        }
    }
    None
}

/// Detect the static offscreen frame-capture flag. Like `headless_arg`, the
/// nested `Option` distinguishes absence from a flag with no scene argument so
/// the capture driver can issue the user-facing diagnostic.
fn capture_arg(args: &[String]) -> Option<Option<&str>> {
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        if arg == "--capture" {
            let value = iter
                .next_if(|value| !value.starts_with("--"))
                .map(String::as_str);
            return Some(value);
        }
        if let Some(value) = arg.strip_prefix("--capture=") {
            return Some((!value.is_empty()).then_some(value));
        }
    }
    None
}

/// The raw `--mod <name>` / `--mod=<name>` value, before it is checked to be a
/// name. [`resolve_content_root`] turns it into `content/<name>`.
fn mod_arg(args: &[String]) -> Option<PathBuf> {
    path_flag_value(args, "--mod")
}

/// Detect `--baked-root <dir>` / `--baked-root=<dir>`: the directory that
/// *contains* `materials/`, not `materials/` itself.
///
/// A plain flag by design. The engine never learns what a project manifest is —
/// `postretro-tool` discovers the project and passes the resolved directory on
/// the command line, so a manifest schema change stays a tool change.
///
/// Absent (the normal case, including every dev run and test) leaves the `.prm`
/// root exactly where `derive_prm_root_dev_layout` puts it. An empty value is
/// treated as absent rather than as the current directory.
fn baked_root_arg(args: &[String]) -> Option<PathBuf> {
    path_flag_value(args, "--baked-root")
}

/// Detect `--core-root <dir>` / `--core-root=<dir>`: the directory that *holds*
/// the engine's own `ui/` and `textures/` trees — the `core/` directory itself.
///
/// The same shape as `--baked-root`, for the same reason: `core/` is resolved
/// from the engine's surroundings, and a launcher that pins the working
/// directory to a game project moves those surroundings out from under it. An
/// external project correctly has no `core/` of its own, so without this flag
/// the pause menu, frontend menu and on-screen keyboard are absent and
/// `load_named_tree` only warns.
///
/// Independent of `--mod` in both directions. `core/` is engine-owned: mounting
/// a game never replaces it, and relocating it never relocates game content.
///
/// Absent (the normal case, including every dev run and test) resolves `core/`
/// against the working directory exactly as before the flag existed.
fn core_root_arg(args: &[String]) -> Option<PathBuf> {
    path_flag_value(args, "--core-root")
}

/// Select the content root: `content/<name>` for `--mod <name>`, otherwise the
/// root derived from the map path (or the dev default).
///
/// The retired `--content-root <path>` is refused by name rather than ignored:
/// it is no longer in [`PATH_FLAGS`], so its value would otherwise be read as
/// the boot map and the directory loaded as a level.
///
/// A `--mod` with no name — bare, `--mod=`, or followed by another flag — is
/// refused too. Treating it as absent would boot the map-derived or default
/// mod, and a caller such as `postretro-tool run --mod "$MOD"` with `MOD`
/// unset would get the wrong mod with no error.
fn resolve_content_root(args: &[String], map_path: Option<&str>) -> Result<PathBuf> {
    if names_flag(args, "--content-root") {
        anyhow::bail!(
            "--content-root was removed; select a mod by name with --mod <name> \
             (for `{CONTENT_DIR}/dev`, pass `--mod dev`)"
        );
    }
    match mod_arg(args) {
        Some(name) => mod_content_root(&name),
        None if names_flag(args, "--mod") => anyhow::bail!(
            "--mod needs a mod name — a directory directly under `{CONTENT_DIR}/` \
             (for `{CONTENT_DIR}/dev`, pass `--mod dev`)"
        ),
        None => Ok(content_root_from_map(map_path)),
    }
}

/// Whether `flag` appears at all, in `--flag`, `--flag <value>`, or
/// `--flag=<value>` form.
fn names_flag(args: &[String], flag: &str) -> bool {
    let equals_form = format!("{flag}=");
    args.iter()
        .skip(1)
        .any(|arg| arg == flag || arg.starts_with(&equals_form))
}

/// `content/<name>` for a mod name, refusing anything that is not one plain
/// directory name. A path is refused rather than joined: `--mod content/dev`
/// would otherwise resolve `content/content/dev`, and every mod path the
/// engine derives from it (scripts, `baked/` materials) would miss without an
/// error at the flag that caused it. A leading `-` is refused to match the
/// tool's manifest check: in the split form such a name reads as a flag.
fn mod_content_root(name: &Path) -> Result<PathBuf> {
    let text = name.to_string_lossy();
    let is_plain_name = !text.is_empty()
        && text != "."
        && text != ".."
        && !text.starts_with('-')
        && !text.contains(['/', '\\', ':']);
    if !is_plain_name {
        anyhow::bail!(
            "--mod takes a mod name — a directory directly under `{CONTENT_DIR}/` — not a \
             path; got `{text}` (for `{CONTENT_DIR}/dev`, pass `--mod dev`)"
        );
    }
    Ok(Path::new(CONTENT_DIR).join(name))
}

/// The boot map to load, from the positional map argument.
///
/// With `--mod`, a relative map path names a file inside that mod —
/// `--mod dev maps/e1m1.prl` loads `content/dev/maps/e1m1.prl` — for the same
/// reason `--mod` takes a name: every mod lives under `content/`, so typing
/// that prefix is noise. A path that starts with `content/` anyway is refused
/// rather than joined, since `content/dev/content/dev/maps/...` fails only as a
/// missing file on the load worker, far from the argument that caused it. An
/// absolute path stays as given.
///
/// Without `--mod` the path is read from the working directory unchanged, and
/// the content root is derived from it instead ([`content_root_from_map`]).
fn boot_map_path(
    args: &[String],
    map_arg: Option<&str>,
    content_root: &Path,
) -> Result<Option<PathBuf>> {
    let Some(map_arg) = map_arg else {
        return Ok(None);
    };
    let map = Path::new(map_arg);
    let Some(mod_name) = mod_arg(args) else {
        return Ok(Some(map.to_path_buf()));
    };
    if map.is_absolute() {
        return Ok(Some(map.to_path_buf()));
    }
    // A leading `./` is still the working directory, so `./content/...` is the
    // same mistake as `content/...`.
    let mut components = map
        .components()
        .skip_while(|component| matches!(component, Component::CurDir));
    let names_content_dir = components
        .next()
        .is_some_and(|first| same_dir_name(first.as_os_str(), OsStr::new(CONTENT_DIR)));
    if names_content_dir {
        let name_a_map = "name a map inside it, such as `maps/<name>.prl`";
        let hint = match components.next() {
            Some(second) if same_dir_name(second.as_os_str(), mod_name.as_os_str()) => {
                let within_mod: PathBuf = components.collect();
                if within_mod.as_os_str().is_empty() {
                    name_a_map.to_string()
                } else {
                    format!("pass `{}`", within_mod.to_string_lossy().replace('\\', "/"))
                }
            }
            Some(_) => "to load another mod's map, name that mod with --mod".to_string(),
            None => name_a_map.to_string(),
        };
        anyhow::bail!(
            "with --mod, a map path is relative to the mod folder `{}`; got `{map_arg}` \
             ({hint})",
            content_root.display().to_string().replace('\\', "/")
        );
    }
    Ok(Some(content_root.join(map)))
}

/// Windows and macOS file systems are case-insensitive by default, so
/// `Content` is `content` there.
const CASE_INSENSITIVE_PATHS: bool = cfg!(any(windows, target_os = "macos"));

/// Whether two directory names name the same directory on this platform.
fn same_dir_name(a: &OsStr, b: &OsStr) -> bool {
    if CASE_INSENSITIVE_PATHS {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Derive the content root from a map path: the map's grandparent directory
/// (`content/<mod>/maps/x.prl` → `content/<mod>`), falling back to the dev-default
/// root when no map is supplied. Windowed boot derives the root from argv; the
/// headless observability path reuses this against the runspec map path.
pub(crate) fn content_root_from_map(map_path: Option<&str>) -> PathBuf {
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
            content_root_from_map(Some("content/example/maps/e1m1.prl")),
            PathBuf::from("content/example"),
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
    fn pool_seed_arg_accepts_split_and_equals_forms_at_u64_boundaries() {
        let split = vec![
            "postretro".to_string(),
            "--pool-seed".to_string(),
            "0".to_string(),
        ];
        assert_eq!(
            SessionBootConfig::from_args(&split).headless_trigger_pool_policy(),
            TriggerPoolSeedPolicy::Seeded(0)
        );

        let equals = vec!["postretro".to_string(), format!("--pool-seed={}", u64::MAX)];
        assert_eq!(
            SessionBootConfig::from_args(&equals).headless_trigger_pool_policy(),
            TriggerPoolSeedPolicy::Seeded(u64::MAX)
        );
    }

    #[test]
    fn invalid_or_missing_pool_seed_uses_headless_arm_all_policy() {
        for args in [
            vec!["postretro".to_string(), "--pool-seed".to_string()],
            vec![
                "postretro".to_string(),
                "--pool-seed=not-a-number".to_string(),
            ],
            vec![
                "postretro".to_string(),
                "--pool-seed".to_string(),
                "18446744073709551616".to_string(),
            ],
        ] {
            assert_eq!(
                SessionBootConfig::from_args(&args).headless_trigger_pool_policy(),
                TriggerPoolSeedPolicy::ArmAll
            );
        }
    }

    #[test]
    fn content_root_from_map_uses_default_dev_root_without_map() {
        assert_eq!(content_root_from_map(None), PathBuf::from("content/dev"));
    }

    #[test]
    fn prm_root_flag_parses_in_split_and_equals_forms() {
        let split = vec![
            "postretro".to_string(),
            "--baked-root".to_string(),
            "/project/baked".to_string(),
        ];
        assert_eq!(
            baked_root_arg(&split),
            Some(PathBuf::from("/project/baked"))
        );

        let equals = vec![
            "postretro".to_string(),
            "--baked-root=/project/baked".to_string(),
        ];
        assert_eq!(
            baked_root_arg(&equals),
            Some(PathBuf::from("/project/baked"))
        );

        assert_eq!(baked_root_arg(&["postretro".to_string()]), None);
        // A bare flag must not silently resolve to the current directory.
        assert_eq!(
            baked_root_arg(&["postretro".to_string(), "--baked-root".to_string()]),
            None
        );
    }

    /// `--baked-root` takes a value, so the positional-map-path scan has to skip
    /// it. Before the flag was added to that scan's list, a run with
    /// `--baked-root <dir>` and no map loaded `<dir>` as the map — and derived
    /// the content root from it.
    #[test]
    fn prm_root_flag_value_is_not_mistaken_for_the_map_path() {
        let args = vec![
            "postretro".to_string(),
            "--baked-root".to_string(),
            "/project/baked".to_string(),
        ];
        assert_eq!(resolve_map_path(&args), None);

        let with_map = vec![
            "postretro".to_string(),
            "--baked-root".to_string(),
            "/project/baked".to_string(),
            "content/dev/maps/campaign-test.prl".to_string(),
        ];
        assert_eq!(
            resolve_map_path(&with_map).as_deref(),
            Some("content/dev/maps/campaign-test.prl"),
        );
    }

    #[test]
    fn core_root_flag_parses_in_split_and_equals_forms() {
        let split = vec![
            "postretro".to_string(),
            "--core-root".to_string(),
            "/install/core".to_string(),
        ];
        assert_eq!(core_root_arg(&split), Some(PathBuf::from("/install/core")));

        let equals = vec![
            "postretro".to_string(),
            "--core-root=/install/core".to_string(),
        ];
        assert_eq!(core_root_arg(&equals), Some(PathBuf::from("/install/core")));

        assert_eq!(core_root_arg(&["postretro".to_string()]), None);
        // A bare flag must not silently resolve to the current directory.
        assert_eq!(
            core_root_arg(&["postretro".to_string(), "--core-root".to_string()]),
            None
        );
        assert_eq!(
            core_root_arg(&["postretro".to_string(), "--core-root=".to_string()]),
            None
        );
    }

    /// The compatibility contract, asserted on the paths themselves rather than
    /// on the flag: with `--core-root` absent, both consumers resolve exactly
    /// what they resolved before the flag existed.
    #[test]
    fn without_the_core_root_flag_both_consumers_keep_the_working_directory_paths() {
        let args = vec![
            "postretro".to_string(),
            "--mod".to_string(),
            "levels".to_string(),
            "--baked-root".to_string(),
            "baked".to_string(),
            "maps/e1m1.prl".to_string(),
        ];
        let core_root = postretro_ui::CoreRoot::from_flag(core_root_arg(&args));

        assert_eq!(core_root.path(), Path::new("core"));
        for file in [
            "hud.json",
            "pauseMenu.json",
            "frontendMenu.json",
            "keyboard.json",
        ] {
            assert_eq!(
                core_root.ui_asset_path(file),
                PathBuf::from("core/ui").join(file),
            );
        }
        assert_eq!(
            crate::startup::SplashSource::base_path(&core_root),
            PathBuf::from("core/textures/splash/postretro-ascii-art.png"),
        );
    }

    /// With the flag, the descriptors and the splash both move under the named
    /// directory — the whole point of the flag, and the thing a launcher that
    /// pins the working directory to a game project depends on.
    #[test]
    fn a_named_core_root_relocates_the_descriptors_and_the_splash() {
        let args = vec![
            "postretro".to_string(),
            "--core-root".to_string(),
            "/install/core".to_string(),
        ];
        let core_root = postretro_ui::CoreRoot::from_flag(core_root_arg(&args));

        assert_eq!(
            core_root.ui_asset_path("frontendMenu.json"),
            PathBuf::from("/install/core/ui/frontendMenu.json"),
        );
        assert_eq!(
            crate::startup::SplashSource::base_path(&core_root),
            PathBuf::from("/install/core/textures/splash/postretro-ascii-art.png"),
        );
    }

    /// `--core-root` takes a value, so the positional-map-path scan has to skip
    /// it. Invariant 11: a value-taking flag missing from that scan leaves its
    /// *value* exposed, and the engine loads a directory as the level.
    #[test]
    fn core_root_flag_value_is_not_mistaken_for_the_map_path() {
        let args = vec![
            "postretro".to_string(),
            "--core-root".to_string(),
            "/install/core".to_string(),
        ];
        assert_eq!(resolve_map_path(&args), None);
        // And the content root is not derived from it either.
        assert_eq!(
            resolve_content_root(&args, resolve_map_path(&args).as_deref())
                .expect("no --mod falls back to the dev default"),
            PathBuf::from("content/dev"),
        );

        let with_map = vec![
            "postretro".to_string(),
            "--core-root".to_string(),
            "/install/core".to_string(),
            "content/dev/maps/campaign-test.prl".to_string(),
        ];
        assert_eq!(
            resolve_map_path(&with_map).as_deref(),
            Some("content/dev/maps/campaign-test.prl"),
        );
    }

    /// The structural half of invariant 11: every directory-naming flag is
    /// stepped over by the positional-map scan. Adding a flag to `PATH_FLAGS`
    /// enrolls it here, so the next one cannot be half-wired.
    #[test]
    fn resolve_map_path_skips_every_directory_naming_flag() {
        for flag in PATH_FLAGS {
            let split = vec![
                "postretro".to_string(),
                flag.to_string(),
                "/some/directory".to_string(),
            ];
            assert_eq!(resolve_map_path(&split), None, "{flag} value leaked");

            let with_map = vec![
                "postretro".to_string(),
                flag.to_string(),
                "/some/directory".to_string(),
                "maps/e1m1.prl".to_string(),
            ];
            assert_eq!(
                resolve_map_path(&with_map).as_deref(),
                Some("maps/e1m1.prl"),
                "{flag} displaced the map path",
            );

            let equals = vec![
                "postretro".to_string(),
                format!("{flag}=/some/directory"),
                "maps/e1m1.prl".to_string(),
            ];
            assert_eq!(
                resolve_map_path(&equals).as_deref(),
                Some("maps/e1m1.prl"),
                "{flag}=<dir> displaced the map path",
            );
        }
    }

    /// The three directory flags are mutually independent: none swallows
    /// another's value, in any order. `--core-root` in particular never becomes
    /// the content root — engine assets are not mod content.
    #[test]
    fn core_root_is_independent_of_the_mod_and_baked_roots() {
        let args = vec![
            "postretro".to_string(),
            "--core-root".to_string(),
            "/install/core".to_string(),
            "--mod".to_string(),
            "campaign".to_string(),
            "--baked-root".to_string(),
            "/project/baked".to_string(),
        ];
        assert_eq!(core_root_arg(&args), Some(PathBuf::from("/install/core")));
        assert_eq!(baked_root_arg(&args), Some(PathBuf::from("/project/baked")));
        assert_eq!(
            resolve_content_root(&args, None).expect("a mod name resolves"),
            PathBuf::from("content/campaign"),
        );

        // Reversed order resolves identically.
        let reversed = vec![
            "postretro".to_string(),
            "--baked-root".to_string(),
            "/project/baked".to_string(),
            "--mod".to_string(),
            "campaign".to_string(),
            "--core-root".to_string(),
            "/install/core".to_string(),
        ];
        assert_eq!(
            core_root_arg(&reversed),
            Some(PathBuf::from("/install/core"))
        );
        assert_eq!(
            baked_root_arg(&reversed),
            Some(PathBuf::from("/project/baked"))
        );
        assert_eq!(
            resolve_content_root(&reversed, None).expect("a mod name resolves"),
            PathBuf::from("content/campaign"),
        );
    }

    /// A flag whose value is missing does not consume the next flag, so the one
    /// after it still parses — the shared scanner's half of `mod_arg`'s existing
    /// guarantee, held for all three.
    #[test]
    fn a_directory_flag_without_a_value_does_not_eat_the_next_flag() {
        let args = vec![
            "postretro".to_string(),
            "--core-root".to_string(),
            "--mod".to_string(),
            "example".to_string(),
            "maps/dev.prl".to_string(),
        ];
        assert_eq!(core_root_arg(&args), None);
        assert_eq!(mod_arg(&args), Some(PathBuf::from("example")));
        assert_eq!(resolve_map_path(&args).as_deref(), Some("maps/dev.prl"));
    }

    /// `--baked-root` and `--mod` are independent: neither swallows the other's
    /// value, and the baked root never becomes the content root.
    #[test]
    fn prm_root_flag_and_mod_flag_stay_independent() {
        let args = vec![
            "postretro".to_string(),
            "--mod".to_string(),
            "campaign".to_string(),
            "--baked-root".to_string(),
            "/project/baked".to_string(),
        ];
        assert_eq!(
            resolve_content_root(&args, None).expect("a mod name resolves"),
            PathBuf::from("content/campaign")
        );
        assert_eq!(baked_root_arg(&args), Some(PathBuf::from("/project/baked")));

        // Reversed order resolves identically.
        let reversed = vec![
            "postretro".to_string(),
            "--baked-root".to_string(),
            "/project/baked".to_string(),
            "--mod".to_string(),
            "campaign".to_string(),
        ];
        assert_eq!(
            resolve_content_root(&reversed, None).expect("a mod name resolves"),
            PathBuf::from("content/campaign")
        );
        assert_eq!(
            baked_root_arg(&reversed),
            Some(PathBuf::from("/project/baked"))
        );
    }

    #[test]
    fn mod_arg_names_a_mod_under_the_content_directory() {
        let args = vec![
            "postretro".to_string(),
            "--mod".to_string(),
            "my-campaign".to_string(),
        ];
        assert_eq!(mod_arg(&args), Some(PathBuf::from("my-campaign")));
        assert_eq!(
            resolve_content_root(&args, None).expect("a mod name resolves"),
            PathBuf::from("content/my-campaign"),
        );
    }

    #[test]
    fn mod_arg_accepts_equals_form_without_creating_a_map_arg() {
        let args = vec!["postretro".to_string(), "--mod=my-campaign".to_string()];
        assert_eq!(mod_arg(&args), Some(PathBuf::from("my-campaign")));
        assert_eq!(resolve_map_path(&args), None);
        assert_eq!(
            resolve_content_root(&args, None).expect("a mod name resolves"),
            PathBuf::from("content/my-campaign"),
        );
    }

    /// `--mod` takes a name, not a path. A path is refused at the flag rather
    /// than joined under `content/`, where `--mod content/dev` would resolve
    /// `content/content/dev` and fail far from the cause.
    #[test]
    fn mod_arg_refuses_a_path_instead_of_a_name() {
        for value in [
            "content/dev",
            "content\\dev",
            "/project/levels",
            "C:dev",
            ".",
            "..",
            "-x",
        ] {
            let args = vec!["postretro".to_string(), format!("--mod={value}")];
            let error = resolve_content_root(&args, None)
                .expect_err("a path is not a mod name")
                .to_string();
            assert!(error.contains("--mod takes a mod name"), "{value}: {error}");
            assert!(error.contains(value), "{value}: {error}");
        }
    }

    /// The retired `--content-root` is refused by name. Silently ignoring it
    /// would leave its value to the positional-map scan, which would then load
    /// the directory as a level.
    #[test]
    fn the_retired_content_root_flag_is_refused_by_name() {
        for args in [
            &["--content-root", "content/example"][..],
            &["--content-root=content/example"][..],
        ] {
            let args: Vec<String> = std::iter::once("postretro")
                .chain(args.iter().copied())
                .map(String::from)
                .collect();
            let error = resolve_content_root(&args, None)
                .expect_err("the retired flag is refused")
                .to_string();
            assert!(error.contains("--content-root was removed"), "{error}");
            assert!(error.contains("--mod <name>"), "{error}");
        }
    }

    #[test]
    fn mod_arg_missing_value_does_not_consume_next_flag_or_corrupt_map_arg() {
        let args = vec![
            "postretro".to_string(),
            "--mod".to_string(),
            "--baked-root".to_string(),
            "/project/baked".to_string(),
            "maps/dev.prl".to_string(),
        ];

        assert_eq!(mod_arg(&args), None);
        assert_eq!(baked_root_arg(&args), Some(PathBuf::from("/project/baked")));
        assert_eq!(resolve_map_path(&args).as_deref(), Some("maps/dev.prl"));
    }

    #[test]
    fn mod_arg_empty_equals_value_is_not_a_map_arg() {
        let args = vec![
            "postretro".to_string(),
            "--mod=".to_string(),
            "maps/dev.prl".to_string(),
        ];

        assert_eq!(mod_arg(&args), None);
        assert_eq!(resolve_map_path(&args).as_deref(), Some("maps/dev.prl"));
    }

    /// A `--mod` with no name is refused rather than treated as absent, which
    /// would boot the default mod — the outcome of an unset `--mod "$MOD"`.
    #[test]
    fn a_mod_flag_without_a_name_is_refused() {
        for args in [
            &["--mod"][..],
            &["--mod="][..],
            &["--mod", "--baked-root", "/project/baked"][..],
            &["--mod", "", "maps/dev.prl"][..],
        ] {
            let args: Vec<String> = std::iter::once("postretro")
                .chain(args.iter().copied())
                .map(String::from)
                .collect();
            let error = resolve_content_root(&args, None)
                .expect_err("a nameless --mod is refused")
                .to_string();
            assert!(
                error.contains("--mod needs a mod name"),
                "{args:?}: {error}"
            );
        }
    }

    #[test]
    fn resolve_map_path_skips_mod_value() {
        let args = vec![
            "postretro".to_string(),
            "--mod".to_string(),
            "my-campaign".to_string(),
        ];
        assert_eq!(resolve_map_path(&args), None);
    }

    #[test]
    fn resolve_map_path_returns_bare_map_after_selected_mod() {
        let args = vec![
            "postretro".to_string(),
            "--mod".to_string(),
            "my-campaign".to_string(),
            "maps/dev-bypass.prl".to_string(),
        ];
        let map_path = resolve_map_path(&args);
        assert_eq!(map_path, Some("maps/dev-bypass.prl".to_string()));
        assert_eq!(
            resolve_content_root(&args, map_path.as_deref()).expect("a mod name resolves"),
            PathBuf::from("content/my-campaign"),
        );
    }

    fn boot_map(args: &[&str]) -> Result<Option<PathBuf>> {
        let args: Vec<String> = std::iter::once("postretro")
            .chain(args.iter().copied())
            .map(String::from)
            .collect();
        let map_arg = resolve_map_path(&args);
        let content_root = resolve_content_root(&args, map_arg.as_deref())?;
        boot_map_path(&args, map_arg.as_deref(), &content_root)
    }

    /// With `--mod`, the map argument names a file inside that mod, so the
    /// `content/<mod>/` prefix is never typed — the same rule `--mod` itself
    /// follows. `postretro-tool run` always passes `--mod`, so this is also
    /// what makes `postretro-tool run maps/e1m1.prl` load the mod's level.
    #[test]
    fn with_a_mod_the_map_argument_is_relative_to_the_mod_folder() {
        assert_eq!(
            boot_map(&["--mod", "my-campaign", "maps/e1m1.prl"]).expect("a mod map resolves"),
            Some(Path::new("content/my-campaign").join("maps/e1m1.prl")),
        );
        assert_eq!(
            boot_map(&["maps/e1m1.prl", "--mod=my-campaign"]).expect("order does not matter"),
            Some(Path::new("content/my-campaign").join("maps/e1m1.prl")),
        );
        assert_eq!(
            boot_map(&["--mod", "my-campaign"]).expect("no map is not an error"),
            None,
        );
    }

    /// Without `--mod` nothing changes: the map is read from the working
    /// directory and the content root is derived from it.
    #[test]
    fn without_a_mod_the_map_argument_is_read_from_the_working_directory() {
        assert_eq!(
            boot_map(&["content/dev/maps/campaign-test.prl"]).expect("a raw map resolves"),
            Some(PathBuf::from("content/dev/maps/campaign-test.prl")),
        );
    }

    #[test]
    fn with_a_mod_an_absolute_map_path_stays_as_given() {
        let absolute = std::env::temp_dir().join("elsewhere.prl");
        let absolute_text = absolute.to_string_lossy().into_owned();
        assert_eq!(
            boot_map(&["--mod", "my-campaign", &absolute_text]).expect("an absolute map resolves"),
            Some(absolute),
        );
    }

    /// A map that still carries the `content/` prefix is refused at boot, with
    /// the path to pass instead, rather than joined into
    /// `content/<mod>/content/<mod>/...` and reported only as a missing file
    /// by the load worker.
    #[test]
    fn with_a_mod_a_content_prefixed_map_is_refused_with_the_path_to_pass() {
        for map in [
            "content/my-campaign/maps/e1m1.prl",
            "./content/my-campaign/maps/e1m1.prl",
            "content\\my-campaign\\maps\\e1m1.prl",
            "Content/My-Campaign/maps/e1m1.prl",
        ] {
            // Backslash separators are Windows path semantics, and
            // case-insensitive names are Windows and macOS semantics; elsewhere
            // those are ordinary file names.
            if map.contains('\\') && !cfg!(windows) {
                continue;
            }
            if map.starts_with('C') && !CASE_INSENSITIVE_PATHS {
                continue;
            }
            let error = boot_map(&["--mod", "my-campaign", map])
                .expect_err("the content prefix is refused")
                .to_string();
            assert!(error.contains("pass `maps/e1m1.prl`"), "{map}: {error}");
            assert!(error.contains("content/my-campaign"), "{map}: {error}");
        }

        let other = boot_map(&["--mod", "my-campaign", "content/other/maps/e1m1.prl"])
            .expect_err("another mod's path is refused")
            .to_string();
        assert!(other.contains("name that mod with --mod"), "{other}");

        // Naming the mod folder, or `content/` itself, has no map to point at.
        for map in ["content/my-campaign", "content"] {
            let error = boot_map(&["--mod", "my-campaign", map])
                .expect_err("a directory is not a map")
                .to_string();
            assert!(error.contains("name a map inside it"), "{map}: {error}");
        }
    }

    #[test]
    fn headless_arg_absent_returns_none() {
        let args = vec!["postretro".to_string()];
        assert_eq!(headless_arg(&args), None);
    }

    #[test]
    fn headless_arg_captures_following_runspec_path() {
        let args = vec![
            "postretro".to_string(),
            "--headless".to_string(),
            "run.json".to_string(),
        ];
        assert_eq!(headless_arg(&args), Some(Some("run.json")));
    }

    #[test]
    fn headless_arg_present_without_value_is_some_none() {
        // A trailing `--headless` (or one followed by another flag) is detected,
        // but with no path — the driver reports the missing runspec.
        let args = vec!["postretro".to_string(), "--headless".to_string()];
        assert_eq!(headless_arg(&args), Some(None));

        let args = vec![
            "postretro".to_string(),
            "--headless".to_string(),
            "--mod".to_string(),
        ];
        assert_eq!(headless_arg(&args), Some(None));
    }

    #[test]
    fn headless_arg_accepts_equals_form() {
        let args = vec!["postretro".to_string(), "--headless=run.json".to_string()];
        assert_eq!(headless_arg(&args), Some(Some("run.json")));
    }

    #[test]
    fn capture_arg_absent_returns_none() {
        let args = vec!["postretro".to_string()];
        assert_eq!(capture_arg(&args), None);
    }

    #[test]
    fn capture_arg_captures_following_scene_path() {
        let args = vec![
            "postretro".to_string(),
            "--capture".to_string(),
            "scene.json".to_string(),
        ];
        assert_eq!(capture_arg(&args), Some(Some("scene.json")));
    }

    #[test]
    fn capture_arg_present_without_value_is_some_none() {
        let args = vec!["postretro".to_string(), "--capture".to_string()];
        assert_eq!(capture_arg(&args), Some(None));

        let args = vec![
            "postretro".to_string(),
            "--capture".to_string(),
            "--mod".to_string(),
        ];
        assert_eq!(capture_arg(&args), Some(None));
    }

    #[test]
    fn capture_arg_accepts_equals_form() {
        let args = vec!["postretro".to_string(), "--capture=scene.json".to_string()];
        assert_eq!(capture_arg(&args), Some(Some("scene.json")));
    }

    #[test]
    fn resolve_map_path_skips_observe_live_port() {
        let args = vec![
            "postretro".to_string(),
            "--observe-live".to_string(),
            "8998".to_string(),
            "content/dev/maps/campaign-test.prl".to_string(),
        ];
        assert_eq!(
            resolve_map_path(&args),
            Some("content/dev/maps/campaign-test.prl".to_string()),
        );
    }

    /// `--host`'s optional port is stepped over like every other value-taking
    /// flag's value, with the net flag preceding the map — the shape the prior
    /// gap missed (`main.rs`'s `net_flags_do_not_clobber_positional_map_path`
    /// only covered the flag trailing the map).
    #[test]
    fn host_flag_value_is_not_mistaken_for_the_map_path() {
        let args = vec![
            "postretro".to_string(),
            "--host".to_string(),
            "30000".to_string(),
        ];
        assert_eq!(resolve_map_path(&args), None);

        let with_map = vec![
            "postretro".to_string(),
            "--host".to_string(),
            "30000".to_string(),
            "content/dev/maps/campaign-test.prl".to_string(),
        ];
        assert_eq!(
            resolve_map_path(&with_map).as_deref(),
            Some("content/dev/maps/campaign-test.prl"),
        );
    }

    /// Same coverage as above for `--connect`, whose address is required rather
    /// than optional, but is stepped over under the identical predicate.
    #[test]
    fn connect_flag_value_is_not_mistaken_for_the_map_path() {
        let args = vec![
            "postretro".to_string(),
            "--connect".to_string(),
            "127.0.0.1:27015".to_string(),
        ];
        assert_eq!(resolve_map_path(&args), None);

        let with_map = vec![
            "postretro".to_string(),
            "--connect".to_string(),
            "127.0.0.1:27015".to_string(),
            "maps/e1m1.prl".to_string(),
        ];
        assert_eq!(
            resolve_map_path(&with_map).as_deref(),
            Some("maps/e1m1.prl"),
        );
    }

    /// Regression: `postretro --mod dev --host 30000` (no CLI map at all) used
    /// to return "30000" as the positional map, and `boot_map_path` joined it
    /// under the mod root as `content/dev/30000`, failing the boot level load.
    /// This is also what `postretro-tool run --host ...` hits, since the tool
    /// forwards args verbatim and always adds `--mod`.
    #[test]
    fn host_port_value_is_not_mistaken_for_the_map_path_with_mod() {
        assert_eq!(
            boot_map(&["--mod", "dev", "--host", "30000"]).expect("no map is not an error"),
            None,
        );
    }

    /// Same regression shape for `--connect`, the flag named in the bug report
    /// (`postretro --connect 127.0.0.1:27015` with `--mod`, and
    /// `postretro-tool run --connect ...`, which always adds `--mod`).
    #[test]
    fn connect_address_value_is_not_mistaken_for_the_map_path_with_mod() {
        assert_eq!(
            boot_map(&["--mod", "dev", "--connect", "127.0.0.1:27015"])
                .expect("no map is not an error"),
            None,
        );
    }

    /// `--host` takes an OPTIONAL port, so a bare `--host` directly followed by
    /// a positional map is ambiguous at the token level: nothing distinguishes
    /// "no port, this token is the map" from "an invalid port". This scan does
    /// not try to disambiguate what `parse_net_config`
    /// (`crates/netcode/src/lib.rs`) itself does not: that parser's
    /// `iter.next_if(|v| !v.is_empty() && !v.starts_with("--"))` consumes the
    /// very same next token as the attempted port and then rejects it with a
    /// `NetArgError` (non-numeric), so `resolve_map_path` mirrors that
    /// consumption and also does not surface the token as the map. A caller
    /// must pass `--host <port>` before the map, or put the map before
    /// `--host`, to avoid tripping this.
    #[test]
    fn bare_host_directly_before_a_map_is_consumed_as_the_attempted_port() {
        let args = vec![
            "postretro".to_string(),
            "--host".to_string(),
            "content/dev/maps/campaign-test.prl".to_string(),
        ];
        assert_eq!(resolve_map_path(&args), None);
    }

    #[cfg(feature = "observe-live")]
    #[test]
    fn observe_live_port_arg_accepts_only_nonzero_u16_ports() {
        let split = vec![
            "postretro".to_string(),
            "--observe-live".to_string(),
            u16::MAX.to_string(),
        ];
        assert_eq!(observe_live_port_arg(&split), Some(u16::MAX));

        let equals = vec!["postretro".to_string(), "--observe-live=8998".to_string()];
        assert_eq!(observe_live_port_arg(&equals), Some(8998));

        let address = vec![
            "postretro".to_string(),
            "--observe-live=127.0.0.1:8998".to_string(),
        ];
        assert_eq!(observe_live_port_arg(&address), None);

        let too_large = vec!["postretro".to_string(), "--observe-live=65536".to_string()];
        assert_eq!(observe_live_port_arg(&too_large), None);

        let zero = vec!["postretro".to_string(), "--observe-live=0".to_string()];
        assert_eq!(observe_live_port_arg(&zero), None);
    }
}
