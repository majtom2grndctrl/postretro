# postretro-visibility

> Epic: `E19--render-stack-decomposition`. Folds `compile-time-reduction` Task 3.

## Goal

Extract runtime portal traversal and frustum visibility into a CPU-only crate so editing visibility code does not recompile the renderer's GPU modules.

## Scope

### In scope
- `postretro-visibility`: move `visibility.rs` + `portal_vis.rs` — `VisibleCells`, `CameraCullVisibility`, `VisibilityStats`, `VisibilityPath`, `VisibilityResult`, `determine_visible_cells`, `Frustum`, `FrustumPlane`, `portal_traverse`, `narrow_frustum`, `clip_polygon_to_frustum`, and the internal clipping helpers + their tests.
- Depend on `postretro-level-loader` (`LevelWorld`, cell data) and `glam`. `postretro-render-data` is a **dev-dependency** only — the `geometry`/`material` use is test-only (`visibility.rs:728`, inside `#[cfg(test)] mod tests`), so the normal build carries no render-data edge.
- Update the ~8 importers.

### Out of scope
- The GPU cull pipelines (`compute_cull`/`candidate_cull`/`shadow_cull`) — those are GPU and move into `postretro-renderer` (`E19--renderer-gpu`).
- Frame-level render policy that reads `FullRenderer` state (stays renderer-side).

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md`. Durable decisions are captured into `context/lib/` per spec as each spec is approved — not in one batch at first promotion.
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; visibility/portal tests pass from their relocated home, including lightweight portal fixtures (no full GPU `Renderer` needed).
- [ ] `cargo tree -p postretro-visibility` shows no wgpu/winit/glyphon/kira/mlua/rquickjs. The normal-build graph carries no `postretro-render-data` edge — it is a dev-dependency (`cargo tree -p postretro-visibility --edges normal` omits render-data; it appears only under `--edges dev`).
- [ ] Editing the visibility crate does not recompile renderer GPU modules.
- [ ] `determine_visible_cells` produces identical visibility results for a fixture set (behavior-preserving).

## Tasks

### Task 1: Extract postretro-visibility
Create the crate, move both files (relying on the `E19--leaf-hygiene-and-boundary-prep` `Frustum`/`FrustumPlane` widening), wire deps, update importers. Depend on `LevelWorld` directly. Wire `postretro-render-data` as a `[dev-dependencies]` entry, not a normal dep — the `geometry`/`material` use is confined to the test module (`visibility.rs:728`).

**Decision (was open question 6): depend on `LevelWorld` directly.** Principle: lean — the crate boundary already cuts the recompile coupling, so a borrowed portal-world view would be speculative abstraction.

**Deferred optimization:** the old draft's borrowed portal-world view (exposing only leaves/portals/adjacency) — add only on a measured rebuild problem.

## Sequencing
**Phase 1:** Task 1. Needs `postretro-level-loader` (`E19--level-loader`) as a normal dep; `postretro-render-data` (`E19--render-data`, the `geometry`/`material` types) is a dev-dependency only (test-side). Milestone 1.
