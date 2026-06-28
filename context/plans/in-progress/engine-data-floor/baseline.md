# Engine-Data Floor Baseline

Captured: 2026-06-28 16:22 PDT.

## Context

Workspace: `/Users/dhiester/Projects/Personal/postretro`

Git:
- Branch: `main`
- HEAD: `afb791b3`
- Existing unrelated local changes before capture: `content/dev/maps/campaign-test.map`, untracked `state.json`

Toolchain:
- `rustc 1.96.0 (ac68faa20 2026-05-25)`
- `cargo 1.96.0 (30a34c682 2026-05-25)`
- Host: `Darwin Dans-MacBook-Pro.local 25.5.0 ... RELEASE_X86_64 x86_64`

No clean build was forced. Do not treat these numbers as a cold sys-crate baseline.

## Commands

```bash
/usr/bin/time -p cargo check -p postretro
touch crates/postretro/src/scripting/registry.rs
/usr/bin/time -p cargo check -p postretro
/usr/bin/time -p cargo build -p postretro --timings
```

No full workspace tests were run.

## Results

| Step | Cargo result | `/usr/bin/time` |
|------|--------------|-----------------|
| Warm `cargo check -p postretro` | `Finished ... in 3.77s` | `real 3.87`, `user 3.80`, `sys 1.04` |
| Touch rebuild after `registry.rs` timestamp update | `Finished ... in 3.18s` | `real 3.27`, `user 3.72`, `sys 0.88` |
| `cargo build -p postretro --timings` | `Finished ... in 6.77s` | `real 6.88`, `user 7.14`, `sys 3.41` |

Timing report:

```text
target/cargo-timings/cargo-timing-20260628T232229473Z-892a9f914c1db502.html
```

## Timings Notes

The `--timings` run was incremental after the registry touch. Only `postretro` units had nonzero duration.

Active timing rows:
- `postretro v0.1.0 postretro "bin"`: `6.0s`
- `postretro v0.1.0 gen-script-types "bin"`: `2.6s`

Unit data:
- `postretro` binary started at `0.79s`, ran `5.99s`, and ended at about `6.78s`.
- `gen-script-types` started at `0.79s`, ran `2.65s`, and ended at about `3.44s`.

Critical-path interpretation:
- The active critical path in this warm incremental report is the `postretro` binary self-compile.
- `gen-script-types` ran in parallel and finished before the binary.
- `rquickjs-sys`, `mlua-sys`, and `luau0-src` were present in the graph but already fresh. They were not active critical-path work in this run.

VM sys crate rows in the report:
- `luau0-src v0.18.3+luau709`: `0.0s`
- `mlua-sys v0.10.0`: `0.0s`
- `mlua-sys v0.10.0 build-script`: `0.0s`
- `mlua-sys v0.10.0 build-script (run)`: `0.0s`
- `rquickjs-sys v0.11.0`: `0.0s`
- `rquickjs-sys v0.11.0 build-script`: `0.0s`
- `rquickjs-sys v0.11.0 build-script (run)`: `0.0s`

Use this baseline for warm edit-loop comparisons. Later extraction tasks should quote their touch rebuilds against the `3.18s` registry-touch check and their `--timings` report against the `6.0s` active `postretro` binary compile.
