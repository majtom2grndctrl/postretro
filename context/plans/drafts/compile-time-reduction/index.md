# Compile-Time Reduction

> **Status:** draft
> **Related:** `context/lib/development_guide.md` §1.4 · `context/lib/testing_guide.md` §3 · `context/lib/build_pipeline.md` · `context/lib/rendering_pipeline.md` §2, §10 · `context/lib/scripting.md`

## Goal

Reduce day-to-day Rust rebuild time for engine work without changing runtime behavior, map output, or scripting semantics. Start with measured baselines, then split stable CPU-only seams out of the `postretro` binary crate so common edits recompile less of the renderer, audio, input, and scripting stack.

## Scope

### In scope

- Establish repeatable compile-time measurements for clean, warm, and targeted edit builds.
- Add low-risk Cargo developer configuration for faster local linking and debug builds.
- Extract runtime portal traversal and frustum visibility math into a small CPU-only library crate behind a borrowed portal-world view.
- Extract runtime PRL loading and runtime level data into a small library crate.
- Stage the PRL split internally before moving the loader crate boundary.
- Audit the scripting build/prelude and serde-heavy descriptor surfaces for later, measured follow-up splits.

### Out of scope

- Runtime performance changes.
- PRL wire-format changes.
- Map bake cache changes.
- Renderer crate split.
- Scripting runtime extraction beyond an audit and follow-up plan.
- Removing QuickJS, Luau, SWC, wgpu, glyphon, kira, or winit from supported builds.
- Forcing one linker on every platform. Platform-specific linker config must degrade cleanly.

## Acceptance Criteria

- [ ] A baseline report records command, toolchain, host OS, target directory state, wall time, peak crate list from `cargo build --timings`, and top rebuild crates for at least: clean `cargo check -p postretro`, warm no-op `cargo check -p postretro`, a `prl.rs`-touch rebuild, a `portal_vis.rs`-touch rebuild, and a scripting descriptor touch rebuild.
- [ ] Baseline commands run in isolated `--target-dir` folders under `/tmp` so existing repo `target/` state and other worktrees do not contaminate results.
- [ ] Baseline metadata includes `cargo -V`, `rustc -vV`, host OS, target triple, CPU/RAM, `RUSTFLAGS`, `RUSTC_WRAPPER`, relevant `CARGO_*` variables, and `cargo metadata --locked`.
- [ ] The baseline covers `cargo check -p postretro`, `cargo check --workspace`, `cargo build -p postretro --bins`, `cargo test -p postretro --no-run`, `cargo build -p postretro-script-compiler`, and `cargo build -p postretro-level-compiler`.
- [ ] The repo has a documented local-dev Cargo config path for faster linking/profile settings, with macOS/Linux/Windows behavior called out. It must not break stable Rust or CI.
- [ ] After the visibility split, editing the portal traversal crate and running its tests does not recompile renderer GPU modules.
- [ ] `crates/postretro/src/prl.rs` is split internally before the loader crate move; the split is behavior-preserving and keeps existing call sites compiling.
- [ ] After the PRL loader split, editing the new loader crate and running its tests does not recompile `wgpu`, `winit`, `glyphon`, `kira`, `mlua`, or `rquickjs`.
- [ ] The scripting/prelude audit either produces a follow-up draft plan or records a measured decision to leave the current build-script path unchanged.
- [ ] The serde/descriptor audit either produces a follow-up draft plan or records that serde macro expansion is not a top compile-time contributor.
- [ ] `cargo test -p postretro-level-format`, `cargo test -p postretro-level-loader`, `cargo test -p postretro-visibility`, and `cargo check -p postretro` pass after the relevant crates exist.
- [ ] No `unsafe` blocks are added.
- [ ] Each extraction PR quotes before/after timings from the baseline commands. If a split fails to improve the targeted edit loop by a meaningful margin, later structural phases are paused and re-evaluated.

## Tasks

### Task 1: Baseline Compile-Time Measurement

Add a small measurement workflow before changing compile structure. Use stable Cargo commands first: `cargo clean` only for the explicit clean case, then `cargo check -p postretro`, `cargo check --workspace`, `cargo build -p postretro --bins`, `cargo test -p postretro --no-run`, `cargo build -p postretro-script-compiler`, and `cargo build -p postretro-level-compiler`. Run each command in an isolated `--target-dir /tmp/postretro-compile-times/<case>` path. Record `cargo -V`, `rustc -vV`, host OS, target triple, CPU/RAM, `RUSTFLAGS`, `RUSTC_WRAPPER`, relevant `CARGO_*` variables, `cargo metadata --locked`, and wall time.

