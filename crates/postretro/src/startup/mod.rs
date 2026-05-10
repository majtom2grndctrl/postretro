// Boot sequencing types: StartupTimings, BootState, SplashSource.
// See: context/lib/boot_sequence.md

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub(crate) mod worker;

pub(crate) use worker::{LoadOutcome, spawn_level_worker};

/// Boot state machine. Drives the splash → first-level-frame transition
/// and gates which per-frame work runs.
///
/// `Booting` covers the slice between `main()` and `App::resumed()` — the
/// window does not exist yet, so neither does the renderer or the splash.
/// `Splash` runs from `resumed()` through worker delivery + install. The
/// frame loop polls the worker channel here and runs `mod_init` on the
/// second frame so the first paint is guaranteed before mod scripts
/// touch the engine. `Running` is the steady-state level frame loop.
#[derive(PartialEq, Eq)]
pub(crate) enum BootState {
    Booting,
    Splash,
    Running,
}

/// Source for the boot splash texture.
///
/// `Base` resolves to the built-in PNG at `content/base/textures/splash/`.
/// `Mod` carries an absolute path registered by a mod's `mod_init`.
///
/// The renderer install path is wired. Mod-side hook is deferred until the mod
/// system lands; today only `Base` is reachable in production.
pub(crate) enum SplashSource {
    Base,
    // Unreachable until the mod system ships a setter for
    // `App::pending_splash_override`; the consume path is already wired.
    #[allow(dead_code)]
    Mod(PathBuf),
}

impl SplashSource {
    /// Path to the base splash PNG, relative to the engine's working directory.
    pub(crate) fn base_path() -> PathBuf {
        PathBuf::from("content/base/textures/splash/postretro-ascii-art.png")
    }
}

/// Ordered list of named stage durations, captured by repeated calls to
/// `record()`. Each call measures the wall-clock delta since the previous
/// `record()` (or construction) and appends `(stage, delta)`.
///
/// The engine holds three independent instances on `App` — one per startup
/// log line (engine boot, mod init, level load).
///
/// **Worker-thread merge (level load).** The level-load worker thread runs
/// `prl_parse`, `texture_decode`, and `uv_normalize` off the main thread and
/// ships its own `Vec<(&'static str, Duration)>` back to the main thread. The
/// main thread splices those entries into `level_timings.entries` between
/// `worker_dispatch` and `worker_delivered` so the summary reads in
/// chronological order. `StartupTimings` does not implement the merge itself;
/// the main-thread orchestrator pushes the sentinel stages and inserts the
/// worker entries at the right spot.
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

    /// Capture the duration since the previous `record()` (or construction),
    /// append `(stage, delta)`, and advance the cursor.
    pub(crate) fn record(&mut self, stage: &'static str) {
        let now = Instant::now();
        let delta = now.duration_since(self.last);
        self.entries.push((stage, delta));
        self.last = now;
    }

    /// Format as `[Startup] stage=1.2ms, stage2=0.4ms, ...`. One decimal,
    /// `ms` suffix, comma-separated. Empty timings produce just `[Startup]`.
    pub(crate) fn summary(&self) -> String {
        let mut parts = String::new();
        let mut first = true;
        for (stage, dur) in &self.entries {
            if !first {
                parts.push_str(", ");
            }
            first = false;
            let ms = dur.as_secs_f64() * 1000.0;
            // `write!` to a String is infallible.
            let _ = write!(&mut parts, "{stage}={ms:.1}ms");
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
        // A short sleep gives `record()` a measurable, but not asserted-exact,
        // delta. The contract is "duration since previous mark"; the test
        // only checks the entry shape and that the duration is sane.
        sleep(Duration::from_millis(1));
        t.record("stage_a");

        assert_eq!(t.entries.len(), 1);
        assert_eq!(t.entries[0].0, "stage_a");
        // `Duration` is unsigned; this is really an "exists and is real" check.
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

    /// The worker-entry splice in `main.rs` inserts worker entries at
    /// `delivered_idx + i` for each entry `i`. This test verifies that the
    /// same pattern produces chronological order: `worker_dispatch`, then the
    /// worker entries in order, then `worker_delivered`.
    #[test]
    fn worker_entries_splice_preserves_chronological_order() {
        let mut t = StartupTimings::new();
        t.record("worker_dispatch");
        t.record("worker_delivered");

        // `worker_delivered` must be at index 1 for the splice to be correct.
        let delivered_idx = t
            .entries
            .iter()
            .position(|(s, _)| *s == "worker_delivered")
            .expect("worker_delivered must be present");
        assert_eq!(delivered_idx, 1);

        // Simulate the main.rs splice: insert worker entries at delivered_idx + i.
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
