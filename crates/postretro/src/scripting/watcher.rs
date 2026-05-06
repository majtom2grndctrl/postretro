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
//!                                            │  (no reload action today)    │
//!                                            └──────────────────────────────┘
//! ```
//!
//! Separation matters: a `scripts-build` subprocess can take hundreds of
//! milliseconds; blocking the debouncer's delivery thread would drop events
//! during a compile. The watcher thread forwards immediately; the
//! compile-worker owns the slow path.

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

/// ~200 ms debounce — well below a one-second reload budget even with a
/// compile step on the critical path.
const DEBOUNCE_MS: u64 = 200;

/// What kind of reload was triggered.
///
/// `Scripts` — a file under the scripts root changed; the engine should
/// re-run definition scripts (today this is just channel housekeeping).
///
/// `ModInit` — `start-script.{ts,js,luau}` (or a `.ts` sibling at the mod
/// root, treated as a likely import) changed; the engine should re-run
/// `run_mod_init` so the mod manifest stays current without restarting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReloadKind {
    Scripts,
    ModInit,
}

/// One reload request enqueued by the compile-worker for the frame loop.
#[derive(Debug, Clone)]
pub(crate) struct ReloadRequest {
    pub(crate) kind: ReloadKind,
}

pub(crate) use super::runtime::ReloadSummary;

