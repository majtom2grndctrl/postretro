---
name: preflight
description: >
  Runs pre-commit quality checks: cargo fmt, clippy, and tests. Reports
  pass/fail status for each check. Use before committing or pushing changes,
  or before opening a pull request.
disable-model-invocation: true
---

# Preflight

## Model guidance

Read [project model guidance](../model-guidance.md) for selection, escalation, availability, and dispatch rules. GPT-6 Luna at low for check execution and reporting; medium for prescribed mechanical fixes. Use GPT-6.1 Sol at high for diagnosis that crosses files or contracts, subject to the auto-fix policy below.

Run quality checks and report results. Fix mechanical issues automatically; escalate design decisions.

## Disk space

After every numbered workflow step or task, check free space on the workspace filesystem. If less than 15 GB remains, clear Cargo incremental build caches in the active target directory and any workflow-owned target directories. Recheck. If space is still below 15 GB, clear Cargo's downloaded crate archive cache. Recheck before continuing. Delete only caches; keep full target directories, source checkouts, worktrees, and unrelated files.

## Checks

Run these **sequentially**, not in parallel. They share one `target/` dir and
a single build lock, so parallel runs contend on the lock and thrash each
other's fingerprints — they serialize anyway, just less predictably and with
more cache churn.

1. **Format:** `cargo fmt --check` (no build)
2. **Lint:** `cargo clippy --target-dir target/preflight-clippy -- -D warnings`
3. **Test:** `cargo test`

Clippy gets its **own target dir** on purpose. It compiles the whole workspace
under the clippy driver, which writes different fingerprints than the `rustc`
builds behind `cargo run` / `cargo test`. Sharing `target/` means every
preflight invalidates the warm dev cache (and the next `cargo run` invalidates
clippy's). Isolating it trades a little disk for a second build tree in exchange
for keeping your day-to-day cache hot — worth it on a machine that builds many
times a day. This keeps full coverage: fmt, `clippy -D warnings`, and the full
`cargo test` suite all still run.

## Reporting

Report each check as ✓ pass or ✗ fail. For failures, include the relevant output.

```
Preflight results:
  ✓ cargo fmt
  ✗ cargo clippy — 2 warnings (see below)
  ✓ cargo test (14 passed)
```

## Auto-fix policy

- **Format failures:** Run `cargo fmt` to fix, then report what changed.
- **Clippy warnings:** Fix if mechanical (unused import, redundant clone, missing `&`). If the fix involves a design choice or changes behavior, report and let the user decide.
- **Test failures:** Never auto-fix. Report the failure with enough context to diagnose.

After auto-fixing, re-run the fixed checks to confirm they pass.
