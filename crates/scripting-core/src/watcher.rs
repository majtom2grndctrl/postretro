//! Dev-mode hot-reload plumbing for the scripting subsystem.
//!
//! Compiled only in debug builds (see `cfg` gate below and the `mod`
//! declaration in `scripting::mod`). In release builds, no watcher exists and
//! `ScriptRuntime::drain_reload_requests` is a no-op.
//! See: `context/lib/scripting.md`
//!
//! # Three-thread design
//!
//! ```text
//!   [fs]
//!     │
//!     ▼
//! ┌─────────────────────────────────┐
//! │ watcher thread                  │   FS events → internal mpsc
//! │  (notify + debouncer-full)      │──────────────────────────────┐
//! │  forwarder only, never classifies│                              │
//! └─────────────────────────────────┘                              │
//!                                                                  ▼
//!                                            ┌──────────────────────────────┐
//!                                            │ event-forwarder thread       │
//!                                            │  - batches changed paths     │
//!                                            │  - no compilation/classify   │
//!                                            └──────────────┬───────────────┘
//!                                                           │ ReloadRequest
//!                                                           ▼
//!                                            ┌──────────────────────────────┐
//!                                            │ frame loop/runtime           │
//!                                            │  dependency membership check │
//!                                            └──────────────────────────────┘
//! ```
//!
//! Separation matters: filesystem event delivery must never block on script
//! work. The watcher records changed paths only; `ScriptRuntime` decides
//! whether those paths affect the active mod-init dependency set.

// Belt-and-braces: the `mod watcher;` in `scripting::mod` is already
// `#[cfg(debug_assertions)]`, but gating the module itself ensures a release
// build can't pull this file in via any future path. Clippy flags the pair
// as duplicated attributes — silenced deliberately.
#![cfg(debug_assertions)]
#![allow(clippy::duplicated_attributes)]

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

use log::{error, info, warn};
use notify::{Config as NotifyConfig, EventKind, PollWatcher, RecursiveMode};
use notify_debouncer_full::{DebouncedEvent, RecommendedCache, new_debouncer_opt};
use serde::Deserialize;

use super::error::ScriptError;

/// ~200 ms debounce — well below a one-second reload budget even with a
/// compile step on the critical path.
const DEBOUNCE_MS: u64 = 200;
/// Polling avoids native-watcher blind spots in sandboxed macOS development
/// and still keeps debug hot reload comfortably under the one-second target.
const POLL_INTERVAL_MS: u64 = 100;

/// One reload request enqueued by the event-forwarder for the frame loop.
#[derive(Debug, Clone)]
pub struct ReloadRequest {
    pub paths: Vec<PathBuf>,
}

/// Where to find the `scripts-build` sidecar, chosen once at startup via the
/// detection cascade in [`TsCompilerPath::detect`].
#[derive(Debug, Clone)]
pub enum TsCompilerPath {
    /// `scripts-build` sitting next to the engine executable. This is how a
    /// self-contained distribution ships — the sidecar travels with the
    /// engine binary, no PATH configuration required.
    ScriptsBuildNextToEngine(PathBuf),
    /// `scripts-build` on `PATH` (developer global install).
    ScriptsBuildOnPath(PathBuf),
}

impl TsCompilerPath {
    /// Run the detection cascade. Returns `None` if nothing was found; the
    /// watcher still starts but `.ts` files fail to reload with a clear message.
    /// This probe is intentionally side-effect-free: development launchers own
    /// building `scripts-build` before the engine starts.
    ///
    /// **Order:**
    ///
    /// 1. `scripts-build` next to `std::env::current_exe()`.
    /// 2. `scripts-build` on `PATH`.
    ///
    // TODO(scripting-tools-dedup): the discovery cascade is duplicated in
    // `crates/level-compiler/src/main.rs` (`find_scripts_build`). The
    // level-compiler runs offline and cannot depend on this module
    // (`#[cfg(debug_assertions)]`-gated, also pulls in wgpu via the engine
    // crate). Consolidate into a shared `postretro-scripts-tools` crate when
    // the level-compiler gains more scripting integration.
    pub fn detect() -> Option<Self> {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        let path_var = std::env::var_os("PATH");
        Self::detect_with(exe_dir.as_deref(), path_var.as_deref())
    }

