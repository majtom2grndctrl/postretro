# Baseline & Dev Cargo Config

> Epic: `E19--render-stack-decomposition`. Folds `compile-time-reduction` Tasks 1–2. Gates all timing claims in later specs.

## Goal

Establish repeatable compile-time measurement and a conservative dev Cargo config before any structural change, so every extraction can quote a real before/after for its targeted edit loop.

## Scope

### In scope
- A measurement workflow using stable Cargo, each case in an isolated `--target-dir /tmp/postretro-compile-times/<case>`.
- A baseline report capturing the case matrix below plus host/toolchain metadata.
- A conservative dev Cargo config path (repo `.cargo/config.toml` or documented per-user equivalent) for faster local linking/profile, degrading cleanly per platform.

### Out of scope
- Requiring any contributor or CI to install a new linker.
- Mandatory `sccache`.
- Any source-structure change.

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md` (these migrate to `context/lib/` at first promotion).
- [ ] Baseline report records, for each case: command, `cargo -V`, `rustc -vV`, host OS, target triple, CPU/RAM, `RUSTFLAGS`, `RUSTC_WRAPPER`, relevant `CARGO_*`, `cargo metadata --locked`, wall time, and `cargo build --timings` critical-path + high-self-time crates.
- [ ] Case matrix covered: clean `cargo check -p postretro`; warm no-op `cargo check -p postretro`; `cargo check --workspace`; `cargo build -p postretro --bins`; `cargo test -p postretro --no-run`; and targeted touch-rebuilds of `prl.rs`, `portal_vis.rs`, `render/mesh_pass.rs`, `render/sh_volume.rs`, and one scripting descriptor under `scripting/data_descriptors/types/`.
- [ ] `--timings` output explicitly notes where `rquickjs-sys`, `mlua-sys`/`luau0-src`, `wgpu`/`naga`, `glyphon`/`cosmic-text`, `kira`, and final link sit on the critical path.
- [ ] Dev Cargo config does not break stable Rust or CI; macOS/Linux/Windows behavior is documented; faster linkers (`mold`/`lld`/`rust-lld`) are opt-in per target only.
- [ ] Baseline saved in this plan folder (append to `research.md`) or in the implementing PR.

## Tasks

### Task 1: Baseline measurement
Run the case matrix in isolated target dirs; capture metadata + `--timings`. Do not use the ignored level-compiler cold-bake integration tests as routine inputs (`testing_guide.md §3` — ~1h).

### Task 2: Dev Cargo config
Add a conservative `.cargo/config.toml` or per-user equivalent: keep incremental on; compare dependency `opt-level` and reduced debug-info; add a named `dev-fast` profile if runtime feel holds. Faster linkers behind target-specific opt-in. Confirm profile override precedence so the SWC-heavy `script-compiler` build path is understood. Document day-to-day defaults (`cargo check -p postretro` + targeted tests while iterating; preserve a `cargo check --workspace` gate).

## Sequencing
**Phase 1:** Task 1, then Task 2 (config measured against the Task 1 baseline). Milestone 1; independent of all other specs and runs first so they can quote it.
