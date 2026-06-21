// Startup TS-compile scan and bundle-rebuild-on-import-change behavior.

use super::*;

// --- compile_stale_scripts tests -----------------------------------------

#[test]
fn compile_stale_scripts_is_noop_for_nonexistent_directory() {
    // Passing a directory that does not exist must not panic or error.
    // No compiler is invoked because `scan_and_compile_stale_ts` returns
    // early when the path is not a directory.
    let (rt, _ctx) = runtime();
    let absent = std::env::temp_dir().join("postretro_scan_absent_dir_test");
    assert!(!absent.exists(), "test setup: dir must not pre-exist");
    // Should silently no-op.
    rt.compile_stale_scripts(&absent, &absent);
}

#[test]
fn compile_stale_scripts_is_noop_when_no_ts_files_present() {
    // `scripts/` directory exists but contains only `.luau` files. The
    // scan walks the directory and finds nothing to compile.
    let (rt, _ctx) = runtime();
    let dir = temp_mod_root("scan_no_ts");
    std::fs::write(dir.join("archetypes.luau"), "-- luau only\n").unwrap();
    // Must complete without panic; no compiler binary needed.
    rt.compile_stale_scripts(&dir, &dir);
}

#[test]
#[cfg(debug_assertions)]
fn compile_stale_scripts_recompiles_ts_with_stale_js_sibling() {
    // Acceptance criterion: a `.ts` file whose sibling `.js` is older than
    // the `.ts` (or absent) gets recompiled by the startup scan.
    use std::time::{Duration, SystemTime};

    let compiler_path = ensure_scripts_build();
    let (_rt, _ctx) = runtime();
    let dir = temp_mod_root("scan_stale_ts");

    let ts_path = dir.join("archetypes.ts");
    let js_path = dir.join("archetypes.js");

    fs::write(&ts_path, "export const x: number = 1;\n").unwrap();

    // Write a JS sibling backdated by 5 seconds so it is definitely older
    // than the TS. Use `set_modified` (std 1.75+) if available; fall back
    // to simply not writing the JS at all (trigger the "missing sibling"
    // code path instead).
    fs::write(&js_path, "// stale\n").unwrap();
    let stale_time = SystemTime::now()
        .checked_sub(Duration::from_secs(5))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    if let Ok(file) = std::fs::File::options().write(true).open(&js_path) {
        // `set_modified` is gated on the platform supporting it; ignore
        // failures gracefully — the missing-sibling path is exercised
        // instead if the mtime cannot be set.
        let _ = file.set_modified(stale_time);
        drop(file);
    }

    // Override PATH so `TsCompilerPath::detect()` finds our binary.
    // `set_var` is only safe in single-threaded contexts; cargo test runs
    // each `#[test]` on its own thread but a cargo test binary runs all
    // threads in the same process, so we use the direct-call variant of
    // the private helper instead of mutating the process environment.
    // Instead, we invoke `scan_and_compile_stale_ts` with a synthesized
    // compiler path via the `watcher` module's public API directly.
    let _ = compiler_path; // compiler path used below via watcher API
    // Since `compile_stale_scripts` relies on `TsCompilerPath::detect()`,
    // which reads `current_exe`, we cannot inject an arbitrary path. But
    // we can test the helper `visit_ts_files` directly, which is what
    // `scan_and_compile_stale_ts` delegates to.
    let mut compiled = 0u32;
    let mut failed = 0u32;
    let compiler =
        crate::scripting::watcher::TsCompilerPath::ScriptsBuildOnPath(ensure_scripts_build());
    super::super::compile::visit_ts_files(&dir, &compiler, &mut compiled, &mut failed);

    assert_eq!(failed, 0, "no compile failures expected; failed={failed}",);
    assert_eq!(
        compiled, 1,
        "exactly one stale .ts file should have been compiled",
    );
    assert!(
        js_path.is_file(),
        "compiled output `{}` must exist after scan",
        js_path.display(),
    );
}