    /// Test-visible core of [`detect`]. Separate from process-global env so
    /// tests can drive the cascade with arbitrary inputs without mutating
    /// `PATH` (mutating env is `unsafe` in Rust 2024).
    pub fn detect_with(exe_dir: Option<&Path>, path_var: Option<&std::ffi::OsStr>) -> Option<Self> {
        if let Some(dir) = exe_dir
            && let Some(p) = scripts_build_in_dir(dir)
        {
            return Some(Self::ScriptsBuildNextToEngine(p));
        }
        if let Some(p) = which_in(path_var, "scripts-build") {
            return Some(Self::ScriptsBuildOnPath(p));
        }
        None
    }

    fn describe(&self) -> String {
        match self {
            Self::ScriptsBuildNextToEngine(p) => {
                format!("scripts-build (from current_exe dir: {})", p.display())
            }
            Self::ScriptsBuildOnPath(p) => format!("scripts-build (from PATH: {})", p.display()),
        }
    }

    fn binary_path(&self) -> &Path {
        match self {
            Self::ScriptsBuildNextToEngine(p) | Self::ScriptsBuildOnPath(p) => p,
        }
    }

    pub fn is_stale(&self) -> bool {
        let source_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("script-compiler")
            .join("src");
        if !source_dir.is_dir() {
            return false;
        }
        let (Some(sidecar_mtime), Some(newest_source_mtime)) = (
            file_mtime(self.binary_path()),
            newest_mtime_under(&source_dir),
        ) else {
            return false;
        };
        sidecar_is_stale(sidecar_mtime, newest_source_mtime)
    }

    /// Warn if the resolved sidecar predates its own source. `cargo run -p
    /// postretro` rebuilds the engine but not the `scripts-build` sidecar, so a
    /// developer editing `crates/script-compiler` launches against a stale
    /// binary — hot reload then breaks silently while the game boots fine.
    ///
    /// The compiler source dir is found via `CARGO_MANIFEST_DIR` (baked at
    /// scripting-core compile time); its sibling holds the compiler. That baked
    /// path only exists on the dev checkout that compiled the engine — exactly
    /// where the footgun lives. Anything missing or unreadable (e.g. a shipped
    /// distribution) skips silently: this is a best-effort heuristic, never a
    /// startup gate.
    pub fn warn_if_stale(&self) {
        if self.is_stale() {
            warn!(
                "[Scripting] `scripts-build` looks stale (older than its source). \
                 raw `cargo run -p postretro` does not rebuild the sidecar — hot reload \
                 may silently fail. Use `cargo run -p xtask -- run ...` or rebuild it: \
                 `cargo build -p postretro-script-compiler --bin scripts-build`."
            );
        }
    }
}

/// Pure staleness comparator. Stale when source was modified after the binary.
fn sidecar_is_stale(
    sidecar_mtime: std::time::SystemTime,
    newest_source_mtime: std::time::SystemTime,
) -> bool {
    newest_source_mtime > sidecar_mtime
}

fn file_mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Newest `modified()` time across all files under `dir`, recursively. `None`
/// if the walk yields no readable file mtime.
fn newest_mtime_under(dir: &Path) -> Option<std::time::SystemTime> {
    let mut newest: Option<std::time::SystemTime> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Some(mtime) = file_mtime(&path) {
                newest = Some(newest.map_or(mtime, |cur| cur.max(mtime)));
            }
        }
    }
    newest
}

