# E19 Compile-Time Baseline

Captured on 2026-06-30 from `/Users/dhiester/Projects/Personal/postretro` on branch `codex/E19-baseline-and-cargo-config`.

This is the canonical Task 1 baseline for `E19--baseline-and-cargo-config`. Measurements used stable Cargo with one isolated `--target-dir /tmp/postretro-compile-times/<case>` per case. Warm/no-op and touch-rebuild cases were primed with a full `cargo check -p postretro` into their own target dir; the prime wall time is listed for audit only and is not the measured result.

## Host And Toolchain

- `cargo -V`: `cargo 1.96.0 (30a34c682 2026-05-25)`
- `rustc -vV`:
  - `rustc 1.96.0 (ac68faa20 2026-05-25)`
  - host: `x86_64-apple-darwin`
  - LLVM: `22.1.2`
- Host OS:
  - `macOS 26.5.1`, build `25F80`
  - `Darwin Dans-MacBook-Pro.local 25.5.0 Darwin Kernel Version 25.5.0: Mon Apr 27 20:31:18 PDT 2026; root:xnu-12377.121.6~2/RELEASE_X86_64 x86_64`
- CPU/RAM:
  - `system_profiler SPHardwareDataType`: 2.4 GHz processor, 1 processor, 8 total cores, hyper-threading enabled, 32 GB memory.
  - Caveat: sandboxed `sysctl` could not read the CPU brand/core sysctls directly; `system_profiler` reported model/processor name as `Unknown`.
- Build environment from `env | grep -E '^(CARGO|RUST)'`:
  - `RUST_LOG=warn`
  - `RUSTFLAGS` not set.
  - `RUSTC_WRAPPER` not set.
  - No `CARGO_*` variables were set in the captured environment.
- `cargo metadata --locked --format-version 1`:
  - Captured at `/tmp/postretro-compile-times/cargo-metadata.json`.
  - SHA-256: `dd65679f60c6b29e5ae46f2451b48529f85d96ec60374f143446d0b268230aa5`.
  - Size: `2847906` bytes.
  - Packages: 613.
  - Workspace members: 8 (`postretro`, `postretro-entities`, `postretro-foundation`, `postretro-level-format`, `postretro-net`, `postretro-scripting-core`, `postretro-script-compiler`, `postretro-level-compiler`).

## Methodology Caveats

- The original runner completed the first five expensive cases, then exited before touch cases because its Bash associative-array syntax was not portable to macOS `/bin/bash` 3.x. Those completed status-0 artifacts were reused; only the missing touch cases were run afterward.
- Cargo `--timings` reports were parsed from the committed stable HTML report data (`UNIT_DATA`). Critical-path notes were reconstructed from Cargo's unblock graph. High-time ranks use the timing report's "Total" table because this stable report exposes total/frontend/codegen columns, not a separate self-time column.
- Cargo did not emit separate `link` sections in these timing reports. For `cargo build` and `cargo test --no-run`, final link work is therefore recorded as folded into the final `postretro` binary unit. For check-only cases there is no final link.
- Ignored level-compiler cold-bake integration tests were not run.

## Case Matrix

