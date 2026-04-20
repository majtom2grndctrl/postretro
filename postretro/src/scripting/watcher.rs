//! Dev-mode hot-reload plumbing for the scripting subsystem.
//!
//! This entire module is compiled only in debug builds (see `cfg` gate at the
//! top of the file and at the `mod` declaration in `scripting::mod`). In
//! release builds, no watcher exists and `ScriptRuntime::drain_reload_requests`
//! is a no-op. See
//! `context/plans/in-progress/scripting-foundation/plan-1-runtime-foundation.md`
//! §Sub-plan 7 for the full architecture.
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
//! │  forwarder only, never compiles │                              │
//! └─────────────────────────────────┘                              │
//!                                                                  ▼
//!                                            ┌──────────────────────────────┐
//!                                            │ compile-worker thread        │
//!                                            │  - .ts → spawn compiler      │
//!                                            │  - .luau → straight through  │
//!                                            │  - logs stderr on failure    │
//!                                            └──────────────┬───────────────┘
//!                                                           │ ReloadRequest
//!                                                           ▼
//!                                            ┌──────────────────────────────┐
//!                                            │ frame loop                   │
//!                                            │  drain_reload_requests()     │
//!                                            │  ScriptRuntime::reload_…()   │
//!                                            └──────────────────────────────┘
//! ```
//!
//! The separation matters: a `tsc` or `scripts-build` subprocess can take
//! hundreds of milliseconds, and blocking the debouncer's delivery thread on
//! that subprocess risks dropping events while a compile is in flight. So
//! the watcher thread forwards immediately and the compile-worker thread owns
//! the slow path.

// Defence-in-depth: the `mod watcher;` registration in `scripting::mod` is
// already `#[cfg(debug_assertions)]`, but gating the module itself belt-and-
// braces means a release build can't accidentally pull this file in via some
// future path. Clippy flags the pair as duplicated — silenced deliberately.
#![cfg(debug_assertions)]
#![allow(clippy::duplicated_attributes)]

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

use log::{error, info, warn};
use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{DebouncedEvent, new_debouncer};

use super::error::ScriptError;
use super::runtime::ScriptRuntime;

/// ~200 ms debounce matches the plan and is well below the one-second
/// acceptance-criteria budget even with a compile step on the critical path.
const DEBOUNCE_MS: u64 = 200;

/// One reload request that the compile-worker enqueues for the frame loop.
///
/// `compiled_output_path` is the path the frame loop should re-evaluate: for
/// `.luau` it's the source file; for `.ts` it's the already-compiled `.js`
/// artifact. The frame loop's reload step doesn't distinguish — it just swaps
/// the definition context; re-evaluation is the caller's concern (sub-plan 7
/// deliberately stops at the context swap).
#[derive(Debug, Clone)]
pub(crate) struct ReloadRequest {
    #[allow(dead_code)]
    pub(crate) compiled_output_path: PathBuf,
}

/// Where to find the TypeScript compiler, chosen once at startup via the
/// detection cascade in [`TsCompilerPath::detect`].
///
/// The cascade is the single source of truth (see sub-plan 7). Do not add
/// new discovery steps without updating the plan.
#[derive(Debug, Clone)]
pub(crate) enum TsCompilerPath {
    /// `scripts-build` sitting next to the engine executable. This is how a
    /// self-contained distribution ships — the sidecar travels with the
    /// engine binary, no PATH configuration required.
    ScriptsBuildNextToEngine(PathBuf),
    /// `tsc` on `PATH`. Invoked as `tsc --project <root>/tsconfig.json`.
    Tsc(PathBuf),
    /// `npx` on `PATH`. Invoked as `npx tsc --project <root>/tsconfig.json`.
    Npx(PathBuf),
    /// `scripts-build` on `PATH` (developer global install).
    ScriptsBuildOnPath(PathBuf),
}

impl TsCompilerPath {
    /// Run the detection cascade. Returns `None` if nothing was found; the
    /// watcher still starts in that case but `.ts` files fail to reload with
    /// a clear message.
    ///
    /// **Order (authoritative — matches sub-plan 7 exactly):**
    ///
    /// 1. `scripts-build` next to `std::env::current_exe()`.
    /// 2. `tsc` on `PATH`.
    /// 3. `npx` on `PATH`.
    /// 4. `scripts-build` on `PATH`.
    pub(crate) fn detect() -> Option<Self> {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        let path_var = std::env::var_os("PATH");
        Self::detect_with(exe_dir.as_deref(), path_var.as_deref())
    }

