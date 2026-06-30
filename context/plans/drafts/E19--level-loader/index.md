# postretro-level-loader

> Epic: `E19--render-stack-decomposition`. Folds `compile-time-reduction` Tasks 4–5 (the internal PRL split already exists as `prl.rs` + `prl_loader.rs`).

## Goal

Extract runtime PRL loading and runtime level data into a CPU-only crate so editing level-load code stops recompiling the wgpu/VM stack, and so visibility, render-cpu, and the renderer depend on a real loader crate.

## Scope

### In scope
- `postretro-level-loader`: move `prl.rs` + `prl_loader.rs` wholesale. Both files move entirely; the list below is the externally-referenced `pub` surface to re-export from the crate root, not the full move set: `LevelWorld`, `load_prl` (defined in `prl_loader.rs`), `CellData`, `MapLight`, `LightType`, `FalloffModel`, `ShadowType`, `LightmapMode`, `CellDrawIndex`, `PortalData`, `FaceMeta`, the cell-locator types (`CellLocatorChild`, `CellLocatorNodeData`), the locate-trace types (`CellLocatorSide`, `CellLocatorTraceStep`, `CellLocatorTrace`), `PrlLoadError`, and `LevelWorld::{locate_cell,spawn_position,cell_count,cell_portal_count,cell_portal_index,cell_is_solid,cell_face_count,cell_bounds}`. The existing PRL loader tests (in `prl.rs`) move with the files.
- Depend on `postretro-level-format`, `postretro-render-data`, `glam`, `thiserror`. (No `serde` — neither file references it. No `script-ffi` feature — the loader holds `DataScriptSection`/`MapEntity` as wire data only, with no VM coupling.)
- Update the importers to the crate path: ~30 files in `crates/postretro/src/` reference `crate::prl`/`prl::`, including ~10 inline `crate::prl::Foo` paths with no `use` statement — search `rg 'crate::prl(_loader)?\b'`, not just `use crate::prl`. Other crates match only in doc-comments; there are no real external importers.
- Sink the bare `LightInfluence` struct (`{ center: Vec3, radius: f32 }` — `Debug, Clone`, no methods/impls, glam-only — today in `lighting/influence.rs`) down into `postretro-render-data`. It is a bounding-sphere cull volume in the `Aabb`/`cone_frustum` family (epic Decision 10), so the loader depends *down* on it, not *up* on `lighting`. Re-point **every** `LightInfluence` import to the `postretro-render-data` path, not only `LevelWorld.light_influences` (`prl.rs:296`) and its construction (`prl_loader.rs:1221`/`1240`): the consumers are `render/mod.rs`, `render/renderer_lighting.rs`, `render/renderer_types.rs`, `render/renderer_init_resources.rs`, `scripting/systems/mesh_render.rs`, and `render/tests/light_filter_tests.rs` — enumerate with the bare-symbol search `rg '\bLightInfluence\b'` (the path-qualified `lighting::influence::LightInfluence` misses the four consumers that import it unqualified). These are distinct from the ~30 `crate::prl` call sites in the bullet above. The GPU-free `pack_influence` packer **and its unit test** stay in `lighting`, importing `LightInfluence` from `render-data` (acyclic — `render-data` depends only on `glam`/`bytemuck`). The PRL wire types `LightInfluenceSection`/`InfluenceRecord` (`postretro-level-format`, section 21) stay in `level-format`; only the runtime cull struct moves.

### Out of scope
- GPU upload of level data (stays in `postretro-renderer`: `LevelGeometry` adaptation + buffer upload).
- PRL wire-format or section-layout changes.
- The renderer-side `level_world_to_geometry` adapter (`render/renderer_geometry.rs`) and `LevelGeometry<'a>` (`render/renderer_types.rs`) — both already live outside `prl.rs`/`prl_loader.rs`, so they stay renderer-adjacent (unless `E19--render-cpu` claims the adapter); nothing to split out here.

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md`. Durable decisions are captured into `context/lib/` per spec as each spec is approved — not in one batch at first promotion.
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; the PRL loader tests pass from their relocated home.
- [ ] `cargo tree -p postretro-level-loader` shows no wgpu/winit/glyphon/kira/mlua/rquickjs.
- [ ] Editing the loader crate and running its tests does not recompile `wgpu`/`naga`/`mlua`/`rquickjs`.
- [ ] Quote before/after warm-edit timings for a `prl.rs` touch vs. the `E19--baseline-and-cargo-config` baseline (global AC #8 + Milestone 1's `prl.rs`/`portal_vis.rs` warm-edit outcome).
- [ ] All ~30 call sites compile against the crate; no PRL wire/section change (a previously-compiled `.prl` loads byte-for-byte the same).

## Tasks

### Task 1: Extract postretro-level-loader
Create the crate, move `prl.rs`/`prl_loader.rs`, and wire the dep on `postretro-render-data` (provides the `geometry`/`material` modules — both files already import `postretro_render_data::geometry`/`::material`) plus `postretro-level-format`. `prl_loader.rs` is currently a child module of `prl` (`#[path = "prl_loader.rs"] mod prl_loader;` + `use super::…`); re-express that as two proper modules under the new `lib.rs`. Re-export every externally-referenced `pub` item from the crate root (the In-scope surface list above); the internal `pub(crate)` loader helpers in `prl_loader.rs` stay `pub(crate)` — both files move together as one crate, so they never cross a boundary. As a behavior-preserving prep step, sink `LightInfluence` from `lighting/influence.rs` into `postretro-render-data` (leaving `pack_influence` and its test in `lighting`) and re-point all its consumers' imports (see In-scope) so `LevelWorld`'s field and the loader compile against the lower leaf, not `lighting` (epic Decision 10). Reword the cosmetic `crate::nav::NavGraph` doc-comment reference at `prl.rs:344`, which won't resolve from the new crate. Update importers, then capture the warm-edit timing for the AC. `prl.rs` is ~4279 lines, `prl_loader.rs` ~1711 — already split; if a further split is needed to keep the move clean, do it as a behavior-preserving step first (split-before-extend).

## Sequencing
**Phase 1:** Task 1. Needs `postretro-render-data` (`E19--render-data`, the `geometry`+`material` types) — no build-order dep on `lighting`. Milestone 1; blocks `E19--visibility` and `E19--render-cpu`.

The `LightInfluence` sink edits `lighting/influence.rs`, which `E19--lighting-cpu` (Milestone 2) later extracts. By milestone order this spec lands first, so the struct has already sunk to `render-data` by the time lighting-cpu extracts the remainder of `influence.rs` (just `pack_influence` + its test). Merge coordination: lighting-cpu must not re-add the struct.