/// Probe a directory for `scripts-build[.exe]`.
fn scripts_build_in_dir(dir: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "scripts-build.exe"
    } else {
        "scripts-build"
    };
    let candidate = dir.join(name);
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

/// Tiny manual `PATH` probe. Avoids the `which` crate — this is the only
/// lookup site. Takes PATH explicitly so tests don't mutate process env.
fn which_in(path_var: Option<&std::ffi::OsStr>, name: &str) -> Option<PathBuf> {
    let path_var = path_var?;
    let exe_name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(&exe_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// The hot-reload watcher. Owns two background threads (watcher,
/// event-forwarder) plus the channel the frame loop drains each tick.
///
/// Dropping `ScriptWatcher` shuts the debouncer down; the event-forwarder
/// thread observes its channel closing and exits on the next iteration.
pub struct ScriptWatcher {
    /// Kept alive so the debouncer thread keeps running. On drop, the
    /// debouncer stops, the internal event channel closes, and the
    /// event-forwarder loop exits naturally when its `recv` returns an error.
    _debouncer: notify_debouncer_full::Debouncer<PollWatcher, RecommendedCache>,
    reload_rx: Receiver<ReloadRequest>,
    /// Join handle for the event-forwarder. Not joined explicitly — the thread
    /// exits once the event-channel sender side drops. Kept in the struct so
    /// its lifetime is tied to `ScriptWatcher`.
    _event_forwarder: std::thread::JoinHandle<()>,
}

impl ScriptWatcher {
    /// Start the watcher against `script_root` plus the mod root.
    ///
    /// `script_root` is watched recursively for `.ts`/`.luau` changes
    /// (definition scripts under `<mod>/scripts/`).
    ///
    /// `mod_root` is watched non-recursively so changes to entry candidates
    /// are observed without double-watching `<mod>/scripts/`.
    ///
    /// `ts_compiler = None` is valid — `.ts` files fail to reload with a
    /// logged message, `.luau` still works.
    pub fn spawn(
        script_root: &Path,
        mod_root: &Path,
        ts_compiler: Option<TsCompilerPath>,
    ) -> Result<Self, ScriptError> {
        let script_root = script_root.to_path_buf();
        let mod_root = mod_root.to_path_buf();

        if let Some(ref c) = ts_compiler {
            info!("[Scripting] TS compiler = {}", c.describe());
        } else {
            error!(
                "[Scripting] `scripts-build` not found — run via \
                 `cargo run -p xtask -- run ...`, install it on PATH, or ship it next \
                 to the engine binary. `.ts` hot reload disabled; `.luau` files still work."
            );
        }

        // Channel 1: watcher (debouncer) thread → event-forwarder.
        let (event_tx, event_rx) = mpsc::channel::<DebouncedEvent>();
        // Channel 2: event-forwarder → frame loop.
        let (reload_tx, reload_rx) = mpsc::channel::<ReloadRequest>();

        // The closure passed to `new_debouncer_opt` runs on the debouncer's
        // internal thread. Its only job: forward each `DebouncedEvent` to the
        // event-forwarder. Never blocks on runtime classification.
        let watcher_config = NotifyConfig::default()
            .with_poll_interval(Duration::from_millis(POLL_INTERVAL_MS))
            .with_compare_contents(true);
        let mut debouncer = new_debouncer_opt::<_, PollWatcher, RecommendedCache>(
            Duration::from_millis(DEBOUNCE_MS),
            None,
            move |res: notify_debouncer_full::DebounceEventResult| match res {
                Ok(events) => {
                    for ev in events {
                        // `send` fails only if the event-forwarder has exited,
                        // which happens during shutdown. Drop quietly.
                        let _ = event_tx.send(ev);
                    }
                }
                Err(errors) => {
                    for e in errors {
                        warn!("scripts: filesystem watch error: {e}");
                    }
                }
            },
            RecommendedCache::new(),
            watcher_config,
        )
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to start script watcher: {e}"),
        })?;

        debouncer
            .watch(&script_root, RecursiveMode::Recursive)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to watch `{}`: {e}", script_root.display(),),
            })?;

        // Watch the mod root non-recursively so `start-script.{ts,js,luau}`
        // edits are observed without double-watching `<mod>/scripts/`.
        // Skipped silently when mod_root == script_root or when the mod root
        // does not exist as a directory (uncommon, but possible during tests).
        if mod_root != script_root && mod_root.is_dir() {
            debouncer
                .watch(&mod_root, RecursiveMode::NonRecursive)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("failed to watch mod root `{}`: {e}", mod_root.display(),),
                })?;
        }

        let event_forwarder = std::thread::Builder::new()
            .name("postretro-scripting-event-forwarder".to_string())
            .spawn(move || event_forwarder_loop(event_rx, reload_tx))
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to spawn script event-forwarder thread: {e}"),
            })?;

        Ok(Self {
            _debouncer: debouncer,
            reload_rx,
            _event_forwarder: event_forwarder,
        })
    }

    /// Drain pending changed-path batches non-blockingly. Classification
    /// intentionally lives in `ScriptRuntime`, which owns the active
    /// dependency set.
    pub fn drain_reload_requests(&mut self) -> Result<Vec<ReloadRequest>, ScriptError> {
        let mut requests = Vec::new();
        loop {
            match self.reload_rx.try_recv() {
                Ok(req) => requests.push(req),
                Err(TryRecvError::Empty) => return Ok(requests),
                Err(TryRecvError::Disconnected) => {
                    // Event-forwarder exited; channel is closed.
                    return Ok(requests);
                }
            }
        }
    }
}