#[test]
#[cfg(debug_assertions)]
fn compile_stale_scripts_skips_fresh_ts_files() {
    // A `.ts` whose `.js` sibling is newer is skipped.
    let dir = temp_mod_root("scan_fresh_ts");
    let ts_path = dir.join("archetypes.ts");
    let js_path = dir.join("archetypes.js");

    // Write the JS first so it has an older mtime, then write the TS so
    // it ends up newer. Because filesystem mtime granularity may be 1s on
    // some platforms, we forcibly set the JS mtime to the future.
    fs::write(&js_path, "// fresh\n").unwrap();
    fs::write(&ts_path, "export const x: number = 1;\n").unwrap();

    // Backdate the TS by 5 seconds to make the JS appear newer.
    let old_time = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(5))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    if let Ok(f) = std::fs::File::options().write(true).open(&ts_path) {
        let _ = f.set_modified(old_time);
    }

    let mut compiled = 0u32;
    let mut failed = 0u32;
    // `ensure_scripts_build` not needed — the TS is fresh so no compile runs.
    // We still need a valid `TsCompilerPath` to pass to `visit_ts_files`.
    // Use a dummy path — it will never be invoked.
    let compiler = crate::scripting::watcher::TsCompilerPath::ScriptsBuildOnPath(
        std::path::PathBuf::from("/dev/null/scripts-build-dummy"),
    );
    super::super::compile::visit_ts_files(&dir, &compiler, &mut compiled, &mut failed);

    assert_eq!(compiled, 0, "fresh .ts must not be recompiled");
    assert_eq!(failed, 0);
}

#[test]
#[cfg(debug_assertions)]
fn compile_stale_scripts_walks_subdirectories() {
    // A stale `.ts` nested inside a subdirectory must be found and
    // compiled.
    let dir = temp_mod_root("scan_nested_ts");
    let sub = dir.join("actors");
    fs::create_dir_all(&sub).unwrap();

    let ts_path = sub.join("player.ts");
    let js_path = sub.join("player.js");
    fs::write(&ts_path, "export const role: string = 'player';\n").unwrap();
    // No JS sibling → needs build.

    let mut compiled = 0u32;
    let mut failed = 0u32;
    let compiler =
        crate::scripting::watcher::TsCompilerPath::ScriptsBuildOnPath(ensure_scripts_build());
    super::super::compile::visit_ts_files(&dir, &compiler, &mut compiled, &mut failed);

    assert_eq!(failed, 0);
    assert_eq!(compiled, 1, "nested stale .ts should be compiled");
    assert!(
        js_path.is_file(),
        "compiled output `{}` must exist",
        js_path.display(),
    );
}

#[test]
#[cfg(debug_assertions)]
fn visit_ts_files_shallow_skips_nested_directories() {
    // Mod-root scope is one level only — nested `.ts` files are the
    // recursive `script_root` walk's territory.
    //
    // Regression: the shallow walk previously gated compilation on
    // `js_mtime <= ts_mtime`, so a fresh-looking `start-script.js`
    // (e.g. a stale bundle whose imports changed) would be left untouched
    // (import freshness). The shallow walk now always rebuilds top-level
    // bundle components, even when the sibling `.js` is newer than the `.ts`.
    use std::time::{Duration, SystemTime};

    let dir = temp_mod_root("scan_shallow");
    let ts_path = dir.join("start-script.ts");
    let js_path = dir.join("start-script.js");
    fs::write(&ts_path, "export {};\n").unwrap();

    // Plant a sibling `.js` with a mtime 60 seconds in the future so any
    // residual `js_mtime <= ts_mtime` gate would skip the rebuild.
    let stale_marker = "// stale bundle — should be overwritten\n";
    fs::write(&js_path, stale_marker).unwrap();
    let future = SystemTime::now() + Duration::from_secs(60);
    let mtime_bump_supported = std::fs::File::options()
        .write(true)
        .open(&js_path)
        .and_then(|f| f.set_modified(future))
        .is_ok();

    let nested = dir.join("scripts");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("nested.ts"), "export {};\n").unwrap();

    let mut compiled = 0u32;
    let mut failed = 0u32;
    let compiler =
        crate::scripting::watcher::TsCompilerPath::ScriptsBuildOnPath(ensure_scripts_build());
    super::super::compile::visit_ts_files_shallow(&dir, &compiler, &mut compiled, &mut failed);

    assert_eq!(failed, 0);
    assert_eq!(
        compiled, 1,
        "top-level start-script.ts must be rebuilt unconditionally; nested/nested.ts \
             is left for the recursive walk",
    );
    assert!(js_path.is_file());
    assert!(
        !nested.join("nested.js").is_file(),
        "shallow walk must not descend into subdirectories",
    );

    // The newer-than-the-ts `.js` must have been overwritten. We verify
    // through content (the stale marker comment is gone) rather than mtime
    // alone, since the rebuild output mtime depends on filesystem
    // granularity. Only enforce when we could actually bump the mtime —
    // otherwise the original `<=` gate wouldn't have skipped anyway.
    if mtime_bump_supported {
        let rebuilt = fs::read_to_string(&js_path).unwrap();
        assert!(
            !rebuilt.contains("stale bundle"),
            "shallow walk left a fresh-looking `.js` in place — mtime gate regression",
        );
    }
}

