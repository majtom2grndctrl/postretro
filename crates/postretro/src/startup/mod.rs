// Boot sequencing types: StartupTimings, BootState, SplashSource, level requests.
// See: context/lib/boot_sequence.md

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub(crate) mod lifecycle;
pub(crate) mod worker;

pub(crate) use lifecycle::FRONTEND_CLEAR_COLOR;
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

pub(crate) enum LevelRequest {
    Load(LevelSource),
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    Unload,
}

pub(crate) enum LevelSource {
    Path(PathBuf),
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
}