    /// Test-visible core of [`detect`]. Kept separate from process-global env
    /// state so tests can drive the cascade with arbitrary inputs without
    /// mutating the shared `PATH` variable (mutating env is `unsafe` in Rust
    /// 2024 and disallowed by the project).
    pub(crate) fn detect_with(
        exe_dir: Option<&Path>,
        path_var: Option<&std::ffi::OsStr>,
    ) -> Option<Self> {
        if let Some(dir) = exe_dir
            && let Some(p) = scripts_build_in_dir(dir)
        {
            return Some(Self::ScriptsBuildNextToEngine(p));
        }
        if let Some(p) = which_in(path_var, "tsc") {
            return Some(Self::Tsc(p));
        }
        if let Some(p) = which_in(path_var, "npx") {
            return Some(Self::Npx(p));
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
            Self::Tsc(p) => format!("tsc ({})", p.display()),
            Self::Npx(p) => format!("npx ({})", p.display()),
            Self::ScriptsBuildOnPath(p) => format!("scripts-build (from PATH: {})", p.display()),
        }
    }
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

/// Tiny manual `PATH` probe. Avoids pulling in the `which` crate — this is
/// the only place we need lookup, and the logic is three lines. Takes the
/// PATH value explicitly so tests don't have to mutate process env.
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
/// compile-worker) plus the channel the frame loop drains each tick.
///
/// Dropping `ScriptWatcher` shuts the debouncer down; the compile-worker
/// thread observes its channel closing and exits on the next iteration.
pub(crate) struct ScriptWatcher {
    /// Kept alive so the debouncer thread keeps running. On drop, the
    /// debouncer stops, the internal event channel closes, and the
    /// compile-worker loop exits naturally when its `recv` returns an error.
    _debouncer: notify_debouncer_full::Debouncer<
        notify::RecommendedWatcher,
        notify_debouncer_full::RecommendedCache,
    >,
    reload_rx: Receiver<ReloadRequest>,
    /// Join handle for the compile-worker. Not joined explicitly — the thread
    /// exits once the event-channel sender side drops. Kept in the struct so
    /// its lifetime is tied to `ScriptWatcher`.
    _compile_worker: std::thread::JoinHandle<()>,
}

impl ScriptWatcher {
    /// Start the watcher against `script_root`, using `ts_compiler` for `.ts`
    /// files. `ts_compiler = None` is valid — `.ts` files will fail to reload
    /// with a logged message, but `.luau` files still work.
    pub(crate) fn spawn(
        script_root: &Path,
        ts_compiler: Option<TsCompilerPath>,
    ) -> Result<Self, ScriptError> {
        let script_root = script_root.to_path_buf();

        if let Some(ref c) = ts_compiler {
            info!("scripts: TS compiler = {}", c.describe());
        } else {
            error!(
                "scripts: no TypeScript compiler found — install `tsc` or `npx`, \
                 or ensure `scripts-build` ships next to the engine binary. \
                 `.ts` hot reload disabled; `.luau` files still work."
            );
        }

        // Channel 1: watcher (debouncer) thread → compile-worker.
        let (event_tx, event_rx) = mpsc::channel::<DebouncedEvent>();
        // Channel 2: compile-worker → frame loop.
        let (reload_tx, reload_rx) = mpsc::channel::<ReloadRequest>();

        // Spin up the debouncer. The closure supplied to `new_debouncer` runs
        // on the debouncer's internal thread — that IS our "watcher thread".
        // Its only job is to forward each `DebouncedEvent` to the
        // compile-worker. It never blocks on compilation.
        let mut debouncer = new_debouncer(
            Duration::from_millis(DEBOUNCE_MS),
            None,
            move |res: notify_debouncer_full::DebounceEventResult| match res {
                Ok(events) => {
                    for ev in events {
                        // `send` fails only if the compile-worker has exited,
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
        )
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to start script watcher: {e}"),
        })?;

        debouncer
            .watch(&script_root, RecursiveMode::Recursive)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to watch `{}`: {e}", script_root.display(),),
            })?;

        // Spawn the compile-worker. This thread runs the slow path.
        let compile_worker = std::thread::Builder::new()
            .name("postretro-scripting-compile-worker".to_string())
            .spawn(move || compile_worker_loop(event_rx, reload_tx, ts_compiler, &script_root))
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to spawn compile-worker thread: {e}"),
            })?;

        Ok(Self {
            _debouncer: debouncer,
            reload_rx,
            _compile_worker: compile_worker,
        })
    }

    /// Drain any pending reload requests non-blockingly. Called at the top of
    /// each frame. For each request, call `reload_definition_context` on the
    /// runtime. Errors are logged; the prior archetype set stays active.
    pub(crate) fn drain_reload_requests(
        &mut self,
        runtime: &mut ScriptRuntime,
    ) -> Result<(), ScriptError> {
        loop {
            match self.reload_rx.try_recv() {
                Ok(_req) => {
                    if let Err(e) = runtime.reload_definition_context() {
                        // Don't propagate — one bad reload mustn't kill the
                        // engine. The prior archetype set stays active.
                        error!("scripts: definition-context reload failed: {e}");
                    } else {
                        info!("scripts: definition context reloaded");
                    }
                }
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => {
                    // Compile-worker exited; nothing more will arrive.
                    return Ok(());
                }
            }
        }
    }
}

