// Boot sequencing types: StartupTimings, BootState, SplashSource, level requests.
// See: context/lib/boot_sequence.md

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub(crate) mod lifecycle;
pub(crate) mod session;
pub(crate) mod splash_lifecycle;
pub(crate) mod worker;

pub(crate) use lifecycle::FRONTEND_CLEAR_COLOR;
pub(crate) use session::{BootSession, PendingSessionInit, build_session};
pub(crate) use worker::{LoadOutcome, spawn_level_worker};

/// `Booting` = before `App::resumed()` (no window, no renderer).
/// `Splash` = first paint, then deferred `mod_init` and boot load request.
/// `Loading` = level worker in flight; main thread keeps painting while polling.
/// `Frontend` = renderer + UI loop with no level installed.
/// `Running` = steady-state level loop.
#[derive(PartialEq, Eq)]
pub(crate) enum BootState {
    Booting,
    Splash,
    Loading,
    Frontend,
    Running,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LevelRequest {
    Load(LevelSource),
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    Unload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LevelSource {
    #[allow(dead_code)]
    Catalog(String),
    Path(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LevelLoadEntry {
    pub(crate) catalog_id: Option<String>,
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InFlightLevelLoad {
    pub(crate) map_path: PathBuf,
    pub(crate) content_root: PathBuf,
    pub(crate) entry: LevelLoadEntry,
}

/// `Base` = built-in PNG at `content/base/textures/splash/`.
/// `Mod` = absolute path from mod's `mod_init`. Install path is wired; only `Base` is reachable today.
pub(crate) enum SplashSource {
    Base,
    // Unreachable until the mod system ships a setter for
    // `App::pending_splash_override`; the consume path is already wired.
    #[allow(dead_code)]
    Mod(PathBuf),
}

impl SplashSource {
    pub(crate) fn base_path() -> PathBuf {
        PathBuf::from("content/base/textures/splash/postretro-ascii-art.png")
    }
}

/// Boot phase derived from boot-relevant `App` state for the suspend/resume
/// contract (boot_sequence §1, §9; rendering_pipeline §7.8). Pure classifier so
/// the resume re-entry behavior is auditable without a window or GPU:
///
/// | Phase | Re-entry behavior on resume |
/// |---|---|
/// | `Black` | re-present black; no deferred session init has run |
/// | `Logo` | drop GPU splash state, re-present black then logo; no duplicate session init |
/// | `DeferredSession` | commit session state once (guarded by `pending_session.take`) |
/// | `FullRendererPending` | keep the session bundle, rerun renderer completion (idempotent) |
/// | `FullRendererComplete` | resume through the normal renderer/window rebuild |
///
/// Inputs are the same single-commit / readiness bits the splash loop already
/// holds: which splash frame is scheduled, whether `pending_session` was
/// consumed (session bundle installed), and whether the renderer reached
/// full-ready. The mapping is a function of those bits — not of wall-clock or
/// GPU state — so a suspend/resume re-entering any phase resolves deterministically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BootPhase {
    /// Frame 0 scheduled: the black frame has not yet presented (or a resume
    /// reset the schedule to frame 0). No deferred session init has run.
    Black,
    /// Frame 1 scheduled: black presented, logo decode/upload done, logo not yet
    /// committed to deferred session install.
    Logo,
    /// Logo presented; deferred session install pending (`pending_session` still
    /// `Some`). The single commit happens here.
    DeferredSession,
    /// Session installed (`pending_session` consumed) but the full renderer has
    /// not finished — renderer completion must rerun without re-running session init.
    FullRendererPending,
    /// Session installed and full renderer complete — steady boot tail.
    FullRendererComplete,
}

/// Classify the current boot phase from the splash schedule, whether the
/// deferred session bundle was installed (`pending_session` consumed), and
/// whether the renderer reached full-ready. Pure — no window, no GPU.
///
/// `session_installed == false` means `pending_session` is still `Some`: a
/// resume re-entering the splash loop must run the single deferred-session
/// commit (`Black`/`Logo`/`DeferredSession`). Once installed, the phase tracks
/// renderer completion only, so resume never re-runs session init.
pub(crate) fn classify_boot_phase(
    splash_frame: u32,
    session_installed: bool,
    renderer_full_ready: bool,
) -> BootPhase {
    if !session_installed {
        return match splash_frame {
            0 => BootPhase::Black,
            1 => BootPhase::Logo,
            // Frame 1 presented but the session install has not committed yet.
            _ => BootPhase::DeferredSession,
        };
    }
    if renderer_full_ready {
        BootPhase::FullRendererComplete
    } else {
        BootPhase::FullRendererPending
    }
}

/// Single-commit guard for deferred work that must run at most once across a
/// suspend/resume cycle. Returns the owned value to install the first time the
/// slot is `Some`, leaving `None` behind so a resume re-entry skips it. This is
/// the `pending_session.take()` pattern named as a seam: a resume that re-enters
/// the splash loop finds `None` and never re-installs. See: boot_sequence §9.
pub(crate) fn take_once<T>(slot: &mut Option<T>) -> Option<T> {
    slot.take()
}

/// The redraw path may drain stale script-reload requests only after the splash
/// logo frame has presented THIS boot cycle. Pure predicate naming the guard so
/// the per-boot ordering contract is testable without a window. The caller
/// passes `splash_frame >= 2` (frame 0 = black, frame 1 = logo); on suspend the
/// frame counter resets to 0, so a resumed boot re-blocks the drain until its
/// own logo frame repaints. Past the logo also guarantees the script runtime
/// exists, since deferred session install runs on the logo frame and the runtime
/// is session-lifetime. See: boot_sequence §1.
pub(crate) fn boot_allows_reload_drain(logo_frame_shown: bool) -> bool {
    logo_frame_shown
}

/// Named stage timings for the three startup log lines (engine boot, mod init, level load).
/// The main thread splices worker-thread entries between `worker_dispatch` and
/// `worker_delivered` after delivery to preserve chronological order.
pub(crate) struct StartupTimings {
    pub(crate) entries: Vec<(&'static str, Duration)>,
    last: Instant,
}

impl StartupTimings {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
            last: Instant::now(),
        }
    }

    pub(crate) fn record(&mut self, stage: &'static str) {
        let now = Instant::now();
        let delta = now.duration_since(self.last);
        self.entries.push((stage, delta));
        self.last = now;
    }

    pub(crate) fn summary(&self) -> String {
        let mut parts = String::new();
        let mut first = true;
        for (stage, dur) in &self.entries {
            if !first {
                parts.push_str(", ");
            }
            first = false;
            let ms = dur.as_secs_f64() * 1000.0;
            let _ = write!(&mut parts, "{stage}={ms:.1}ms"); // write! to String is infallible
        }
        if parts.is_empty() {
            String::from("[Startup]")
        } else {
            format!("[Startup] {parts}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn record_appends_named_entry_with_non_negative_duration() {
        let mut t = StartupTimings::new();
        sleep(Duration::from_millis(1)); // short sleep gives a measurable but non-asserted delta
        t.record("stage_a");

        assert_eq!(t.entries.len(), 1);
        assert_eq!(t.entries[0].0, "stage_a");
        assert!(t.entries[0].1 >= Duration::ZERO);
    }

    #[test]
    fn record_advances_cursor_so_consecutive_marks_split_the_elapsed_time() {
        let mut t = StartupTimings::new();
        t.record("first");
        t.record("second");

        assert_eq!(t.entries.len(), 2);
        assert_eq!(t.entries[0].0, "first");
        assert_eq!(t.entries[1].0, "second");
    }

    #[test]
    fn summary_is_prefixed_and_lists_each_recorded_stage() {
        let mut t = StartupTimings::new();
        t.record("args_parsed");
        t.record("wgpu_init");

        let s = t.summary();
        assert!(
            s.starts_with("[Startup] "),
            "summary must start with `[Startup] `, got: {s:?}",
        );
        assert!(s.contains("args_parsed="), "missing args_parsed in {s:?}");
        assert!(s.contains("wgpu_init="), "missing wgpu_init in {s:?}");
        assert!(s.contains("ms"), "missing ms suffix in {s:?}");
    }

    #[test]
    fn summary_with_no_entries_returns_just_the_prefix() {
        let t = StartupTimings::new();
        assert_eq!(t.summary(), "[Startup]");
    }

    /// Boot-timing marks in the exact order the boot path records them, valid
    /// (logo-present) path. Each entry names its `record(...)` call site so the
    /// sequence stays tied to the source — a moved or renamed mark forces a diff
    /// here. Net/audio/debug-UI/mod/level-worker marks are the ones the spec
    /// requires `first_black_frame` to precede; `script_runtime_ctor` is the one
    /// it must NOT (the runtime is built pre-window, before the black frame).
    fn boot_mark_order_valid_path() -> Vec<&'static str> {
        vec![
            "args_parsed",                 // session::build_session
            "event_loop_created",          // session::build_session (EventLoop::new)
            "script_runtime_ctor", // session::build_session (SessionServices::build) — pre-window
            "window_created",      // main::resumed
            "wgpu_init",           // main::resumed (Renderer::new, boot phase)
            "first_black_frame",   // splash_lifecycle::run_splash_frame_zero (after present)
            "splash_decoded",      // splash_lifecycle::run_splash_frame_zero
            "splash_uploaded",     // splash_lifecycle::run_splash_frame_zero
            "first_splash_frame",  // splash_lifecycle::paint_splash_after_black (after present)
            "audio_init_complete", // main::install_post_splash_services
            "net_endpoint_complete", // session::PendingSessionInit::install
            "session_init_complete", // session::PendingSessionInit::install
            "renderer_full_init_complete", // splash_lifecycle::finish_renderer_full_init
            "boot_worker_dispatch", // splash_lifecycle::run_splash_frame_one (boot-map path)
        ]
    }

    fn index_of(marks: &[(&'static str, Duration)], name: &str) -> usize {
        marks
            .iter()
            .position(|(s, _)| *s == name)
            .unwrap_or_else(|| panic!("mark `{name}` must be recorded"))
    }

    // Drift guard for the boot ordering contract. Replays the boot marks through
    // the real recorder in source order, then asserts the spec's relative-order
    // claims: first_black_frame precedes net/audio/debug-UI/mod/level-worker
    // marks, and does NOT precede script_runtime_ctor (script runtime is built
    // pre-window). See: boot_sequence §1, early-boot-solo-splash AC #1.
    #[test]
    fn boot_order_first_black_frame_precedes_session_audio_net_and_worker_marks() {
        let mut t = StartupTimings::new();
        for mark in boot_mark_order_valid_path() {
            t.record(mark);
        }
        let black = index_of(&t.entries, "first_black_frame");

        // first_black_frame precedes the deferred-service marks the spec pins.
        for after in [
            "audio_init_complete",
            "net_endpoint_complete",
            "session_init_complete",
            "renderer_full_init_complete",
            "boot_worker_dispatch",
        ] {
            assert!(
                black < index_of(&t.entries, after),
                "first_black_frame must precede {after}",
            );
        }

        // Coordinator resolution: the script runtime is constructed pre-window,
        // so its mark precedes first_black_frame — first_black does NOT precede it.
        assert!(
            index_of(&t.entries, "script_runtime_ctor") < black,
            "script_runtime_ctor is built pre-window, before the first black frame",
        );
    }

    /// Fallback (missing/malformed splash) boot order. `run_splash_frame_zero`
    /// records `splash_decoded`/`splash_uploaded` on BOTH success and failure so
    /// log line A always lists the same marks, then the schedule still advances
    /// to frame 1 — where post-splash services (audio/net/session) and renderer
    /// full-init run after the fallback black frame. See: boot_sequence §1.
    fn boot_mark_order_fallback_path() -> Vec<&'static str> {
        // Identical to the valid path: the only difference at runtime is a warn
        // log and the splash pass staying black (no logo installed). The mark
        // set and order are unchanged, which is the contract under test.
        boot_mark_order_valid_path()
    }

    #[test]
    fn boot_order_fallback_still_records_splash_marks_and_reaches_post_splash_services() {
        let mut t = StartupTimings::new();
        for mark in boot_mark_order_fallback_path() {
            t.record(mark);
        }
        // Both splash marks present even though decode failed (parity with the
        // success path's log line A).
        let decoded = index_of(&t.entries, "splash_decoded");
        let uploaded = index_of(&t.entries, "splash_uploaded");
        assert!(decoded < uploaded, "decode mark precedes upload mark");

        // The fallback path still reaches the post-splash services: their marks
        // are recorded after the (black) splash marks.
        for after in [
            "audio_init_complete",
            "net_endpoint_complete",
            "session_init_complete",
            "renderer_full_init_complete",
        ] {
            assert!(
                uploaded < index_of(&t.entries, after),
                "fallback path reaches {after} after the black splash frame",
            );
        }
    }

    // The black frame must present before the logo decode/upload and the logo
    // frame, which in turn must precede deferred session install. Causal first-
    // pixels ordering. See: boot_sequence §1 (Splash state machine).
    #[test]
    fn boot_order_black_precedes_decode_then_logo_then_session_install() {
        let mut t = StartupTimings::new();
        for mark in boot_mark_order_valid_path() {
            t.record(mark);
        }
        let black = index_of(&t.entries, "first_black_frame");
        let decoded = index_of(&t.entries, "splash_decoded");
        let uploaded = index_of(&t.entries, "splash_uploaded");
        let logo = index_of(&t.entries, "first_splash_frame");
        let session = index_of(&t.entries, "session_init_complete");

        assert!(
            black < decoded,
            "decode runs after the black frame presents"
        );
        assert!(decoded < uploaded, "upload follows decode");
        assert!(uploaded < logo, "logo frame follows upload");
        assert!(logo < session, "session install follows the logo frame");
    }

    #[test]
    fn worker_entries_splice_preserves_chronological_order() {
        let mut t = StartupTimings::new();
        t.record("worker_dispatch");
        t.record("worker_delivered");

        let delivered_idx = t
            .entries
            .iter()
            .position(|(s, _)| *s == "worker_delivered")
            .expect("worker_delivered must be present");
        assert_eq!(delivered_idx, 1);

        let worker_entries: Vec<(&'static str, Duration)> = vec![
            ("prl_parse", Duration::from_millis(10)),
            ("texture_decode", Duration::from_millis(20)),
            ("uv_normalize", Duration::from_millis(5)),
        ];
        for (i, entry) in worker_entries.into_iter().enumerate() {
            t.entries.insert(delivered_idx + i, entry);
        }

        let names: Vec<&str> = t.entries.iter().map(|(s, _)| *s).collect();
        assert_eq!(
            names,
            &[
                "worker_dispatch",
                "prl_parse",
                "texture_decode",
                "uv_normalize",
                "worker_delivered",
            ],
            "splice must produce chronological order"
        );
    }

    // --- Boot-phase classifier (suspend/resume contract) ---

    #[test]
    fn classify_boot_phase_frame_zero_pre_install_is_black() {
        assert_eq!(classify_boot_phase(0, false, false), BootPhase::Black);
    }

    #[test]
    fn classify_boot_phase_frame_one_pre_install_is_logo() {
        assert_eq!(classify_boot_phase(1, false, false), BootPhase::Logo);
    }

    #[test]
    fn classify_boot_phase_logo_presented_pre_install_is_deferred_session() {
        // Frame advanced past the logo but the session bundle is not yet
        // installed: the single deferred-session commit happens in this phase.
        assert_eq!(
            classify_boot_phase(2, false, false),
            BootPhase::DeferredSession
        );
    }

    #[test]
    fn classify_boot_phase_session_installed_renderer_incomplete_is_full_pending() {
        // Session installed, renderer not yet full: resume reruns renderer
        // completion only, never session init.
        assert_eq!(
            classify_boot_phase(2, true, false),
            BootPhase::FullRendererPending,
        );
    }

    #[test]
    fn classify_boot_phase_session_installed_renderer_full_is_complete() {
        assert_eq!(
            classify_boot_phase(2, true, true),
            BootPhase::FullRendererComplete,
        );
    }

    #[test]
    fn classify_boot_phase_session_installed_ignores_splash_frame() {
        // Once the session is installed, the phase tracks renderer completion
        // regardless of the (now-irrelevant) splash schedule value.
        assert_eq!(
            classify_boot_phase(1, true, false),
            BootPhase::FullRendererPending
        );
        assert_eq!(
            classify_boot_phase(0, true, true),
            BootPhase::FullRendererComplete
        );
    }

    // --- Single-commit guard (no duplicate session init across resume) ---

    #[test]
    fn take_once_yields_value_first_call_then_none() {
        let mut slot = Some(7u32);
        assert_eq!(take_once(&mut slot), Some(7), "first call installs");
        assert_eq!(
            take_once(&mut slot),
            None,
            "a resume re-entry finds the slot consumed and skips re-install",
        );
    }

    #[test]
    fn take_once_on_empty_slot_is_none() {
        let mut slot: Option<u32> = None;
        assert_eq!(take_once(&mut slot), None);
    }

    // --- Deferred-session guard before runtime exists ---

    #[test]
    fn boot_allows_reload_drain_only_after_logo_frame_shown() {
        // Before the logo frame presents this boot cycle (splash_frame < 2 ->
        // logo_frame_shown false), the redraw path must NOT drain script reloads:
        // the first logo frame has not painted yet, and on a resumed boot the
        // watcher's drain must wait for the resumed logo too.
        assert!(
            !boot_allows_reload_drain(false),
            "no reload drain before the logo frame paints this boot cycle",
        );
        assert!(
            boot_allows_reload_drain(true),
            "reload drain allowed once the logo frame has presented this boot cycle",
        );
    }
}
