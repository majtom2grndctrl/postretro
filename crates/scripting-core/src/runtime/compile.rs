// Startup TypeScript compilation scan (debug builds) and the extension-keyed
// `run_script_file` dispatch.
// See: context/lib/scripting.md §8 (Compilation Tooling)

use std::path::Path;

use crate::error::ScriptError;
use crate::quickjs::run_script;

use super::types::{ScriptRuntime, Which};

impl ScriptRuntime {
    /// Read `path` from disk and run it in the appropriate subsystem, chosen
    /// by extension:
    ///
    ///   * `.ts`, `.js`  → QuickJS
    ///   * `.luau`       → Luau
    ///
    /// `.ts` is accepted as a convenience for upstream layers that strip types
    /// before passing the file in; QuickJS parses it as plain JS. Unknown
    /// extensions return `ScriptError::InvalidArgument`.
    pub fn run_script_file(&self, which: Which, path: &Path) -> Result<(), ScriptError> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let source = std::fs::read_to_string(path).map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to read script `{}`: {e}", path.display()),
        })?;
        let name = path.to_string_lossy().into_owned();

        match ext {
            "ts" | "js" => {
                let ctx = match which {
                    Which::Definition => self.quickjs.definition_ctx(),
                };
                ctx.with(|ctx| run_script::<()>(&ctx, &source, &name))?;
                Ok(())
            }
            "luau" => {
                self.luau.run_source::<()>(which.into(), &source, &name)?;
                Ok(())
            }
            other => Err(ScriptError::InvalidArgument {
                reason: format!(
                    "unsupported script extension `.{other}` for `{}` (expected .ts/.js/.luau)",
                    path.display(),
                ),
            }),
        }
    }
}

/// Always rebuild `start-script.js` from `start-script.ts` in debug builds.
///
/// The mtime gate was removed because `start-script.ts` is a bundle entry, not
/// a single-file compile: `swc_bundler` traces its imports and re-bundles every
/// invocation. A `js_mtime > ts_mtime` check missed the case where an imported
/// helper changed without touching `start-script.ts`, leaving a stale bundle on
/// disk. Correctness over rebuild-skip — acceptable at current mod scale.
/// Per-file mtime gating still applies to nested scripts under `script_root`
/// (see `compile_one_if_stale`).
#[cfg(debug_assertions)]
pub(super) fn compile_start_script(ts_path: &Path, js_path: &Path) -> Result<(), String> {
    let compiler = crate::watcher::TsCompilerPath::detect().ok_or_else(|| {
        "scripts-build not found — run via `cargo run -p xtask -- run ...`, install it on PATH, or ship it next to the engine binary"
            .to_string()
    })?;
    compiler.warn_if_stale();
    crate::watcher::run_ts_compiler(&compiler, ts_path, js_path)
}

/// In debug builds: walk `script_root` recursively and recompile any `.ts`
/// file whose sibling `.js` is missing or older than the `.ts`. Detects the
/// compiler once up front; logs a warning and returns early if not found.
/// Per-file compile failures are logged as warnings; the scan continues so one
/// broken file does not block the rest.
#[cfg(debug_assertions)]
pub(super) fn scan_and_compile_stale_ts(script_root: &Path, mod_root: &Path) {
    let script_root_present = script_root.is_dir();
    let mod_root_present = mod_root.is_dir() && mod_root != script_root;
    if !script_root_present && !mod_root_present {
        return;
    }

    let compiler = match crate::watcher::TsCompilerPath::detect() {
        Some(c) => c,
        None => {
            log::warn!(
                "[Scripting] startup TS scan: `scripts-build` not found — \
                 stale `.ts` files will not be recompiled. \
                 Run via `cargo run -p xtask -- run ...`, install `scripts-build` \
                 on PATH, or place it next to the engine binary.",
            );
            return;
        }
    };
    compiler.warn_if_stale();

    let mut compiled = 0u32;
    let mut failed = 0u32;
    if script_root_present {
        visit_ts_files(script_root, &compiler, &mut compiled, &mut failed);
    }
    // mod_root walked one level only — nested helpers belong under scripts/.
    if mod_root_present {
        visit_ts_files_shallow(mod_root, &compiler, &mut compiled, &mut failed);
    }

    if compiled > 0 || failed > 0 {
        log::info!("[Scripting] startup TS scan: {compiled} recompiled, {failed} failed");
    }
}

