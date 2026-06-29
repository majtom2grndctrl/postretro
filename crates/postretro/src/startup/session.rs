// Session-lifetime boot construction: argument parsing, content-root selection,
// `App` assembly, and the `PendingSessionInit` owner that constructs the entire
// `Session` after the first visible logo frame. `Session::build` is the sole
// session construction site; `App` holds only boot-lifetime fields plus
// `session: Option<Session>`.
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
use crate::startup::StartupTimings;
use crate::{App, collision, view_feel};
use postretro_foundation::ModThemeTokens;

/// Dev-default boot map when no content root or map argument is supplied. Used by
/// `content_root_from_map` to derive the default `content/dev` root.
const DEFAULT_MAP_PATH: &str = "content/dev/maps/campaign-test.prl";

/// Built session: the winit event loop plus the constructed `App`, handed back to
/// `main` so it can drive the loop and return the app's exit result.
pub(crate) struct BootSession {
    pub(crate) event_loop: EventLoop<()>,
    pub(crate) app: App,
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
        let session = crate::session::Session::build(&self.raw_args, &mut app.boot_timings)
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
    let map_path = resolve_map_path(&args);
    let content_root = resolve_content_root(&args, map_path.as_deref());
    log::info!("[Engine] Content root: {}", content_root.display());
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

    let app = App {
        renderer: None,
        window_state: None,
        level: None,
        nav_graph: None,
        map_path: map_path.map(PathBuf::from),
        content_root,
        exit_result: Ok(()),
        camera: Camera::new(initial_camera_pos, 0.0, 0.0),
        // The entire `Session` (options, audio, scripting core, input/UI/modal
        // group, net endpoint) is built post-first-pixel by
        // `PendingSessionInit::install`; `None` through the boot phase.
        session: None,
        crouch_toggle_active: false,
        ai_warned: std::collections::HashSet::new(),
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
        // Every session-lifetime field (scripting core, options, frontend, net
        // endpoint, audio, debug UI) is owned by `Session`, built post-first-pixel
        // by `PendingSessionInit::install`. See: context/lib/boot_sequence.md §1.
        mod_theme_override: ModThemeTokens::default(),
        pending_mode_signal: None,
        pending_menu_toggle: false,
        pending_exit_to_desktop: false,
        ui_focused_id: None,
        particle_live_counts: std::collections::HashMap::new(),
        collision_world: collision::CollisionWorld::new(),
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
        pending_session: Some(PendingSessionInit { raw_args: args }),
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
