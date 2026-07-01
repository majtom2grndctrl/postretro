# postretro-visibility

> Epic: `E19--render-stack-decomposition`. Folds `compile-time-reduction` Task 3.

## Goal

Extract runtime portal traversal and frustum visibility into a CPU-only crate so editing visibility code does not recompile the renderer's GPU modules.

## Scope

### In scope
- `postretro-visibility`: move `visibility.rs` + `portal_vis.rs` — `VisibleCells`, `CameraCullVisibility`, `VisibilityStats`, `VisibilityPath`, `VisibilityResult`, `determine_visible_cells`, `extract_frustum_planes`, `Frustum`, `FrustumPlane`, `portal_traverse`, `narrow_frustum`, `clip_polygon_to_frustum`, and the internal clipping/frustum helpers (`is_aabb_outside_frustum`, `slide_near_plane_to`, `NEAR_PLANE_INDEX`) + their tests. `Frustum`/`FrustumPlane` are already `pub` in `visibility.rs` (widened in place by `E19--leaf-hygiene-and-boundary-prep`, not homed in render-data), so the cross-crate move compiles; the `pub(crate)` helpers travel with the code and stay crate-visible (both files co-move — no widening needed). `extract_frustum_planes` is `pub` and called from `main.rs:2161`, so re-export it (and the other public symbols) at the `postretro_visibility::` crate root.
- Depend on `postretro-level-loader` (`LevelWorld`, `CellData`, `FaceMeta`) and `glam`. `postretro-render-data` is a **dev-dependency** only — no non-test reference to render-data types exists in either moved file; the sole uses are `visibility.rs:729-730` and `portal_vis.rs:766`, both inside `#[cfg(test)] mod tests`, so the normal build carries no render-data edge.
- Update the importers (9 files in `crates/postretro`, all in-binary): re-point `crate::visibility::` / `crate::portal_vis::` to `postretro_visibility::`, and delete the `mod visibility;` / `mod portal_vis;` declarations (`main.rs:51` / `main.rs:38`). Leave the unrelated `render/ui/tree/tests/visibility.rs` module untouched. Verify: `rg 'crate::(visibility|portal_vis)'` over the binary returns empty afterward.

### Out of scope
- The GPU cull pipelines (`compute_cull`/`candidate_cull`/`shadow_cull`) — those are GPU and move into `postretro-renderer` (`E19--renderer-gpu`).
- Frame-level render policy that reads `FullRenderer` state (stays renderer-side).

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md`. Durable decisions are captured into `context/lib/` per spec as each spec is approved — not in one batch at first promotion.
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; visibility/portal tests pass from their relocated home, including lightweight portal fixtures (no full GPU `Renderer` needed).
- [ ] `cargo tree -p postretro-visibility` shows no wgpu/winit/glyphon/kira/mlua/rquickjs. The normal-build graph carries no `postretro-render-data` edge — it is a dev-dependency (`cargo tree -p postretro-visibility --edges normal` omits render-data; it appears only under `--edges dev`).
- [ ] Editing the visibility crate and running its tests recompiles no `wgpu`/`naga`/`winit`/`kira` (the epic's cpu-only firewall AC; the renderer GPU modules sit downstream of that toolchain).
- [ ] `determine_visible_cells` produces identical visibility results for a fixture set (behavior-preserving).
- [ ] Before/after warm-edit timing captured for a `portal_vis.rs`/`visibility.rs` touch vs. the `E19--baseline-and-cargo-config` baseline (epic global AC; Milestone 1 names this touch specifically).

## Tasks

### Task 1: Extract postretro-visibility
Create the crate, move both files (relying on the `E19--leaf-hygiene-and-boundary-prep` `Frustum`/`FrustumPlane` widening), wire deps, update importers. Depend on `LevelWorld` directly. Wire `postretro-render-data` as a `[dev-dependencies]` entry, not a normal dep — the `geometry`/`material` use is confined to the test modules (`visibility.rs:729-730`, `portal_vis.rs:766`).

**Test fixups on move.** `portal_vis.rs` tests read `crate::camera::{HFOV, NEAR, FAR}` (`camera` stays in the binary and does not move). Inline them as test-local consts in the relocated file — `HFOV = 100°` in radians, `NEAR = 0.1`, `FAR = 4096.0` (source: `camera.rs:7-10`) — or the relocated tests won't compile.

**Decision (was open question 6): depend on `LevelWorld` directly.** Principle: lean — the crate boundary already cuts the recompile coupling, so a borrowed portal-world view would be speculative abstraction.

**Deferred optimization:** the old draft's borrowed portal-world view (exposing only leaves/portals/adjacency) — add only on a measured rebuild problem.

## Sequencing
**Phase 1:** Task 1. Preconditions (all landed first): `E19--render-data` and `E19--level-loader` (level-loader depends on render-data), plus `E19--leaf-hygiene-and-boundary-prep` (the `Frustum`/`FrustumPlane` widening). Needs `postretro-level-loader` (`E19--level-loader`) as a normal dep; `postretro-render-data` (`E19--render-data`, the `geometry`/`material` types) is a dev-dependency only (test-side). Milestone 1.

**Downstream consumer.** `E19--render-cpu` (Milestone 2) will depend on `postretro-visibility` — `mesh_visible` (`render/mesh_pass.rs`) is a pure `LevelWorld`+`VisibleCells` predicate that descends into `postretro-render-cpu` (epic Decision 5). Keep `VisibleCells` (and the types the predicate reads) public.
