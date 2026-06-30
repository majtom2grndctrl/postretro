# postretro-level-loader

> Epic: `E19--render-stack-decomposition`. Folds `compile-time-reduction` Tasks 4–5 (the internal PRL split already exists as `prl.rs` + `prl_loader.rs`).

## Goal

Extract runtime PRL loading and runtime level data into a CPU-only crate so editing level-load code stops recompiling the wgpu/VM stack, and so visibility, render-cpu, and the renderer depend on a real loader crate.

## Scope

### In scope
- `postretro-level-loader`: move `prl.rs` + `prl_loader.rs` — `LevelWorld`, `load_prl`, `MapLight`, `LightType`, `FalloffModel`, `ShadowType`, `LightmapMode`, `CellDrawIndex`, `PortalData`, `FaceMeta`, the cell-locator types (`CellLocatorChild`, `CellLocatorNodeData`), `PrlLoadError`, `LevelWorld::{locate_cell,spawn_position,cell_count,cell_portal_*,cell_is_solid,cell_face_count,cell_bounds}`, and the existing PRL loader tests.
- Depend on `postretro-level-format`, `postretro-render-data`, `glam`, `thiserror`, `serde`.
- Update the ~21 importers to the crate path.

### Out of scope
- GPU upload of level data (stays in `postretro-renderer`: `LevelGeometry` adaptation + buffer upload).
- PRL wire-format or section-layout changes.
- The renderer-side `level_world_to_geometry` adapter (it produces the borrowed `LevelGeometry<'a>` handoff and stays renderer-adjacent unless `E19--render-cpu` claims it).

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md` (these migrate to `context/lib/` at first promotion).
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; the PRL loader tests pass from their relocated home.
- [ ] `cargo tree -p postretro-level-loader` shows no wgpu/winit/glyphon/kira/mlua/rquickjs.
- [ ] Editing the loader crate and running its tests does not recompile `wgpu`/`naga`/`mlua`/`rquickjs`.
- [ ] All ~21 call sites compile against the crate; no PRL wire/section change (a previously-compiled `.prl` loads byte-for-byte the same).

## Tasks

### Task 1: Extract postretro-level-loader
Create the crate, move `prl.rs`/`prl_loader.rs`, wire deps on `geometry`/`material`/`level-format`, widen boundary-crossing `pub(crate)` to `pub`, update importers. `prl.rs` is ~4279 lines but already split with `prl_loader.rs` — if a further split is needed to keep the move clean, do it as a behavior-preserving step first (split-before-extend).

## Sequencing
**Phase 1:** Task 1. Needs `postretro-render-data` (`E19--render-data`, the `geometry`+`material` types). Milestone 1; blocks `E19--visibility` and `E19--render-cpu`.
