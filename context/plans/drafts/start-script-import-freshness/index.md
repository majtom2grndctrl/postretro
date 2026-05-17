# Start-script import freshness

## Goal

`start-script.js` must be rebuilt when any file it transitively imports changes.
The current mtime check compares only `start-script.ts` vs `start-script.js` —
editing a bundled import (e.g. `scripts/player.ts`) leaves the stale bundle running.

Fix: in debug builds, always rebuild `start-script.js` before evaluating it.
`swc_bundler` already re-bundles from scratch on every invocation; removing the
mtime gate makes the freshness model accurate without adding parsing complexity.

## Scope

### In scope

- Remove the mtime gate on the mod-root bundle entry in debug builds.
  `run_mod_init` always invokes `scripts-build` before evaluating
  `start-script.js` when `start-script.ts` is present.
- Apply the same policy in the startup scan: mod-root bundle entry is always
  rebuilt, not conditionally.
- Nested `.ts` files under `script_root` keep their per-file mtime check —
  those are individual compilation targets, not bundle entries.

### Out of scope

- Release builds: unchanged. Pre-compiled `start-script.js` is required;
  no `scripts-build` invocation.
- Transitive import graph parsing in the engine.
- Bundler-level incremental compilation.
- Extending this policy to `.luau` `require` graphs.

## Acceptance criteria

- [ ] Editing any `.ts` file that `start-script.ts` imports (directly or
      transitively) causes the next `run_mod_init` in a debug build to
      recompile `start-script.js`. Observable: `[Scripting]` compile log line
      appears and `start-script.js` has an updated mtime.
- [ ] `run_mod_init` rebuilds the bundle even when `start-script.ts` and all
      its imports are unchanged. Correctness over rebuild-skip — this is the
      intended tradeoff at current mod scale.
- [ ] `start-script.ts` present and `scripts-build` missing: `run_mod_init` returns
      `ScriptError::InvalidArgument`. Behavior change from today — a fresh `.js`
      previously masked the missing compiler.
- [ ] The startup scan rebuilds the mod-root bundle entry unconditionally.
      A single `compile_stale_scripts` call leaves no stale `start-script.js`
      behind.
- [ ] Nested `.ts` files under `script_root` still use the per-file mtime
      check. No change to their compilation behavior.
- [ ] Release builds (`cfg(not(debug_assertions))`) are unchanged.

## Tasks

### Task 1: Remove the mtime gate from `compile_start_script_if_stale`

The function currently compares `js_mtime <= ts_mtime` and returns early if
the `.js` is fresh. Remove that check — always invoke `scripts-build` when
`start-script.ts` is present. Rename the function to `compile_start_script` (it is no longer a staleness check).

### Task 2: Apply the same policy in the startup scan

`visit_ts_files_shallow` calls `compile_one_if_stale` for each `.ts` it finds
in the mod root. For the mod-root bundle entry, drop the mtime guard and always
compile. Files under `script_root` (the recursive walk) are unchanged.

### Task 3: Update tests

- `visit_ts_files_shallow_skips_nested_directories` exercises the shallow path.
  Audit which assertions become wrong under always-rebuild and update them.
  (`compile_stale_scripts_skips_fresh_ts_files` calls the deep walk — unchanged.)
- Integration test: write `start-script.ts` importing `./helper.ts`, build the
  bundle, modify only `helper.ts`, call `run_mod_init`, assert `start-script.js`
  was rewritten.
- Integration test: `scripts-build` absent → error surfaces through
  `run_mod_init` as `ScriptError::InvalidArgument`.

## Sequencing

Task 1 and Task 2 in parallel → Task 3.

Cold start: `compile_stale_scripts` (startup scan) and the subsequent
`run_mod_init` both rebuild `start-script.js` unconditionally — two builds on
first launch. Accepted at current mod scale.

## Future path

When mod scripts grow large enough that always-rebuilding causes noticeable
latency, switch to a deps manifest: `scripts-build` writes `start-script.js.deps`
alongside the `.js` on every successful build (one resolved input path per line).
The freshness check reads the manifest, stats each listed file, and rebuilds only
when any input is newer than the `.js`. Same staleness contract; exact import
graph from the bundler itself; no in-engine parsing.
