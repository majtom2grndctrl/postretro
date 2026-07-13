---
name: preflight
description: >
  Runs pre-commit quality checks: cargo fmt, clippy, and tests. Reports
  pass/fail status for each check. Use before committing or pushing changes,
  or before opening a pull request.
disable-model-invocation: true
---

# Preflight

Run quality checks and report results. Fix mechanical issues automatically; escalate design decisions.

## Checks

Run all four in parallel:

1. **Format:** `cargo fmt --check`
2. **Lint:** `cargo clippy -- -D warnings`
3. **Test:** `cargo test`
4. **Crate graph:** `cargo run -p xtask -- crate-graph --check` — fails if the
   committed `context/lib/crate-graph.md` snapshot is stale after an internal
   dependency change.

## Reporting

Report each check as ✓ pass or ✗ fail. For failures, include the relevant output.

```
Preflight results:
  ✓ cargo fmt
  ✗ cargo clippy — 2 warnings (see below)
  ✓ cargo test (14 passed)
  ✓ crate-graph --check
```

## Auto-fix policy

- **Format failures:** Run `cargo fmt` to fix, then report what changed.
- **Clippy warnings:** Fix if mechanical (unused import, redundant clone, missing `&`). If the fix involves a design choice or changes behavior, report and let the user decide.
- **Test failures:** Never auto-fix. Report the failure with enough context to diagnose. A failing `layering_invariants_hold` test means a new crate edge broke the layering rules (`development_guide.md` §Workspace) — report it as a design issue, don't paper over the assertion.
- **Stale crate graph:** Run `cargo run -p xtask -- crate-graph --write` to regenerate `context/lib/crate-graph.md`, then report the edge change it captured.

After auto-fixing, re-run the fixed checks to confirm they pass.
