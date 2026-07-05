# Scripting-Core Extraction Baseline

Captured: 2026-06-28 23:22 PDT.

## Source Check

Existing committed baseline checked:
`context/plans/done/engine-data-floor/baseline.md`.

It is post-floor, but not sufficient for this task. It records a
`scripting/registry.rs` touch rebuild, not the required primitive-handler
touch of `scripting/primitives/light.rs`.

## Context

Workspace: `/Users/dhiester/Projects/Personal/postretro`

Git:
- Branch: `main`
- HEAD: `cd191258`
- Existing unrelated local changes before capture:
  `content/dev/maps/campaign-test.map`, untracked `state.json`

Toolchain:
- `rustc 1.96.0 (ac68faa20 2026-05-25)`
- `cargo 1.96.0 (30a34c682 2026-05-25)`
- Host: `Darwin Dans-MacBook-Pro.local 25.5.0 ... RELEASE_X86_64 x86_64`

No clean build was forced. Treat this as a warm edit-loop baseline, not a
cold sys-crate baseline.

## Commands

```bash
/usr/bin/time -p cargo check -p postretro
/usr/bin/time -p cargo check -p postretro
touch crates/postretro/src/scripting/primitives/light.rs
/usr/bin/time -p cargo check -p postretro
/usr/bin/time -p cargo build -p postretro --timings
```

The primitive-handler touch was run as a separate `touch` followed by the
timed check. The recorded wall-clock is the rebuild check time.

No full workspace tests were run.

## Results

| Step | Cargo result | `/usr/bin/time` |
|------|--------------|-----------------|
| Warm-up `cargo check -p postretro` | `Finished ... in 6.58s` | `real 6.72`, `user 1.81`, `sys 1.06` |
| Warm `cargo check -p postretro` | `Finished ... in 0.46s` | `real 0.54`, `user 0.32`, `sys 0.20` |
| Light primitive touch rebuild | `Finished ... in 2.57s` | `real 2.65`, `user 2.03`, `sys 0.81` |
| `cargo build -p postretro --timings` | `Finished ... in 39.06s` | `real 39.18`, `user 235.91`, `sys 17.42` |

Timing report:

```text
target/cargo-timings/cargo-timing-20260629T062058175Z-892a9f914c1db502.html
```

## Timings Notes

The `--timings` report had 493 fresh units and 2 dirty units.

Active timing rows:
- `postretro v0.1.0 postretro "bin"`: `38.3s`
- `postretro v0.1.0 gen-script-types "bin"`: `5.6s`

Unit data:
- `postretro` binary started at `0.78s`, ran `38.29s`, and ended at
  about `39.07s`.
- `gen-script-types` started at `0.78s`, ran `5.6s`, and ended at about
  `6.38s`.

Critical-path interpretation:
- The active critical path in this warm incremental report is the
  `postretro` binary self-compile.
- `gen-script-types` ran in parallel and finished before the binary.
- `rquickjs-sys`, `mlua-sys`, and `luau0-src` were present in the graph
  but already fresh. They were not active critical-path work in this run.

VM sys crate rows in the report:
- `luau0-src v0.18.3+luau709`: row 187, `0.0s`, start `0.0`,
  duration `0.0`
- `mlua-sys v0.10.0`: rows 198-200, `0.0s`, start `0.0`,
  duration `0.0`
- `rquickjs-sys v0.11.0`: rows 312-314, `0.0s`, start `0.0`,
  duration `0.0`

Use this baseline for scripting-core extraction before/after claims:
warm no-op check `0.54s`, light primitive touch rebuild `2.65s`, and
timings-build active `postretro` binary compile `38.3s`.

---

## Post-Extraction Observation

Captured: 2026-06-29 03:43 PDT.

Context:
- The earlier review-fix pass ran `cargo clean` to recover local disk space, so
  the first post-extraction check was a cold cache warm-up and is not used for
  the comparison.
- The relocated handler touched for the firewall gate was
  `crates/postretro/src/lighting/script_primitives.rs`.

Commands:

```bash
/usr/bin/time -p cargo check -p postretro
touch crates/postretro/src/lighting/script_primitives.rs
/usr/bin/time -p cargo check -p postretro
/usr/bin/time -p cargo build -p postretro --timings
```

Results:

| Step | Cargo result | `/usr/bin/time` |
|------|--------------|-----------------|
| Warm `cargo check -p postretro` | `Finished ... in 0.44s` | `real 0.52`, `user 0.32`, `sys 0.19` |
| Relocated lighting primitive touch rebuild | `Checking postretro v0.1.0 ...`; `Finished ... in 1.69s` | `real 1.78`, `user 1.38`, `sys 0.49` |
| `cargo build -p postretro --timings` | `Compiling postretro v0.1.0 ...`; `Finished ... in 25.45s` | `real 25.58`, `user 165.67`, `sys 12.67` |

Timing report:

```text
target/cargo-timings/cargo-timing-20260629T104323743Z-892a9f914c1db502.html
```

Firewall observation:
- Touching the relocated lighting primitive rebuilt only `postretro`.
- `postretro-scripting-core` did not rebuild.
- The VM sys crates (`rquickjs-sys`, `mlua-sys`, `luau0-src`) did not rebuild.
- `glyphon`, `wgpu`, `kira`, and `winit` did not rebuild in the touched-handler
  check.

Before/after:
- Warm no-op check: `0.54s` before, `0.52s` after.
- Light primitive edit-loop check: `2.65s` before, `1.78s` after.
- Timings build: `39.18s` before, `25.58s` after.