/// Body of the event-forwarder thread. Loops until the event channel closes
/// (debouncer dropped). It forwards changed paths without deciding whether
/// those paths affect mod-init.
fn event_forwarder_loop(event_rx: Receiver<DebouncedEvent>, reload_tx: Sender<ReloadRequest>) {
    while let Ok(ev) = event_rx.recv() {
        // Include Remove so deletion/rename-away of an active dependency can
        // be classified against the previous committed dependency set.
        if !matches!(
            ev.event.kind,
            EventKind::Create(_)
                | EventKind::Modify(_)
                | EventKind::Remove(_)
                | EventKind::Any
                | EventKind::Other
        ) {
            continue;
        }

        if ev.event.paths.is_empty() {
            continue;
        }

        let _ = reload_tx.send(ReloadRequest {
            paths: ev.event.paths,
        });
    }
}

#[cfg(test)]
fn handle_path(path: &Path, reload_tx: &Sender<ReloadRequest>) {
    let _ = reload_tx.send(ReloadRequest {
        paths: vec![path.to_path_buf()],
    });
}

/// The `.js` artifact path for a given `.ts` source: same directory, same stem.
/// Sibling placement avoids an extra config knob.
pub fn compiled_output_for(ts_source: &Path) -> PathBuf {
    ts_source.with_extension("js")
}

