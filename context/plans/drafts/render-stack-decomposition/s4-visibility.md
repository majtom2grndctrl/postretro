# s4 — postretro-visibility

> Epic: `render-stack-decomposition`. Folds `compile-time-reduction` Task 3.

## Goal

Extract runtime portal traversal and frustum visibility into a CPU-only crate so editing visibility code does not recompile the renderer's GPU modules.

## Scope

### In scope
- `postretro-visibility`: move `visibility.rs` + `portal_vis.rs` — `VisibleCells`, `CameraCullVisibility`, `VisibilityStats`, `VisibilityPath`, `VisibilityResult`, `determine_visible_cells`, `Frustum`, `FrustumPlane`, `portal_traverse`, `narrow_frustum`, `clip_polygon_to_frustum`, and the internal clipping helpers + their tests.
- Depend on `postretro-level-loader` (`LevelWorld`, cell data) and `postretro-render-data`, `glam`.
- Update the ~8 importers.

### Out of scope
- The GPU cull pipelines (`compute_cull`/`candidate_cull`/`shadow_cull`) — those are GPU and move into `postretro-renderer` (`s8`).
- Frame-level render policy that reads `FullRenderer` state (stays renderer-side).

## Acceptance criteria
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; visibility/portal tests pass from their relocated home, including lightweight portal fixtures (no full GPU `Renderer` needed).
- [ ] `cargo tree -p postretro-visibility` shows no wgpu/winit/glyphon/kira/mlua/rquickjs.
- [ ] Editing the visibility crate does not recompile renderer GPU modules.
- [ ] `determine_visible_cells` produces identical visibility results for a fixture set (behavior-preserving).

## Tasks

### Task 1: Extract postretro-visibility
Create the crate, move both files (relying on the `s1` `Frustum`/`FrustumPlane` widening), wire deps, update importers. Depend on `LevelWorld` directly.

**Decision (was open question 6): depend on `LevelWorld` directly.** Principle: lean — the crate boundary already cuts the recompile coupling, so a borrowed portal-world view would be speculative abstraction.

**Deferred optimization:** the old draft's borrowed portal-world view (exposing only leaves/portals/adjacency) — add only on a measured rebuild problem.

## Sequencing
**Phase 1:** Task 1. Needs `s3` (`level-loader`) + `s2` (`geometry`). Milestone 1.