/// Body of the compile-worker thread. Loops until the event channel closes
/// (i.e. the debouncer dropped, which happens when `ScriptWatcher` is
/// dropped). Each event targeting a definition file gets compiled (if `.ts`)
/// or forwarded as-is (if `.luau`).
fn compile_worker_loop(
    event_rx: Receiver<DebouncedEvent>,
    reload_tx: Sender<ReloadRequest>,
    ts_compiler: Option<TsCompilerPath>,
    script_root: &Path,
) {
    while let Ok(ev) = event_rx.recv() {
        // Filter out event kinds that can't represent a content edit. Modify
        // and Create are the interesting ones. `Remove` is ignored — a file
        // being removed is not a reload trigger.
        if !matches!(
            ev.event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any | EventKind::Other
        ) {
            continue;
        }

        for path in &ev.event.paths {
            handle_path(path, &reload_tx, ts_compiler.as_ref(), script_root);
        }
    }
}

/// Handle one changed path. Decides by extension whether to compile or
/// forward, enqueues a `ReloadRequest` on success, logs on failure.
fn handle_path(
    path: &Path,
    reload_tx: &Sender<ReloadRequest>,
    ts_compiler: Option<&TsCompilerPath>,
    script_root: &Path,
) {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default();

    match ext {
        "luau" => {
            // Straight-through: Luau reads source directly.
            let _ = reload_tx.send(ReloadRequest {
                compiled_output_path: path.to_path_buf(),
            });
        }
        "ts" => {
            let Some(compiler) = ts_compiler else {
                error!(
                    "scripts: `.ts` file changed but no TypeScript compiler was detected at \
                     startup — cannot hot-reload `{}`",
                    path.display(),
                );
                return;
            };

            let out_path = compiled_output_for(path);
            match run_ts_compiler(compiler, path, &out_path, script_root) {
                Ok(()) => {
                    let _ = reload_tx.send(ReloadRequest {
                        compiled_output_path: out_path,
                    });
                }
                Err(msg) => {
                    // Compiler stderr already logged inside run_ts_compiler;
                    // this is the summary line. The prior archetype set stays
                    // active because no ReloadRequest was enqueued.
                    error!("scripts: TS compile failed for `{}`: {msg}", path.display());
                }
            }
        }
        _ => {
            // Not a definition file. Ignore. (Behavior-script hot reload is a
            // later plan — see sub-plan 7 scope limits.)
        }
    }
}

/// The `.js` artifact path that corresponds to a given `.ts` source. Siblings
/// in the same directory, sharing the stem. Keeping the output next to the
/// input avoids another config knob.
fn compiled_output_for(ts_source: &Path) -> PathBuf {
    ts_source.with_extension("js")
}