| Case | Command | Target dir | Method | Wall time | Timing report |
| --- | --- | --- | --- | ---: | --- |
| Clean check postretro | `cargo check --timings -p postretro --target-dir /tmp/postretro-compile-times/clean-check-postretro` | `/tmp/postretro-compile-times/clean-check-postretro` | Clean first build into fresh dir | 261.67s | `/tmp/postretro-compile-times/clean-check-postretro/cargo-timings/cargo-timing-20260630T132741685Z-892a9f914c1db502.html` |
| Warm no-op check postretro | `cargo check --timings -p postretro --target-dir /tmp/postretro-compile-times/warm-noop-check-postretro` | `/tmp/postretro-compile-times/warm-noop-check-postretro` | Primed full check first; prime discarded (`real 294.20s`); measured no-op | 1.04s | `/tmp/postretro-compile-times/warm-noop-check-postretro/cargo-timings/cargo-timing-20260630T133657601Z-892a9f914c1db502.html` |
| Workspace check | `cargo check --timings --workspace --target-dir /tmp/postretro-compile-times/workspace-check` | `/tmp/postretro-compile-times/workspace-check` | Clean first build into fresh dir | 329.92s | `/tmp/postretro-compile-times/workspace-check/cargo-timings/cargo-timing-20260630T133658674Z-892a9f914c1db502.html` |
| Build postretro bins | `cargo build --timings -p postretro --bins --target-dir /tmp/postretro-compile-times/build-postretro-bins` | `/tmp/postretro-compile-times/build-postretro-bins` | Clean first build into fresh dir; builds `postretro` plus `gen-script-types` | 452.29s | `/tmp/postretro-compile-times/build-postretro-bins/cargo-timings/cargo-timing-20260630T134228645Z-892a9f914c1db502.html` |
| Test postretro no-run | `cargo test --timings -p postretro --no-run --target-dir /tmp/postretro-compile-times/test-postretro-no-run` | `/tmp/postretro-compile-times/test-postretro-no-run` | Clean first build into fresh dir; compile tests only | 566.90s | `/tmp/postretro-compile-times/test-postretro-no-run/cargo-timings/cargo-timing-20260630T135000925Z-892a9f914c1db502.html` |
| Touch `prl.rs` | `cargo check --timings -p postretro --target-dir /tmp/postretro-compile-times/touch-prl` | `/tmp/postretro-compile-times/touch-prl` | Primed full check first; prime discarded (`real 221.04s`); `touch crates/postretro/src/prl.rs` | 2.21s | `/tmp/postretro-compile-times/touch-prl/cargo-timings/cargo-timing-20260630T145249494Z-892a9f914c1db502.html` |
| Touch `portal_vis.rs` | `cargo check --timings -p postretro --target-dir /tmp/postretro-compile-times/touch-portal-vis` | `/tmp/postretro-compile-times/touch-portal-vis` | Primed full check first; prime discarded (`real 244.04s`); `touch crates/postretro/src/portal_vis.rs` | 2.02s | `/tmp/postretro-compile-times/touch-portal-vis/cargo-timings/cargo-timing-20260630T145731405Z-892a9f914c1db502.html` |
| Touch `render/mesh_pass.rs` | `cargo check --timings -p postretro --target-dir /tmp/postretro-compile-times/touch-render-mesh-pass` | `/tmp/postretro-compile-times/touch-render-mesh-pass` | Primed full check first; prime discarded (`real 265.44s`); `touch crates/postretro/src/render/mesh_pass.rs` | 1.99s | `/tmp/postretro-compile-times/touch-render-mesh-pass/cargo-timings/cargo-timing-20260630T150255612Z-892a9f914c1db502.html` |
| Touch `render/sh_volume.rs` | `cargo check --timings -p postretro --target-dir /tmp/postretro-compile-times/touch-render-sh-volume` | `/tmp/postretro-compile-times/touch-render-sh-volume` | Primed full check first; prime discarded (`real 278.58s`); `touch crates/postretro/src/render/sh_volume.rs` | 2.02s | `/tmp/postretro-compile-times/touch-render-sh-volume/cargo-timings/cargo-timing-20260630T150902299Z-892a9f914c1db502.html` |
| Touch scripting descriptor | `cargo check --timings -p postretro --target-dir /tmp/postretro-compile-times/touch-scripting-js-entity` | `/tmp/postretro-compile-times/touch-scripting-js-entity` | Primed full check first; prime discarded (`real 303.30s`); `touch crates/scripting-core/src/data_descriptors/js/entity.rs` | 2.85s | `/tmp/postretro-compile-times/touch-scripting-js-entity/cargo-timings/cargo-timing-20260630T151526212Z-892a9f914c1db502.html` |

## Timing Notes By Case

### Clean `cargo check -p postretro`