For targeted rebuilds, touch or minimally edit and restore one file at a time: `crates/postretro/src/prl.rs`, `crates/postretro/src/portal_vis.rs`, and a scripting descriptor file under `crates/postretro/src/scripting/data_descriptors/types/`. Run the same cases with `--timings` and report critical-path crates and high self-time crates, explicitly checking `postretro-script-compiler`/SWC, `rquickjs-sys`, `mlua-sys`/`luau0-src`, `wgpu`/`naga`, `glyphon`/`cosmic-text`, `kira`/`cpal`/`symphonia`, and final link time. Do not use ignored level-compiler cold-bake integration tests as routine timing inputs; `context/lib/testing_guide.md` says those can take about an hour. Save results in the plan folder as `research.md` or in the implementing PR description; do not update `context/lib/` yet.

### Task 2: Developer Cargo Configuration

Add a conservative `.cargo/config.toml` or documented per-user equivalent. Favor settings that work on the current stable toolchain: keep incremental enabled, compare the current dev profile against lower dependency `opt-level`, reduced debug info, and a named `dev-fast` profile if runtime feel remains acceptable. Use faster linkers only behind target-specific opt-in config. Linux may use `mold` or `lld` when installed. macOS should prefer the platform linker defaults unless a measured alternative is available locally. Windows should document `rust-lld`/`lld-link` as an experiment, not a committed requirement. Confirm Cargo profile override precedence for build dependencies so the SWC-heavy `postretro-script-compiler` path is understood. The config must not require all contributors or CI to install a new linker.

Also document day-to-day workflow defaults: prefer `cargo check -p postretro` and targeted tests while iterating, preserve an explicit `cargo check --workspace` gate, and decide whether `[workspace] default-members = ["crates/postretro"]` would help root-command ergonomics without hiding full-workspace checks. Optional `sccache` guidance is in scope, but it must remain optional.

### Task 3: Extract Runtime Portal Visibility First

Create a new workspace library crate, tentatively `postretro-visibility`, around the CPU-only portal traversal currently in `crates/postretro/src/portal_vis.rs`. Do not require the full `LevelWorld` type as the crate boundary. Introduce a borrowed portal-world view that exposes only the data traversal needs: leaves, portals, and leaf→portal adjacency. Move or duplicate only the minimal frustum surface from `crates/postretro/src/visibility.rs`: `Frustum`, `FrustumPlane`, and the polygon/frustum clipping helpers the traversal owns. Keep frame-level policy in `postretro`: `VisibilityResult`, `VisibilityStats`, fallback selection, render diagnostics, and candidate-cull provenance.

The new crate must not depend on `wgpu`, `winit`, `glyphon`, `kira`, `mlua`, `rquickjs`, `notify`, `parry3d`, or `postretro-script-compiler`. Its tests should construct lightweight portal fixtures instead of full `LevelWorld` instances. `postretro` continues to call portal traversal from `determine_visible_cells`; behavior and fallback paths stay unchanged.

### Task 4: Stage Runtime Level Data Internally

Split `crates/postretro/src/prl.rs` before crate extraction. Keep behavior unchanged, but separate responsibilities into smaller modules such as `types`, `decode`, and `validation` under the engine crate. This reduces the blast radius of the later crate move and avoids a one-step 3.4k-line transplant. Preserve the public surface used by current call sites: `LevelWorld`, `BspChild`, `NodeData`, `LeafData`, `PortalData`, `FaceMeta`, `MapLight`, `LightType`, `FalloffModel`, `ShadowType`, `LightmapMode`, `CellDrawIndex`, `PrlLoadError`, and `load_prl`.

This stage should also decide where the small CPU-only dependencies belong:

- `crates/postretro/src/geometry.rs` owns `WorldVertex`, `BvhNode`, `BvhLeaf`, and `BvhTree`.
- `crates/postretro/src/material.rs` owns `Material` and texture-prefix derivation.
- `crates/postretro/src/lighting/influence.rs` owns `LightInfluence`.

If they are consumed broadly, create a narrower CPU-only runtime-level/type crate. If they remain loader-owned, move them with the loader. Do not move GPU code. Renderer still owns all `wgpu` calls.

### Task 5: Move PRL Decode and Validation Into a Loader Crate

Create a new workspace library crate, tentatively `postretro-level-loader` or `postretro-runtime-level`, after Task 4 has reduced churn. Move `load_prl`, section conversion, portal adjacency construction, `LevelWorld::find_leaf`, `LevelWorld::spawn_position`, and the existing PRL loader tests into that crate. The loader crate should depend on `postretro-level-format`, `glam`, `thiserror`, and any extracted CPU-only type crate. It must not depend on `wgpu`, `winit`, `glyphon`, `kira`, `mlua`, `rquickjs`, `notify`, or `postretro-script-compiler`.

Update all call sites in `postretro`: startup level loading, startup install, visibility, collision, renderer resource upload, lighting, scripting systems that read level data, and tests that construct `LevelWorld`. Keep the renderer boundary intact: level loaders produce CPU data and renderer uploads it.

