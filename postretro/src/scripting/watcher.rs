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
//! Separation matters: a `tsc` or `scripts-build` subprocess can take
//! hundreds of milliseconds; blocking the debouncer's delivery thread would
//! drop events during a compile. The watcher thread forwards immediately;
//! the compile-worker owns the slow path.

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
use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{DebouncedEvent, new_debouncer};

use super::error::ScriptError;
use super::runtime::ScriptRuntime;

/// ~200 ms debounce — well below a one-second reload budget even with a
/// compile step on the critical path.
const DEBOUNCE_MS: u64 = 200;

/// One reload request enqueued by the compile-worker for the frame loop.
///
/// `compiled_output_path` is the path to re-evaluate: for `.luau`, the source
/// file; for `.ts`, the compiled `.js` artifact. The frame loop swaps the
/// definition context; re-evaluation is the caller's concern.
#[derive(Debug, Clone)]
pub(crate) struct ReloadRequest {
    #[allow(dead_code)]
    pub(crate) compiled_output_path: PathBuf,
}

/// Where to find the TypeScript compiler, chosen once at startup via the
/// detection cascade in [`TsCompilerPath::detect`].
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
    /// watcher still starts but `.ts` files fail to reload with a clear message.
    ///
    /// **Order:**
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

    /// Test-visible core of [`detect`]. Separate from process-global env so
    /// tests can drive the cascade with arbitrary inputs without mutating
    /// `PATH` (mutating env is `unsafe` in Rust 2024).
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
    /// Start the watcher against `script_root`. `ts_compiler = None` is valid
    /// — `.ts` files fail to reload with a logged message, `.luau` still works.
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

        // The closure passed to `new_debouncer` runs on the debouncer's
        // internal thread. Its only job: forward each `DebouncedEvent` to the
        // compile-worker. Never blocks on compilation.
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

    /// Drain pending reload requests non-blockingly. Call at the top of each
    /// frame. Reload errors are logged; the prior archetype set stays active.
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
                    // Compile-worker exited; channel is closed.
                    return Ok(());
                }
            }
        }
    }
}

/// Body of the compile-worker thread. Loops until the event channel closes
/// (debouncer dropped). Compiles `.ts` files and forwards `.luau` files as-is.
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
            // Not a definition file — ignore. Behavior-script hot reload is
            // out of scope for this module.
        }
    }
}

/// The `.js` artifact path for a given `.ts` source: same directory, same stem.
/// Sibling placement avoids an extra config knob.
pub(crate) fn compiled_output_for(ts_source: &Path) -> PathBuf {
    ts_source.with_extension("js")
}

/// Spawn the configured TS compiler and wait for it. Logs stderr on failure;
/// returns a short `Err` with the exit status.
pub(crate) fn run_ts_compiler(
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
            // `tsc` is project-oriented: `--project <root>/tsconfig.json`
            // produces a whole-project build. Per-file `--out` via tsc is
            // awkward; the project config places artifacts where the engine
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
        // Log compiler output at `error` level so the modder sees exactly
        // what `tsc`/`scripts-build` said.
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

    /// Locate the freshly-built `scripts-build` binary. `env!` only works
    /// for binaries declared as a dep of the current crate; falls back to
    /// walking relative to `CARGO_MANIFEST_DIR`.
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

    /// Build `scripts-build` on demand if not already present. No-op when the
    /// workspace has already been built.
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