#[test]
#[cfg(debug_assertions)]
fn run_mod_init_rebuilds_bundle_when_import_changes() {
    // Regression: editing a helper imported by `start-script.ts` left a
    // stale `start-script.js` running (import freshness). The mtime gate
    // compared only the entry `.ts` vs the `.js`, so a newer helper was
    // invisible. `run_mod_init` must rebuild the bundle on every call.
    //
    // Skipped (test passes trivially) when the test process cannot make
    // `scripts-build` discoverable via `TsCompilerPath::detect()` —
    // detection reads `current_exe`'s parent and `PATH`, neither of which
    // is hermetically controllable from inside a Rust test. We place a
    // copy of the binary next to `current_exe` to satisfy the
    // next-to-engine arm of the cascade.
    use std::time::{Duration, SystemTime};

    if !install_scripts_build_next_to_current_exe() {
        eprintln!("skipping: could not install scripts-build next to test binary");
        return;
    }

    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("import_changes_rebuild");

    // `start-script.ts` imports a helper. `scripts-build` (swc_bundler)
    // inlines the import at compile time, so the bundled `.js` ends up
    // containing the helper's literal value.
    fs::write(
        dir.join("helper.ts"),
        "export const NAME: string = 'ImportFreshV1';\n",
    )
    .unwrap();
    fs::write(
        dir.join("start-script.ts"),
        "import { NAME } from './helper.ts';\n\
             export default { name: NAME };\n",
    )
    .unwrap();

    rt.run_mod_init(&dir).expect("first run_mod_init");
    let js_path = dir.join("start-script.js");
    assert!(js_path.is_file(), "first run must produce start-script.js");
    let first_content = fs::read_to_string(&js_path).unwrap();
    assert!(
        first_content.contains("ImportFreshV1"),
        "bundled JS must reflect the initial helper value; got: {first_content}",
    );
    let manifest = rt.mod_manifest().expect("manifest after first run");
    assert_eq!(manifest.name, "ImportFreshV1");

    // Change only the helper and force its mtime to *not* exceed the
    // existing `start-script.ts` mtime (it normally would; the point is
    // that even when only the helper changes, the bundle must rebuild).
    // We additionally bump the `.js` mtime well into the future so any
    // residual mtime gate against `start-script.ts` would skip the
    // rebuild — that would fail this regression test.
    fs::write(
        dir.join("helper.ts"),
        "export const NAME: string = 'ImportFreshV2';\n",
    )
    .unwrap();
    let future = SystemTime::now() + Duration::from_secs(60);
    let bumped = std::fs::File::options()
        .write(true)
        .open(&js_path)
        .and_then(|f| f.set_modified(future))
        .is_ok();

    rt.run_mod_init(&dir).expect("second run_mod_init");
    let second_content = fs::read_to_string(&js_path).unwrap();
    assert!(
        second_content.contains("ImportFreshV2"),
        "bundle must reflect the changed helper after second run_mod_init; got: {second_content}",
    );
    assert!(
        !second_content.contains("ImportFreshV1"),
        "stale bundle content must be gone after rebuild; got: {second_content}",
    );
    let manifest = rt.mod_manifest().expect("manifest after second run");
    assert_eq!(manifest.name, "ImportFreshV2");

    // Only enforce the mtime-moved-backwards check when we could prove the
    // setup bumped the `.js` mtime forward. Otherwise the assertion is
    // vacuous (the rebuild could legitimately produce the same mtime on
    // coarse-granularity filesystems).
    if bumped {
        let new_mtime = fs::metadata(&js_path)
            .and_then(|m| m.modified())
            .expect("mtime");
        assert!(
            new_mtime < future,
            "rebuild must overwrite the future-dated `.js` mtime",
        );
    }
}