/// Non-recursive variant of `visit_ts_files` for the mod-root scope.
/// Subdirectories are not descended — they are the watcher's `script_root`
/// territory and are handled by the recursive walk.
#[cfg(debug_assertions)]
pub(super) fn visit_ts_files_shallow(
    dir: &Path,
    compiler: &crate::watcher::TsCompilerPath,
    compiled: &mut u32,
    failed: &mut u32,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            log::warn!(
                "[Scripting] startup TS scan: cannot read directory `{}`: {err}",
                dir.display(),
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !file_type.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("ts") {
            continue;
        }
        // Top-level mod-root `.ts` files are bundle components — `swc_bundler`
        // re-bundles them from scratch on every invocation, so an mtime gate
        // here would only mask import-graph changes. Always rebuild; see
        // `compile_start_script` for the matching rationale on the entry path.
        compile_one_unconditional(&path, compiler, compiled, failed);
    }
}

/// Recursively walk `dir`, compiling stale `.ts` files. Subdirectory traversal
/// errors (e.g. permission denied) are logged and skipped.
#[cfg(debug_assertions)]
pub(super) fn visit_ts_files(
    dir: &Path,
    compiler: &crate::watcher::TsCompilerPath,
    compiled: &mut u32,
    failed: &mut u32,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            log::warn!(
                "[Scripting] startup TS scan: cannot read directory `{}`: {err}",
                dir.display(),
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if file_type.is_dir() {
            visit_ts_files(&path, compiler, compiled, failed);
            continue;
        }

        if path.extension().and_then(|s| s.to_str()) != Some("ts") {
            continue;
        }
        compile_one_if_stale(&path, compiler, compiled, failed);
    }
}

/// Compile a single `.ts` file unconditionally — used for top-level mod-root
/// bundle components. The bundler re-runs from scratch every time, so any
/// per-file mtime gate would only hide import-graph changes.
#[cfg(debug_assertions)]
fn compile_one_unconditional(
    path: &Path,
    compiler: &crate::watcher::TsCompilerPath,
    compiled: &mut u32,
    failed: &mut u32,
) {
    let js_path = crate::watcher::compiled_output_for(path);
    match crate::watcher::run_ts_compiler(compiler, path, &js_path) {
        Ok(()) => {
            log::debug!("[Scripting] startup TS scan: compiled `{}`", path.display(),);
            *compiled += 1;
        }
        Err(msg) => {
            log::warn!(
                "[Scripting] startup TS scan: compile failed for `{}`: {msg}",
                path.display(),
            );
            *failed += 1;
        }
    }
}

/// Compile a single `.ts` file when its sibling `.js` is missing or older.
/// Used by the recursive `script_root` walk where each `.ts` is an individual
/// compilation target (not a bundle component), so mtime gating is meaningful.
#[cfg(debug_assertions)]
fn compile_one_if_stale(
    path: &Path,
    compiler: &crate::watcher::TsCompilerPath,
    compiled: &mut u32,
    failed: &mut u32,
) {
    let js_path = crate::watcher::compiled_output_for(path);
    let ts_mtime = match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(err) => {
            log::warn!(
                "[Scripting] startup TS scan: stat `{}`: {err}",
                path.display(),
            );
            return;
        }
    };
    let needs_build = match std::fs::metadata(&js_path).and_then(|m| m.modified()) {
        Ok(js_mtime) => js_mtime <= ts_mtime,
        Err(_) => true,
    };
    if !needs_build {
        return;
    }
    match crate::watcher::run_ts_compiler(compiler, path, &js_path) {
        Ok(()) => {
            log::debug!("[Scripting] startup TS scan: compiled `{}`", path.display(),);
            *compiled += 1;
        }
        Err(msg) => {
            log::warn!(
                "[Scripting] startup TS scan: compile failed for `{}`: {msg}",
                path.display(),
            );
            *failed += 1;
        }
    }
}
