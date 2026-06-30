# Baseline & Dev Cargo Config

> Epic: `E19--render-stack-decomposition`. Folds `compile-time-reduction` Tasks 1–2. Gates all timing claims in later specs.

## Goal

Establish repeatable compile-time measurement and a conservative dev Cargo config before any structural change, so every extraction can quote a real before/after for its targeted edit loop.

## Scope

### In scope
- A measurement workflow using stable Cargo, each case in an isolated `--target-dir /tmp/postretro-compile-times/<case>`. Warm/incremental cases (warm no-op, touch-rebuilds) prime their dir with a full build first — that priming time is discarded — then measure the no-op or touch-and-rebuild; clean cases measure the first build into a fresh dir.
- A baseline report capturing the case matrix below plus host/toolchain metadata.
- A conservative dev Cargo config path (repo `.cargo/config.toml` or documented per-user equivalent) for faster local linking/profile; cross-platform behavior pinned by the CI/linker AC below.

### Out of scope
- Requiring any contributor or CI to install a new linker.
- Mandatory `sccache`.
- Any source-structure change.

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md` (these migrate to `context/lib/` at first promotion).
- [ ] Baseline report records, once per run, the host/toolchain block: `cargo -V`, `rustc -vV`, host OS, target triple, CPU/RAM, the build environment captured via `env | grep -E '^(CARGO|RUST)'` (every set `CARGO_*`/`RUST*` var, including `RUSTFLAGS` and `RUSTC_WRAPPER`), and `cargo metadata --locked`; and, per case: the command, wall time, and the `--timings` critical-path + high-self-time crates (`--timings` works with `build`, `check`, and `test --no-run`).
- [ ] Case matrix covered: clean `cargo check -p postretro`; warm no-op `cargo check -p postretro`; `cargo check --workspace`; `cargo build -p postretro --bins` (builds both `postretro` and the `gen-script-types` helper); `cargo test -p postretro --no-run`; and targeted touch-rebuilds of `crates/postretro/src/prl.rs`, `crates/postretro/src/portal_vis.rs`, `crates/postretro/src/render/mesh_pass.rs`, `crates/postretro/src/render/sh_volume.rs`, and one scripting descriptor under `crates/scripting-core/src/data_descriptors/` (e.g. `js/entity.rs`). Warm-no-op and touch-rebuild cases are measured incrementally against a primed dir per Task 1's methodology, not into a cold dir.
- [ ] `--timings` output explicitly notes where `rquickjs-sys`, `mlua-sys`/`luau0-src`, `wgpu`/`naga`, `glyphon`/`cosmic-text`, `kira`, and final link sit on the critical path.
- [ ] Dev Cargo config does not break stable Rust or CI: by construction the committed config introduces no non-default linker or flag override, so default `cargo check` is unchanged on macOS, Linux, and Windows; per-OS behavior and any opt-in deltas are documented, and faster linkers (`mold`/`lld`/`rust-lld`) ship as commented-out per-target opt-in only.
- [ ] The `dev-fast` profile decision is recorded (adopted or rejected) with the dependency-`opt-level`/debug-info compile-time comparison numbers that drove it. dev-fast is runtime-neutral by construction — reduced debug-info only, dependency `opt-level` kept at its current floor — so adoption rests on compile-time evidence with no runtime or GPU/frame-timer measurement. Lowering dependency `opt-level` (runtime-affecting) is out of scope for this conservative profile.
- [ ] The baseline report states which profile and `opt-level` apply to the `script-compiler` build-dependency path — observed from the actual `-C opt-level` rustc flag (`cargo build -v` / `--timings` unit data), not inferred from the manifest — and whether the dev config change affects it.
- [ ] Baseline committed to this plan folder as `baseline.md` (the canonical artifact later specs cite — distinct from the epic-hub `research.md`, which is discovery input, not a deliverable); the implementing PR may additionally quote it, but the in-repo file is authoritative.

## Tasks

### Task 1: Baseline measurement
Run the case matrix, each case in its own isolated `--target-dir /tmp/postretro-compile-times/<case>`. Methodology: warm/incremental cases (warm no-op, touch-rebuilds) first prime their dir with a full build whose time is discarded, then measure the no-op or touch-and-rebuild; clean cases measure the first build into a fresh dir. A "touch" updates the file's mtime only (`touch <file>`, no content change), so the measurement is the pure downstream-recompile cost. Capture metadata + `--timings` (which also applies to the `check`/`test --no-run` cases), and annotate from each `--timings` report the critical-path position and self-time of the named dependency crates (`rquickjs-sys`, `mlua-sys`/`luau0-src`, `wgpu`/`naga`, `glyphon`/`cosmic-text`, `kira`, and final link). Do not use the ignored level-compiler cold-bake integration tests as routine inputs (`testing_guide.md §3` — ~1h). Write the captured results to `baseline.md` in this plan folder and commit it as the baseline report.

### Task 2: Dev Cargo config
Consume the `baseline.md` produced by Task 1: append this task's findings there and measure every comparison below against the Task 1 baseline numbers — do not start a fresh baseline. Add a conservative `.cargo/config.toml` or per-user equivalent: keep incremental on; compare dependency `opt-level` and reduced debug-info against that baseline. Add a named `dev-fast` profile if it yields a meaningful compile-time win and is runtime-neutral by construction — reduced debug-info only (no runtime cost), keeping dependency `opt-level` at its current floor (the root `[profile.dev.package."*"]` sets deps to opt-level 2). Decide adopt/reject on the compile-time numbers alone; no runtime or GPU/frame-timer measurement is required or used. Lowering dependency `opt-level` is runtime-affecting and out of scope for this conservative profile — defer it rather than gating on a frame timer. The committed config sets no non-default linker; faster linkers (`mold`/`lld`/`rust-lld`) ship as commented-out per-`[target.*]` opt-in blocks with install notes. Document, in `baseline.md`, which profile/`opt-level` actually applies to the SWC-heavy `script-compiler` build-dependency path — observe the real `-C opt-level` rustc flag on a swc crate via `cargo build -v` (or the `--timings` unit data) rather than inferring from the manifest, and check whether a `[profile.dev.build-override]` exists or should be added (the root `[profile.dev.package."*"]` override already sets deps to opt-level 2) — and whether the config change affects it. Document day-to-day defaults (`cargo check -p postretro` + targeted tests while iterating; preserve a `cargo check --workspace` gate).

## Sequencing
**Phase 1:** Task 1, then Task 2 (config measured against the Task 1 baseline). Milestone 1; independent of all other specs and runs first so they can quote it.
