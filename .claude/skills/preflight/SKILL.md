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

Run all three in parallel:

1. **Format:** `cargo fmt --check`
2. **Lint:** `cargo clippy -- -D warnings`
3. **Test:** `cargo test`

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