### Task 6: Audit Scripting and Prelude Build Coupling

Measure before splitting. The current `postretro` build script calls `postretro_script_compiler::write_prelude` from `crates/postretro/build.rs`, which pulls the SWC-based `postretro-script-compiler` library into engine compile work. The scripting module also reaches into renderer UI descriptor types, renderer mesh metadata, PRL types, input modes, and visibility. Audit whether a smaller prelude-generator crate or checked-in generated prelude would reduce engine rebuilds without violating `context/lib/scripting.md` §8.

Separately identify GPU-free UI descriptor/layout/style types that can move out of `render::ui` into a CPU-only model crate shared by scripting, typedef generation, and render UI. Keep engine bridge systems such as mesh render, light bridge, fog bridge, and particle render in `postretro` initially; they intentionally connect scripting data to render, PRL, lighting, FX, and visibility.

### Task 7: Serde and Descriptor Surface Follow-Up

Use the baseline and `cargo build --timings` output to decide whether serde macro expansion is a top contributor. Current derives cluster around scripting components, scripting reactions, scripting data descriptors, render UI descriptors, level compiler cache inputs, and level-format cfg-gated serde support. If serde is measurable, draft a separate plan to consolidate script/UI descriptor data into CPU-only crates. Do not fold that work into the PRL or visibility extraction.

## Sequencing

**Phase 1 (sequential):** Task 1 — all later work consumes the baseline.
**Phase 2 (sequential):** Task 2 — independent config win; measure before and after.
**Phase 3 (sequential):** Task 3 — smallest crate seam first; validates the borrowed portal-world boundary.
**Phase 4 (sequential):** Task 4, then Task 5 — loader crate movement depends on the internal PRL split.
**Phase 5 (concurrent):** Task 6, Task 7 — audits only; each may produce a separate follow-up plan.

## Rough Sketch

Current engine crate shape confirmed:

- Workspace members: `postretro`, `postretro-level-format`, `postretro-level-compiler`, `postretro-script-compiler`, `postretro-net`.
- `postretro` depends directly on the heavy runtime stacks: `wgpu`, `winit`, `glyphon`, `kira`, `mlua`, `rquickjs`, `notify`, and `notify-debouncer-full`.
- `crates/postretro/src/prl.rs` is about 3.4k lines. It uses `postretro-level-format`, `glam`, `thiserror`, plus three engine-local CPU modules: `geometry`, `material`, and `lighting::influence`.
- `crates/postretro/src/portal_vis.rs` is about 2.3k lines. It uses `glam::Vec3`, `LevelWorld`, and frustum types from `visibility.rs`. Its main external call is from `determine_visible_cells`.
- `crates/postretro/src/main.rs` is about 6.7k lines. Avoid using compile-time work as a reason to grow it further.
- `crates/postretro/src/scripting/luau.rs`, `quickjs.rs`, `primitives_registry.rs`, and `reaction_dispatch.rs` are large enough that future scripting extraction should avoid adding more to them before a split.

Visibility extraction should prefer a borrowed view over a dependency on all runtime level data. That view should expose only the fields portal traversal needs, so the visibility crate can move before the PRL loader crate.

PRL extraction should then prefer one of two paths:

1. Move `geometry`, `material`, and `LightInfluence` into `postretro-level-loader` or `postretro-runtime-level` when they remain pure CPU data.
2. Or create a narrower `postretro-runtime-types` crate first if renderer, scripting, and loader all need those types without depending on loader code.

Pick after Task 1, based on rebuild attribution and call-site churn. Do not move GPU code. Renderer still owns all `wgpu` calls.

Cargo config sketch:

```toml
# Proposed shape, not final bytes.
[profile.dev]
opt-level = 1
codegen-units = 4

[profile.dev.package."*"]
opt-level = 2

[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

Only land linker settings if the platform has the tool installed or the setting is gated/documented so missing tools do not break ordinary builds.

Renderer extraction is deliberately deferred. A future renderer plan must first move root GPU modules under renderer ownership and hide direct `wgpu::SurfaceTexture` exposure from engine-facing APIs. This compile-time plan should not leave any `wgpu` call outside the renderer.

## Open Questions

- Should the fast-linker config live in committed `.cargo/config.toml`, in `.cargo/config.local.toml` documentation, or in a script that developers opt into?
- Should `Material` stay in the engine because it will drive audio/gameplay behavior, or move with level data because it is derived during load and consumed broadly?
- Should the SDK prelude remain build-time generated, or should generated prelude output be committed with drift checks to decouple normal engine rebuilds from SWC?
- What minimum improvement counts as meaningful for targeted edit loops: 10%, 20%, or an absolute wall-time threshold?
