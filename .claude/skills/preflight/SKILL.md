---
name: preflight
description: >
  Runs pre-commit quality checks: cargo fmt, clippy, tests, a release-profile
  compile check, and the crate-graph snapshot. Reports pass/fail status for each
  check. Use before committing or pushing changes, or before opening a pull
  request.
disable-model-invocation: true
---

# Preflight

Run quality checks and report results. Fix mechanical issues automatically; escalate design decisions.

## Checks

Run these **sequentially**, not in parallel. They share one `target/` dir and
a single build lock, so parallel runs contend on the lock and thrash each
other's fingerprints — they serialize anyway, just less predictably and with
more cache churn.

1. **Format:** `cargo fmt --check` (no build)
2. **Lint:** `cargo clippy --target-dir target/preflight-clippy -- -D warnings`
3. **Test:** `cargo test`
4. **Release build:** `cargo check --release` — catches what a debug-only gate
   cannot see. Code inside `#[cfg(debug_assertions)]`
   items vanishes in release while its callers do not, so a reference to a
   debug-gated helper still name-resolves at debug and fails to compile at
   release (`development_guide.md` §3.6). Every other check here builds debug,
   so without this the whole class is invisible until someone builds a release
   binary.
5. **Crate graph:** `cargo run -p xtask -- crate-graph --check` — fails if the
   committed `context/lib/crate-graph.md` snapshot is stale after an internal
   dependency change.

Clippy gets its **own target dir** on purpose. It compiles the whole workspace
under the clippy driver, which writes different fingerprints than the `rustc`
builds behind `cargo run` / `cargo test`. Sharing `target/` means every
preflight invalidates the warm dev cache (and the next `cargo run` invalidates
clippy's). Isolating it trades a little disk for a second build tree in exchange
for keeping your day-to-day cache hot — worth it on a machine that builds many
times a day. This keeps full coverage: fmt, `clippy -D warnings`, and the full
`cargo test` suite all still run.

The release check needs no such isolation: cargo already separates profiles into
`target/debug` and `target/release`, so it cannot invalidate the debug cache.
It is `cargo check`, not `cargo build` — the gate is "does it compile in
release", and codegen would cost minutes without catching anything a check
misses. Its first run is still a cold build of the workspace's dependencies
under the release profile, so budget for it once; `target/release` and
`target/preflight-clippy` are the first things to delete when disk runs short.

## Reporting

Report each check as ✓ pass or ✗ fail. For failures, include the relevant output.

```
Preflight results:
  ✓ cargo fmt
  ✗ cargo clippy — 2 warnings (see below)
  ✓ cargo test (14 passed)
  ✓ cargo check --release
  ✓ crate-graph --check
```

## Auto-fix policy

- **Format failures:** Run `cargo fmt` to fix, then report what changed.
- **Clippy warnings:** Fix if mechanical (unused import, redundant clone, missing `&`). If the fix involves a design choice or changes behavior, report and let the user decide.
- **Test failures:** Never auto-fix. Report the failure with enough context to diagnose. A failing `layering_invariants_hold` test means a new crate edge broke the layering rules (`development_guide.md` §Workspace) — report it as a design issue, don't paper over the assertion.
- **Release-only compile errors:** Fix if mechanical — the usual cause is a
  `#[cfg(debug_assertions)]` helper referenced from an ungated call site, and the
  fix is to gate the statement too (`development_guide.md` §3.6). Report anything
  that needs a design call instead of guessing.
- **Stale crate graph:** Run `cargo run -p xtask -- crate-graph --write` to regenerate `context/lib/crate-graph.md`, then report the edge change it captured.

After auto-fixing, re-run the fixed checks to confirm they pass.