/// Spawn the configured TS compiler and wait for it. Logs stderr on failure.
/// Returns a short `Err` describing the exit status so the caller can emit a
/// summary log line.
fn run_ts_compiler(
    compiler: &TsCompilerPath,
    input: &Path,
    output: &Path,
    script_root: &Path,
) -> Result<(), String> {
    use std::process::Command;

    let mut cmd = match compiler {
        TsCompilerPath::ScriptsBuildNextToEngine(p) | TsCompilerPath::ScriptsBuildOnPath(p) => {
            let mut c = Command::new(p);
            c.arg("--in").arg(input).arg("--out").arg(output);
            c
        }
        TsCompilerPath::Tsc(p) => {
            // `tsc` is project-oriented; `--project <root>/tsconfig.json`
            // produces a whole-project build, matching the single-source-of-
            // truth in sub-plan 7. Per-file `--out` via tsc is awkward, so we
            // trust the project config to place artifacts where the engine
            // expects them.
            let mut c = Command::new(p);
            c.arg("--project").arg(script_root.join("tsconfig.json"));
            c
        }
        TsCompilerPath::Npx(p) => {
            let mut c = Command::new(p);
            c.arg("tsc")
                .arg("--project")
                .arg(script_root.join("tsconfig.json"));
            c
        }
    };

    let out = cmd.output().map_err(|e| format!("spawn failed: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        // Log the compiler's own output at `error` level so the modder sees
        // exactly what `tsc`/`scripts-build` said. Acceptance criterion.
        if !stderr.trim().is_empty() {
            error!("scripts: TS compiler stderr:\n{stderr}");
        }
        if !stdout.trim().is_empty() {
            error!("scripts: TS compiler stdout:\n{stdout}");
        }
        return Err(format!("exit status {}", out.status));
    }
    Ok(())
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

    #[test]
    fn luau_edit_triggers_reload() {
        let dir = temp_dir("luau_edit");
        let file = dir.join("archetypes.luau");
        fs::write(&file, "-- initial\n").unwrap();

        let watcher = ScriptWatcher::spawn(&dir, None).unwrap();
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
        // platforms). `notify-debouncer-full` is expected to surface this as
        // an event the compile-worker treats as a modify.
        let dir = temp_dir("luau_rename");
        let file = dir.join("archetypes.luau");
        fs::write(&file, "-- initial\n").unwrap();

        let watcher = ScriptWatcher::spawn(&dir, None).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        let staging = dir.join("archetypes.luau.tmp");
        fs::write(&staging, "-- via rename\n").unwrap();
        fs::rename(&staging, &file).unwrap();

        assert!(
            wait_for_reload(&watcher, Duration::from_secs(2)),
            "expected a reload request after atomic-rename save"
        );
    }

    /// Locate the freshly-built `scripts-build` binary for tests. The harness
    /// places it in `target/debug` (or wherever `CARGO_BIN_EXE_*` points for
    /// workspace sidecar binaries). `env!` exposes that at compile time — but
    /// only for binaries declared as a dep of the *current* crate, which we
    /// can't do without a circular dep. So fall back to walking relative to
    /// this crate's `target` dir.
    fn scripts_build_binary() -> Option<PathBuf> {
        // `CARGO_MANIFEST_DIR` is always set under `cargo test`.
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace = manifest.parent()?;
        let name = if cfg!(windows) {
            "scripts-build.exe"
        } else {
            "scripts-build"
        };
        for profile in ["debug", "release"] {
            let candidate = workspace.join("target").join(profile).join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }

    /// Build `scripts-build` on demand if it's not already present in the
    /// target dir. Keeps the test hermetic without slowing down cargo-test
    /// when the binary is already there (the common case when the dev has
    /// already built the workspace).
    fn ensure_scripts_build() -> PathBuf {
        if let Some(p) = scripts_build_binary() {
            return p;
        }
        let status = std::process::Command::new(env!("CARGO"))
            .args(["build", "-p", "postretro-script-compiler"])
            .status()
            .expect("cargo build scripts-build");
        assert!(status.success(), "failed to build scripts-build");
        scripts_build_binary().expect("scripts-build should exist after build")
    }

    #[test]
    fn ts_edit_triggers_reload_via_scripts_build() {
        let compiler_path = ensure_scripts_build();
        let dir = temp_dir("ts_edit");
        let file = dir.join("archetypes.ts");
        fs::write(&file, "export const x: number = 1;\n").unwrap();

        let watcher = ScriptWatcher::spawn(
            &dir,
            Some(TsCompilerPath::ScriptsBuildOnPath(compiler_path.clone())),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(100));

        fs::write(&file, "export const x: number = 2;\n").unwrap();

        assert!(
            wait_for_reload(&watcher, Duration::from_secs(5)),
            "expected a reload request after editing a .ts file"
        );

        let js = dir.join("archetypes.js");
        assert!(
            js.is_file(),
            "expected compiled output `{}` to exist",
            js.display()
        );
    }

    #[test]
    fn ts_syntax_error_does_not_enqueue_reload() {
        let compiler_path = ensure_scripts_build();
        let dir = temp_dir("ts_broken");
        let file = dir.join("archetypes.ts");
        // Valid first, just so the file exists before we start watching.
        fs::write(&file, "export const x: number = 1;\n").unwrap();

        let watcher = ScriptWatcher::spawn(
            &dir,
            Some(TsCompilerPath::ScriptsBuildOnPath(compiler_path.clone())),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(100));

        // Overwrite with garbage. The compiler should reject this and NOT
        // enqueue a ReloadRequest.
        fs::write(&file, "export const x: = = broken !@#$\n").unwrap();

        // Give the compile worker time to try and fail.
        std::thread::sleep(Duration::from_millis(1500));

        // Drain any pending requests. We expect none.
        let got = watcher.reload_rx.try_recv().ok();
        assert!(
            got.is_none(),
            "syntax-error save must not enqueue a ReloadRequest (got {got:?})"
        );
    }

    #[test]
    fn detect_finds_scripts_build_next_to_engine() {
        // Acceptance criterion: with `tsc` and `npx` absent from PATH but
        // `scripts-build` sitting next to the engine executable, the cascade
        // picks step 1. We simulate by pointing `exe_dir` at the directory
        // containing the freshly-built sidecar and passing an empty PATH.
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
    fn detect_falls_through_to_scripts_build_on_path() {
        // Step 4 of the cascade: `tsc` and `npx` absent, no `scripts-build`
        // next to current_exe, but one on PATH.
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