- Critical-path tail: `swc_common` 24.28s -> `swc_ecma_ast` 22.24s -> `swc_ecma_parser` 54.40s -> `swc_ecma_transforms_base` 14.57s -> `swc_ecma_transforms_optimization` 33.57s -> `swc_bundler` 28.84s -> `postretro-scripting-core` build/check 8.21s total -> final `postretro` check 8.87s.
- High-time crates: `mlua-sys` build-script run 151.12s (#1), `rquickjs-sys` build-script run 71.34s (#2), `syn` 64.88s (#3), `swc_ecma_parser` 54.40s (#5), `wgpu-core` check 16.17s (#46), `naga` check 15.66s (#47).
- Required dependency notes: `rquickjs-sys` and `mlua-sys` dominate high-time but are not on the reconstructed final critical-path tail; `luau0-src` appears as a feature/source under `mlua-sys` and separately at 3.76s (#119). `wgpu`/`naga`, `cosmic-text` 4.07s (#114), and `kira` 6.81s (#81) are high-time contributors but off the final critical path. `glyphon` is small at 0.98s (#291). No final link for check.

### Warm no-op `cargo check -p postretro`

- Critical path: no rebuild work; report end duration rounds to 0.00s and wall time is 1.04s.
- High-time crates: all units are already fresh and reported as 0.00s.
- Required dependency notes: `rquickjs-sys`, `mlua-sys`/`luau0-src`, `wgpu`/`naga`, `glyphon`/`cosmic-text`, and `kira` are present in the graph but all at 0.00s. No final link for check.

### `cargo check --workspace`

- Critical-path tail: `swc_common` 42.00s -> `swc_ecma_ast` 45.07s -> `swc_ecma_parser` 67.40s -> `swc_ecma_transforms_base` 21.04s -> `swc_ecma_transforms_optimization` 39.10s -> `swc_bundler` 29.84s -> `postretro-scripting-core` build/check 10.70s total -> final `postretro` check 10.43s.
- High-time crates: `mlua-sys` build-script run 200.79s (#1), `syn` 92.45s (#2), `zerocopy-derive` 80.08s (#3), `rquickjs-sys` build-script run 68.59s (#4), `swc_ecma_parser` 67.40s (#5), `naga` check 26.86s (#39), `wgpu-core` check 20.09s (#51).
- Required dependency notes: `mlua-sys`/`luau0-src` and `rquickjs-sys` are high-time leaders but not on the final reconstructed tail. `wgpu`/`naga` are visible high-time units off the final tail. `cosmic-text` is 4.34s (#158), `glyphon` 0.97s (#381), `kira` 5.94s (#133). No final link for check.

### `cargo build -p postretro --bins`

- Critical-path tail: `swc_common` 32.57s -> `swc_ecma_ast` 39.95s -> `swc_ecma_parser` 61.74s -> `swc_ecma_transforms_base` 45.89s -> `swc_ecma_transforms_optimization` 54.25s -> `postretro-scripting-core` build/lib 15.42s total -> final `postretro` bin 27.45s.
- High-time crates: `mlua-sys` build-script run 222.85s (#1), `syn` 157.29s (#2), `objc2-foundation` 149.97s (#3), `image` 124.62s (#4), `read-fonts` 116.07s (#5), `naga` 91.49s (#9), `rquickjs-sys` build-script run 87.44s (#12).
- Required dependency notes: `mlua-sys`/`luau0-src` and `rquickjs-sys` are high-time leaders but not on the final reconstructed tail. `wgpu-core` is 67.73s (#23), `wgpu-hal` 29.94s (#66), `wgpu` 29.40s (#68), and `naga` 91.49s (#9). `cosmic-text` 42.96s (#46), `kira` 39.18s (#52), `glyphon` 2.61s (#245). Final link is folded into the final `postretro` bin unit, which is on the critical tail at 27.45s.

### `cargo test -p postretro --no-run`

- Critical-path tail: `syn` 125.76s -> `serde_derive` 84.67s -> `swc_common` 49.22s -> `swc_ecma_ast` 65.24s -> `swc_ecma_parser` 112.77s -> `swc_ecma_transforms_base` 85.39s -> `swc_ecma_transforms_optimization` 104.80s -> `postretro-scripting-core` build/lib 27.12s total -> final `postretro` test bin 101.60s.
- High-time crates: `naga` 187.48s (#1), `mlua-sys` build-script run 177.06s (#2), `gltf-json` 167.03s (#3), `image` 155.04s (#4), `wgpu-core` 145.32s (#5), `gltf` 136.30s (#6), `cosmic-text` 71.72s (#28), `rquickjs-sys` build-script run 78.69s (#23).
- Required dependency notes: `naga`/`wgpu-core` move into the top high-time set for test compilation, but the reconstructed final tail still runs through proc macros, SWC, scripting-core, and the final test binary. `mlua-sys`/`luau0-src` and `rquickjs-sys` are high-time leaders off the final tail. `kira` is 53.45s (#37), `glyphon` 4.42s (#159). Final link is folded into the final `postretro` test binary unit, which is on the critical tail at 101.60s.

### Touch rebuilds

- `crates/postretro/src/prl.rs`: critical path is final `postretro` check only, 1.55s unit duration; all watched dependencies are already fresh at 0.00s; no final link.
- `crates/postretro/src/portal_vis.rs`: critical path is final `postretro` check only, 1.37s unit duration; all watched dependencies are already fresh at 0.00s; no final link.
- `crates/postretro/src/render/mesh_pass.rs`: critical path is final `postretro` check only, 1.36s unit duration; all watched dependencies are already fresh at 0.00s; no final link.
- `crates/postretro/src/render/sh_volume.rs`: critical path is final `postretro` check only, 1.38s unit duration; all watched dependencies are already fresh at 0.00s; no final link.
- `crates/scripting-core/src/data_descriptors/js/entity.rs`: critical path is `postretro-scripting-core` check 0.74s -> final `postretro` check 1.42s; `gen-script-types` also checks in 0.13s. All watched third-party dependencies are already fresh at 0.00s; no final link.

## Status

All measurement commands in the case matrix exited with status `0`.

## Task 2 Dev Cargo Config

Captured on 2026-06-30 by Worker 2, against the Task 1 baseline numbers above. These runs used isolated target dirs under `/tmp/postretro-compile-times/`. The Task 1 `baseline.md` remains the canonical artifact; these are appended findings, not a replacement baseline.

### Committed Config

- Added `.cargo/config.toml` as a comment-only repository config. It has no active `[build]`, `[target.*]`, `linker`, `rustflags`, or wrapper settings, so default Cargo behavior remains Cargo's platform default on macOS, Linux, Windows, and CI.
- Faster linkers are documented only as commented-out local examples:
  - macOS: optional `ld64.lld` examples for Apple targets, with LLVM install notes.
  - Linux: optional `mold` example for `x86_64-unknown-linux-gnu`, with package-manager install notes.
  - Windows MSVC: optional `rust-lld.exe` example.
- Incremental compilation remains on through Cargo's normal dev-profile default; the committed config does not override it.

### `dev-fast` Decision

Decision: adopt `dev-fast` as an opt-in profile.

The committed profile is runtime-neutral by construction:

- `[profile.dev-fast]` inherits `dev`.
- It changes only `debug = "line-tables-only"`.
- `[profile.dev-fast.package."*"] opt-level = 2` keeps dependencies at the existing dev-profile floor from `[profile.dev.package."*"]`.
- It does not change linker, rustflags, codegen units, LTO, panic behavior, or dependency opt level.

Measured comparisons:

| Case | Command | Target dir | Wall time | Compared to Task 1 |
| --- | --- | --- | ---: | --- |
| Task 1 build baseline | `cargo build --timings -p postretro --bins --target-dir /tmp/postretro-compile-times/build-postretro-bins` | `/tmp/postretro-compile-times/build-postretro-bins` | 452.29s | Baseline |
| Same-session default build sanity run | `cargo build -p postretro --bins --target-dir /tmp/postretro-compile-times/task2-reduced-debug-build` | `/tmp/postretro-compile-times/task2-reduced-debug-build` | 309.41s | 142.88s faster than Task 1, but treated as cache-warmed context, not a new baseline |
| Reduced debug-info build | `CARGO_PROFILE_DEV_DEBUG=line-tables-only cargo build -p postretro --bins --target-dir /tmp/postretro-compile-times/task2-reduced-debug-build-line-tables` | `/tmp/postretro-compile-times/task2-reduced-debug-build-line-tables` | 261.94s | 190.35s faster than Task 1 build baseline |

The reduced-debug measurement used Cargo's profile env override before the manifest profile was added. It is equivalent to the committed `dev-fast` debug setting for this comparison. Compared to the same-session default build, reduced debug info saved 47.47s (about 15%). Compared to the Task 1 build baseline, it saved 190.35s (about 42%). Because the change is opt-in and only reduces debug-info detail, the win is meaningful enough to adopt `dev-fast`.

### Dependency `opt-level` Comparison

Lowering dependency `opt-level` was measured only to document the trade-off; it is rejected/deferred for Task 2 because it changes runtime code generation for dependencies.

| Case | Command | Target dir | Wall time | Compared to Task 1 |
| --- | --- | --- | ---: | --- |
| Task 1 clean check baseline | `cargo check --timings -p postretro --target-dir /tmp/postretro-compile-times/clean-check-postretro` | `/tmp/postretro-compile-times/clean-check-postretro` | 261.67s | Baseline |
| Dependencies at `opt-level = 1` | `cargo check -p postretro --config 'profile.dev.package."*".opt-level=1' --target-dir /tmp/postretro-compile-times/task2-deps-opt-level-1-check` | `/tmp/postretro-compile-times/task2-deps-opt-level-1-check` | 121.85s | 139.82s faster than Task 1 clean check baseline |

Despite the compile-time win, dependency `opt-level = 1` is runtime-affecting and therefore out of scope for the conservative `dev-fast` profile. It should not be adopted without a separate runtime/build-iteration decision.

### Script-Compiler Build-Dependency Path

Observed with:

`cargo build -v -p postretro --bins --target-dir /tmp/postretro-compile-times/task2-observe-script-compiler`

Result: status `0`; wall time `576.54s`. This verbose run is not used as a timing comparison because `-v` emits every rustc invocation and ran slower than the quiet build cases. It is used for flag evidence.

Observed rustc flags:

- `swc_common v21.0.1`: `-C opt-level=2`
- `swc_ecma_ast v23.0.0`: `-C opt-level=2`
- `swc_ecma_parser v39.0.0`: `-C opt-level=2`
- `swc_bundler v48.0.0`: `-C opt-level=2`
- Local `postretro-script-compiler` build-dependency library: no explicit `-C opt-level` flag in the rustc invocation, which means rustc's default opt level for that local unit.

Interpretation: the SWC-heavy third-party crates on the `postretro-script-compiler` build-dependency path are actually compiled at `opt-level = 2` today, observed from real rustc flags. That comes from the existing dependency package override, not from a build override. No `[profile.dev.build-override]` exists. Task 2 does not add one: adding a build override would change build-script/proc-macro/build-dependency behavior and needs its own measurement. The committed `.cargo/config.toml` does not affect this path by default. The opt-in `dev-fast` profile keeps dependency `opt-level = 2`; it may reduce debug info where Cargo emits debug info, but it does not lower the SWC dependency opt level.

### Day-To-Day Defaults

- During ordinary iteration, prefer `cargo check -p postretro` plus targeted tests for the touched crate/module.
- Use `cargo build --profile dev-fast -p postretro --bins` when a local full build is needed and line-table debug info is enough.
- Preserve `cargo check --workspace` as the broader gate before handing off or integrating.
- Keep full workspace tests and ignored/cold-bake level-compiler tests out of routine Task 2 verification.

### Per-OS Behavior

- macOS default: unchanged. No active linker or rustflag override.
- Linux default: unchanged. No active linker or rustflag override.
- Windows default: unchanged. No active linker or rustflag override.
- Opt-in deltas: only a developer who manually uncomments a target linker example in `.cargo/config.toml` changes local linker behavior. The committed file ships those examples disabled.
