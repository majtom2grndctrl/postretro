# Baseline & Dev Cargo Config

> Epic: `E19--render-stack-decomposition`. Folds `compile-time-reduction` Tasks 1–2. Gates all timing claims in later specs.

## Goal

Establish repeatable compile-time measurement and a conservative dev Cargo config before any structural change, so every extraction can quote a real before/after for its targeted edit loop.

## Scope

### In scope
- A measurement workflow using stable Cargo, each case in an isolated `--target-dir /tmp/postretro-compile-times/<case>`. Warm/incremental cases (warm no-op, touch-rebuilds) prime their dir with a full build first — that priming time is discarded — then measure the no-op or touch-and-rebuild; clean cases measure the first build into a fresh dir.
- A baseline report capturing the case matrix below plus host/toolchain metadata.
- A conservative dev Cargo config path (repo `.cargo/config.toml` or documented per-user equivalent) for faster local linking/profile, degrading cleanly per platform.

### Out of scope
- Requiring any contributor or CI to install a new linker.
- Mandatory `sccache`.
- Any source-structure change.

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md` (these migrate to `context/lib/` at first promotion).
- [ ] Baseline report records, for each case: command, `cargo -V`, `rustc -vV`, host OS, target triple, CPU/RAM, the build environment captured via `env | grep -E '^(CARGO|RUST)'` (every set `CARGO_*`/`RUST*` var, including `RUSTFLAGS` and `RUSTC_WRAPPER`), `cargo metadata --locked`, wall time, and `cargo build --timings` critical-path + high-self-time crates.
- [ ] Case matrix covered: clean `cargo check -p postretro`; warm no-op `cargo check -p postretro`; `cargo check --workspace`; `cargo build -p postretro --bins` (builds both `postretro` and the `gen-script-types` helper); `cargo test -p postretro --no-run`; and targeted touch-rebuilds of `crates/postretro/src/prl.rs`, `crates/postretro/src/portal_vis.rs`, `crates/postretro/src/render/mesh_pass.rs`, `crates/postretro/src/render/sh_volume.rs`, and one scripting descriptor under `crates/scripting-core/src/data_descriptors/` (e.g. `js/entity.rs`).
- [ ] `--timings` output explicitly notes where `rquickjs-sys`, `mlua-sys`/`luau0-src`, `wgpu`/`naga`, `glyphon`/`cosmic-text`, `kira`, and final link sit on the critical path.
- [ ] Dev Cargo config does not break stable Rust or CI: with the default (un-opted-in) config, `cargo check` succeeds unchanged on macOS, Linux, and Windows — no error and no behavior change; per-OS notes are documented and faster linkers (`mold`/`lld`/`rust-lld`) are commented-out per-target opt-in only.
- [ ] The `dev-fast` profile decision is recorded (adopted or rejected) with the dependency-`opt-level`/debug-info comparison numbers that drove it.
- [ ] The baseline report states which profile and `opt-level` apply to the `script-compiler` build-dependency path and whether the dev config change affects it.
- [ ] Baseline committed to this plan folder's `research.md` (the canonical, referenceable artifact later specs cite); the implementing PR may additionally quote it, but the in-repo file is authoritative.

## Tasks

### Task 1: Baseline measurement
Run the case matrix in isolated target dirs; capture metadata + `--timings`, and annotate from each `--timings` report the critical-path position and self-time of the named dependency crates (`rquickjs-sys`, `mlua-sys`/`luau0-src`, `wgpu`/`naga`, `glyphon`/`cosmic-text`, `kira`, and final link). Do not use the ignored level-compiler cold-bake integration tests as routine inputs (`testing_guide.md §3` — ~1h).

### Task 2: Dev Cargo config
Add a conservative `.cargo/config.toml` or per-user equivalent: keep incremental on; compare dependency `opt-level` and reduced debug-info; add a named `dev-fast` profile if the engine still runs without perceptible regression (sanity-checked by running `cargo run -p postretro` on a dev map). The committed config sets no non-default linker; faster linkers (`mold`/`lld`/`rust-lld`) ship as commented-out per-`[target.*]` opt-in blocks with install notes. Document, in the baseline report, which profile/`opt-level` applies to the SWC-heavy `script-compiler` build-dependency path (note the existing root `[profile.dev.package."*"]` override) and whether the config change affects it. Document day-to-day defaults (`cargo check -p postretro` + targeted tests while iterating; preserve a `cargo check --workspace` gate).

## Sequencing
**Phase 1:** Task 1, then Task 2 (config measured against the Task 1 baseline). Milestone 1; independent of all other specs and runs first so they can quote it.