/// Where to find the `scripts-build` sidecar, chosen once at startup via the
/// detection cascade in [`TsCompilerPath::detect`].
#[derive(Debug, Clone)]
pub(crate) enum TsCompilerPath {
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
    // the level-compiler gains more scripting integration. See:
    // context/plans/drafts/scripting-tools-dedup/index.md
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
    /// Start the watcher against `script_root` plus the mod root.
    ///
    /// `script_root` is watched recursively for `.ts`/`.luau` changes
    /// (definition scripts under `<mod>/scripts/`).
    ///
    /// `mod_root` is watched non-recursively so changes to
    /// `start-script.{ts,js,luau}` (and any `.ts` siblings, treated as likely
    /// start-script imports) trigger a `ReloadKind::ModInit` request.
    /// Non-recursive watching avoids double-watching `<mod>/scripts/`.
    ///
    /// `ts_compiler = None` is valid — `.ts` files fail to reload with a
    /// logged message, `.luau` still works.
    pub(crate) fn spawn(
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
                "[Scripting] `scripts-build` not found — install it on PATH or \
                 ship it next to the engine binary. `.ts` hot reload disabled; \
                 `.luau` files still work."
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

        // The compile-worker classifies events by comparing the parent of
        // each changed path against `mod_root`. On macOS, `notify` reports
        // canonical paths (e.g. `/private/tmp/...`) while a caller may pass
        // a symlinked path (`/tmp/...`). Canonicalize once up front so the
        // comparison is path-form-agnostic; fall back to the original on
        // failure (e.g. directory removed).
        let mod_root_for_worker =
            std::fs::canonicalize(&mod_root).unwrap_or_else(|_| mod_root.clone());
        let compile_worker = std::thread::Builder::new()
            .name("postretro-scripting-compile-worker".to_string())
            .spawn(move || {
                compile_worker_loop(event_rx, reload_tx, ts_compiler, mod_root_for_worker)
            })
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to spawn compile-worker thread: {e}"),
            })?;

        Ok(Self {
            _debouncer: debouncer,
            reload_rx,
            _compile_worker: compile_worker,
        })
    }

    /// Drain pending reload requests non-blockingly. Returns a
    /// [`ReloadSummary`] describing which kinds of reload were observed.
    ///
    /// Every `Scripts` reload re-runs all definition scripts (full rebuild;
    /// targeted single-file reload not implemented). Every `ModInit` reload
    /// signals that `run_mod_init` should be re-run.
    pub(crate) fn drain_reload_requests(&mut self) -> Result<ReloadSummary, ScriptError> {
        let mut summary = ReloadSummary::default();
        loop {
            match self.reload_rx.try_recv() {
                Ok(req) => match req.kind {
                    ReloadKind::Scripts => summary.scripts = true,
                    ReloadKind::ModInit => summary.mod_init = true,
                },
                Err(TryRecvError::Empty) => return Ok(summary),
                Err(TryRecvError::Disconnected) => {
                    // Compile-worker exited; channel is closed.
                    return Ok(summary);
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
    mod_root: PathBuf,
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
            handle_path(path, &reload_tx, ts_compiler.as_ref(), &mod_root);
        }
    }
}

/// Classify a changed path as a `ModInit` or `Scripts` reload.
///
/// A path counts as `ModInit` if its parent directory equals `mod_root` and
/// its file stem is `start-script` (any extension), OR if it is any `.ts`
/// file directly at the mod root (treated as a likely start-script import).
/// Everything else under the watched scripts subtree is a `Scripts` reload.
fn classify_reload(path: &Path, mod_root: &Path) -> ReloadKind {
    if path.parent() == Some(mod_root) {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if stem == "start-script" || ext == "ts" {
            return ReloadKind::ModInit;
        }
    }
    ReloadKind::Scripts
}

/// Handle one changed path. Decides by extension whether to compile or
/// forward, enqueues a `ReloadRequest` on success, logs on failure.
fn handle_path(
    path: &Path,
    reload_tx: &Sender<ReloadRequest>,
    ts_compiler: Option<&TsCompilerPath>,
    mod_root: &Path,
) {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let kind = classify_reload(path, mod_root);

    match ext {
        "luau" => {
            // Straight-through: Luau reads source directly.
            let _ = reload_tx.send(ReloadRequest { kind });
        }
        "js" => {
            // A `.js` change at the mod root (typically `start-script.js`,
            // possibly hand-shipped or freshly emitted by `compile_start_script_if_stale`)
            // should still trigger mod-init re-run. Definition `.js` artifacts
            // under `scripts/` are emitted next to their `.ts` source by the
            // TS compile path below; observing them in isolation would
            // double-fire, so we only react to mod-root `.js` files.
            if kind == ReloadKind::ModInit {
                let _ = reload_tx.send(ReloadRequest { kind });
            }
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
            match run_ts_compiler(compiler, path, &out_path) {
                Ok(()) => {
                    let _ = reload_tx.send(ReloadRequest { kind });
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
            // Not a definition script — ignore.
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
) -> Result<(), String> {
    use std::process::Command;

    let mut cmd = match compiler {
        TsCompilerPath::ScriptsBuildNextToEngine(p) | TsCompilerPath::ScriptsBuildOnPath(p) => {
            let mut c = Command::new(p);
            c.arg("--in").arg(input).arg("--out").arg(output);
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

    /// Like [`wait_for_reload`] but returns the kind of the first request.
    fn wait_for_reload_kind(watcher: &ScriptWatcher, deadline: Duration) -> Option<ReloadKind> {
        let start = Instant::now();
        while start.elapsed() < deadline {
            if let Ok(req) = watcher.reload_rx.try_recv() {
                return Some(req.kind);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    #[test]
    fn classify_reload_mod_root_start_script_is_mod_init() {
        let mod_root = PathBuf::from("/tmp/fake-mod");
        // start-script.{ts,js,luau} at the mod root → ModInit.
        for name in ["start-script.ts", "start-script.js", "start-script.luau"] {
            let p = mod_root.join(name);
            assert_eq!(
                classify_reload(&p, &mod_root),
                ReloadKind::ModInit,
                "{name} at mod root should classify as ModInit"
            );
        }
        // Any `.ts` at the mod root is a likely start-script import → ModInit.
        let import = mod_root.join("helpers.ts");
        assert_eq!(classify_reload(&import, &mod_root), ReloadKind::ModInit);
    }

    #[test]
    fn classify_reload_scripts_subdir_is_scripts() {
        let mod_root = PathBuf::from("/tmp/fake-mod");
        let p = mod_root.join("scripts").join("archetypes.ts");
        assert_eq!(classify_reload(&p, &mod_root), ReloadKind::Scripts);
        let p = mod_root.join("scripts").join("nested").join("a.luau");
        assert_eq!(classify_reload(&p, &mod_root), ReloadKind::Scripts);
    }

    #[test]
    fn start_script_luau_edit_at_mod_root_triggers_mod_init_reload() {
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

        let kind = wait_for_reload_kind(&watcher, Duration::from_secs(2));
        assert_eq!(
            kind,
            Some(ReloadKind::ModInit),
            "editing start-script.luau at the mod root should produce a ModInit reload",
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
        // platforms). `notify-debouncer-full` is expected to surface this as
        // an event the compile-worker treats as a modify.
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
        // above this crate (`crates/postretro/..`), but be tolerant of layout
        // changes by searching upward.
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