/// Spawn the configured TS compiler and wait for it. Logs stderr on failure;
/// returns a short `Err` with the exit status.
pub fn run_ts_compiler(
    compiler: &TsCompilerPath,
    input: &Path,
    output: &Path,
) -> Result<(), String> {
    run_ts_compiler_command(compiler, input, output, false).map(|_| ())
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct TsDependencyReport {
    pub entry: PathBuf,
    pub output: PathBuf,
    pub dependencies: Vec<PathBuf>,
}

/// Spawn the configured TS compiler in dependency-report mode and parse its
/// machine-readable stdout. Malformed JSON, extra stdout text, or missing
/// fields are reported as compile failures so staged manifest builds can keep
/// the previous committed snapshot active.
pub fn run_ts_compiler_with_dependency_report(
    compiler: &TsCompilerPath,
    input: &Path,
    output: &Path,
) -> Result<TsDependencyReport, String> {
    let stdout = run_ts_compiler_command(compiler, input, output, true)?;
    parse_ts_dependency_report(&stdout)
}

fn parse_ts_dependency_report(stdout: &[u8]) -> Result<TsDependencyReport, String> {
    serde_json::from_slice::<TsDependencyReport>(stdout)
        .map_err(|e| format!("invalid dependency report from scripts-build: {e}"))
}

fn run_ts_compiler_command(
    compiler: &TsCompilerPath,
    input: &Path,
    output: &Path,
    dep_json: bool,
) -> Result<Vec<u8>, String> {
    use std::process::Command;

    let mut cmd = match compiler {
        TsCompilerPath::ScriptsBuildNextToEngine(p) | TsCompilerPath::ScriptsBuildOnPath(p) => {
            let mut c = Command::new(p);
            c.arg("--in").arg(input).arg("--out").arg(output);
            if dep_json {
                c.arg("--dep-json");
            }
            c
        }
    };

    let out = cmd.output().map_err(|e| format!("spawn failed: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !stderr.trim().is_empty() {
            error!("[Scripting] scripts-build stderr:\n{stderr}");
        }
        if !stdout.trim().is_empty() {
            error!("[Scripting] scripts-build stdout:\n{stdout}");
        }
        return Err(format!("exit status {}", out.status));
    }
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    /// Create a unique temp directory under `std::env::temp_dir`. No external
    /// crate — matches the convention used by `runtime.rs` tests.
    fn temp_dir(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "postretro_watcher_test_{}_{}_{name}",
            std::process::id(),
            n,
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// Poll `rx` for up to `deadline`, returning true if any request arrived.
    /// The watcher is not plumbed through `ScriptRuntime` in these tests —
    /// we're verifying the request-production pipeline end-to-end.
    fn wait_for_reload(watcher: &ScriptWatcher, deadline: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < deadline {
            if watcher.reload_rx.try_recv().is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// Like [`wait_for_reload`] but returns the paths from the first request.
    fn wait_for_reload_paths(watcher: &ScriptWatcher, deadline: Duration) -> Option<Vec<PathBuf>> {
        let start = Instant::now();
        while start.elapsed() < deadline {
            if let Ok(req) = watcher.reload_rx.try_recv() {
                return Some(req.paths);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    #[test]
    fn watcher_forwards_changed_path_without_classifying_js_artifact() {
        let js = PathBuf::from("/tmp/fake-mod/start-script.js");
        let (tx, rx) = mpsc::channel::<ReloadRequest>();
        handle_path(&js, &tx);

        assert_eq!(
            rx.try_recv().map(|r| r.paths).ok(),
            Some(vec![js]),
            "watcher forwards paths only; runtime classifies JS artifacts against dependencies",
        );
    }

    #[test]
    fn start_script_luau_edit_at_mod_root_forwards_changed_path() {
        // mod_root has a `scripts/` subdir (watched recursively) and a
        // `start-script.luau` at the mod root (covered by the non-recursive
        // mod-root watch).
        let mod_root = temp_dir("mod_init_luau");
        let scripts_root = mod_root.join("scripts");
        fs::create_dir_all(&scripts_root).unwrap();
        let start = mod_root.join("start-script.luau");
        fs::write(&start, "-- initial\n").unwrap();

        let watcher = ScriptWatcher::spawn(&scripts_root, &mod_root, None).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        fs::write(&start, "-- edited\n").unwrap();

        let paths = wait_for_reload_paths(&watcher, Duration::from_secs(2));
        assert!(
            paths
                .as_ref()
                .is_some_and(|paths| paths.iter().any(|path| path == &start)),
            "editing start-script.luau at the mod root should forward its path, got {paths:?}",
        );
    }

    #[test]
    fn luau_edit_triggers_reload() {
        let dir = temp_dir("luau_edit");
        let file = dir.join("archetypes.luau");
        fs::write(&file, "-- initial\n").unwrap();

        let watcher = ScriptWatcher::spawn(&dir, &dir, None).unwrap();
        // Give the watcher a moment to install itself before mutating.
        std::thread::sleep(Duration::from_millis(100));

        fs::write(&file, "-- edited\n").unwrap();

        assert!(
            wait_for_reload(&watcher, Duration::from_secs(2)),
            "expected a reload request after editing a .luau file"
        );
    }

    #[test]
    fn luau_rename_triggers_reload() {
        // Atomic-rename save pattern (editors like vim, VS Code on some
        // platforms). The watcher forwards event paths; runtime classifies
        // both old and new paths when notify supplies both.
        let dir = temp_dir("luau_rename");
        let file = dir.join("archetypes.luau");
        fs::write(&file, "-- initial\n").unwrap();

        let watcher = ScriptWatcher::spawn(&dir, &dir, None).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        let staging = dir.join("archetypes.luau.tmp");
        fs::write(&staging, "-- via rename\n").unwrap();
        fs::rename(&staging, &file).unwrap();

        assert!(
            wait_for_reload(&watcher, Duration::from_secs(2)),
            "expected a reload request after atomic-rename save"
        );
    }

    /// Locate the freshly-built `scripts-build` binary. `env!` only works
    /// for binaries declared as a dep of the current crate; falls back to
    /// walking relative to `CARGO_MANIFEST_DIR`.
    fn scripts_build_binary() -> Option<PathBuf> {
        // `CARGO_MANIFEST_DIR` is always set under `cargo test`. Walk up until
        // we find a `target/` directory — the workspace root sits two parents
        // above this crate, but be tolerant of layout changes by searching
        // upward.
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let name = if cfg!(windows) {
            "scripts-build.exe"
        } else {
            "scripts-build"
        };
        let mut dir: Option<&std::path::Path> = Some(manifest.as_path());
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

    /// Build `scripts-build` on demand if not already present. No-op when the
    /// workspace has already been built.
    fn ensure_scripts_build() -> PathBuf {
        if let Some(p) = scripts_build_binary() {
            return p;
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

    #[test]
    fn ts_edit_forwards_path_without_running_scripts_build() {
        let dir = temp_dir("ts_edit");
        let file = dir.join("archetypes.ts");
        fs::write(&file, "export const x: number = 1;\n").unwrap();

        let watcher = ScriptWatcher::spawn(&dir, &dir, None).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        fs::write(&file, "export const x: number = 2;\n").unwrap();

        assert!(
            wait_for_reload(&watcher, Duration::from_secs(5)),
            "expected a reload request after editing a .ts file"
        );
    }

    #[test]
    fn ts_syntax_error_still_forwards_path_for_runtime_classification() {
        let dir = temp_dir("ts_broken");
        let file = dir.join("archetypes.ts");
        fs::write(&file, "export const x: number = 1;\n").unwrap();

        let watcher = ScriptWatcher::spawn(&dir, &dir, None).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        fs::write(&file, "export const x: = = broken !@#$\n").unwrap();

        assert!(
            wait_for_reload(&watcher, Duration::from_secs(2)),
            "watcher should forward syntactically broken TS; staged build reports compile failure",
        );
    }

    #[test]
    fn ts_dependency_report_parser_rejects_missing_fields() {
        let err = parse_ts_dependency_report(
            br#"{"entry":"/tmp/start-script.ts","dependencies":["/tmp/start-script.ts"]}"#,
        )
        .unwrap_err();
        assert!(
            err.contains("missing field `output`"),
            "expected missing output field error, got {err}"
        );
    }

    #[test]
    fn ts_dependency_report_parser_rejects_extra_stdout_text() {
        let err = parse_ts_dependency_report(
            br#"{"entry":"/tmp/start-script.ts","output":"/tmp/start-script.js","dependencies":["/tmp/start-script.ts"]}
human diagnostic
"#,
        )
        .unwrap_err();
        assert!(
            err.contains("trailing characters"),
            "expected trailing stdout text error, got {err}"
        );
    }

    #[test]
    fn detect_finds_scripts_build_next_to_engine() {
        // Acceptance criterion: with `scripts-build` sitting next to the
        // engine executable, the cascade picks step 1. We simulate by pointing
        // `exe_dir` at the directory containing the freshly-built sidecar and
        // passing an empty PATH.
        let binary = ensure_scripts_build();
        let dir = binary.parent().unwrap().to_path_buf();

        let detected = TsCompilerPath::detect_with(Some(&dir), Some(std::ffi::OsStr::new("")));
        match detected {
            Some(TsCompilerPath::ScriptsBuildNextToEngine(p)) => {
                assert_eq!(p.parent().unwrap(), dir);
            }
            other => {
                panic!("expected detect_with() to return ScriptsBuildNextToEngine; got {other:?}")
            }
        }
    }

    #[test]
    fn sidecar_stale_when_source_newer_than_binary() {
        let dir = temp_dir("stale_source_newer");
        let binary = dir.join("scripts-build");
        let source = dir.join("lib.rs");
        fs::write(&binary, b"bin").unwrap();
        fs::write(&source, b"src").unwrap();
        set_mtime(&binary, 1_000);
        set_mtime(&source, 2_000);

        let sidecar = file_mtime(&binary).unwrap();
        let newest = newest_mtime_under(&dir).unwrap();
        assert!(
            sidecar_is_stale(sidecar, newest),
            "source modified after the binary is stale",
        );
    }

    #[test]
    fn sidecar_fresh_when_binary_newer_than_source() {
        let dir = temp_dir("fresh_binary_newer");
        let binary = dir.join("scripts-build");
        let source_dir = dir.join("src");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("lib.rs");
        fs::write(&binary, b"bin").unwrap();
        fs::write(&source, b"src").unwrap();
        set_mtime(&source, 1_000);
        set_mtime(&binary, 2_000);

        let sidecar = file_mtime(&binary).unwrap();
        let newest = newest_mtime_under(&source_dir).unwrap();
        assert!(
            !sidecar_is_stale(sidecar, newest),
            "binary modified after the source is fresh",
        );
    }

    #[test]
    fn missing_source_dir_yields_no_mtime() {
        // A shipped distribution has no compiler source dir; the walk yields
        // nothing and the caller skips the check rather than warning.
        let dir = temp_dir("missing_source");
        let absent = dir.join("does-not-exist");
        assert!(
            newest_mtime_under(&absent).is_none(),
            "walking an absent dir yields no mtime",
        );
    }

    /// Pin a file's mtime so stale-vs-fresh ordering is deterministic rather
    /// than depending on wall-clock write order.
    fn set_mtime(path: &Path, secs: u64) {
        let when = std::time::UNIX_EPOCH + Duration::from_secs(secs);
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(when).unwrap();
    }

    #[test]
    fn detect_falls_through_to_scripts_build_on_path() {
        // Step 2 of the cascade: no `scripts-build` next to current_exe, but
        // one on PATH.
        let binary = ensure_scripts_build();
        let dir = binary.parent().unwrap().to_path_buf();

        // Empty exe_dir — point at a directory we know doesn't contain the
        // sidecar. Use a temp dir (guaranteed to not contain `scripts-build`).
        let empty_dir = temp_dir("no_sidecar");

        let detected = TsCompilerPath::detect_with(Some(&empty_dir), Some(dir.as_os_str()));
        match detected {
            Some(TsCompilerPath::ScriptsBuildOnPath(p)) => {
                assert_eq!(p.parent().unwrap(), dir);
            }
            other => panic!("expected detect_with() to return ScriptsBuildOnPath; got {other:?}"),
        }
    }
}
