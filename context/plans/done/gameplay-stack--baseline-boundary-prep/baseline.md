# Gameplay-Stack Compile-Time Baseline

Captured on 2026-09-13 from the pre-move tree at commit
`c27a8657b0d184e8d1d64674d2aec2f41c5c72e5` (`Move
gameplay-stack--baseline-boundary-prep to in-progress`). This is the M1 reference
for later gameplay-stack moves; no engine source content changed for these runs.

## Host And Toolchain

- `cargo -V`: `cargo 1.98.0 (797e8a9bc 2026-08-05) (Homebrew)`
- `rustc -vV`:
  - `rustc 1.98.0 (88d9e12ae 2026-08-18) (Homebrew)`
  - host/target: `x86_64-apple-darwin`
  - LLVM: `22.1.8`
- Host OS: macOS `26.6.2` (build `25G83`); Darwin `25.6.0` on x86_64.
- Build environment: `RUST_LOG=warn`; no `CARGO_*`, `RUSTFLAGS`, or
  `RUSTC_WRAPPER` variables were set.
- `cargo metadata --locked --format-version 1`: captured at
  `/private/tmp/postretro-gameplay-stack-baseline-task1-metadata.json`; SHA-256
  `c80fbb487e55c317ee401a838d4828c91ff9534109bbdde88a69b3c9570c3536`,
  3,027,821 bytes, 644 packages, and 18 workspace members.

## Method

Every case used its own `--target-dir` below
`/private/tmp/postretro-gameplay-stack-baseline-task1/`. The workspace case was a
clean build. Each warm case first ran a complete `cargo check -p postretro` in its
own target directory; that prime time is recorded for audit but discarded from the
result. The touch cases then used `touch` only, changing file mtime with no content
edit, followed by their rebuild. The three discarded touch-case primes ran together
within the three-build concurrency cap; the recorded touch rebuilds ran one at a
time after all primes completed.

The committed E19 config is already present at `.cargo/config.toml` (commit
`5e5a58cf7`): it is comment-only, leaves default Cargo behavior unchanged, and
documents opt-in `dev-fast`/linker settings. No duplicate config was added.

## Case Matrix

| Case | Command | Method | Wall time |
| --- | --- | --- | ---: |
| Workspace check | `cargo check --workspace --target-dir /private/tmp/postretro-gameplay-stack-baseline-task1/workspace-check-final` | Clean isolated build | 343.08s |
| Warm no-op | `cargo check -p postretro --target-dir /private/tmp/postretro-gameplay-stack-baseline-task1/warm-noop` | Prime discarded (376.69s), then no-op | 1.30s |
| Touch AI churn locus | `touch crates/postretro/src/scripting/systems/ai/targeting.rs`; `cargo check -p postretro --target-dir /private/tmp/postretro-gameplay-stack-baseline-task1/touch-targeting` | Prime discarded (788.06s), then mtime-only rebuild | 3.38s |
| Touch netcode | `touch crates/postretro/src/netcode/endpoint.rs`; `cargo check -p postretro --target-dir /private/tmp/postretro-gameplay-stack-baseline-task1/touch-netcode-endpoint` | Prime discarded (783.04s), then mtime-only rebuild | 2.89s |
| Touch binary-only startup | `touch crates/postretro/src/startup/lifecycle.rs`; `cargo check -p postretro --target-dir /private/tmp/postretro-gameplay-stack-baseline-task1/touch-startup-lifecycle` | Prime discarded (786.62s), then mtime-only rebuild | 2.87s |

All recorded commands exited with status 0. The concurrently primed cases' wall
times are audit-only and not comparable measurements; only the uncontended recorded
rebuild wall times above are the baseline.
